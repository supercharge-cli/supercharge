//! Records, at session load, a turn the previous process never finished so it does not look like a silent stop.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use crate::session::persistence::Summary;

/// `stop_reason` for a turn lost with its process; distinct from `error` and `cancelled` so consumers can tell them apart.
pub const INTERRUPTED_STOP_REASON: &str = "interrupted";

/// Transcript line and failure text shown for the interrupted turn.
pub const INTERRUPTED_MESSAGE: &str = "Supercharge stopped before this turn finished (the agent process exited or was restarted). Committed tool results were kept.";

/// Only the tail of `events.jsonl` is scanned; one turn's events fit here many times over.
const EVENTS_TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedTurn {
    /// Trace turn number the previous process was running.
    pub trace_turn: u64,
    /// Prompt id of that turn.
    pub prompt_id: String,
    /// `ts` of the unclosed `turn_started`, when readable.
    pub started_at: Option<String>,
}

impl InterruptedTurn {
    pub(crate) fn model_reminder(&self) -> String {
        "The previous turn was interrupted (the system exited or was restarted). Work before this message may have been lost."
            .to_string()
    }

    /// Closes the turn on the replay rail so clients paint an end marker.
    pub(crate) fn turn_completed_update(
        &self,
        session_id: &agent_client_protocol::SessionId,
    ) -> crate::session::storage::SessionUpdate {
        let update = crate::session::turn_completion::build_turn_completed(
            self.prompt_id.clone(),
            serde_json::json!(INTERRUPTED_STOP_REASON),
            serde_json::json!(INTERRUPTED_MESSAGE),
            None,
            None,
            None,
        );
        let notification = crate::extensions::notification::SessionNotification {
            session_id: session_id.clone(),
            update,
            meta: Some(serde_json::json!({
                "eventId": crate::util::event_id::generate_event_id(&session_id.0),
                "agentTimestampMs": chrono::Utc::now().timestamp_millis(),
            })),
        };
        crate::session::storage::SessionUpdate::Xai(Box::new(notification))
    }

    /// Closes the open `turn_started` in `events.jsonl` so the next load does not report this turn again.
    pub(crate) fn close_events_turn(&self, session_dir: &Path) {
        supercharge_session_events::EventWriter::open(session_dir).emit(
            supercharge_session_events::Event::TurnEnded {
                outcome: supercharge_session_events::TurnOutcomeLabel::Interrupted,
                cancellation_category: None,
                cancellation_context: None,
            },
        );
    }
}

/// `Some` when the newest turn in `session_dir` started but never ended and `summary` still names it.
pub(crate) fn detect_interrupted_turn(
    session_dir: &Path,
    summary: &Summary,
) -> Option<InterruptedTurn> {
    let prompt_id = summary.request_id.clone()?;
    let trace_turn = summary.next_trace_turn.checked_sub(1)?;
    let tail = read_events_tail(&session_dir.join("events.jsonl"))?;
    let started_at = open_turn_started_at(&tail)?;
    Some(InterruptedTurn {
        trace_turn,
        prompt_id,
        started_at,
    })
}

/// Last `EVENTS_TAIL_BYTES` of the file, cut to a line boundary.
fn read_events_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(EVENTS_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity(usize::try_from(len - start).unwrap_or(0));
    file.read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        // The first line is cut mid-way; drop it.
        match text.find('\n') {
            Some(idx) => text.drain(..=idx),
            None => return None,
        };
    }
    Some(text)
}

