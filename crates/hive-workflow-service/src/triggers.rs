//! Trigger manager: watches event bus topics, cron schedules, and MCP
//! notifications to auto-launch workflows when trigger conditions are met.

use chrono::Utc;
use cron::Schedule;
use hive_core::{EventBus, EventLog};
use hive_workflow::store::WorkflowPersistence;
use hive_workflow::types::{
    InstanceFilter, StepStatus, StepType, TaskDef, TriggerType, WorkflowDefinition, WorkflowStatus,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn, Instrument};

use super::WorkflowEventGateRegistrar;
use super::WorkflowService;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An active trigger registration, linking a workflow definition to an event
/// source that can auto-launch instances.
struct ActiveTrigger {
    definition_id: String,
    definition_name: String,
    definition_version: String,
    trigger_type: TriggerType,
    /// For cron triggers: the next scheduled run time (ms since epoch).
    next_run_ms: Option<u64>,
}

/// Bucketed trigger storage for efficient event matching.
///
/// Event-pattern triggers are split into two groups:
///  - **exact**: topic string contains no wildcards → stored in a HashMap for O(1) lookup.
///  - **wildcard**: topic pattern contains `*` → stored in a Vec for linear scan.
///
/// MCP notifications and incoming-message triggers are stored separately so
/// `evaluate_event` only iterates the relevant bucket.
struct TriggerRegistry {
    event_exact: HashMap<String, Vec<ActiveTrigger>>,
    event_wildcard: Vec<ActiveTrigger>,
    mcp_notification: Vec<ActiveTrigger>,
    incoming_message: Vec<ActiveTrigger>,
    schedules: Vec<ActiveTrigger>,
    other: Vec<ActiveTrigger>,
}

impl TriggerRegistry {
    fn new() -> Self {
        Self {
            event_exact: HashMap::new(),
            event_wildcard: Vec::new(),
            mcp_notification: Vec::new(),
            incoming_message: Vec::new(),
            schedules: Vec::new(),
            other: Vec::new(),
        }
    }

    fn push(&mut self, trigger: ActiveTrigger) {
        match &trigger.trigger_type {
            TriggerType::EventPattern { topic, .. } => {
                if topic.contains('*') {
                    self.event_wildcard.push(trigger);
                } else {
                    self.event_exact.entry(topic.clone()).or_default().push(trigger);
                }
            }
            TriggerType::McpNotification { .. } => self.mcp_notification.push(trigger),
            TriggerType::IncomingMessage { .. } => self.incoming_message.push(trigger),
            TriggerType::Schedule { .. } => self.schedules.push(trigger),
            _ => self.other.push(trigger),
        }
    }

    fn remove_definition(&mut self, definition_id: &str) {
        let retain = |t: &ActiveTrigger| t.definition_id != definition_id;
        for bucket in self.event_exact.values_mut() {
            bucket.retain(retain);
        }
        self.event_exact.retain(|_, v| !v.is_empty());
        self.event_wildcard.retain(retain);
        self.mcp_notification.retain(retain);
        self.incoming_message.retain(retain);
        self.schedules.retain(retain);
        self.other.retain(retain);
    }

    fn remove_definition_version(&mut self, definition_id: &str, version: &str) {
        let retain = |t: &ActiveTrigger| {
            !(t.definition_id == definition_id && t.definition_version == version)
        };
        for bucket in self.event_exact.values_mut() {
            bucket.retain(retain);
        }
        self.event_exact.retain(|_, v| !v.is_empty());
        self.event_wildcard.retain(retain);
        self.mcp_notification.retain(retain);
        self.incoming_message.retain(retain);
        self.schedules.retain(retain);
        self.other.retain(retain);
    }

    /// Iterate all triggers (for listing / snapshots).
    fn iter_all(&self) -> impl Iterator<Item = &ActiveTrigger> {
        self.event_exact
            .values()
            .flat_map(|v| v.iter())
            .chain(self.event_wildcard.iter())
            .chain(self.mcp_notification.iter())
            .chain(self.incoming_message.iter())
            .chain(self.schedules.iter())
            .chain(self.other.iter())
    }
}

/// An active event gate subscription from a running workflow step.
struct ActiveEventGate {
    subscription_id: String,
    instance_id: i64,
    step_id: String,
    topic: String,
    filter: Option<String>,
    expires_at_ms: Option<u64>,
}

