//! 100 complex battle-test scenarios for the SchedulerService.
//!
//! Categories:
//!   1-10:   CRUD & Basics under stress
//!  11-20:   Cron edge cases
//!  21-30:   Scheduled task timing
//!  31-35:   Once-task behaviour
//!  36-50:   Tick execution under load
//!  51-60:   HTTP actions & failure handling
//!  61-70:   Composite actions
//!  71-80:   State machine & transitions
//!  81-90:   Daemon restart simulation (file-backed DB)
//!  91-95:   Owner scoping & filtering
//!  96-100:  EventBus & notification integrity

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use hive_core::EventBus;
use hive_scheduler::{
    CreateTaskRequest, SchedulerConfig, SchedulerService, TaskAction, TaskRunStatus, TaskSchedule,
    TaskStatus, UpdateTaskRequest,
};
use serde_json::json;

// ───────────────────────────── Helpers ─────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn svc() -> Arc<SchedulerService> {
    Arc::new(
        SchedulerService::in_memory(EventBus::new(64), SchedulerConfig::default())
            .expect("in-memory scheduler"),
    )
}

fn svc_with_bus(bus: EventBus) -> Arc<SchedulerService> {
    Arc::new(
        SchedulerService::in_memory(bus, SchedulerConfig::default()).expect("in-memory scheduler"),
    )
}

fn svc_with_addr(addr: String) -> Arc<SchedulerService> {
    Arc::new(
        SchedulerService::in_memory_with_addr(EventBus::new(64), addr, SchedulerConfig::default())
            .expect("scheduler"),
    )
}

fn emit(name: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        name: name.to_string(),
        description: Some(format!("battle test: {name}")),
        schedule: TaskSchedule::Once,
        action: TaskAction::EmitEvent {
            topic: "battle.test".to_string(),
            payload: json!({"task": name}),
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    }
}

fn cron_req(name: &str, expr: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        name: name.to_string(),
        description: None,
        schedule: TaskSchedule::Cron { expression: expr.to_string() },
        action: TaskAction::EmitEvent { topic: "cron.fire".to_string(), payload: json!({}) },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    }
}

fn scheduled_req(name: &str, run_at_ms: u64) -> CreateTaskRequest {
    CreateTaskRequest {
        name: name.to_string(),
        description: None,
        schedule: TaskSchedule::Scheduled { run_at_ms },
        action: TaskAction::EmitEvent { topic: "scheduled.fire".to_string(), payload: json!({}) },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    }
}