/// `Some(ts)` when the last turn event in `tail` is a `turn_started`; `None` when it is a `turn_ended` or there is none.
/// Lines that are not valid JSON (a torn final write, say) are skipped.
fn open_turn_started_at(tail: &str) -> Option<Option<String>> {
    let mut open: Option<Option<String>> = None;
    for line in tail.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("turn_started") => {
                open = Some(
                    event
                        .get("ts")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
            Some("turn_ended") => open = None,
            _ => {}
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(request_id: Option<&str>, next_trace_turn: u64) -> Summary {
        let info = crate::session::info::Info {
            id: agent_client_protocol::SessionId::new("test"),
            cwd: "/tmp".into(),
        };
        Summary {
            request_id: request_id.map(str::to_owned),
            next_trace_turn,
            ..Summary::new(&info, agent_client_protocol::ModelId::new("test-model")).unwrap()
        }
    }

    fn write_events(dir: &Path, lines: &[&str]) {
        std::fs::write(dir.join("events.jsonl"), lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn open_turn_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        write_events(
            dir.path(),
            &[
                r#"{"type":"turn_started","ts":"2026-01-01T00:00:00Z"}"#,
                r#"{"type":"turn_ended","outcome":"completed"}"#,
                r#"{"type":"turn_started","ts":"2026-01-02T00:00:00Z"}"#,
                r#"{"type":"tool_started"}"#,
            ],
        );
        let turn = detect_interrupted_turn(dir.path(), &summary(Some("p-1"), 3)).unwrap();
        assert_eq!(
            turn,
            InterruptedTurn {
                trace_turn: 2,
                prompt_id: "p-1".into(),
                started_at: Some("2026-01-02T00:00:00Z".into()),
            }
        );
    }

    #[test]
    fn closed_turn_is_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        write_events(
            dir.path(),
            &[
                r#"{"type":"turn_started","ts":"t"}"#,
                r#"{"type":"turn_ended","outcome":"completed"}"#,
            ],
        );
        assert!(detect_interrupted_turn(dir.path(), &summary(Some("p"), 1)).is_none());
    }

    #[test]
    fn missing_inputs_never_report() {
        let dir = tempfile::tempdir().unwrap();
        // No events file.
        assert!(detect_interrupted_turn(dir.path(), &summary(Some("p"), 1)).is_none());
        write_events(dir.path(), &[r#"{"type":"turn_started"}"#]);
        // No prompt id, or no turn ever counted.
        assert!(detect_interrupted_turn(dir.path(), &summary(None, 1)).is_none());
        assert!(detect_interrupted_turn(dir.path(), &summary(Some("p"), 0)).is_none());
    }

    #[test]
    fn garbage_and_torn_lines_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("events.jsonl"),
            b"\xff\xfe not json\n{\"type\":\"turn_started\"}\n{\"type\":\"turn_en",
        )
        .unwrap();
        let turn = detect_interrupted_turn(dir.path(), &summary(Some("p"), 1)).unwrap();
        assert_eq!(turn.started_at, None);
    }

    #[test]
    fn large_file_only_reads_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let filler = format!(r#"{{"type":"noise","pad":"{}"}}"#, "x".repeat(1024));
        let mut lines: Vec<String> = vec![r#"{"type":"turn_started","ts":"old"}"#.into()];
        lines.extend(std::iter::repeat_n(filler, 400));
        lines.push(r#"{"type":"turn_ended","outcome":"completed"}"#.into());
        lines.push(r#"{"type":"turn_started","ts":"new"}"#.into());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        write_events(dir.path(), &refs);
        let turn = detect_interrupted_turn(dir.path(), &summary(Some("p"), 5)).unwrap();
        assert_eq!(turn.started_at.as_deref(), Some("new"));
    }

    #[test]
    fn closing_the_turn_stops_future_detection() {
        let dir = tempfile::tempdir().unwrap();
        write_events(dir.path(), &[r#"{"type":"turn_started","ts":"t"}"#]);
        let turn = detect_interrupted_turn(dir.path(), &summary(Some("p"), 1)).unwrap();
        turn.close_events_turn(dir.path());
        assert!(detect_interrupted_turn(dir.path(), &summary(Some("p"), 1)).is_none());
    }

    #[test]
    fn marker_update_carries_interrupted_stop_reason() {
        let turn = InterruptedTurn {
            trace_turn: 0,
            prompt_id: "p-9".into(),
            started_at: None,
        };
        let update = turn.turn_completed_update(&agent_client_protocol::SessionId::new("s"));
        let json = serde_json::to_string(&update).unwrap();
        assert!(json.contains(INTERRUPTED_STOP_REASON), "{json}");
        assert!(json.contains("p-9"), "{json}");
    }
}
