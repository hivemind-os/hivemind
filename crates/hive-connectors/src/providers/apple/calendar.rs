//! Apple Calendar service via EventKit framework.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use objc2_event_kit::{
    EKAuthorizationStatus, EKEntityType, EKEvent, EKEventStatus, EKEventStore, EKSpan,
};
use objc2_foundation::NSError;

use hive_classification::DataClass;
use hive_contracts::connectors::{
    Attendee, AttendeeResponse, CalendarEvent, EventStatus, EventUpdate, FreeBusySlot,
    FreeBusyStatus, NewCalendarEvent,
};

use super::bridge::{self, SendRetained};
use crate::services::CalendarService;

/// Apple Calendar service backed by EventKit.
pub struct AppleCalendar {
    connector_id: String,
    store: SendRetained<EKEventStore>,
    /// Serialize all EventKit operations — EKEventStore is not thread-safe.
    lock: std::sync::Arc<std::sync::Mutex<()>>,
}

impl AppleCalendar {
    pub fn new(connector_id: &str) -> Self {
        let store = unsafe { EKEventStore::new() };
        Self {
            connector_id: connector_id.to_string(),
            store: SendRetained(store),
            lock: std::sync::Arc::new(std::sync::Mutex::new(())),
        }
    }
}

/// Request full access to calendar events. Blocks until the user responds
/// to the TCC prompt (or the cached authorization is available).
fn request_calendar_access(store: &EKEventStore) -> Result<()> {
    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    #[allow(deprecated)]
    if status == EKAuthorizationStatus::Authorized || status == EKAuthorizationStatus::FullAccess {
        return Ok(());
    }

    if status == EKAuthorizationStatus::Denied {
        anyhow::bail!(
            "Calendar access has not been granted. \
             Please open System Settings → Privacy & Security → Calendars \
             and enable access for HiveMind OS, then try again."
        );
    }

    let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
    let block: block2::RcBlock<dyn Fn(objc2::runtime::Bool, *mut NSError)> =
        block2::RcBlock::new(move |granted: objc2::runtime::Bool, error: *mut NSError| {
            if granted.as_bool() {
                let _ = tx.send(Ok(()));
            } else if !error.is_null() {
                let err_ref = unsafe { &*error };
                let desc = err_ref.localizedDescription();
                let _ = tx.send(Err(anyhow!("{}", bridge::nsstring_to_string(&desc))));
            } else {
                let _ = tx.send(Err(anyhow!(
                    "Calendar access was not granted. \
                     Please open System Settings → Privacy & Security → Calendars \
                     and enable access for HiveMind OS, then try again."
                )));
            }
        });

    unsafe {
        store.requestFullAccessToEventsWithCompletion(&*block as *const _ as *mut _);
    }

    rx.recv().map_err(|_| anyhow!("Calendar access request channel closed"))?
}

/// Convert an EKEvent to our CalendarEvent contract type.
fn ekevent_to_calendar_event(event: &EKEvent, connector_id: &str) -> CalendarEvent {
    let id = unsafe { event.eventIdentifier() }
        .map(|s| bridge::nsstring_to_string(&s))
        .unwrap_or_default();

    let title = bridge::nsstring_to_string(unsafe { &event.title() });

    let start = bridge::nsdate_to_rfc3339(unsafe { &event.startDate() });
    let end = bridge::nsdate_to_rfc3339(unsafe { &event.endDate() });

    let is_all_day = unsafe { event.isAllDay() };

    let location = unsafe { event.location() }.map(|s| bridge::nsstring_to_string(&s));
    let description = unsafe { event.notes() }.map(|s| bridge::nsstring_to_string(&s));

    let organizer_name = unsafe { event.organizer() }
        .and_then(|p| unsafe { p.name() })
        .map(|s| bridge::nsstring_to_string(&s));

    let attendees = unsafe { event.attendees() }
        .map(|arr| {
            arr.to_vec()
                .into_iter()
                .filter_map(|participant| {
                    let url = unsafe { participant.URL() };
                    let url_str = url
                        .absoluteString()
                        .map(|s| bridge::nsstring_to_string(&s))
                        .unwrap_or_default();
                    let email = url_str.strip_prefix("mailto:").unwrap_or(&url_str).to_string();
                    if email.is_empty() {
                        return None;
                    }
                    let name =
                        unsafe { participant.name() }.map(|s| bridge::nsstring_to_string(&s));
                    Some(Attendee { email, name, response: AttendeeResponse::None })
                })
                .collect()
        })
        .unwrap_or_default();

    let status = unsafe { event.status() };
    let event_status = match status {
        EKEventStatus::Confirmed => EventStatus::Confirmed,
        EKEventStatus::Tentative => EventStatus::Tentative,
        EKEventStatus::Canceled => EventStatus::Cancelled,
        _ => EventStatus::Confirmed,
    };

    CalendarEvent {
        id,
        connector_id: connector_id.to_string(),
        title,
        description,
        start,
        end,
        is_all_day,
        timezone: None,
        location,
        attendees,
        organizer: organizer_name,
        status: event_status,
        data_class: DataClass::Internal,
        web_link: None,
    }
}

#[async_trait]
impl CalendarService for AppleCalendar {
    fn name(&self) -> &str {
        "Apple Calendar"
    }