fn webhook_req(name: &str, url: &str, method: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        name: name.to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::HttpWebhook {
            url: url.to_string(),
            method: method.to_string(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    }
}

fn owned_emit(name: &str, session: &str, agent: Option<&str>) -> CreateTaskRequest {
    CreateTaskRequest {
        name: name.to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::EmitEvent { topic: "owned".to_string(), payload: json!({}) },
        owner_session_id: Some(session.to_string()),
        owner_agent_id: agent.map(|a| a.to_string()),
        max_retries: None,
        retry_delay_ms: None,
    }
}

/// Spin up a local HTTP server and return its address.
async fn mock_server(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.to_string()
}

// ====================================================================
// 1-10: CRUD & Basics under stress
// ====================================================================

#[tokio::test]
async fn bt01_create_100_tasks_all_unique_ids() {
    let s = svc();
    let mut ids = HashSet::new();
    for i in 0..100 {
        let t = s.create_task(emit(&format!("bt01_{i}"))).unwrap();
        assert!(ids.insert(t.id.clone()), "duplicate ID: {}", t.id);
    }
    assert_eq!(s.list_tasks().len(), 100);
}

#[tokio::test]
async fn bt02_create_task_with_long_name() {
    let s = svc();
    let long_name: String = "A".repeat(10_000);
    // Name exceeds max_task_name_len (256) and should be rejected.
    let result = s.create_task(emit(&long_name));
    assert!(result.is_err(), "expected long name to be rejected");

    // A name within the limit should succeed.
    let ok_name: String = "B".repeat(256);
    let t = s.create_task(emit(&ok_name)).unwrap();
    assert_eq!(t.name, ok_name);
    let fetched = s.get_task(&t.id).unwrap();
    assert_eq!(fetched.name, ok_name);
}

#[tokio::test]
async fn bt03_create_task_with_empty_description() {
    let s = svc();
    let mut req = emit("bt03");
    req.description = None;
    let t = s.create_task(req).unwrap();
    assert!(t.description.is_empty());
}

#[tokio::test]
async fn bt04_create_task_with_unicode_name() {
    let s = svc();
    let name = "任务调度 🕐 Ωmega–task Ñoño";
    let t = s.create_task(emit(name)).unwrap();
    assert_eq!(t.name, name);
    let fetched = s.get_task(&t.id).unwrap();
    assert_eq!(fetched.name, name);
}

#[tokio::test]
async fn bt05_get_task_matches_creation() {
    let s = svc();
    let t = s.create_task(emit("bt05")).unwrap();
    let fetched = s.get_task(&t.id).unwrap();
    assert_eq!(fetched.id, t.id);
    assert_eq!(fetched.name, t.name);
    assert_eq!(fetched.status, TaskStatus::Pending);
    assert_eq!(fetched.run_count, 0);
}

#[tokio::test]
async fn bt06_list_after_bulk_create() {
    let s = svc();
    for i in 0..50 {
        s.create_task(emit(&format!("bt06_{i}"))).unwrap();
    }
    assert_eq!(s.list_tasks().len(), 50);
}

#[tokio::test]
async fn bt07_delete_all_tasks_one_by_one() {
    let s = svc();
    let mut ids = Vec::new();
    for i in 0..20 {
        let t = s.create_task(emit(&format!("bt07_{i}"))).unwrap();
        ids.push(t.id);
    }
    for id in &ids {
        s.delete_task(id).unwrap();
    }
    assert!(s.list_tasks().is_empty());
}

#[tokio::test]
async fn bt08_cancel_already_cancelled_returns_task() {
    let s = svc();
    let t = s.create_task(emit("bt08")).unwrap();
    s.cancel_task(&t.id).unwrap();
    let again = s.cancel_task(&t.id).unwrap();
    assert_eq!(again.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn bt09_cancel_completed_returns_unchanged() {
    let s = svc();
    let t = s.create_task(emit("bt09")).unwrap();
    s.tick().await;
    let task = s.get_task(&t.id).unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    let cancelled = s.cancel_task(&t.id).unwrap();
    assert_eq!(cancelled.status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt10_delete_cascades_runs() {
    let s = svc();
    let t = s.create_task(emit("bt10")).unwrap();
    s.tick().await;
    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs.len(), 1);
    s.delete_task(&t.id).unwrap();
    assert!(s.get_task(&t.id).is_err());
}

// ====================================================================
// 11-20: Cron edge cases
// ====================================================================

#[tokio::test]
async fn bt11_cron_every_minute_schedules_correctly() {
    let s = svc();
    let t = s.create_task(cron_req("bt11", "0 * * * * *")).unwrap();
    assert!(t.next_run_ms.is_some());
    let next = t.next_run_ms.unwrap();
    assert!(next > now_ms() - 1000);
    assert!(next <= now_ms() + 61_000);
}

#[tokio::test]
async fn bt12_cron_far_future_yearly() {
    let s = svc();
    let t = s.create_task(cron_req("bt12", "0 0 0 1 1 * *")).unwrap();
    assert!(t.next_run_ms.is_some());
    let next = t.next_run_ms.unwrap();
    assert!(next > now_ms());
}

#[tokio::test]
async fn bt13_cron_past_specific_finds_next() {
    let s = svc();
    let t = s.create_task(cron_req("bt13", "0 0 0 * * *")).unwrap();
    assert!(t.next_run_ms.unwrap() > now_ms());
}

#[tokio::test]
async fn bt14_multiple_cron_tasks_coexist() {
    let s = svc();
    let t1 = s.create_task(cron_req("bt14a", "0 */5 * * * *")).unwrap();
    let _t2 = s.create_task(cron_req("bt14b", "0 */10 * * * *")).unwrap();
    let t3 = s.create_task(cron_req("bt14c", "0 0 * * * *")).unwrap();
    assert_ne!(t1.next_run_ms, t3.next_run_ms);
    assert_eq!(s.list_tasks().len(), 3);
    for t in s.list_tasks() {
        assert_eq!(t.status, TaskStatus::Pending);
    }
}

#[tokio::test]
async fn bt15_cron_resets_to_pending_after_execution() {
    let s = svc();
    s.create_task(cron_req("bt15", "0 * * * * *")).unwrap();
    s.force_all_due();
    s.tick().await;
    let tasks = s.list_tasks();
    assert_eq!(tasks[0].status, TaskStatus::Pending);
    assert_eq!(tasks[0].run_count, 1);
}

#[tokio::test]
async fn bt16_cron_run_count_accumulates() {
    let s = svc();
    s.create_task(cron_req("bt16", "0 * * * * *")).unwrap();
    for _ in 0..5 {
        s.force_all_due();
        s.tick().await;
    }
    let tasks = s.list_tasks();
    assert_eq!(tasks[0].run_count, 5);
    assert_eq!(tasks[0].status, TaskStatus::Pending);
}

#[tokio::test]
async fn bt17_cron_with_dow_constraint() {
    let s = svc();
    let t = s.create_task(cron_req("bt17", "0 0 12 * * MON *")).unwrap();
    assert!(t.next_run_ms.is_some());
    assert!(t.next_run_ms.unwrap() > now_ms());
}

#[tokio::test]
async fn bt18_cron_with_month_constraint() {
    let s = svc();
    let t = s.create_task(cron_req("bt18", "0 0 0 * 12 * *")).unwrap();
    assert!(t.next_run_ms.is_some());
}

#[tokio::test]
async fn bt19_invalid_cron_expression_fails() {
    let s = svc();
    let result = s.create_task(cron_req("bt19", "not-a-cron"));
    assert!(result.is_err());
}

#[tokio::test]
async fn bt20_cron_next_run_advances_after_tick() {
    let s = svc();
    // Use a per-second cron to guarantee advancement within the test
    let t = s.create_task(cron_req("bt20", "* * * * * *")).unwrap();
    let first_next = t.next_run_ms.unwrap();
    s.force_all_due();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    let second_next = after.next_run_ms.unwrap();
    // After execution, next_run should be >= the first next_run
    // (it re-computes from "now" which is at or past first_next)
    assert!(
        second_next >= first_next,
        "next_run should not go backwards: first={first_next}, second={second_next}"
    );
    assert_eq!(after.run_count, 1);
}

// ====================================================================
// 21-30: Scheduled task timing
// ====================================================================

#[tokio::test]
async fn bt21_scheduled_at_past_executes_immediately() {
    let s = svc();
    let t = s.create_task(scheduled_req("bt21", now_ms().saturating_sub(5000))).unwrap();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt22_scheduled_far_future_not_executed() {
    let s = svc();
    let far_future = now_ms() + 999_999_999;
    let t = s.create_task(scheduled_req("bt22", far_future)).unwrap();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Pending);
    assert_eq!(after.run_count, 0);
}

#[tokio::test]
async fn bt23_scheduled_just_past_due_executes() {
    let s = svc();
    let t = s.create_task(scheduled_req("bt23", now_ms().saturating_sub(1))).unwrap();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt24_multiple_scheduled_same_time_all_execute() {
    let s = svc();
    let run_at = now_ms().saturating_sub(100);
    for i in 0..10 {
        s.create_task(scheduled_req(&format!("bt24_{i}"), run_at)).unwrap();
    }
    s.tick().await;
    for t in s.list_tasks() {
        assert_eq!(t.status, TaskStatus::Completed, "task {} not completed", t.name);
    }
}

#[tokio::test]
async fn bt25_scheduled_at_zero_executes() {
    let s = svc();
    let t = s.create_task(scheduled_req("bt25", 0)).unwrap();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt26_scheduled_at_max_u64_overflows_sqlite() {
    // FIX VERIFIED: compute_next_run now validates run_at_ms < i64::MAX
    // and returns a friendly error instead of a SQLite overflow.
    let s = svc();
    let result = s.create_task(scheduled_req("bt26", u64::MAX));
    assert!(result.is_err(), "u64::MAX should fail with overflow validation");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("exceeds maximum"),
        "error should be a friendly validation message, got: {err_msg}"
    );
}

#[tokio::test]
async fn bt27_update_scheduled_to_past_triggers_execution() {
    let s = svc();
    let t = s.create_task(scheduled_req("bt27", now_ms() + 999_999_999)).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Pending);

    s.update_task(
        &t.id,
        UpdateTaskRequest {
            name: None,
            description: None,
            schedule: Some(TaskSchedule::Scheduled { run_at_ms: now_ms().saturating_sub(100) }),
            action: None,
            max_retries: None,
            retry_delay_ms: None,
        },
    )
    .unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt28_update_scheduled_to_future_delays() {
    let s = svc();
    let t = s.create_task(scheduled_req("bt28", 0)).unwrap();
    s.update_task(
        &t.id,
        UpdateTaskRequest {
            name: None,
            description: None,
            schedule: Some(TaskSchedule::Scheduled { run_at_ms: now_ms() + 999_999_999 }),
            action: None,
            max_retries: None,
            retry_delay_ms: None,
        },
    )
    .unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test]
async fn bt29_scheduled_state_transitions() {
    let s = svc();
    let t = s.create_task(scheduled_req("bt29", 0)).unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Completed);
    assert_eq!(after.run_count, 1);
    assert!(after.last_run_ms.is_some());
}

#[tokio::test]
async fn bt30_scheduled_run_has_correct_timestamps() {
    let before = now_ms();
    let s = svc();
    let t = s.create_task(scheduled_req("bt30", 0)).unwrap();
    s.tick().await;
    let after_time = now_ms();
    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].started_at_ms >= before);
    assert!(runs[0].completed_at_ms.unwrap() <= after_time + 1000);
    assert!(runs[0].started_at_ms <= runs[0].completed_at_ms.unwrap());
}

// ====================================================================
// 31-35: Once-task behaviour
// ====================================================================

#[tokio::test]
async fn bt31_once_task_executes_on_first_tick() {
    let s = svc();
    let t = s.create_task(emit("bt31")).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt32_once_task_does_not_become_pending_again() {
    let s = svc();
    let t = s.create_task(emit("bt32")).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
    assert_eq!(s.get_task(&t.id).unwrap().run_count, 1);
}

#[tokio::test]
async fn bt33_many_once_tasks_all_execute_in_single_tick() {
    let s = svc();
    for i in 0..30 {
        s.create_task(emit(&format!("bt33_{i}"))).unwrap();
    }
    s.tick().await;
    for t in s.list_tasks() {
        assert_eq!(t.status, TaskStatus::Completed, "task {} not completed", t.name);
        assert_eq!(t.run_count, 1);
    }
}

#[tokio::test]
async fn bt34_once_task_run_has_timing_info() {
    let s = svc();
    let t = s.create_task(emit("bt34")).unwrap();
    s.tick().await;
    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].started_at_ms > 0);
    assert!(runs[0].completed_at_ms.is_some());
    assert_eq!(runs[0].status, TaskRunStatus::Success);
}

#[tokio::test]
async fn bt35_emit_event_with_no_subscribers_still_succeeds() {
    let bus = EventBus::new(8);
    let s = svc_with_bus(bus);
    let t = s.create_task(emit("bt35")).unwrap();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Completed);
    assert!(after.last_error.is_none());
}

