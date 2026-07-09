use navi_sdk::{SessionId, SessionSnapshot, SessionStore};

pub(crate) fn load_saved_sessions(store: &SessionStore) -> Vec<SessionSnapshot> {
    tokio::task::block_in_place(|| store.list())
}

pub(crate) fn session_created_at(session_id: &SessionId) -> Option<u64> {
    session_id
        .as_str()
        .strip_prefix("session-")?
        .parse::<u128>()
        .ok()
        .map(|millis| (millis / 1000) as u64)
}

pub(crate) fn format_session_timestamp(timestamp: u64) -> String {
    if timestamp == 0 {
        return "unknown time".to_string();
    }

    let (year, month, day, hour, minute) = unix_timestamp_parts(timestamp);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

pub(crate) fn unix_timestamp_parts(timestamp: u64) -> (i64, u32, u32, u32, u32) {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let hour = (seconds / 3_600) as u32;
    let minute = ((seconds % 3_600) / 60) as u32;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, hour, minute)
}

pub(crate) fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use navi_sdk::{AgentEvent, session_title_from_events};

    use super::*;

    #[test]
    fn session_title_prefers_model_heading() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "make a dashboard".to_string(),
                content_parts: vec![],
                submitted_at: None,
            },
            AgentEvent::ModelOutput {
                text: "## Cyberpunk Analytics Dashboard\n\nImplemented.".to_string(),
                thinking: None,
            },
        ];

        assert_eq!(
            session_title_from_events(&events).as_deref(),
            Some("Cyberpunk Analytics Dashboard")
        );
    }

    #[test]
    fn session_timestamp_formats_date_and_time() {
        assert_eq!(format_session_timestamp(0), "unknown time");
        assert_eq!(format_session_timestamp(1_700_000_000), "2023-11-14 22:13");
    }
}
