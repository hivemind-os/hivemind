use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::connectors::*;
use tokio::sync::OnceCell;
use tracing::info;

use super::google_client::GoogleClient;
use crate::services::CalendarService;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

pub struct GoogleCalendar {
    google: Arc<GoogleClient>,
    default_class: DataClass,
    user_timezone: OnceCell<String>,
}

impl GoogleCalendar {
    pub fn new(google: Arc<GoogleClient>, default_class: DataClass) -> Self {
        Self { google, default_class, user_timezone: OnceCell::new() }
    }

    /// Fetch and cache the user's IANA timezone from Google Calendar settings.
    async fn timezone(&self) -> &str {
        self.user_timezone
            .get_or_init(|| async {
                let url = format!("{CALENDAR_API}/users/me/settings/timezone");
                match self.google.get(&url).await {
                    Ok(body) => body["value"]
                        .as_str()
                        .unwrap_or("UTC")
                        .to_string(),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to fetch calendar timezone, defaulting to UTC");
                        "UTC".to_string()
                    }
                }
            })
            .await
            .as_str()
    }

    fn parse_event(&self, item: &serde_json::Value, tz: Option<&str>) -> CalendarEvent {
        let is_all_day = item["start"]["date"].as_str().is_some()
            && item["start"]["dateTime"].as_str().is_none();

        let start = item["start"]["dateTime"]
            .as_str()
            .or_else(|| item["start"]["date"].as_str())
            .unwrap_or("")
            .to_string();

        let end = item["end"]["dateTime"]
            .as_str()
            .or_else(|| item["end"]["date"].as_str())
            .unwrap_or("")
            .to_string();

        let event_tz = item["start"]["timeZone"].as_str().or(tz).map(|s| s.to_string());

        let attendees: Vec<Attendee> = item["attendees"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|a| Attendee {
                        email: a["email"].as_str().unwrap_or("").to_string(),
                        name: a["displayName"].as_str().map(|s| s.to_string()),
                        response: match a["responseStatus"].as_str().unwrap_or("needsAction") {
                            "accepted" => AttendeeResponse::Accepted,
                            "declined" => AttendeeResponse::Declined,
                            "tentative" => AttendeeResponse::Tentative,
                            _ => AttendeeResponse::None,
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();

        let status = match item["status"].as_str().unwrap_or("confirmed") {
            "tentative" => EventStatus::Tentative,
            "cancelled" => EventStatus::Cancelled,
            _ => EventStatus::Confirmed,
        };

        CalendarEvent {
            id: item["id"].as_str().unwrap_or("").to_string(),
            connector_id: self.google.connector_id().to_string(),
            title: item["summary"].as_str().unwrap_or("").to_string(),
            description: item["description"].as_str().map(|s| s.to_string()),
            start,
            end,
            is_all_day,
            timezone: event_tz,
            location: item["location"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            attendees,
            organizer: item["organizer"]["email"].as_str().map(|s| s.to_string()),
            status,
            data_class: self.default_class,
            web_link: item["htmlLink"].as_str().map(|s| s.to_string()),
        }
    }
}

#[async_trait]
impl CalendarService for GoogleCalendar {
    fn name(&self) -> &str {
        "Google Calendar"
    }

    async fn test_connection(&self) -> Result<()> {
        let url = format!("{CALENDAR_API}/users/me/calendarList?maxResults=1");
        self.google.get(&url).await?;
        info!(
            connector = %self.google.connector_id(),
            "Calendar connection test OK"
        );
        Ok(())
    }

    async fn list_events(&self, start: &str, end: &str) -> Result<Vec<CalendarEvent>> {
        let tz = self.timezone().await;
        let url = format!(
            "{CALENDAR_API}/calendars/primary/events\
             ?timeMin={}&timeMax={}\
             &maxResults=50&orderBy=startTime&singleEvents=true",
            urlencoding::encode(start),
            urlencoding::encode(end),
        );
        let body = self.google.get(&url).await?;
        let items = body["items"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|item| self.parse_event(item, Some(tz))).collect())
    }

    async fn get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        let tz = self.timezone().await;
        let url = format!("{CALENDAR_API}/calendars/primary/events/{event_id}");
        let body = self.google.get(&url).await?;
        Ok(self.parse_event(&body, Some(tz)))
    }

    async fn create_event(&self, event: &NewCalendarEvent) -> Result<CalendarEvent> {
        let user_tz = self.timezone().await;
        let tz = event.timezone.as_deref().unwrap_or(user_tz);

        let start_obj = if event.is_all_day {
            serde_json::json!({ "date": &event.start })
        } else {
            serde_json::json!({ "dateTime": &event.start, "timeZone": tz })
        };
        let end_obj = if event.is_all_day {
            serde_json::json!({ "date": &event.end })
        } else {
            serde_json::json!({ "dateTime": &event.end, "timeZone": tz })
        };

        let mut payload = serde_json::json!({
            "summary": event.title,
            "start": start_obj,
            "end": end_obj,
        });
        if let Some(desc) = &event.description {
            payload["description"] = serde_json::json!(desc);
        }
        if let Some(loc) = &event.location {
            payload["location"] = serde_json::json!(loc);
        }
        if !event.attendees.is_empty() {
            let attendees: Vec<serde_json::Value> =
                event.attendees.iter().map(|email| serde_json::json!({ "email": email })).collect();
            payload["attendees"] = serde_json::json!(attendees);
        }

        let url = format!("{CALENDAR_API}/calendars/primary/events");
        let resp = self.google.post(&url, &payload).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Create event failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await.context("parsing created event")?;
        Ok(self.parse_event(&body, Some(tz)))
    }

    async fn update_event(&self, event_id: &str, update: &EventUpdate) -> Result<CalendarEvent> {
        let user_tz = self.timezone().await;
        let tz = update.timezone.as_deref().unwrap_or(user_tz);

        let mut payload = serde_json::Map::new();
        if let Some(title) = &update.title {
            payload.insert("summary".to_string(), serde_json::json!(title));
        }
        if let Some(desc) = &update.description {
            payload.insert("description".to_string(), serde_json::json!(desc));
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
            payload.insert("location".to_string(), serde_json::json!(loc));
        }

        let url = format!("{CALENDAR_API}/calendars/primary/events/{event_id}");
        let resp = self.google.patch(&url, &serde_json::Value::Object(payload)).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Update event failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await.context("parsing updated event")?;
        Ok(self.parse_event(&body, Some(tz)))
    }

    async fn delete_event(&self, event_id: &str) -> Result<()> {
        let url = format!("{CALENDAR_API}/calendars/primary/events/{event_id}");
        let resp = self.google.delete(&url).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Delete event failed ({status}): {err}");
        }
        Ok(())
    }

    async fn check_availability(&self, start: &str, end: &str) -> Result<Vec<FreeBusySlot>> {
        let tz = self.timezone().await;
        let payload = serde_json::json!({
            "timeMin": start,
            "timeMax": end,
            "timeZone": tz,
            "items": [{ "id": "primary" }]
        });
        let url = format!("{CALENDAR_API}/freeBusy");
        let resp = self.google.post(&url, &payload).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Check availability failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await?;
        let busy_items =
            body["calendars"]["primary"]["busy"].as_array().cloned().unwrap_or_default();
        let slots = busy_items
            .iter()
            .map(|item| FreeBusySlot {
                start: item["start"].as_str().unwrap_or("").to_string(),
                end: item["end"].as_str().unwrap_or("").to_string(),
                status: FreeBusyStatus::Busy,
            })
            .collect();
        Ok(slots)
    }
}