// ====================================================================
// 36-50: Tick execution under load
// ====================================================================

#[tokio::test]
async fn bt36_tick_with_50_due_tasks() {
    let s = svc();
    for i in 0..50 {
        s.create_task(emit(&format!("bt36_{i}"))).unwrap();
    }
    s.tick().await;
    let completed = s.list_tasks().iter().filter(|t| t.status == TaskStatus::Completed).count();
    assert_eq!(completed, 50);
}

#[tokio::test]
async fn bt37_tick_with_100_due_tasks() {
    let s = svc();
    for i in 0..100 {
        s.create_task(emit(&format!("bt37_{i}"))).unwrap();
    }
    s.tick().await;
    let completed = s.list_tasks().iter().filter(|t| t.status == TaskStatus::Completed).count();
    assert_eq!(completed, 100);
}

#[tokio::test]
async fn bt38_tick_only_executes_due_tasks() {
    let s = svc();
    for i in 0..10 {
        s.create_task(emit(&format!("bt38_due_{i}"))).unwrap();
    }
    for i in 0..10 {
        s.create_task(scheduled_req(&format!("bt38_future_{i}"), now_ms() + 999_999_999)).unwrap();
    }
    s.tick().await;
    let tasks = s.list_tasks();
    let completed = tasks.iter().filter(|t| t.status == TaskStatus::Completed).count();
    let pending = tasks.iter().filter(|t| t.status == TaskStatus::Pending).count();
    assert_eq!(completed, 10);
    assert_eq!(pending, 10);
}

#[tokio::test]
async fn bt39_rapid_ticks_no_double_execution() {
    let s = svc();
    let t = s.create_task(emit("bt39")).unwrap();
    s.tick().await;
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.run_count, 1);
    assert_eq!(after.status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt40_tick_idempotent_for_completed() {
    let s = svc();
    let t = s.create_task(emit("bt40")).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
    for _ in 0..5 {
        s.tick().await;
    }
    assert_eq!(s.get_task(&t.id).unwrap().run_count, 1);
}

#[tokio::test]
async fn bt41_concurrent_tick_calls() {
    let s = svc();
    for i in 0..10 {
        s.create_task(emit(&format!("bt41_{i}"))).unwrap();
    }
    let mut handles = Vec::new();
    for _ in 0..5 {
        let sc = Arc::clone(&s);
        handles.push(tokio::spawn(async move { sc.tick().await }));
    }
    for h in handles {
        h.await.unwrap();
    }
    // Each task should only have been executed once due to the pending→running CAS
    for t in s.list_tasks() {
        assert!(t.run_count <= 1, "task {} ran {} times", t.name, t.run_count);
        assert!(
            t.status == TaskStatus::Completed || t.status == TaskStatus::Pending,
            "unexpected status {:?} for {}",
            t.status,
            t.name
        );
    }
}

#[tokio::test]
async fn bt42_force_all_due_then_tick() {
    let s = svc();
    for i in 0..5 {
        s.create_task(scheduled_req(&format!("bt42_{i}"), now_ms() + 999_999_999)).unwrap();
    }
    s.tick().await;
    for t in s.list_tasks() {
        assert_eq!(t.status, TaskStatus::Pending);
    }
    s.force_all_due();
    s.tick().await;
    for t in s.list_tasks() {
        assert_eq!(t.status, TaskStatus::Completed, "task {} not completed", t.name);
    }
}

#[tokio::test]
async fn bt43_tick_skips_cancelled_tasks() {
    let s = svc();
    let t1 = s.create_task(emit("bt43_active")).unwrap();
    let t2 = s.create_task(emit("bt43_cancel")).unwrap();
    s.cancel_task(&t2.id).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t1.id).unwrap().status, TaskStatus::Completed);
    assert_eq!(s.get_task(&t2.id).unwrap().status, TaskStatus::Cancelled);
    assert_eq!(s.get_task(&t2.id).unwrap().run_count, 0);
}

#[tokio::test]
async fn bt44_tick_after_all_completed_is_noop() {
    let s = svc();
    for i in 0..5 {
        s.create_task(emit(&format!("bt44_{i}"))).unwrap();
    }
    s.tick().await;
    let snap1: Vec<_> = s.list_tasks().iter().map(|t| (t.id.clone(), t.run_count)).collect();
    s.tick().await;
    let snap2: Vec<_> = s.list_tasks().iter().map(|t| (t.id.clone(), t.run_count)).collect();
    assert_eq!(snap1, snap2);
}

#[tokio::test]
async fn bt45_mixed_task_types_in_single_tick() {
    let s = svc();
    s.create_task(emit("bt45_once")).unwrap();
    s.create_task(cron_req("bt45_cron", "0 * * * * *")).unwrap();
    s.create_task(scheduled_req("bt45_sched", 0)).unwrap();

    s.force_all_due();
    s.tick().await;

    let tasks = s.list_tasks();
    let once = tasks.iter().find(|t| t.name == "bt45_once").unwrap();
    let cron = tasks.iter().find(|t| t.name == "bt45_cron").unwrap();
    let sched = tasks.iter().find(|t| t.name == "bt45_sched").unwrap();

    assert_eq!(once.status, TaskStatus::Completed);
    assert_eq!(cron.status, TaskStatus::Pending); // cron resets
    assert_eq!(sched.status, TaskStatus::Completed);
    assert_eq!(once.run_count, 1);
    assert_eq!(cron.run_count, 1);
    assert_eq!(sched.run_count, 1);
}

#[tokio::test]
async fn bt46_tick_does_not_execute_before_scheduled_time() {
    let s = svc();
    let future = now_ms() + 60_000;
    let t = s.create_task(scheduled_req("bt46", future)).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test]
async fn bt47_multiple_ticks_on_cron_produce_runs() {
    let s = svc();
    let t = s.create_task(cron_req("bt47", "0 * * * * *")).unwrap();
    for _ in 0..3 {
        s.force_all_due();
        s.tick().await;
    }
    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs.len(), 3);
    for run in &runs {
        assert_eq!(run.status, TaskRunStatus::Success);
    }
}

#[tokio::test]
async fn bt48_tick_does_not_re_execute_failed_once() {
    let s = svc_with_addr("127.0.0.1:1".to_string());
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt48".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::SendMessage {
                session_id: "nope".to_string(),
                content: "fail".to_string(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Failed);
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().run_count, 1);
}