/// Manages trigger registrations and evaluates them against incoming events.
pub struct TriggerManager {
    triggers: RwLock<TriggerRegistry>,
    event_gates: RwLock<Vec<ActiveEventGate>>,
    event_bus: EventBus,
    event_log: Mutex<Option<Arc<EventLog>>>,
    workflow_service: Arc<Mutex<Option<std::sync::Weak<WorkflowService>>>>,
    store: Arc<dyn WorkflowPersistence>,
    connector_service: Mutex<Option<Arc<hive_connectors::ConnectorService>>>,
    running: std::sync::atomic::AtomicBool,
    event_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    cron_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl TriggerManager {
    pub fn new(event_bus: EventBus, store: Arc<dyn WorkflowPersistence>) -> Self {
        Self {
            triggers: RwLock::new(TriggerRegistry::new()),
            event_gates: RwLock::new(Vec::new()),
            event_bus,
            event_log: Mutex::new(None),
            workflow_service: Arc::new(Mutex::new(None)),
            store,
            connector_service: Mutex::new(None),
            running: std::sync::atomic::AtomicBool::new(false),
            event_task: Arc::new(Mutex::new(None)),
            cron_task: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the connector service reference (for mark_as_read support).
    pub async fn set_connector_service(&self, svc: Arc<hive_connectors::ConnectorService>) {
        *self.connector_service.lock().await = Some(svc);
    }

    /// Set the event log reference (for replaying missed events on startup).
    pub async fn set_event_log(&self, log: Arc<EventLog>) {
        *self.event_log.lock().await = Some(log);
    }

    /// Set the workflow service reference (called after construction).
    pub async fn set_workflow_service(&self, svc: Arc<WorkflowService>) {
        *self.workflow_service.lock().await = Some(Arc::downgrade(&svc));
    }

    /// Register all triggers from a workflow definition.
    /// Removes any existing triggers for this definition identity first,
    /// so that re-saves and version updates don't accumulate duplicates.
    ///
    /// For cron triggers, checks for a persisted `last_run_ms` so that
    /// missed runs during downtime are caught up immediately.
    pub async fn register_definition(&self, def: &WorkflowDefinition) {
        let mut triggers = self.triggers.write().await;

        // Only one active trigger version is allowed per immutable definition id.
        triggers.remove_definition(&def.id);

        for trigger_def in def.trigger_defs() {
            let next_run = match &trigger_def.trigger_type {
                TriggerType::Schedule { cron } => {
                    // Try to resume from the last persisted run time so that
                    // missed cron runs during downtime fire immediately.
                    match self.store.get_cron_last_run(&def.id, &def.version, cron) {
                        Ok(Some(last_run)) => compute_next_cron_run_after(cron, last_run)
                            .or_else(|| compute_next_cron_run(cron)),
                        _ => compute_next_cron_run(cron),
                    }
                }
                _ => None,
            };

            triggers.push(ActiveTrigger {
                definition_id: def.id.clone(),
                definition_name: def.name.clone(),
                definition_version: def.version.clone(),
                trigger_type: trigger_def.trigger_type.clone(),
                next_run_ms: next_run,
            });

            info!(
                "Registered trigger for {} v{}: {:?}",
                def.name,
                def.version,
                trigger_type_label(&trigger_def.trigger_type)
            );
        }
    }

    /// Unregister triggers for a definition id (optionally scoped to a specific version).
    pub async fn unregister_definition(&self, definition_id: &str, version: Option<&str>) {
        let mut triggers = self.triggers.write().await;
        if let Some(version) = version {
            triggers.remove_definition_version(definition_id, version);
            info!("Unregistered triggers for definition {} v{}", definition_id, version);
        } else {
            triggers.remove_definition(definition_id);
            info!("Unregistered all triggers for definition {}", definition_id);
        }

        // Clean up persisted cron state.
        if let Err(e) = self.store.delete_cron_state(definition_id, version) {
            warn!(
                error = %e,
                definition_id,
                version = version.unwrap_or("*"),
                "failed to clean up cron state"
            );
        }
    }

    /// Start the background evaluation loop. Subscribes to event bus and
    /// periodically checks cron triggers.
    pub async fn start(self: &Arc<Self>) {
        if self
            .running
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            return; // already running
        }

        // Replay events that were persisted to EventLog but may have been
        // missed during the restart window (between shutdown and now).
        self.replay_missed_events().await;

        // Spawn event listener task with a bounded queue to prevent unbounded
        // memory growth if event evaluation falls behind.
        let this = Arc::clone(self);
        let mut event_rx = self.event_bus.subscribe_queued_bounded("", 10_000);
        let event_handle = tokio::spawn(
            async move {
                tracing::info!("trigger manager event listener started");
                while this.running.load(std::sync::atomic::Ordering::SeqCst) {
                    match event_rx.recv().await {
                        Some(envelope) => {
                            this.evaluate_event(&envelope.topic, &envelope.payload).await;
                        }
                        None => break,
                    }
                }
            }
            .instrument(tracing::info_span!("service", service = "trigger-manager")),
        );

        // Spawn cron ticker task
        let this = Arc::clone(self);
        let cron_handle = tokio::spawn(
            async move {
                tracing::info!("trigger manager cron ticker started");
                let mut prune_counter: u32 = 0;
                while this.running.load(std::sync::atomic::Ordering::SeqCst) {
                    this.tick_cron().await;
                    // Prune stale dedup entries every ~10 minutes
                    prune_counter += 1;
                    if prune_counter >= 600 {
                        prune_counter = 0;
                        let week_ms = 7 * 24 * 3600 * 1000;
                        if let Err(e) = this.store.prune_trigger_dedup(week_ms) {
                            warn!(error = %e, "failed to prune trigger dedup table");
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            .instrument(tracing::info_span!("service", service = "trigger-manager")),
        );

        // Store handles so stop() can find them.
        *self.event_task.lock().await = Some(event_handle);
        *self.cron_task.lock().await = Some(cron_handle);

        info!("TriggerManager started");
    }

    /// Stop the background loop and abort spawned tasks.
    pub async fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.event_task.lock().await.take() {
            handle.abort();
        }
        if let Some(handle) = self.cron_task.lock().await.take() {
            handle.abort();
        }
        info!("TriggerManager stopped");
    }

    /// Returns `true` if the trigger manager background loops are running.
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Return a serializable snapshot of all active triggers and event gates.
    pub async fn list_active(&self) -> ActiveTriggersResponse {
        let triggers = self.triggers.read().await;
        let gates = self.event_gates.read().await;

        ActiveTriggersResponse {
            triggers: triggers
                .iter_all()
                .map(|t| ActiveTriggerSnapshot {
                    definition_name: t.definition_name.clone(),
                    definition_version: t.definition_version.clone(),
                    trigger_kind: trigger_type_label(&t.trigger_type).to_string(),
                    trigger_type: t.trigger_type.clone(),
                    next_run_ms: t.next_run_ms,
                })
                .collect(),
            event_gates: gates
                .iter()
                .map(|g| ActiveEventGateSnapshot {
                    subscription_id: g.subscription_id.clone(),
                    instance_id: g.instance_id,
                    step_id: g.step_id.clone(),
                    topic: g.topic.clone(),
                    filter: g.filter.clone(),
                    expires_at_ms: g.expires_at_ms,
                })
                .collect(),
        }
    }

    /// Evaluate an incoming event against registered event-pattern and
    /// MCP notification triggers.
    async fn evaluate_event(&self, topic: &str, payload: &Value) {
        let triggers = self.triggers.read().await;
        // (definition_id, definition_name, version, inputs, mark_as_read_info)
        // mark_as_read_info: Option<(channel_id, external_id)>
        #[allow(clippy::type_complexity)]
        let mut to_launch: Vec<(String, String, String, Value, Option<(String, String)>)> =
            Vec::new();

        // --- EventPattern triggers: exact-topic O(1) lookup, then wildcard scan ---
        if let Some(exact_triggers) = triggers.event_exact.get(topic) {
            for trigger in exact_triggers {
                if let TriggerType::EventPattern { filter, .. } = &trigger.trigger_type {
                    if filter_matches(payload, filter.as_deref()) {
                        to_launch.push((
                            trigger.definition_id.clone(),
                            trigger.definition_name.clone(),
                            trigger.definition_version.clone(),
                            payload.clone(),
                            None,
                        ));
                    }
                }
            }
        }
        for trigger in &triggers.event_wildcard {
            if let TriggerType::EventPattern { topic: pattern, filter } = &trigger.trigger_type {
                if topic_matches(topic, pattern) && filter_matches(payload, filter.as_deref()) {
                    to_launch.push((
                        trigger.definition_id.clone(),
                        trigger.definition_name.clone(),
                        trigger.definition_version.clone(),
                        payload.clone(),
                        None,
                    ));
                }
            }
        }

        // --- MCP notification triggers ---
        if topic.starts_with("mcp.notification") {
            for trigger in &triggers.mcp_notification {
                if let TriggerType::McpNotification { server_id, kind } = &trigger.trigger_type {
                    if payload_matches_mcp(payload, server_id, kind.as_deref()) {
                        to_launch.push((
                            trigger.definition_id.clone(),
                            trigger.definition_name.clone(),
                            trigger.definition_version.clone(),
                            payload.clone(),
                            None,
                        ));
                    }
                }
            }
        }

        // --- Incoming message triggers ---
        if topic.starts_with("comm.message.received") {
            let payload_channel = payload.get("channel_id").and_then(|v| v.as_str());
            info!(
                topic,
                payload_channel_id = ?payload_channel,
                registered_incoming_triggers = triggers.incoming_message.len(),
                "evaluating incoming message event"
            );
            for trigger in &triggers.incoming_message {
                if let TriggerType::IncomingMessage {
                    channel_id,
                    listen_channel_id,
                    filter,
                    from_filter,
                    subject_filter,
                    body_filter,
                    mark_as_read,
                    ignore_replies,
                } = &trigger.trigger_type
                {
                    let matched = payload_matches_incoming(
                        payload,
                        channel_id,
                        listen_channel_id.as_deref(),
                        filter.as_deref(),
                        from_filter.as_deref(),
                        subject_filter.as_deref(),
                        body_filter.as_deref(),
                        *ignore_replies,
                    );
                    info!(
                        definition = %trigger.definition_name,
                        trigger_channel_id = %channel_id,
                        ignore_replies,
                        listen_channel_id = ?listen_channel_id,
                        from_filter = ?from_filter,
                        subject_filter = ?subject_filter,
                        body_filter = ?body_filter,
                        filter = ?filter,
                        matched,
                        "incoming message trigger evaluation"
                    );
                    if matched {
                        // Persistent dedup: skip if we already triggered this
                        // workflow for the same external_id.
                        if let Some(ext_id) = payload.get("external_id").and_then(|v| v.as_str()) {
                            match self.store.is_trigger_seen(&trigger.definition_id, ext_id) {
                                Ok(true) => {
                                    info!(
                                        definition_id = %trigger.definition_id,
                                        definition = %trigger.definition_name,
                                        external_id = ext_id,
                                        "skipping duplicate incoming message trigger (already seen)"
                                    );
                                    continue;
                                }
                                Err(e) => {
                                    warn!(error = %e, "trigger dedup check failed, proceeding");
                                }
                                _ => {
                                    info!(
                                        definition = %trigger.definition_name,
                                        external_id = ext_id,
                                        "trigger dedup: first time seeing this message"
                                    );
                                }
                            }
                        } else {
                            warn!(
                                definition = %trigger.definition_name,
                                "matched trigger has no external_id — dedup skipped"
                            );
                        }

                        let mark_info = if *mark_as_read {
                            payload
                                .get("external_id")
                                .and_then(|v| v.as_str())
                                .map(|eid| (channel_id.clone(), eid.to_string()))
                        } else {
                            None
                        };

                        to_launch.push((
                            trigger.definition_id.clone(),
                            trigger.definition_name.clone(),
                            trigger.definition_version.clone(),
                            payload.clone(),
                            mark_info,
                        ));
                    }
                }
            }
        }

        drop(triggers);

        if !to_launch.is_empty() {
            info!(
                topic,
                matched_triggers = to_launch.len(),
                "trigger evaluation matched — launching workflows"
            );
        }

        // Check active event gates
        let mut matched_gates = Vec::new();
        {
            let mut gates = self.event_gates.write().await;
            gates.retain(|gate| {
                if topic_matches(topic, &gate.topic)
                    && filter_matches(payload, gate.filter.as_deref())
                {
                    matched_gates.push((gate.instance_id, gate.step_id.clone(), payload.clone()));
                    false // remove matched gate
                } else {
                    true // keep
                }
            });
        }

        // Launch workflows first, then persist dedup and mark_as_read only
        // for successful launches. This prevents permanently suppressing a
        // trigger when the launch itself fails.
        for (definition_id, name, version, inputs, mark_info) in to_launch {
            let launched = self.auto_launch(&name, Some(&version), inputs.clone()).await;

            if launched {
                if let Some(ext_id) = inputs.get("external_id").and_then(|v| v.as_str()) {
                    if let Err(e) = self.store.mark_trigger_seen(&definition_id, ext_id) {
                        warn!(error = %e, "failed to record trigger dedup");
                    }
                }
                if let Some((channel_id, external_id)) = mark_info {
                    let connector_svc = self.connector_service.lock().await.clone();
                    if let Some(ref connector_svc) = connector_svc {
                        if let Err(e) =
                            connector_svc.mark_message_seen(&channel_id, &external_id).await
                        {
                            warn!(error = %e, channel_id, external_id, "failed to mark message as read");
                        }
                    }
                }
            }
        }

        for (instance_id, step_id, event_data) in matched_gates {
            self.resolve_event_gate(instance_id, &step_id, event_data).await;
        }
    }

    /// Check and fire any due cron triggers.
    pub async fn tick_cron(&self) {
        const MAX_CATCHUP_RUNS_PER_TICK: usize = 256;

        #[derive(Debug)]
        struct CronLaunchPlan {
            definition_id: String,
            definition_name: String,
            definition_version: String,
            cron: String,
            due_runs: Vec<u64>,
            next_after_catchup: Option<u64>,
        }

        #[derive(Debug)]
        struct CronExecutionResult {
            definition_id: String,
            definition_version: String,
            cron: String,
            next_run_ms: Option<u64>,
        }

        let now = now_ms();
        let mut plans = Vec::new();

        {
            let mut triggers = self.triggers.write().await;
            for trigger in triggers.schedules.iter_mut() {
                let TriggerType::Schedule { cron } = &trigger.trigger_type else {
                    continue;
                };

                if trigger.next_run_ms.is_none() {
                    trigger.next_run_ms = compute_next_cron_run(cron);
                }

                let mut cursor = trigger.next_run_ms;
                let mut due_runs = Vec::new();
                while let Some(next_ms) = cursor {
                    if now < next_ms {
                        break;
                    }
                    due_runs.push(next_ms);
                    if due_runs.len() >= MAX_CATCHUP_RUNS_PER_TICK {
                        cursor = compute_next_cron_run_after(cron, next_ms);
                        warn!(
                            definition = %trigger.definition_name,
                            version = %trigger.definition_version,
                            cron = %cron,
                            max = MAX_CATCHUP_RUNS_PER_TICK,
                            "cron catch-up capped for this tick; remaining overdue runs will continue next tick"
                        );
                        break;
                    }
                    cursor = compute_next_cron_run_after(cron, next_ms);
                }

                if due_runs.is_empty() {
                    continue;
                }

                plans.push(CronLaunchPlan {
                    definition_id: trigger.definition_id.clone(),
                    definition_name: trigger.definition_name.clone(),
                    definition_version: trigger.definition_version.clone(),
                    cron: cron.clone(),
                    due_runs,
                    next_after_catchup: cursor,
                });
            }
        }

        let mut results = Vec::new();
        for plan in plans {
            let mut last_successful_due = None;
            let mut failed_due = None;

            for due_at in &plan.due_runs {
                let launched = self
                    .auto_launch(
                        &plan.definition_name,
                        Some(&plan.definition_version),
                        json!({"trigger": "cron", "scheduled_for_ms": due_at}),
                    )
                    .await;

                if launched {
                    last_successful_due = Some(*due_at);
                } else {
                    failed_due = Some(*due_at);
                    warn!(
                        definition = %plan.definition_name,
                        version = %plan.definition_version,
                        cron = %plan.cron,
                        scheduled_for_ms = *due_at,
                        "cron launch failed; keeping schedule pinned to failed occurrence for retry"
                    );
                    break;
                }
            }

            if let Some(last_run_ms) = last_successful_due {
                if let Err(e) = self.store.set_cron_last_run(
                    &plan.definition_id,
                    &plan.definition_version,
                    &plan.cron,
                    last_run_ms,
                ) {
                    warn!(error = %e, "failed to persist cron last_run_ms");
                }
            }

            results.push(CronExecutionResult {
                definition_id: plan.definition_id,
                definition_version: plan.definition_version,
                cron: plan.cron,
                next_run_ms: failed_due.or(plan.next_after_catchup),
            });
        }

        if !results.is_empty() {
            let mut triggers = self.triggers.write().await;
            for result in results {
                for trigger in triggers.schedules.iter_mut() {
                    let TriggerType::Schedule { cron } = &trigger.trigger_type else {
                        continue;
                    };
                    if trigger.definition_id == result.definition_id
                        && trigger.definition_version == result.definition_version
                        && cron == &result.cron
                    {
                        trigger.next_run_ms = result.next_run_ms;
                    }
                }
            }
        }

        // Also check for expired event gates
        self.tick_event_gate_timeouts().await;
    }

    /// Replay events from the persistent EventLog using a persisted cursor.
    /// Falls back to a short lookback only when no cursor has been stored yet.
    pub async fn replay_missed_events(&self) {
        let log = self.event_log.lock().await.clone();
        let Some(log) = log else {
            debug!("no EventLog available, skipping event replay");
            return;
        };

        const INITIAL_LOOKBACK_MS: u64 = 5 * 60 * 1000;
        const REPLAY_BATCH_SIZE: usize = 1000;

        let cursor = match self.store.get_event_replay_cursor() {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "failed to read replay cursor; using initial lookback");
                None
            }
        };

        let startup_now = now_ms();
        let since_ms = cursor.unwrap_or_else(|| startup_now.saturating_sub(INITIAL_LOOKBACK_MS));

        let mut before_id: Option<i64> = None;
        let mut replay_events = Vec::new();
        let mut max_seen_cursor = since_ms;

        loop {
            let events = log.query_events(
                None,
                Some(since_ms as u128),
                before_id,
                None,
                Some(REPLAY_BATCH_SIZE),
            );
            if events.is_empty() {
                break;
            }

            before_id = events.last().map(|e| e.id);
            replay_events.extend(events);
        }

        replay_events.sort_by_key(|e| e.id);
        let replayed = replay_events.len();
        let comm_events = replay_events.iter().filter(|e| e.topic.starts_with("comm.message.received")).count();
        if replayed > 0 {
            info!(replayed, comm_events, since_ms, "replaying missed events");
        }
        for event in replay_events {
            self.evaluate_event(&event.topic, &event.payload).await;
            let event_ts = event.timestamp_ms.min(u64::MAX as u128) as u64;
            max_seen_cursor = max_seen_cursor.max(event_ts.saturating_add(1));
        }

        if replayed > 0 {
            info!(replayed, since_ms, "event replay complete");
        } else {
            debug!("no replayable events found");
        }

        let cursor_to_persist = if replayed > 0 {
            max_seen_cursor
        } else if cursor.is_none() {
            startup_now
        } else {
            since_ms
        };

        if let Err(e) = self.store.set_event_replay_cursor(cursor_to_persist) {
            warn!(error = %e, "failed to persist replay cursor");
        }
    }

    /// Launch a workflow instance from a trigger with retry on transient failures.
    /// Returns `true` if the launch eventually succeeded.
    async fn auto_launch(&self, name: &str, version: Option<&str>, inputs: Value) -> bool {
        const MAX_ATTEMPTS: u32 = 3;
        const BASE_DELAY_MS: u64 = 1000;

        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let delay = BASE_DELAY_MS * (1 << (attempt - 1)); // 1s, 2s
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }

            let svc_guard = self.workflow_service.lock().await;
            let Some(weak) = svc_guard.as_ref() else {
                warn!("TriggerManager: cannot launch workflow — service not set");
                return false;
            };
            let Some(svc) = weak.upgrade() else {
                warn!("TriggerManager: WorkflowService dropped");
                return false;
            };
            drop(svc_guard);

            match svc
                .launch(name, version, inputs.clone(), "trigger-manager", None, None, None, None)
                .await
            {
                Ok(id) => {
                    info!("Trigger auto-launched workflow {} → instance {}", name, id);
                    let _ = self.event_bus.publish(
                        "workflow.trigger.fired",
                        "trigger-manager",
                        json!({
                            "definition": name,
                            "instance_id": id,
                        }),
                    );
                    return true;
                }
                Err(e) => {
                    if attempt + 1 < MAX_ATTEMPTS {
                        warn!(
                            attempt = attempt + 1,
                            max = MAX_ATTEMPTS,
                            "Trigger launch attempt for {} failed, retrying: {}",
                            name,
                            e
                        );
                    } else {
                        error!(
                            "Trigger failed to launch workflow {} after {} attempts: {}",
                            name, MAX_ATTEMPTS, e
                        );
                    }
                }
            }
        }
        false
    }

    /// Resolve an event gate by forwarding the event data to the workflow engine.
    async fn resolve_event_gate(&self, instance_id: i64, step_id: &str, event_data: Value) {
        let svc_guard = self.workflow_service.lock().await;
        let Some(weak) = svc_guard.as_ref() else {
            warn!("TriggerManager: cannot resolve event gate — service not set");
            return;
        };
        let Some(svc) = weak.upgrade() else {
            warn!("TriggerManager: WorkflowService dropped");
            return;
        };
        drop(svc_guard);

        match svc.respond_to_event(instance_id, step_id, event_data).await {
            Ok(()) => {
                info!("Event gate resolved: instance={} step={}", instance_id, step_id);
            }
            Err(e) => {
                error!(
                    "Failed to resolve event gate: instance={} step={}: {}",
                    instance_id, step_id, e
                );
            }
        }
    }

    /// Check for expired event gates and fail them.
    async fn tick_event_gate_timeouts(&self) {
        let now = now_ms();
        let mut expired = Vec::new();
        {
            let mut gates = self.event_gates.write().await;
            gates.retain(|gate| {
                if let Some(expires_at) = gate.expires_at_ms {
                    if now >= expires_at {
                        expired.push((gate.instance_id, gate.step_id.clone()));
                        return false;
                    }
                }
                true
            });
        }

        if expired.is_empty() {
            return;
        }

        let svc_guard = self.workflow_service.lock().await;
        let Some(weak) = svc_guard.as_ref() else {
            return;
        };
        let Some(svc) = weak.upgrade() else {
            warn!("TriggerManager: WorkflowService dropped");
            return;
        };
        drop(svc_guard);

        for (instance_id, step_id) in expired {
            warn!("Event gate timed out: instance={} step={}", instance_id, step_id);
            let timeout_data =
                json!({ "error": "event_gate_timeout", "message": "Event gate timed out" });
            if let Err(e) = svc.respond_to_event(instance_id, &step_id, timeout_data).await {
                error!(
                    "Failed to timeout event gate: instance={} step={}: {}",
                    instance_id, step_id, e
                );
            }
        }
    }
}

#[async_trait::async_trait]
impl WorkflowEventGateRegistrar for TriggerManager {
    async fn register_event_gate(
        &self,
        instance_id: i64,
        step_id: &str,
        topic: &str,
        filter: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<String, String> {
        let subscription_id = uuid::Uuid::new_v4().to_string();
        let expires_at_ms =
            timeout_secs.map(|secs| now_ms().saturating_add(secs.saturating_mul(1000)));

        let gate = ActiveEventGate {
            subscription_id: subscription_id.clone(),
            instance_id,
            step_id: step_id.to_string(),
            topic: topic.to_string(),
            filter: filter.map(|f| f.to_string()),
            expires_at_ms,
        };

        self.event_gates.write().await.push(gate);
        info!(
            "Registered event gate: instance={} step={} topic={} filter={:?}",
            instance_id, step_id, topic, filter
        );
        Ok(subscription_id)
    }

    async fn unregister_instance_gates(&self, instance_id: i64) {
        let mut gates = self.event_gates.write().await;
        let before = gates.len();
        gates.retain(|g| g.instance_id != instance_id);
        let removed = before - gates.len();
        if removed > 0 {
            info!("Unregistered {} event gate(s) for instance {}", removed, instance_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

impl TriggerManager {
    /// Re-register event gate subscriptions for instances that were waiting on
    /// events when the daemon last shut down. Called during recovery to restore
    /// the in-memory subscription state from the persisted DB.
    pub async fn recover_event_gates(&self) {
        let filter =
            InstanceFilter { statuses: vec![WorkflowStatus::WaitingOnEvent], ..Default::default() };
        let instances = match self.store.list_instances(&filter) {
            Ok(result) => result.items,
            Err(e) => {
                warn!("Failed to query WaitingOnEvent instances for recovery: {e}");
                return;
            }
        };

        let mut recovered = 0u32;
        for summary in &instances {
            let instance = match self.store.get_instance(summary.id) {
                Ok(Some(inst)) => inst,
                _ => continue,
            };

            for (step_id, state) in &instance.step_states {
                if state.status != StepStatus::WaitingOnEvent {
                    continue;
                }

                // Find the step definition to extract topic/filter/timeout
                let step_def = instance.definition.steps.iter().find(|s| s.id == *step_id);
                let (topic, filter_expr, timeout_secs) = match step_def {
                    Some(s) => match &s.step_type {
                        StepType::Task {
                            task: TaskDef::EventGate { topic, filter, timeout_secs },
                        } => (topic.clone(), filter.clone(), *timeout_secs),
                        _ => continue,
                    },
                    None => continue,
                };

                let subscription_id = state
                    .interaction_request_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

                // Use the persisted deadline from StepState instead of
                // recomputing, so recovered gates keep their original timeout.
                let expires_at_ms = state.resume_at_ms.or_else(|| {
                    // Fallback for instances created before this fix
                    timeout_secs.map(|secs| now_ms().saturating_add(secs.saturating_mul(1000)))
                });

                // Skip gates whose timeout has already elapsed
                if let Some(expires) = expires_at_ms {
                    if expires <= now_ms() {
                        info!(
                            instance_id = %instance.id,
                            step_id = %step_id,
                            "Skipping expired event gate during recovery"
                        );
                        continue;
                    }
                }

                let gate = ActiveEventGate {
                    subscription_id,
                    instance_id: instance.id,
                    step_id: step_id.clone(),
                    topic,
                    filter: filter_expr,
                    expires_at_ms,
                };

                self.event_gates.write().await.push(gate);
                recovered += 1;
                info!(
                    instance_id = %instance.id,
                    step_id = %step_id,
                    "Recovered event gate subscription"
                );
            }
        }

        if recovered > 0 {
            info!("Recovered {recovered} event gate subscription(s)");
        }
    }
}

// ---------------------------------------------------------------------------
// Public snapshot types for the list_active() API
// ---------------------------------------------------------------------------

/// Serializable snapshot of an active trigger registration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveTriggerSnapshot {
    pub definition_name: String,
    pub definition_version: String,
    pub trigger_type: TriggerType,
    pub trigger_kind: String,
    pub next_run_ms: Option<u64>,
}

/// Serializable snapshot of an active event gate subscription.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveEventGateSnapshot {
    pub subscription_id: String,
    pub instance_id: i64,
    pub step_id: String,
    pub topic: String,
    pub filter: Option<String>,
    pub expires_at_ms: Option<u64>,
}

/// Combined response for the active triggers API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveTriggersResponse {
    pub triggers: Vec<ActiveTriggerSnapshot>,
    pub event_gates: Vec<ActiveEventGateSnapshot>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn compute_next_cron_run(expression: &str) -> Option<u64> {
    let normalized = hive_workflow::normalize_cron(expression);
    let schedule = Schedule::from_str(&normalized).ok()?;
    let next = schedule.upcoming(Utc).next()?;
    Some(next.timestamp_millis() as u64)
}

/// Compute the next cron run time after a given timestamp (ms since epoch).
/// Used to catch up missed runs: if the result is in the past, the caller
/// should fire the trigger immediately.
fn compute_next_cron_run_after(expression: &str, after_ms: u64) -> Option<u64> {
    use chrono::TimeZone;
    let normalized = hive_workflow::normalize_cron(expression);
    let schedule = Schedule::from_str(&normalized).ok()?;
    let after = Utc.timestamp_millis_opt(after_ms as i64).single()?;
    let next = schedule.after(&after).next()?;
    Some(next.timestamp_millis() as u64)
}

fn topic_matches(event_topic: &str, pattern: &str) -> bool {
    if pattern.contains('*') {
        // Wildcard matching: split pattern on '*' segments
        // e.g. "chat.session.*" matches "chat.session.created"
        // e.g. "*.completed" matches "chat.message.completed"
        let segments: Vec<&str> = pattern.split('*').collect();
        let mut pos = 0;
        for (i, seg) in segments.iter().enumerate() {
            if seg.is_empty() {
                continue;
            }
            match event_topic[pos..].find(seg) {
                Some(idx) => {
                    // First segment must match at start
                    if i == 0 && idx != 0 {
                        return false;
                    }
                    pos += idx + seg.len();
                }
                None => return false,
            }
        }
        // If pattern doesn't end with *, remaining topic must be consumed
        if !pattern.ends_with('*') {
            return pos == event_topic.len();
        }
        true
    } else {
        // Exact match or prefix match (e.g. "chat.session" matches "chat.session.created")
        event_topic == pattern
            || (event_topic.starts_with(pattern)
                && event_topic.as_bytes().get(pattern.len()) == Some(&b'.'))
    }
}

/// Evaluate an event filter expression against a payload.
///
/// Syntax:
///   Single condition:  `event.path <op> value`
///   Quoted values:     `event.path == "hello world"`
///   Logical AND:       `event.a == x && event.b == y`
///   Logical OR:        `event.a == x || event.b == y`
///
/// Operators: `==`, `!=`, `<`, `>`, `<=`, `>=`
///
/// The left-hand side is a dot-path into the event payload (the `event.`
/// prefix is optional). The right-hand side is a literal value that may
/// be wrapped in double quotes to preserve whitespace.
///
/// Falls back to substring match when no operator is detected.
fn filter_matches(payload: &Value, filter: Option<&str>) -> bool {
    match filter {
        None | Some("") => true,
        Some(f) => {
            // Try logical operators first (split outside of quoted strings)
            if let Some((left, right)) = split_logical_outside_quotes(f, "||") {
                return filter_matches(payload, Some(left.trim()))
                    || filter_matches(payload, Some(right.trim()));
            }
            if let Some((left, right)) = split_logical_outside_quotes(f, "&&") {
                return filter_matches(payload, Some(left.trim()))
                    && filter_matches(payload, Some(right.trim()));
            }

            // Try comparison operators
            static OPS: &[&str] = &["!=", "<=", ">=", "==", "<", ">"];
            if let Some((op, lhs, rhs)) = OPS.iter().find_map(|op| {
                split_comparison_outside_quotes(f, op).map(|(l, r)| (*op, l.trim(), r.trim()))
            }) {
                let lhs_val = resolve_event_path(lhs, payload);
                let rhs_val = strip_quotes(rhs).to_string();
                match op {
                    "==" => lhs_val == rhs_val,
                    "!=" => lhs_val != rhs_val,
                    "<" | ">" | "<=" | ">=" => {
                        match (lhs_val.parse::<f64>(), rhs_val.parse::<f64>()) {
                            (Ok(l), Ok(r)) => match op {
                                "<" => l < r,
                                ">" => l > r,
                                "<=" => l <= r,
                                ">=" => l >= r,
                                _ => false,
                            },
                            _ => false,
                        }
                    }
                    _ => false,
                }
            } else {
                // Fallback: simple substring match on serialized payload
                let s = payload.to_string();
                s.contains(f)
            }
        }
    }
}

/// Split on a logical operator (`&&` or `||`) but only outside of quoted strings.
fn split_logical_outside_quotes<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if !in_quotes && i + op_len <= bytes.len() && &bytes[i..i + op_len] == op_bytes {
            return Some((&expr[..i], &expr[i + op_len..]));
        }
        i += 1;
    }
    None
}

/// Split on a comparison operator but only outside of quoted strings.
/// For single-char ops (`<`, `>`), avoids matching two-char ops (`<=`, `>=`, `!=`).
fn split_comparison_outside_quotes<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if !in_quotes && i + op_len <= bytes.len() && &bytes[i..i + op_len] == op_bytes {
            if op_len == 1 {
                if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                    i += 1;
                    continue;
                }
                if op == "<" && i > 0 && bytes[i - 1] == b'!' {
                    i += 1;
                    continue;
                }
            }
            return Some((&expr[..i], &expr[i + op_len..]));
        }
        i += 1;
    }
    None
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

