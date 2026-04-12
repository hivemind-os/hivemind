use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::connectors::*;
use tokio::sync::OnceCell;
use tracing::info;

use super::graph_client::GraphClient;
use crate::services::CalendarService;

pub struct MicrosoftCalendar {
    graph: Arc<GraphClient>,
    default_class: DataClass,
    user_timezone: OnceCell<String>,
}

impl MicrosoftCalendar {
    pub fn new(graph: Arc<GraphClient>, default_class: DataClass) -> Self {
        Self { graph, default_class, user_timezone: OnceCell::new() }
    }

    /// Fetch and cache the user's mailbox timezone from Graph mailboxSettings.
    async fn timezone(&self) -> &str {
        self.user_timezone
            .get_or_init(|| async {
                match self.graph.get("/me/mailboxSettings/timeZone").await {
                    Ok(body) => body["value"]
                        .as_str()
                        .unwrap_or("UTC")
                        .to_string(),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to fetch mailbox timezone, defaulting to UTC");
                        "UTC".to_string()
                    }
                }
            })
            .await
            .as_str()
    }

    fn parse_event(&self, item: &serde_json::Value) -> CalendarEvent {
        let attendees: Vec<Attendee> = item["attendees"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|a| Attendee {
                        email: a["emailAddress"]["address"].as_str().unwrap_or("").to_string(),
                        name: a["emailAddress"]["name"].as_str().map(|s| s.to_string()),
                        response: match a["status"]["response"].as_str().unwrap_or("none") {
                            "accepted" => AttendeeResponse::Accepted,
                            "declined" => AttendeeResponse::Declined,
                            "tentativelyAccepted" => AttendeeResponse::Tentative,
                            _ => AttendeeResponse::None,
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();

        let status = if item["isCancelled"].as_bool().unwrap_or(false) {
            EventStatus::Cancelled
        } else {
            match item["showAs"].as_str().unwrap_or("busy") {
                "tentative" | "free" => EventStatus::Tentative,
                _ => EventStatus::Confirmed,
            }
        };

        CalendarEvent {
            id: item["id"].as_str().unwrap_or("").to_string(),
            connector_id: self.graph.connector_id().to_string(),
            title: item["subject"].as_str().unwrap_or("").to_string(),
            description: item["bodyPreview"].as_str().map(|s| s.to_string()),
            start: item["start"]["dateTime"].as_str().unwrap_or("").to_string(),
            end: item["end"]["dateTime"].as_str().unwrap_or("").to_string(),
            is_all_day: item["isAllDay"].as_bool().unwrap_or(false),
            timezone: item["start"]["timeZone"].as_str().map(|s| s.to_string()),
            location: item["location"]["displayName"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            attendees,
            organizer: item["organizer"]["emailAddress"]["address"].as_str().map(|s| s.to_string()),
            status,
            data_class: self.default_class,
            web_link: item["webLink"].as_str().map(|s| s.to_string()),
        }
    }
}

#[async_trait]
impl CalendarService for MicrosoftCalendar {
    fn name(&self) -> &str {
        "Microsoft 365 Calendar"
    }

    async fn test_connection(&self) -> Result<()> {
        self.graph.get("/me/calendars?$top=1").await?;
        info!(
            connector = %self.graph.connector_id(),
            "Calendar connection test OK"
        );
        Ok(())
    }

    async fn list_events(&self, start: &str, end: &str) -> Result<Vec<CalendarEvent>> {
        let path = format!(
            "/me/calendarview?startDateTime={}&endDateTime={}\
             &$top=50&$orderby=start/dateTime\
             &$select=id,subject,bodyPreview,start,end,isAllDay,\
             location,attendees,organizer,showAs,isCancelled,webLink",
            urlencoding::encode(start),
            urlencoding::encode(end)
        );
        let body = self.graph.get(&path).await?;
        let items = body["value"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|item| self.parse_event(item)).collect())
    }

    async fn get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        let path = format!(
            "/me/events/{event_id}?$select=id,subject,bodyPreview,start,end,\
             isAllDay,location,attendees,organizer,showAs,isCancelled,webLink"
        );
        let body = self.graph.get(&path).await?;
        Ok(self.parse_event(&body))
    }

    async fn create_event(&self, event: &NewCalendarEvent) -> Result<CalendarEvent> {
        let user_tz = self.timezone().await;
        let tz = event.timezone.as_deref().unwrap_or(user_tz);

        let mut payload = serde_json::json!({
            "subject": event.title,
            "start": { "dateTime": &event.start, "timeZone": tz },
            "end": { "dateTime": &event.end, "timeZone": tz },
            "isAllDay": event.is_all_day,
        });
        if let Some(desc) = &event.description {
            payload["body"] = serde_json::json!({ "contentType": "Text", "content": desc });
        }
        if let Some(loc) = &event.location {
            payload["location"] = serde_json::json!({ "displayName": loc });
        }
        if !event.attendees.is_empty() {
            let attendees: Vec<serde_json::Value> = event
                .attendees
                .iter()
                .map(|email| {
                    serde_json::json!({
                        "emailAddress": { "address": email },
                        "type": "required"
                    })
                })
                .collect();
            payload["attendees"] = serde_json::json!(attendees);
        }

        let resp = self.graph.post("/me/events", &payload).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Create event failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await.context("parsing created event")?;
        Ok(self.parse_event(&body))
    }

    async fn update_event(&self, event_id: &str, update: &EventUpdate) -> Result<CalendarEvent> {
        let user_tz = self.timezone().await;
        let tz = update.timezone.as_deref().unwrap_or(user_tz);

        let mut payload = serde_json::Map::new();
        if let Some(title) = &update.title {
            payload.insert("subject".to_string(), serde_json::json!(title));
        }
        if let Some(desc) = &update.description {
            payload.insert(
                "body".to_string(),
                serde_json::json!({ "contentType": "Text", "content": desc }),
            );
        }
        if let Some(start) = &update.start {
            payload.insert(
                "start".to_string(),
                serde_json::json!({ "dateTime": start, "timeZone": tz }),
            );
        }
        if let Some(end) = &update.end {
            payload
                .insert("end".to_string(), serde_json::json!({ "dateTime": end, "timeZone": tz }));
        }
        if let Some(loc) = &update.location {
            payload.insert("location".to_string(), serde_json::json!({ "displayName": loc }));
        }

        let path = format!("/me/events/{event_id}");
        let resp = self.graph.patch(&path, &serde_json::Value::Object(payload)).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Update event failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await.context("parsing updated event")?;
        Ok(self.parse_event(&body))
    }

    async fn delete_event(&self, event_id: &str) -> Result<()> {
        let path = format!("/me/events/{event_id}");
        let resp = self.graph.delete(&path).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Delete event failed ({status}): {err}");
        }
        Ok(())
    }

    async fn check_availability(&self, start: &str, end: &str) -> Result<Vec<FreeBusySlot>> {
        let payload = serde_json::json!({
            "schedules": ["me"],
            "startTime": { "dateTime": start, "timeZone": "UTC" },
            "endTime": { "dateTime": end, "timeZone": "UTC" },
            "availabilityViewInterval": 30
        });
        let resp = self.graph.post("/me/calendar/getSchedule", &payload).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Check availability failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await?;
        let schedules = body["value"].as_array().cloned().unwrap_or_default();
        let mut slots = Vec::new();
        for schedule in &schedules {
            if let Some(items) = schedule["scheduleItems"].as_array() {
                for item in items {
                    let status_str = item["status"].as_str().unwrap_or("busy");
                    slots.push(FreeBusySlot {
                        start: item["start"]["dateTime"].as_str().unwrap_or("").to_string(),
                        end: item["end"]["dateTime"].as_str().unwrap_or("").to_string(),
                        status: match status_str {
                            "free" => FreeBusyStatus::Free,
                            "tentative" => FreeBusyStatus::Tentative,
                            "oof" => FreeBusyStatus::OutOfOffice,
                            _ => FreeBusyStatus::Busy,
                        },
                    });
                }
            }
        }
        Ok(slots)
    }
}