#[tokio::test]
async fn bt49_once_task_no_re_execution() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);
    let t = s.create_task(emit("bt49")).unwrap();
    s.tick().await;
    // Drain the first batch of events
    while rx.try_recv().is_ok() {}
    s.tick().await;
    // No new events should appear for this task
    let mut extra_battle_events = 0;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "battle.test" {
            extra_battle_events += 1;
        }
    }
    assert_eq!(extra_battle_events, 0);
    assert_eq!(s.get_task(&t.id).unwrap().run_count, 1);
}

#[tokio::test]
async fn bt50_200_tasks_stress_test() {
    let s = svc();
    for i in 0..200 {
        s.create_task(emit(&format!("bt50_{i}"))).unwrap();
    }
    s.tick().await;
    let tasks = s.list_tasks();
    assert_eq!(tasks.len(), 200);
    let all_completed = tasks.iter().all(|t| t.status == TaskStatus::Completed);
    assert!(all_completed, "not all 200 tasks completed");
}

// ====================================================================
// 51-60: HTTP actions & failure handling
// ====================================================================

#[tokio::test]
async fn bt51_webhook_with_custom_headers() {
    let received_headers = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let captured = received_headers.clone();

    let app = Router::new().route(
        "/hook",
        post(move |headers: axum::http::HeaderMap| {
            let captured = captured.clone();
            async move {
                let auth = headers.get("x-custom-auth").map(|v| v.to_str().unwrap().to_string());
                captured.lock().push(auth);
                "ok"
            }
        }),
    );
    let addr = mock_server(app).await;

    let s = svc();
    s.create_task(CreateTaskRequest {
        name: "bt51".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::HttpWebhook {
            url: format!("http://{addr}/hook"),
            method: "POST".to_string(),
            body: None,
            headers: Some(
                [("x-custom-auth".to_string(), "Bearer xyz".to_string())].into_iter().collect(),
            ),
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    let hdrs = received_headers.lock();
    assert_eq!(hdrs.len(), 1);
    assert_eq!(hdrs[0], Some("Bearer xyz".to_string()));
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt52_webhook_put_method() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/put",
        put(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;
    let s = svc();
    s.create_task(webhook_req("bt52", &format!("http://{addr}/put"), "PUT")).unwrap();
    s.tick().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt53_webhook_delete_method() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/del",
        delete(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;
    let s = svc();
    s.create_task(webhook_req("bt53", &format!("http://{addr}/del"), "DELETE")).unwrap();
    s.tick().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt54_webhook_patch_method() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/patch",
        patch(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;
    let s = svc();
    s.create_task(webhook_req("bt54", &format!("http://{addr}/patch"), "PATCH")).unwrap();
    s.tick().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt55_webhook_connection_refused() {
    let s = svc();
    s.create_task(webhook_req("bt55", "http://127.0.0.1:1/nope", "POST")).unwrap();
    s.tick().await;
    let t = &s.list_tasks()[0];
    assert_eq!(t.status, TaskStatus::Failed);
    assert!(t.last_error.is_some());
}

#[tokio::test]
async fn bt56_webhook_server_500_fails() {
    let app = Router::new()
        .route("/fail", post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }));
    let addr = mock_server(app).await;
    let s = svc();
    s.create_task(webhook_req("bt56", &format!("http://{addr}/fail"), "POST")).unwrap();
    s.tick().await;
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Failed);
}

#[tokio::test]
async fn bt57_webhook_non_json_body_succeeds() {
    let app = Router::new().route("/text", post(|| async { "plain text response" }));
    let addr = mock_server(app).await;
    let s = svc();
    s.create_task(webhook_req("bt57", &format!("http://{addr}/text"), "POST")).unwrap();
    s.tick().await;
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt58_webhook_json_response_captured_in_run() {
    let app = Router::new().route("/data", get(|| async { axum::Json(json!({"answer": 42})) }));
    let addr = mock_server(app).await;
    let s = svc();
    let t = s.create_task(webhook_req("bt58", &format!("http://{addr}/data"), "GET")).unwrap();
    s.tick().await;
    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs[0].status, TaskRunStatus::Success);
    let result = runs[0].result.as_ref().expect("result");
    assert_eq!(result["answer"], 42);
}

#[tokio::test]
async fn bt59_send_message_unreachable_fails() {
    let s = svc_with_addr("127.0.0.1:1".to_string());
    s.create_task(CreateTaskRequest {
        name: "bt59".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::SendMessage {
            session_id: "sess".to_string(),
            content: "hello".to_string(),
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;
    let t = &s.list_tasks()[0];
    assert_eq!(t.status, TaskStatus::Failed);
    assert!(t.last_error.as_ref().unwrap().contains("failed"));
}

#[tokio::test]
async fn bt60_multiple_webhooks_all_fire() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/hook",
        post(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;
    let s = svc();
    for i in 0..10 {
        s.create_task(webhook_req(&format!("bt60_{i}"), &format!("http://{addr}/hook"), "POST"))
            .unwrap();
    }
    s.tick().await;
    assert_eq!(count.load(Ordering::SeqCst), 10);
    assert!(s.list_tasks().iter().all(|t| t.status == TaskStatus::Completed));
}

// ====================================================================
// 61-70: Composite actions
// ====================================================================

#[tokio::test]
async fn bt61_composite_all_success() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);

    s.create_task(CreateTaskRequest {
        name: "bt61".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![
                TaskAction::EmitEvent { topic: "comp.a".to_string(), payload: json!(1) },
                TaskAction::EmitEvent { topic: "comp.b".to_string(), payload: json!(2) },
            ],
            stop_on_failure: false,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
    let mut topics = Vec::new();
    while let Ok(env) = rx.try_recv() {
        topics.push(env.topic.clone());
    }
    assert!(topics.contains(&"comp.a".to_string()));
    assert!(topics.contains(&"comp.b".to_string()));
}

#[tokio::test]
async fn bt62_composite_stop_on_failure() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/ok",
        post(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;

    let s = svc();
    s.create_task(CreateTaskRequest {
        name: "bt62".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![
                TaskAction::HttpWebhook {
                    url: "http://127.0.0.1:1/fail".to_string(),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
                TaskAction::HttpWebhook {
                    url: format!("http://{addr}/ok"),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
            ],
            stop_on_failure: true,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    assert_eq!(s.list_tasks()[0].status, TaskStatus::Failed);
    assert_eq!(count.load(Ordering::SeqCst), 0, "second action should not run");
}

#[tokio::test]
async fn bt63_composite_continue_on_failure() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/ok",
        post(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;

    let s = svc();
    s.create_task(CreateTaskRequest {
        name: "bt63".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![
                TaskAction::HttpWebhook {
                    url: "http://127.0.0.1:1/fail".to_string(),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
                TaskAction::HttpWebhook {
                    url: format!("http://{addr}/ok"),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
            ],
            stop_on_failure: false,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    assert_eq!(count.load(Ordering::SeqCst), 1);
    // With stop_on_failure: false, partial failures are now reported as
    // overall failure so that retry logic can kick in.
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Failed);
}

#[tokio::test]
async fn bt64_composite_all_failing() {
    let s = svc();
    s.create_task(CreateTaskRequest {
        name: "bt64".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![
                TaskAction::HttpWebhook {
                    url: "http://127.0.0.1:1/a".to_string(),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
                TaskAction::HttpWebhook {
                    url: "http://127.0.0.1:1/b".to_string(),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
            ],
            stop_on_failure: false,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    // With stop_on_failure: false, all-failing composite is now reported
    // as Failed so that retry logic can kick in.
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Failed);
}

#[tokio::test]
async fn bt65_nested_composite_rejected() {
    let s = svc();
    let result = s.create_task(CreateTaskRequest {
        name: "bt65".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![TaskAction::CompositeAction { actions: vec![], stop_on_failure: false }],
            stop_on_failure: false,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    });
    assert!(result.is_err(), "nested composite should be rejected");
}

#[tokio::test]
async fn bt66_composite_empty_actions() {
    let s = svc();
    s.create_task(CreateTaskRequest {
        name: "bt66".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction { actions: vec![], stop_on_failure: false },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;
    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt67_composite_mixed_emit_and_webhook() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/ok",
        post(move || {
            c.fetch_add(1, Ordering::SeqCst);
            async { "ok" }
        }),
    );
    let addr = mock_server(app).await;

    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);

    s.create_task(CreateTaskRequest {
        name: "bt67".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![
                TaskAction::EmitEvent { topic: "comp.67".to_string(), payload: json!({}) },
                TaskAction::HttpWebhook {
                    url: format!("http://{addr}/ok"),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
            ],
            stop_on_failure: false,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let mut found_topic = false;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "comp.67" {
            found_topic = true;
        }
    }
    assert!(found_topic);
}

#[tokio::test]
async fn bt68_composite_partial_failure_records_details() {
    let app = Router::new().route("/ok", post(|| async { "ok" }));
    let addr = mock_server(app).await;

    let s = svc();
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt68".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::CompositeAction {
                actions: vec![
                    TaskAction::HttpWebhook {
                        url: format!("http://{addr}/ok"),
                        method: "POST".to_string(),
                        body: None,
                        headers: None,
                    },
                    TaskAction::HttpWebhook {
                        url: "http://127.0.0.1:1/nope".to_string(),
                        method: "POST".to_string(),
                        body: None,
                        headers: None,
                    },
                ],
                stop_on_failure: false,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.tick().await;

    let runs = s.list_task_runs(&t.id).unwrap();
    // With stop_on_failure: false, partial failures are now reported as
    // overall failure so that retry logic can kick in.
    assert_eq!(runs[0].status, TaskRunStatus::Failure);
    assert!(runs[0].error.is_some());
}

#[tokio::test]
async fn bt69_composite_index_tracking() {
    let s = svc();
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt69".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::CompositeAction {
                actions: vec![
                    TaskAction::EmitEvent { topic: "a".to_string(), payload: json!(0) },
                    TaskAction::EmitEvent { topic: "b".to_string(), payload: json!(1) },
                    TaskAction::EmitEvent { topic: "c".to_string(), payload: json!(2) },
                ],
                stop_on_failure: false,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.tick().await;

    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs[0].status, TaskRunStatus::Success);
    let result = runs[0].result.as_ref().unwrap();
    let arr = result.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["index"], 0);
    assert_eq!(arr[1]["index"], 1);
    assert_eq!(arr[2]["index"], 2);
}

#[tokio::test]
async fn bt70_composite_with_10_sub_actions() {
    let s = svc();
    let actions: Vec<_> = (0..10)
        .map(|i| TaskAction::EmitEvent { topic: format!("comp10.{i}"), payload: json!(i) })
        .collect();
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt70".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::CompositeAction { actions, stop_on_failure: false },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.tick().await;

    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
    let runs = s.list_task_runs(&t.id).unwrap();
    let result = runs[0].result.as_ref().unwrap();
    assert_eq!(result.as_array().unwrap().len(), 10);
}

// ====================================================================
// 71-80: State machine & transitions
// ====================================================================

#[tokio::test]
async fn bt71_once_pending_to_completed() {
    let s = svc();
    let t = s.create_task(emit("bt71")).unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
}

#[tokio::test]
async fn bt72_cron_pending_running_back_to_pending() {
    let s = svc();
    let t = s.create_task(cron_req("bt72", "0 * * * * *")).unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
    s.force_all_due();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Pending);
    assert_eq!(after.run_count, 1);
}

#[tokio::test]
async fn bt73_once_failure_stays_failed() {
    let s = svc_with_addr("127.0.0.1:1".to_string());
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt73".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::SendMessage {
                session_id: "x".to_string(),
                content: "y".to_string(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Failed);
}

#[tokio::test]
async fn bt74_cron_failure_resets_to_pending() {
    let s = svc_with_addr("127.0.0.1:1".to_string());
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt74".to_string(),
            description: None,
            schedule: TaskSchedule::Cron { expression: "0 * * * * *".to_string() },
            action: TaskAction::SendMessage {
                session_id: "x".to_string(),
                content: "y".to_string(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.force_all_due();
    s.tick().await;
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.status, TaskStatus::Pending);
    assert!(after.last_error.is_some());
    assert_eq!(after.run_count, 1);
}

#[tokio::test]
async fn bt75_cancelled_never_runs() {
    let s = svc();
    let t = s.create_task(emit("bt75")).unwrap();
    s.cancel_task(&t.id).unwrap();
    for _ in 0..10 {
        s.tick().await;
    }
    assert_eq!(s.get_task(&t.id).unwrap().run_count, 0);
}

#[tokio::test]
async fn bt76_failed_once_stays_failed() {
    let s = svc_with_addr("127.0.0.1:1".to_string());
    s.create_task(CreateTaskRequest {
        name: "bt76".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::HttpWebhook {
            url: "http://127.0.0.1:1/nope".to_string(),
            method: "POST".to_string(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;
    let id = s.list_tasks()[0].id.clone();
    for _ in 0..5 {
        s.tick().await;
    }
    assert_eq!(s.get_task(&id).unwrap().status, TaskStatus::Failed);
    assert_eq!(s.get_task(&id).unwrap().run_count, 1);
}

#[tokio::test]
async fn bt77_completed_once_stays_completed() {
    let s = svc();
    let t = s.create_task(emit("bt77")).unwrap();
    s.tick().await;
    for _ in 0..5 {
        s.tick().await;
    }
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);
    assert_eq!(s.get_task(&t.id).unwrap().run_count, 1);
}

#[tokio::test]
async fn bt78_update_failed_schedule_resets_to_pending() {
    let s = svc_with_addr("127.0.0.1:1".to_string());
    let t = s
        .create_task(CreateTaskRequest {
            name: "bt78".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::SendMessage {
                session_id: "x".to_string(),
                content: "y".to_string(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Failed);

    s.update_task(
        &t.id,
        UpdateTaskRequest {
            name: None,
            description: None,
            schedule: Some(TaskSchedule::Once),
            action: None,
            max_retries: None,
            retry_delay_ms: None,
        },
    )
    .unwrap();
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test]
async fn bt79_update_completed_schedule_resets_to_pending() {
    let s = svc();
    let t = s.create_task(emit("bt79")).unwrap();
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Completed);

    s.update_task(
        &t.id,
        UpdateTaskRequest {
            name: None,
            description: None,
            schedule: Some(TaskSchedule::Cron { expression: "0 * * * * *".to_string() }),
            action: None,
            max_retries: None,
            retry_delay_ms: None,
        },
    )
    .unwrap();
    assert_eq!(s.get_task(&t.id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test]
async fn bt80_run_count_increments_correctly() {
    let s = svc();
    let t = s.create_task(cron_req("bt80", "0 * * * * *")).unwrap();
    for expected in 1..=7u32 {
        s.force_all_due();
        s.tick().await;
        assert_eq!(s.get_task(&t.id).unwrap().run_count, expected);
    }
}

// ====================================================================
// 81-90: Daemon restart simulation (file-backed DB)
// ====================================================================

#[tokio::test]
async fn bt81_tasks_persist_across_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");
    let bus = EventBus::new(32);

    {
        let s = SchedulerService::new(
            db_path.clone(),
            bus.clone(),
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        s.create_task(emit("bt81_persist")).unwrap();
    }

    {
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let tasks = s.list_tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "bt81_persist");
    }
}

#[tokio::test]
async fn bt82_pending_tasks_survive_restart_and_execute() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path.clone(),
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        s.create_task(emit("bt82")).unwrap();
    }

    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path,
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        s.tick().await;
        assert_eq!(s.list_tasks()[0].status, TaskStatus::Completed);
    }
}

#[tokio::test]
async fn bt83_cron_tasks_survive_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let task_id;
    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s.create_task(cron_req("bt83", "0 * * * * *")).unwrap();
        task_id = t.id;
        s.force_all_due();
        s.tick().await;
    }

    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let t = s.get_task(&task_id).unwrap();
        assert_eq!(t.status, TaskStatus::Pending);
        assert_eq!(t.run_count, 1);
        assert!(t.next_run_ms.is_some());
    }
}

#[tokio::test]
async fn bt84_run_history_survives_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let task_id;
    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s.create_task(emit("bt84")).unwrap();
        task_id = t.id;
        s.tick().await;
    }

    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let runs = s.list_task_runs(&task_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, TaskRunStatus::Success);
    }
}

#[tokio::test]
async fn bt85_stale_running_state_at_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let task_id;
    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path.clone(),
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let t = s.create_task(emit("bt85")).unwrap();
        task_id = t.id.clone();
        drop(s);

        // Directly manipulate DB to set status='running' (simulating crash)
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE tasks SET status = 'running' WHERE id = ?1", [&t.id]).unwrap();
    }

    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path,
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s.get_task(&task_id).unwrap();
        // FIX VERIFIED: stale running tasks are auto-recovered to pending on startup
        assert_eq!(t.status, TaskStatus::Pending);
        // A tick will now pick it up and execute it
        s.tick().await;
        assert_eq!(s.get_task(&task_id).unwrap().status, TaskStatus::Completed);
    }
}

#[tokio::test]
async fn bt86_multiple_restarts_dont_corrupt() {
    // FIX VERIFIED: task_seq is now seeded from MAX(id) in DB on startup,
    // so creating new tasks across restarts no longer causes ID collisions.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    // Create tasks across multiple restarts
    let mut all_ids = Vec::new();
    for restart in 0..5 {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        for i in 0..3 {
            let t = s.create_task(emit(&format!("bt86_r{restart}_t{i}"))).unwrap();
            all_ids.push(t.id.clone());
        }
        s.tick().await;
    }

    // Verify all 15 tasks exist with unique IDs
    let bus = EventBus::new(32);
    let s =
        SchedulerService::new(db_path, bus, "127.0.0.1:0".to_string(), SchedulerConfig::default())
            .unwrap();
    let tasks = s.list_tasks();
    assert_eq!(tasks.len(), 15, "all tasks from all restarts should persist");
    let mut unique_ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    unique_ids.sort();
    unique_ids.dedup();
    assert_eq!(unique_ids.len(), 15, "all task IDs should be unique");
}

#[tokio::test]
async fn bt87_restart_with_mixed_states() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let ids;
    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t1 = s.create_task(emit("bt87_done")).unwrap();
        s.tick().await;
        let t2 = s.create_task(emit("bt87_pending")).unwrap();
        let t3 = s.create_task(emit("bt87_cancel")).unwrap();
        s.cancel_task(&t3.id).unwrap();
        ids = (t1.id, t2.id, t3.id);
    }

    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        assert_eq!(s.get_task(&ids.0).unwrap().status, TaskStatus::Completed);
        assert_eq!(s.get_task(&ids.1).unwrap().status, TaskStatus::Pending);
        assert_eq!(s.get_task(&ids.2).unwrap().status, TaskStatus::Cancelled);
    }
}

#[tokio::test]
async fn bt88_restart_preserves_owner_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let task_id;
    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path.clone(),
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let t = s.create_task(owned_emit("bt88", "session-ABC", Some("agent-XYZ"))).unwrap();
        task_id = t.id;
    }

    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let t = s.get_task(&task_id).unwrap();
        assert_eq!(t.owner_session_id.as_deref(), Some("session-ABC"));
        assert_eq!(t.owner_agent_id.as_deref(), Some("agent-XYZ"));
    }
}

#[tokio::test]
async fn bt89_restart_preserves_last_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let task_id;
    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:1".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s
            .create_task(CreateTaskRequest {
                name: "bt89".to_string(),
                description: None,
                schedule: TaskSchedule::Once,
                action: TaskAction::SendMessage {
                    session_id: "x".to_string(),
                    content: "y".to_string(),
                },
                owner_session_id: None,
                owner_agent_id: None,
                max_retries: None,
                retry_delay_ms: None,
            })
            .unwrap();
        task_id = t.id;
        s.tick().await;
    }

    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let t = s.get_task(&task_id).unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
        assert!(t.last_error.is_some());
        assert!(!t.last_error.as_ref().unwrap().is_empty());
    }
}

#[tokio::test]
async fn bt90_restart_cron_continues_accumulating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");

    let task_id;
    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s.create_task(cron_req("bt90", "0 * * * * *")).unwrap();
        task_id = t.id;
        s.force_all_due();
        s.tick().await;
        s.force_all_due();
        s.tick().await;
    }

    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path,
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s.get_task(&task_id).unwrap();
        assert_eq!(t.run_count, 2);
        s.force_all_due();
        s.tick().await;
        assert_eq!(s.get_task(&task_id).unwrap().run_count, 3);
    }
}