/// Resolve a dot-path like `event.data.type` into the payload.
/// The `event.` prefix is optional and stripped if present.
fn resolve_event_path(path: &str, payload: &Value) -> String {
    let path = path.strip_prefix("event.").unwrap_or(path);
    let mut current = payload.clone();
    for segment in path.split('.') {
        current = match current {
            Value::Object(ref map) => map.get(segment).cloned().unwrap_or(Value::Null),
            Value::Array(ref arr) => segment
                .parse::<usize>()
                .ok()
                .and_then(|i| arr.get(i).cloned())
                .unwrap_or(Value::Null),
            _ => Value::Null,
        };
    }
    match current {
        Value::String(s) => s,
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(&current).unwrap_or_default(),
    }
}

fn payload_matches_mcp(payload: &Value, server_id: &str, kind: Option<&str>) -> bool {
    let sid = payload.get("server_id").and_then(|v| v.as_str());
    if sid != Some(server_id) {
        return false;
    }
    match kind {
        None => true,
        Some(k) => payload.get("kind").and_then(|v| v.as_str()) == Some(k),
    }
}

#[allow(clippy::too_many_arguments)]
fn payload_matches_incoming(
    payload: &Value,
    channel_id: &str,
    listen_channel_id: Option<&str>,
    filter: Option<&str>,
    from_filter: Option<&str>,
    subject_filter: Option<&str>,
    body_filter: Option<&str>,
    ignore_replies: bool,
) -> bool {
    let cid = payload.get("channel_id").and_then(|v| v.as_str());
    if cid != Some(channel_id) {
        debug!(payload_channel = ?cid, trigger_channel = channel_id, "rejected: channel_id mismatch");
        return false;
    }
    // When ignore_replies is set, skip messages that are replies.
    // Detected via provider-specific metadata keys:
    //   Discord: referenced_message_id
    //   Slack: thread_ts
    //   Email (IMAP/Gmail/Microsoft): in_reply_to
    // Note: `references` alone is NOT used — some providers (e.g. Microsoft
    // Graph) set it for conversation threading even on new/original emails.
    if ignore_replies {
        if let Some(meta) = payload.get("metadata").and_then(|m| m.as_object()) {
            // Helper: key exists and has a non-empty string value.
            let has = |key: &str| {
                meta.get(key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
            };
            let is_reply = has("referenced_message_id")
                || has("thread_ts")
                || has("in_reply_to");
            if is_reply {
                info!(
                    has_referenced_message_id = has("referenced_message_id"),
                    has_thread_ts = has("thread_ts"),
                    has_in_reply_to = has("in_reply_to"),
                    "rejected: message is a reply (ignore_replies=true)"
                );
                return false;
            }
        }
    }
    // Filter by specific channel within the connector (e.g. Slack/Discord channel)
    if let Some(lc) = listen_channel_id {
        if !lc.is_empty() {
            let msg_channel = payload
                .get("metadata")
                .and_then(|m| m.get("channel_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if msg_channel != lc {
                info!(listen_channel_id = lc, msg_channel, "rejected: listen_channel_id mismatch");
                return false;
            }
        }
    }
    // Structured field filters (case-insensitive substring)
    if let Some(f) = from_filter {
        if !f.is_empty() {
            let from = payload.get("from").and_then(|v| v.as_str()).unwrap_or("");
            if !from.to_lowercase().contains(&f.to_lowercase()) {
                info!(from_filter = f, from, "rejected: from_filter mismatch");
                return false;
            }
        }
    }
    if let Some(f) = subject_filter {
        if !f.is_empty() {
            let subject = payload.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            if !subject.to_lowercase().contains(&f.to_lowercase()) {
                info!(subject_filter = f, subject, "rejected: subject_filter mismatch");
                return false;
            }
        }
    }
    if let Some(f) = body_filter {
        if !f.is_empty() {
            let body = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");
            if !body.to_lowercase().contains(&f.to_lowercase()) {
                info!(body_filter = f, "rejected: body_filter mismatch");
                return false;
            }
        }
    }
    // Legacy generic filter (substring on full payload)
    let result = filter_matches(payload, filter);
    if !result {
        info!(filter = ?filter, "rejected: legacy filter mismatch");
    }
    result
}

fn trigger_type_label(tt: &TriggerType) -> &'static str {
    match tt {
        TriggerType::Manual { .. } => "manual",
        TriggerType::IncomingMessage { .. } => "incoming_message",
        TriggerType::EventPattern { .. } => "event_pattern",
        TriggerType::McpNotification { .. } => "mcp_notification",
        TriggerType::Schedule { .. } => "schedule",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::{EventLog, QueuedSubscriber};
    use hive_workflow::store::WorkflowStore;
    use tokio::time::{sleep, Duration};

    fn trigger_definition(
        id: &str,
        name: &str,
        version: &str,
        trigger_type: TriggerType,
    ) -> WorkflowDefinition {
        WorkflowDefinition {
            id: id.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            description: None,
            variables: serde_json::json!({"type":"object","properties":{}}),
            steps: vec![
                hive_workflow::types::StepDef {
                    id: "start".to_string(),
                    step_type: StepType::Trigger {
                        trigger: hive_workflow::types::TriggerDef { trigger_type },
                    },
                    outputs: std::collections::HashMap::new(),
                    on_error: None,
                    next: vec!["end".to_string()],
                    timeout_secs: None,
                },
                hive_workflow::types::StepDef {
                    id: "end".to_string(),
                    step_type: StepType::ControlFlow {
                        control: hive_workflow::types::ControlFlowDef::EndWorkflow,
                    },
                    outputs: std::collections::HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                },
            ],
            output: None,
            result_message: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            mode: hive_workflow::types::WorkflowMode::default(),
            bundled: false,
            archived: false,
            triggers_paused: false,
        }
    }

    #[tokio::test]
    async fn test_register_definition_replaces_by_definition_id() {
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus, store);

        let def_v1 = trigger_definition(
            "def-1",
            "user/workflow-a",
            "1.0",
            TriggerType::Manual { inputs: vec![], input_schema: None },
        );
        let def_v2 = trigger_definition(
            "def-1",
            "user/workflow-renamed",
            "2.0",
            TriggerType::Manual { inputs: vec![], input_schema: None },
        );

        tm.register_definition(&def_v1).await;
        tm.register_definition(&def_v2).await;

        let guard = tm.triggers.read().await;
        let active: Vec<_> = guard.iter_all().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].definition_id, "def-1");
        assert_eq!(active[0].definition_name, "user/workflow-renamed");
        assert_eq!(active[0].definition_version, "2.0");
    }

    #[tokio::test]
    async fn test_replay_uses_persisted_cursor() {
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus.clone(), Arc::clone(&store));
        let log = Arc::new(EventLog::in_memory().unwrap());
        bus.register_subscriber(Arc::clone(&log) as Arc<dyn QueuedSubscriber>);
        tm.set_event_log(Arc::clone(&log)).await;

        bus.publish("test.topic", "test", json!({"value": 1})).unwrap();
        sleep(Duration::from_millis(100)).await;

        tm.replay_missed_events().await;
        let first_cursor = store.get_event_replay_cursor().unwrap().unwrap();
        assert!(first_cursor > 0);

        let first_event = log.query_events(None, None, None, None, Some(1));
        let event_ts = first_event[0].timestamp_ms as u64;
        let pinned_cursor = event_ts.saturating_add(1);
        store.set_event_replay_cursor(pinned_cursor).unwrap();

        tm.replay_missed_events().await;
        assert_eq!(store.get_event_replay_cursor().unwrap(), Some(pinned_cursor));
    }

    #[tokio::test]
    async fn test_cron_catchup_persists_last_successful_due_run() {
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus.clone(), Arc::clone(&store));
        let svc = Arc::new(super::WorkflowService::in_memory().unwrap());

        tm.set_workflow_service(Arc::clone(&svc)).await;

        let def = trigger_definition(
            "def-cron",
            "user/cron-workflow",
            "1.0",
            TriggerType::Schedule { cron: "*/1 * * * * *".to_string() },
        );
        let yaml = serde_yaml::to_string(&def).unwrap();
        svc.save_definition(&yaml).await.unwrap();
        tm.register_definition(&def).await;

        {
            let mut guard = tm.triggers.write().await;
            let trig = guard.schedules.iter_mut().find(|t| t.definition_id == "def-cron").unwrap();
            trig.next_run_ms = Some(now_ms().saturating_sub(2500));
        }

        tm.tick_cron().await;

        let persisted = store
            .get_cron_last_run("def-cron", "1.0", "*/1 * * * * *")
            .unwrap()
            .expect("cron last_run_ms should be persisted");
        assert!(persisted <= now_ms());

        let guard = tm.triggers.read().await;
        let trig = guard.schedules.iter().find(|t| t.definition_id == "def-cron").unwrap();
        assert!(trig.next_run_ms.is_some());
        assert!(trig.next_run_ms.unwrap() >= persisted);
    }

    #[test]
    fn test_topic_matches_exact() {
        assert!(topic_matches("chat.session.created", "chat.session.created"));
        assert!(!topic_matches("chat.session.created", "chat.session.resumed"));
    }

    #[test]
    fn test_topic_matches_prefix() {
        assert!(topic_matches("chat.session.created", "chat.session"));
        assert!(topic_matches("chat.session.created", "chat"));
        assert!(!topic_matches("chat.session", "chat.session.created"));
        assert!(!topic_matches("chat.session_extra", "chat.session"));
    }

    #[test]
    fn test_topic_matches_wildcard_suffix() {
        assert!(topic_matches("chat.session.created", "chat.session.*"));
        assert!(topic_matches("chat.session.resumed", "chat.session.*"));
        assert!(!topic_matches("tool.invoked", "chat.session.*"));
    }

    #[test]
    fn test_topic_matches_wildcard_middle() {
        assert!(topic_matches("scheduler.task.completed.agent.123", "scheduler.*.completed.*"));
        assert!(!topic_matches("scheduler.task.failed.agent.123", "scheduler.*.completed.*"));
    }

    #[test]
    fn test_topic_matches_wildcard_prefix() {
        assert!(topic_matches("chat.message.completed", "*.completed"));
        assert!(topic_matches("scheduler.task.completed", "*.completed"));
        assert!(!topic_matches("chat.message.failed", "*.completed"));
    }

    #[test]
    fn test_topic_matches_single_star() {
        assert!(topic_matches("anything.at.all", "*"));
    }

    #[test]
    fn test_incoming_message_dedup() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());

        // Initially, event should NOT be seen
        assert!(!store.is_trigger_seen("my-workflow", "conn-1:graph:msg-abc").unwrap());

        // Record the event
        store.mark_trigger_seen("my-workflow", "conn-1:graph:msg-abc").unwrap();

        // Now the same event should be seen
        assert!(store.is_trigger_seen("my-workflow", "conn-1:graph:msg-abc").unwrap());

        // Different external_id should NOT be seen
        assert!(!store.is_trigger_seen("my-workflow", "conn-1:graph:msg-def").unwrap());

        // Same external_id but different workflow should NOT be seen
        assert!(!store.is_trigger_seen("other-workflow", "conn-1:graph:msg-abc").unwrap());
    }

    #[test]
    fn test_trigger_dedup_prune() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        store.mark_trigger_seen("wf", "msg-1").unwrap();
        store.mark_trigger_seen("wf", "msg-2").unwrap();

        // Prune with 0 max age removes everything
        let pruned = store.prune_trigger_dedup(0).unwrap();
        assert_eq!(pruned, 2);
        assert!(!store.is_trigger_seen("wf", "msg-1").unwrap());
    }

    #[test]
    fn test_filter_matches_none_or_empty() {
        let payload = json!({"status": "ok"});
        assert!(filter_matches(&payload, None));
        assert!(filter_matches(&payload, Some("")));
    }

    #[test]
    fn test_filter_matches_substring_fallback() {
        let payload = json!({"status": "important", "id": 42});
        assert!(filter_matches(&payload, Some("important")));
        assert!(!filter_matches(&payload, Some("missing")));
    }

    #[test]
    fn test_filter_matches_equality_expression() {
        let payload = json!({"instance_id": "abc-123", "status": "completed"});
        assert!(filter_matches(&payload, Some("event.instance_id == abc-123")));
        assert!(!filter_matches(&payload, Some("event.instance_id == xyz-999")));
    }

    #[test]
    fn test_filter_matches_not_equal_expression() {
        let payload = json!({"priority": "high"});
        assert!(filter_matches(&payload, Some("event.priority != low")));
        assert!(!filter_matches(&payload, Some("event.priority != high")));
    }

    #[test]
    fn test_filter_matches_numeric_comparison() {
        let payload = json!({"score": 85});
        assert!(filter_matches(&payload, Some("event.score > 50")));
        assert!(!filter_matches(&payload, Some("event.score > 90")));
        assert!(filter_matches(&payload, Some("event.score <= 85")));
    }

    #[test]
    fn test_filter_matches_nested_field() {
        let payload = json!({"data": {"type": "email", "from": "alice@example.com"}});
        assert!(filter_matches(&payload, Some("event.data.type == email")));
        assert!(!filter_matches(&payload, Some("event.data.type == slack")));
    }

    #[test]
    fn test_filter_matches_without_event_prefix() {
        // event. prefix is optional
        let payload = json!({"status": "active"});
        assert!(filter_matches(&payload, Some("status == active")));
        assert!(!filter_matches(&payload, Some("status == inactive")));
    }

    #[test]
    fn test_filter_matches_quoted_string_with_spaces() {
        let payload = json!({"name": "Hello World"});
        assert!(filter_matches(&payload, Some("event.name == \"Hello World\"")));
        assert!(!filter_matches(&payload, Some("event.name == \"Goodbye World\"")));
    }

    #[test]
    fn test_filter_matches_quoted_string_with_operators() {
        let payload = json!({"expr": "a >= b"});
        assert!(filter_matches(&payload, Some("event.expr == \"a >= b\"")));
    }

    #[test]
    fn test_filter_matches_logical_and() {
        let payload = json!({"status": "completed", "priority": "high"});
        assert!(filter_matches(
            &payload,
            Some("event.status == completed && event.priority == high")
        ));
        assert!(!filter_matches(
            &payload,
            Some("event.status == completed && event.priority == low")
        ));
    }

    #[test]
    fn test_filter_matches_logical_or() {
        let payload = json!({"status": "failed"});
        assert!(filter_matches(
            &payload,
            Some("event.status == completed || event.status == failed")
        ));
        assert!(!filter_matches(
            &payload,
            Some("event.status == completed || event.status == pending")
        ));
    }

    #[test]
    fn test_filter_matches_and_with_quoted_values() {
        let payload = json!({"name": "John Doe", "role": "admin user"});
        assert!(filter_matches(
            &payload,
            Some("event.name == \"John Doe\" && event.role == \"admin user\"")
        ));
    }

    #[test]
    fn test_ignore_replies_discord() {
        let payload = json!({
            "channel_id": "discord-ch",
            "from": "user",
            "body": "hello",
            "metadata": { "referenced_message_id": "12345" }
        });
        // With ignore_replies=true, a Discord reply should be rejected
        assert!(!payload_matches_incoming(
            &payload,
            "discord-ch",
            None,
            None,
            None,
            None,
            None,
            true
        ));
        // With ignore_replies=false, same payload should pass
        assert!(payload_matches_incoming(
            &payload,
            "discord-ch",
            None,
            None,
            None,
            None,
            None,
            false
        ));
    }

    #[test]
    fn test_ignore_replies_slack_thread() {
        let payload = json!({
            "channel_id": "slack-ch",
            "from": "user",
            "body": "hello",
            "metadata": { "thread_ts": "1234567890.123456" }
        });
        assert!(!payload_matches_incoming(
            &payload, "slack-ch", None, None, None, None, None, true
        ));
        assert!(payload_matches_incoming(
            &payload, "slack-ch", None, None, None, None, None, false
        ));
    }

    #[test]
    fn test_ignore_replies_email() {
        let payload = json!({
            "channel_id": "email-ch",
            "from": "alice@example.com",
            "body": "Re: hello",
            "metadata": { "in_reply_to": "<msg-id@example.com>" }
        });
        assert!(!payload_matches_incoming(
            &payload, "email-ch", None, None, None, None, None, true
        ));
        assert!(payload_matches_incoming(
            &payload, "email-ch", None, None, None, None, None, false
        ));

        // `references` header alone should NOT trigger reply rejection —
        // providers like Microsoft Graph set it for conversation threading
        // even on original (non-reply) emails.
        let payload2 = json!({
            "channel_id": "email-ch",
            "from": "bob@example.com",
            "body": "Re: thread",
            "metadata": { "references": "<msg-id@example.com>" }
        });
        assert!(payload_matches_incoming(
            &payload2, "email-ch", None, None, None, None, None, true
        ));

        // But `references` WITH `in_reply_to` should still be rejected
        let payload3 = json!({
            "channel_id": "email-ch",
            "from": "carol@example.com",
            "body": "Re: thread",
            "metadata": { "in_reply_to": "<orig@example.com>", "references": "<orig@example.com>" }
        });
        assert!(!payload_matches_incoming(
            &payload3, "email-ch", None, None, None, None, None, true
        ));
    }

    #[test]
    fn test_ignore_replies_new_message_passes() {
        let payload = json!({
            "channel_id": "any-ch",
            "from": "user",
            "body": "brand new message",
            "metadata": { "channel_id": "general" }
        });
        // New message (no reply indicators) should pass even with ignore_replies=true
        assert!(payload_matches_incoming(&payload, "any-ch", None, None, None, None, None, true));
    }

    #[test]
    fn test_ignore_replies_empty_values_pass() {
        // Empty-string metadata values should NOT trigger reply rejection
        let payload = json!({
            "channel_id": "ch",
            "from": "user",
            "body": "hello",
            "metadata": { "in_reply_to": "", "referenced_message_id": "", "thread_ts": "" }
        });
        assert!(payload_matches_incoming(&payload, "ch", None, None, None, None, None, true));
    }

    #[test]
    fn test_ignore_replies_no_metadata() {
        let payload = json!({
            "channel_id": "any-ch",
            "from": "user",
            "body": "message without metadata"
        });
        // No metadata at all should pass with ignore_replies=true
        assert!(payload_matches_incoming(&payload, "any-ch", None, None, None, None, None, true));
    }

    // ── End-to-end trigger tests ──────────────────────────────────

    /// Helper: create a workflow definition with an IncomingMessage trigger,
    /// save it to the store, and register it with the TriggerManager.
    async fn setup_incoming_trigger(
        _bus: &EventBus,
        svc: &Arc<super::WorkflowService>,
        tm: &TriggerManager,
        def_id: &str,
        def_name: &str,
        channel_id: &str,
    ) {
        let def = trigger_definition(
            def_id,
            def_name,
            "1.0",
            TriggerType::IncomingMessage {
                channel_id: channel_id.to_string(),
                listen_channel_id: None,
                filter: None,
                from_filter: None,
                subject_filter: None,
                body_filter: None,
                mark_as_read: false,
                ignore_replies: false,
            },
        );
        let yaml = serde_yaml::to_string(&def).unwrap();
        svc.save_definition(&yaml).await.unwrap();
        tm.register_definition(&def).await;
    }

    /// Build the standard event payload that `poll_connector_once` publishes.
    fn incoming_email_payload(connector_id: &str, external_id: &str) -> Value {
        json!({
            "channel_id": connector_id,
            "provider": "gmail",
            "external_id": external_id,
            "from": "alice@example.com",
            "to": ["bob@example.com"],
            "subject": "Hello",
            "body": "Test email body",
            "timestamp_ms": 1700000000000u64,
            "metadata": {}
        })
    }

    #[tokio::test]
    async fn test_incoming_message_trigger_launches_workflow() {
        // Wire up the full pipeline: EventBus → TriggerManager → WorkflowService
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus.clone(), Arc::clone(&store));
        let svc = Arc::new(super::WorkflowService::in_memory().unwrap());
        tm.set_workflow_service(Arc::clone(&svc)).await;

        setup_incoming_trigger(&bus, &svc, &tm, "def-email", "user/email-wf", "gmail-connector").await;

        // Simulate the event that poll_connector_once publishes
        let payload = incoming_email_payload("gmail-connector", "gmail:msg-001");
        tm.evaluate_event("comm.message.received.gmail-connector", &payload).await;

        // Verify: a workflow instance should have been launched
        let result = svc.list_instances(&InstanceFilter {
            statuses: vec![],
            definition_names: vec!["user/email-wf".to_string()],
            definition_id: None,
            parent_session_id: None,
            parent_agent_id: None,
            mode: None,
            limit: None,
            offset: None,
            include_archived: false,
        }).await.unwrap();
        assert_eq!(result.total, 1, "expected exactly one workflow instance to be launched");
    }

    #[tokio::test]
    async fn test_incoming_message_wrong_channel_does_not_launch() {
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus.clone(), Arc::clone(&store));
        let svc = Arc::new(super::WorkflowService::in_memory().unwrap());
        tm.set_workflow_service(Arc::clone(&svc)).await;

        // Register trigger for "gmail-connector" but send event for "outlook-connector"
        setup_incoming_trigger(&bus, &svc, &tm, "def-email", "user/email-wf", "gmail-connector").await;

        let payload = incoming_email_payload("outlook-connector", "ms:msg-001");
        tm.evaluate_event("comm.message.received.outlook-connector", &payload).await;

        let result = svc.list_instances(&InstanceFilter {
            statuses: vec![],
            definition_names: vec!["user/email-wf".to_string()],
            definition_id: None,
            parent_session_id: None,
            parent_agent_id: None,
            mode: None,
            limit: None,
            offset: None,
            include_archived: false,
        }).await.unwrap();
        assert_eq!(result.total, 0, "should NOT launch for wrong channel_id");
    }

    #[tokio::test]
    async fn test_incoming_message_dedup_prevents_double_launch() {
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus.clone(), Arc::clone(&store));
        let svc = Arc::new(super::WorkflowService::in_memory().unwrap());
        tm.set_workflow_service(Arc::clone(&svc)).await;

        setup_incoming_trigger(&bus, &svc, &tm, "def-email", "user/email-wf", "gmail-connector").await;

        let payload = incoming_email_payload("gmail-connector", "gmail:msg-dup");

        // First evaluation should launch
        tm.evaluate_event("comm.message.received.gmail-connector", &payload).await;
        // Second evaluation with same external_id should be deduped
        tm.evaluate_event("comm.message.received.gmail-connector", &payload).await;

        let result = svc.list_instances(&InstanceFilter {
            statuses: vec![],
            definition_names: vec!["user/email-wf".to_string()],
            definition_id: None,
            parent_session_id: None,
            parent_agent_id: None,
            mode: None,
            limit: None,
            offset: None,
            include_archived: false,
        }).await.unwrap();
        assert_eq!(result.total, 1, "dedup should prevent second launch for same external_id");
    }

    #[tokio::test]
    async fn test_incoming_message_full_pipeline_via_event_bus() {
        // Full end-to-end: publish event on bus → TriggerManager.start() picks it up → launches workflow
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
        let svc = Arc::new(super::WorkflowService::in_memory().unwrap());
        tm.set_workflow_service(Arc::clone(&svc)).await;

        setup_incoming_trigger(&bus, &svc, &tm, "def-email", "user/email-wf", "test-connector").await;

        // Start the TriggerManager background loop
        tm.start().await;

        // Give the event listener a moment to subscribe
        sleep(Duration::from_millis(50)).await;

        // Publish the event on the bus (simulating poll_connector_once)
        let payload = incoming_email_payload("test-connector", "test:msg-e2e");
        bus.publish("comm.message.received.test-connector", "connector-poll", payload).unwrap();

        // Wait for the trigger manager to process the event and launch
        sleep(Duration::from_millis(500)).await;

        let result = svc.list_instances(&InstanceFilter {
            statuses: vec![],
            definition_names: vec!["user/email-wf".to_string()],
            definition_id: None,
            parent_session_id: None,
            parent_agent_id: None,
            mode: None,
            limit: None,
            offset: None,
            include_archived: false,
        }).await.unwrap();
        assert_eq!(result.total, 1, "event bus → trigger manager → workflow launch should work end-to-end");

        // Clean up: stop the trigger manager
        tm.stop().await;
    }

    #[tokio::test]
    async fn test_incoming_message_with_from_filter() {
        let bus = EventBus::new(128);
        let store: Arc<dyn hive_workflow::store::WorkflowPersistence> =
            Arc::new(WorkflowStore::in_memory().unwrap());
        let tm = TriggerManager::new(bus.clone(), Arc::clone(&store));
        let svc = Arc::new(super::WorkflowService::in_memory().unwrap());
        tm.set_workflow_service(Arc::clone(&svc)).await;

        // Register trigger with from_filter
        let def = trigger_definition(
            "def-filtered",
            "user/filtered-wf",
            "1.0",
            TriggerType::IncomingMessage {
                channel_id: "ch1".to_string(),
                listen_channel_id: None,
                filter: None,
                from_filter: Some("vip@example.com".to_string()),
                subject_filter: None,
                body_filter: None,
                mark_as_read: false,
                ignore_replies: false,
            },
        );
        let yaml = serde_yaml::to_string(&def).unwrap();
        svc.save_definition(&yaml).await.unwrap();
        tm.register_definition(&def).await;

        // Non-matching sender should not trigger
        let payload_other = json!({
            "channel_id": "ch1",
            "provider": "gmail",
            "external_id": "msg-1",
            "from": "random@example.com",
            "to": ["me@example.com"],
            "subject": "Hi",
            "body": "nope",
            "timestamp_ms": 1700000000000u64,
            "metadata": {}
        });
        tm.evaluate_event("comm.message.received.ch1", &payload_other).await;

        // Matching sender should trigger
        let payload_vip = json!({
            "channel_id": "ch1",
            "provider": "gmail",
            "external_id": "msg-2",
            "from": "vip@example.com",
            "to": ["me@example.com"],
            "subject": "Important",
            "body": "hello",
            "timestamp_ms": 1700000000001u64,
            "metadata": {}
        });
        tm.evaluate_event("comm.message.received.ch1", &payload_vip).await;

        let result = svc.list_instances(&InstanceFilter {
            statuses: vec![],
            definition_names: vec!["user/filtered-wf".to_string()],
            definition_id: None,
            parent_session_id: None,
            parent_agent_id: None,
            mode: None,
            limit: None,
            offset: None,
            include_archived: false,
        }).await.unwrap();
        assert_eq!(result.total, 1, "only the VIP email should trigger the workflow");
    }
}