    async fn test_connection(&self) -> Result<()> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_calendar_access(&store)
        })
        .await
        .context("EventKit test_connection task panicked")?
    }

    async fn list_events(&self, start: &str, end: &str) -> Result<Vec<CalendarEvent>> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let start_date = bridge::rfc3339_to_nsdate(start)?;
        let end_date = bridge::rfc3339_to_nsdate(end)?;
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_calendar_access(&store)?;

            let predicate = unsafe {
                store.predicateForEventsWithStartDate_endDate_calendars(
                    &start_date,
                    &end_date,
                    None,
                )
            };

            let events = unsafe { store.eventsMatchingPredicate(&predicate) };
            let mut result = Vec::with_capacity(events.count());
            for event in events.to_vec() {
                result.push(ekevent_to_calendar_event(&event, &connector_id));
            }
            Ok(result)
        })
        .await
        .context("EventKit list_events task panicked")?
    }

    async fn get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let eid = event_id.to_string();
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_calendar_access(&store)?;
            let ns_id = bridge::string_to_nsstring(&eid);
            let event = unsafe { store.eventWithIdentifier(&ns_id) }
                .ok_or_else(|| anyhow!("event not found: {}", eid))?;
            Ok(ekevent_to_calendar_event(&event, &connector_id))
        })
        .await
        .context("EventKit get_event task panicked")?
    }

    async fn create_event(&self, new_event: &NewCalendarEvent) -> Result<CalendarEvent> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let new = new_event.clone();
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_calendar_access(&store)?;
            let event = unsafe { EKEvent::eventWithEventStore(&store) };

            // Set the target calendar — required before saving.
            let calendar = unsafe { store.defaultCalendarForNewEvents() }
                .ok_or_else(|| anyhow!("no default calendar configured for new events"))?;
            unsafe { event.setCalendar(Some(&calendar)) };

            let title = bridge::string_to_nsstring(&new.title);
            unsafe { event.setTitle(Some(&title)) };

            let start = bridge::rfc3339_to_nsdate(&new.start)?;
            unsafe { event.setStartDate(Some(&start)) };

            let end = bridge::rfc3339_to_nsdate(&new.end)?;
            unsafe { event.setEndDate(Some(&end)) };

            unsafe { event.setAllDay(new.is_all_day) };

            if let Some(ref loc) = new.location {
                let ns_loc = bridge::string_to_nsstring(loc);
                unsafe { event.setLocation(Some(&ns_loc)) };
            }

            if let Some(ref desc) = new.description {
                let ns_desc = bridge::string_to_nsstring(desc);
                unsafe { event.setNotes(Some(&ns_desc)) };
            }

            unsafe {
                store
                    .saveEvent_span_error(&event, EKSpan::ThisEvent)
                    .map_err(|e| bridge::retained_nserror_to_anyhow(&e))?;
            }

            Ok(ekevent_to_calendar_event(&event, &connector_id))
        })
        .await
        .context("EventKit create_event task panicked")?
    }

    async fn update_event(&self, event_id: &str, update: &EventUpdate) -> Result<CalendarEvent> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let eid = event_id.to_string();
        let upd = update.clone();
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_calendar_access(&store)?;
            let ns_id = bridge::string_to_nsstring(&eid);
            let event = unsafe { store.eventWithIdentifier(&ns_id) }
                .ok_or_else(|| anyhow!("event not found: {}", eid))?;

            if let Some(ref title) = upd.title {
                let ns_title = bridge::string_to_nsstring(title);
                unsafe { event.setTitle(Some(&ns_title)) };
            }

            if let Some(ref desc) = upd.description {
                let ns_desc = bridge::string_to_nsstring(desc);
                unsafe { event.setNotes(Some(&ns_desc)) };
            }

            if let Some(ref start) = upd.start {
                let date = bridge::rfc3339_to_nsdate(start)?;
                unsafe { event.setStartDate(Some(&date)) };
            }

            if let Some(ref end) = upd.end {
                let date = bridge::rfc3339_to_nsdate(end)?;
                unsafe { event.setEndDate(Some(&date)) };
            }

            if let Some(ref loc) = upd.location {
                let ns_loc = bridge::string_to_nsstring(loc);
                unsafe { event.setLocation(Some(&ns_loc)) };
            }

            unsafe {
                store
                    .saveEvent_span_error(&event, EKSpan::ThisEvent)
                    .map_err(|e| bridge::retained_nserror_to_anyhow(&e))?;
            }

            Ok(ekevent_to_calendar_event(&event, &connector_id))
        })
        .await
        .context("EventKit update_event task panicked")?
    }

    async fn delete_event(&self, event_id: &str) -> Result<()> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let eid = event_id.to_string();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_calendar_access(&store)?;
            let ns_id = bridge::string_to_nsstring(&eid);
            let event = unsafe { store.eventWithIdentifier(&ns_id) }
                .ok_or_else(|| anyhow!("event not found: {}", eid))?;

            unsafe {
                store
                    .removeEvent_span_error(&event, EKSpan::ThisEvent)
                    .map_err(|e| bridge::retained_nserror_to_anyhow(&e))?;
            }

            Ok(())
        })
        .await
        .context("EventKit delete_event task panicked")?
    }

    async fn check_availability(&self, start: &str, end: &str) -> Result<Vec<FreeBusySlot>> {
        let events = self.list_events(start, end).await?;

        let mut busy: Vec<FreeBusySlot> = events
            .iter()
            .filter(|e| e.status != EventStatus::Cancelled)
            .map(|e| FreeBusySlot {
                start: e.start.clone(),
                end: e.end.clone(),
                status: match e.status {
                    EventStatus::Tentative => FreeBusyStatus::Tentative,
                    _ => FreeBusyStatus::Busy,
                },
            })
            .collect();

        busy.sort_by(|a, b| a.start.cmp(&b.start));
        Ok(busy)
    }
}