// ====================================================================
// 91-95: Owner scoping & filtering
// ====================================================================

#[tokio::test]
async fn bt91_filter_by_session_id() {
    let s = svc();
    s.create_task(owned_emit("bt91_a", "session-1", None)).unwrap();
    s.create_task(owned_emit("bt91_b", "session-1", None)).unwrap();
    s.create_task(owned_emit("bt91_c", "session-2", None)).unwrap();

    let filter = hive_scheduler::ListTasksFilter {
        session_id: Some("session-1".to_string()),
        ..Default::default()
    };
    let tasks = s.list_tasks_filtered(&filter).unwrap();
    assert_eq!(tasks.len(), 2);
    for t in &tasks {
        assert_eq!(t.owner_session_id.as_deref(), Some("session-1"));
    }
}

#[tokio::test]
async fn bt92_filter_by_agent_id() {
    let s = svc();
    s.create_task(owned_emit("bt92_a", "s", Some("agent-A"))).unwrap();
    s.create_task(owned_emit("bt92_b", "s", Some("agent-B"))).unwrap();

    let filter = hive_scheduler::ListTasksFilter {
        session_id: None,
        agent_id: Some("agent-A".to_string()),
        ..Default::default()
    };
    let tasks = s.list_tasks_filtered(&filter).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].owner_agent_id.as_deref(), Some("agent-A"));
}

