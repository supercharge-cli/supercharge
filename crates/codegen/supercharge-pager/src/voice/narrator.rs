//! Narrator: speak a short summary of each completed turn with the system TTS binary.
//!
//! Off by default (`[ui].narrator = false`, or `/narrator` to toggle for this run).
//! Speaks the opening of the turn's final assistant message through the first
//! available of: `spd-say` (Linux), `say` (macOS). No TTS binary installed is a
//! silent no-op — voice dictation and the narrator are independent features.

use tokio::process::Command;

/// Speak this much at most: the narrator summarizes, it does not read.
const MAX_SPEAK_CHARS: usize = 240;

/// First sentences of the text, up to [`MAX_SPEAK_CHARS`].
fn speak_snippet(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let mut snippet = String::new();
    for sentence in text.split(|c: char| c == '.' || c == '!' || c == '?' || c == '\n') {
        let sentence = sentence.trim();
        if sentence.is_empty() {
            continue;
        }
        let candidate_len = snippet.chars().count() + sentence.chars().count() + 1;
        if !snippet.is_empty() && candidate_len > 120 {
            break;
        }
        if !snippet.is_empty() {
            snippet.push(' ');
        }
        snippet.push_str(sentence);
        snippet.push('.');
        if snippet.chars().count() >= 60 {
            break;
        }
    }
    if snippet.is_empty() {
        // One long unbroken line: hard-trim.
        snippet = text.chars().take(MAX_SPEAK_CHARS).collect();
    }
    snippet.chars().take(MAX_SPEAK_CHARS).collect()
}

/// Spawn the system TTS binary over the text; fire-and-forget.
async fn speak_once(text: String) -> bool {
    let attempts: [(&str, Vec<String>); 2] = [
        ("spd-say", vec!["-q".to_string(), text.clone()]),
        ("say", vec![text]),
    ];
    for (program, args) in attempts {
        let mut child = Command::new(program);
        child
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match child.spawn() {
            Ok(mut child) => {
                // Bound the wait: a wedged TTS process must not linger.
                let _ = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await;
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return true,
        }
    }
    false
}

/// Speak the opening of the latest assistant message, if the narrator is on.
pub fn speak_turn_summary(scrollback_last_agent_text: Option<String>) {
    if !crate::appearance::cache::load_narrator() {
        return;
    }
    let Some(text) = scrollback_last_agent_text else {
        return;
    };
    let snippet = speak_snippet(&text);
    if snippet.is_empty() {
        return;
    }
    tokio::spawn(speak_once(snippet));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_takes_first_sentences() {
        let text = "First sentence is short. Second one, with commas, adds detail. Third is unspoken.";
        let s = speak_snippet(text);
        assert!(s.starts_with("First sentence is short."));
        assert!(s.contains("Second"));
        assert!(!s.contains("Third"));
        assert!(s.ends_with('.'));
    }

    #[test]
    fn snippet_hard_trims_unbroken_text() {
        let s = speak_snippet(&"a".repeat(1000));
        assert_eq!(s.chars().count(), MAX_SPEAK_CHARS);
    }

    #[test]
    fn snippet_empty_input() {
        assert_eq!(speak_snippet("   "), "");
    }
}