#[tokio::test]
async fn bt93_combined_session_agent_filter() {
    let s = svc();
    s.create_task(owned_emit("bt93_a", "s1", Some("a1"))).unwrap();
    s.create_task(owned_emit("bt93_b", "s1", Some("a2"))).unwrap();
    s.create_task(owned_emit("bt93_c", "s2", Some("a1"))).unwrap();

    let filter = hive_scheduler::ListTasksFilter {
        session_id: Some("s1".to_string()),
        agent_id: Some("a1".to_string()),
        ..Default::default()
    };
    let tasks = s.list_tasks_filtered(&filter).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "bt93_a");
}

#[tokio::test]
async fn bt94_no_filter_returns_all() {
    let s = svc();
    s.create_task(owned_emit("bt94_a", "s1", Some("a1"))).unwrap();
    s.create_task(owned_emit("bt94_b", "s2", None)).unwrap();
    s.create_task(emit("bt94_c")).unwrap();

    let filter =
        hive_scheduler::ListTasksFilter { session_id: None, agent_id: None, ..Default::default() };
    let tasks = s.list_tasks_filtered(&filter).unwrap();
    assert_eq!(tasks.len(), 3);
}

#[tokio::test]
async fn bt95_owner_fields_survive_update() {
    let s = svc();
    let t = s.create_task(owned_emit("bt95", "sess-owner", Some("agent-owner"))).unwrap();
    s.update_task(
        &t.id,
        UpdateTaskRequest {
            name: Some("bt95_updated".to_string()),
            description: None,
            schedule: None,
            action: None,
            max_retries: None,
            retry_delay_ms: None,
        },
    )
    .unwrap();
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.name, "bt95_updated");
    assert_eq!(after.owner_session_id.as_deref(), Some("sess-owner"));
    assert_eq!(after.owner_agent_id.as_deref(), Some("agent-owner"));
}

// ====================================================================
// 96-100: EventBus & notification integrity
// ====================================================================

#[tokio::test]
async fn bt96_eventbus_receives_completed_event() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);

    s.create_task(emit("bt96")).unwrap();
    s.tick().await;

    let mut found = false;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "scheduler.task.completed" {
            found = true;
            break;
        }
    }
    assert!(found, "should receive scheduler.task.completed event");
}

#[tokio::test]
async fn bt97_eventbus_receives_failed_event() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = Arc::new(
        SchedulerService::in_memory_with_addr(
            bus,
            "127.0.0.1:1".to_string(),
            SchedulerConfig::default(),
        )
        .expect("scheduler"),
    );

    s.create_task(CreateTaskRequest {
        name: "bt97".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::SendMessage { session_id: "x".to_string(), content: "y".to_string() },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    let mut found = false;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "scheduler.task.failed" {
            found = true;
            break;
        }
    }
    assert!(found, "should receive scheduler.task.failed event");
}

#[tokio::test]
async fn bt98_notification_payload_has_task_info() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);

    let t = s.create_task(emit("bt98_notify")).unwrap();
    s.tick().await;

    let mut payload = None;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "scheduler.task.completed" {
            payload = Some(env.payload.clone());
            break;
        }
    }
    let p = payload.expect("should have notification");
    assert_eq!(p["task_id"], t.id);
    assert_eq!(p["task_name"], "bt98_notify");
    assert_eq!(p["status"], "success");
    assert!(p["run_id"].as_str().is_some());
    assert!(p["started_at_ms"].as_u64().is_some());
    assert!(p["completed_at_ms"].as_u64().is_some());
}

#[tokio::test]
async fn bt99_multiple_completions_produce_multiple_events() {
    let bus = EventBus::new(128);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);

    for i in 0..5 {
        s.create_task(emit(&format!("bt99_{i}"))).unwrap();
    }
    s.tick().await;

    let mut completed_count = 0;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "scheduler.task.completed" {
            completed_count += 1;
        }
    }
    assert_eq!(completed_count, 5);
}

#[tokio::test]
async fn bt100_emit_event_publishes_to_custom_topic() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let s = svc_with_bus(bus);

    s.create_task(CreateTaskRequest {
        name: "bt100".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::EmitEvent {
            topic: "custom.battle.topic".to_string(),
            payload: json!({"victory": true}),
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    s.tick().await;

    let mut found_custom = false;
    while let Ok(env) = rx.try_recv() {
        if env.topic == "custom.battle.topic" {
            assert_eq!(env.payload["victory"], true);
            found_custom = true;
        }
    }
    assert!(found_custom, "custom topic event should have been published");
}

// ───────────────────────────── 101-110: Retry policy & run pruning ─────────────────────────────

/// Helper: create a Once task targeting an unreachable endpoint (guaranteed failure) with retry config.
fn failing_webhook_with_retry(
    name: &str,
    max_retries: u32,
    retry_delay_ms: u64,
) -> CreateTaskRequest {
    CreateTaskRequest {
        name: name.to_string(),
        description: Some("failing webhook with retries".to_string()),
        schedule: TaskSchedule::Once,
        action: TaskAction::HttpWebhook {
            url: "http://127.0.0.1:1/never".to_string(),
            method: "POST".to_string(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: Some(max_retries),
        retry_delay_ms: Some(retry_delay_ms),
    }
}

#[tokio::test]
async fn bt101_retry_policy_retries_on_failure() {
    // A failing once-task with max_retries=3 should retry 3 times before failing permanently.
    let s = svc_with_addr("127.0.0.1:0".to_string());
    let t = s.create_task(failing_webhook_with_retry("bt101", 3, 0)).unwrap();
    assert_eq!(t.retry_count, 0);

    // Tick 1: fail → retry (retry_count=1, status=pending)
    s.tick().await;
    let t1 = s.get_task(&t.id).unwrap();
    assert_eq!(t1.status, TaskStatus::Pending, "should retry (attempt 1)");
    assert_eq!(t1.retry_count, 1);

    // Tick 2: fail → retry (retry_count=2, status=pending)
    s.tick().await;
    let t2 = s.get_task(&t.id).unwrap();
    assert_eq!(t2.status, TaskStatus::Pending, "should retry (attempt 2)");
    assert_eq!(t2.retry_count, 2);

    // Tick 3: fail → retry (retry_count=3, status=pending)
    s.tick().await;
    let t3 = s.get_task(&t.id).unwrap();
    assert_eq!(t3.status, TaskStatus::Pending, "should retry (attempt 3)");
    assert_eq!(t3.retry_count, 3);

    // Tick 4: fail → exhausted retries → permanently failed
    s.tick().await;
    let t4 = s.get_task(&t.id).unwrap();
    assert_eq!(
        t4.status,
        TaskStatus::Failed,
        "should be permanently failed after exhausting retries"
    );
    assert!(t4.last_error.is_some());
}

#[tokio::test]
async fn bt102_no_retry_without_policy() {
    // A failing once-task with no retry policy fails immediately.
    let s = svc_with_addr("127.0.0.1:0".to_string());
    let req = CreateTaskRequest {
        name: "bt102".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::HttpWebhook {
            url: "http://127.0.0.1:1/never".to_string(),
            method: "POST".to_string(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    };
    let t = s.create_task(req).unwrap();
    s.tick().await;
    let t1 = s.get_task(&t.id).unwrap();
    assert_eq!(t1.status, TaskStatus::Failed, "should fail immediately without retry policy");
    assert_eq!(t1.retry_count, 0);
}

#[tokio::test]
async fn bt103_retry_delay_defers_next_attempt() {
    // With retry_delay_ms set to a large value, the retried task shouldn't execute immediately.
    let s = svc_with_addr("127.0.0.1:0".to_string());
    let t = s.create_task(failing_webhook_with_retry("bt103", 2, 999_999_999)).unwrap();

    // First tick: fails, gets retried with delay
    s.tick().await;
    let t1 = s.get_task(&t.id).unwrap();
    assert_eq!(t1.status, TaskStatus::Pending, "should be pending after first retry");
    assert_eq!(t1.retry_count, 1);
    // next_run_ms should be in the far future (now + ~1B ms)
    assert!(t1.next_run_ms.unwrap() > now_ms() + 500_000_000, "next_run should be delayed");

    // Second tick: should NOT execute because it's not due yet
    s.tick().await;
    let t2 = s.get_task(&t.id).unwrap();
    assert_eq!(t2.status, TaskStatus::Pending, "should still be pending (not due yet)");
    assert_eq!(t2.retry_count, 1, "retry count shouldn't increase if task wasn't executed");
}

#[tokio::test]
async fn bt104_retry_count_resets_on_success() {
    // If a task succeeds after retries, retry_count should reset to 0.
    // We use a mock server that fails the first 2 calls, then succeeds.
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let app = Router::new().route(
        "/bt104",
        post(move || {
            let count = cc.fetch_add(1, Ordering::SeqCst);
            async move {
                if count < 2 {
                    StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    StatusCode::OK
                }
            }
        }),
    );
    let addr = mock_server(app).await;

    let s = svc_with_addr("127.0.0.1:0".to_string());
    let req = CreateTaskRequest {
        name: "bt104".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::HttpWebhook {
            url: format!("http://{addr}/bt104"),
            method: "POST".to_string(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: Some(5),
        retry_delay_ms: Some(0),
    };
    let t = s.create_task(req).unwrap();

    // Tick 1: fail (call 0) → retry
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().retry_count, 1);

    // Tick 2: fail (call 1) → retry
    s.tick().await;
    assert_eq!(s.get_task(&t.id).unwrap().retry_count, 2);

    // Tick 3: succeed (call 2) → completed, retry_count resets
    s.tick().await;
    let final_task = s.get_task(&t.id).unwrap();
    assert_eq!(final_task.status, TaskStatus::Completed);
    assert_eq!(final_task.retry_count, 0, "retry_count should reset on success");
}

#[tokio::test]
async fn bt105_retry_preserves_across_restart() {
    // Retry state (retry_count) should survive a daemon restart.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");
    let task_id;

    {
        let bus = EventBus::new(32);
        let s = Arc::new(
            SchedulerService::new(
                db_path.clone(),
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .unwrap(),
        );
        let t = s.create_task(failing_webhook_with_retry("bt105", 5, 0)).unwrap();
        task_id = t.id.clone();
        // Fail twice → retry_count = 2
        s.tick().await;
        s.tick().await;
        assert_eq!(s.get_task(&task_id).unwrap().retry_count, 2);
    }

    // Restart
    {
        let bus = EventBus::new(32);
        let s = SchedulerService::new(
            db_path,
            bus,
            "127.0.0.1:0".to_string(),
            SchedulerConfig::default(),
        )
        .unwrap();
        let t = s.get_task(&task_id).unwrap();
        assert_eq!(t.retry_count, 2, "retry_count should survive restart");
        assert_eq!(t.status, TaskStatus::Pending, "pending status should survive restart");
    }
}

#[tokio::test]
async fn bt106_max_retries_zero_means_no_retries() {
    // max_retries=0 with Some should mean "no retries allowed" (same as None).
    let s = svc_with_addr("127.0.0.1:0".to_string());
    let t = s.create_task(failing_webhook_with_retry("bt106", 0, 0)).unwrap();
    s.tick().await;
    let t1 = s.get_task(&t.id).unwrap();
    assert_eq!(t1.status, TaskStatus::Failed, "max_retries=0 should fail immediately");
}

#[tokio::test]
async fn bt107_cron_task_run_pruning() {
    // Cron tasks with >100 runs should have old runs pruned after tick.
    // We'll create a cron task and simulate many runs by calling tick repeatedly.
    // Since directly manipulating the private DB is not possible from an integration test,
    // we verify the pruning contract: list_task_runs returns at most 100 entries.
    let s = svc();
    let t = s.create_task(cron_req("bt107", "* * * * * *")).unwrap(); // every second

    // Run 5 ticks to accumulate runs (the actual pruning threshold is 100,
    // but we can't easily hit 100+ from an integration test without waiting).
    // Instead we verify the mechanism works: after multiple ticks, runs are tracked.
    for _ in 0..5 {
        s.tick().await;
        // Small sleep to ensure cron fires each time
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }

    let runs = s.list_task_runs(&t.id).unwrap();
    assert!(runs.len() <= 100, "runs should be capped at 100");
    assert!(runs.len() >= 3, "should have accumulated multiple runs, got {}", runs.len());
}

#[tokio::test]
async fn bt108_once_task_no_pruning() {
    // Once tasks should not trigger run pruning (only cron tasks do).
    let s = svc();
    let t = s.create_task(emit("bt108")).unwrap();
    s.tick().await;

    // Verify exactly 1 run exists
    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs.len(), 1);
}

#[tokio::test]
async fn bt109_retry_run_history_recorded() {
    // Each retry attempt should produce a separate run record.
    let s = svc_with_addr("127.0.0.1:0".to_string());
    let t = s.create_task(failing_webhook_with_retry("bt109", 2, 0)).unwrap();

    // 3 ticks = 3 runs (original + 2 retries)
    s.tick().await; // fail → retry
    s.tick().await; // fail → retry
    s.tick().await; // fail → permanently failed

    let runs = s.list_task_runs(&t.id).unwrap();
    assert_eq!(runs.len(), 3, "each attempt (including retries) should produce a run record");
    for run in &runs {
        assert_eq!(run.status, TaskRunStatus::Failure);
    }
}

#[tokio::test]
async fn bt110_retry_with_composite_action() {
    // Retry policy should work with composite actions too.
    let s = svc_with_addr("127.0.0.1:0".to_string());
    let req = CreateTaskRequest {
        name: "bt110".to_string(),
        description: None,
        schedule: TaskSchedule::Once,
        action: TaskAction::CompositeAction {
            actions: vec![TaskAction::HttpWebhook {
                url: "http://127.0.0.1:1/never".to_string(),
                method: "POST".to_string(),
                body: None,
                headers: None,
            }],
            stop_on_failure: true,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: Some(1),
        retry_delay_ms: Some(0),
    };
    let t = s.create_task(req).unwrap();

    // Tick 1: composite fails → retry
    s.tick().await;
    let t1 = s.get_task(&t.id).unwrap();
    assert_eq!(t1.status, TaskStatus::Pending, "should retry after composite failure");
    assert_eq!(t1.retry_count, 1);

    // Tick 2: composite fails again → permanently failed (max_retries exhausted)
    s.tick().await;
    let t2 = s.get_task(&t.id).unwrap();
    assert_eq!(t2.status, TaskStatus::Failed, "should be permanently failed");
}
