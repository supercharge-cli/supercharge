//! Reassemble a bracketed paste that arrived as ordinary key events.
//!
//! Windows console input does not emit `Event::Paste`. Windows Terminal still
//! wraps Ctrl+V in `ESC [ 2 0 0 ~` … `ESC [ 2 0 1 ~` once this process asks for
//! bracketed paste. Those bytes show up as an Escape key followed by the
//! marker and the text, so Escape cancels the composer and the paste never
//! lands. The same split happens on any terminal whose reader does not parse
//! the markers itself.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::event_loop::{TimedInputEvent, is_bare_esc_press};

const INTRO: [Marker; 6] = [
    Marker::Esc,
    Marker::Char('['),
    Marker::Char('2'),
    Marker::Char('0'),
    Marker::Char('0'),
    Marker::Char('~'),
];
const END: [Marker; 6] = [
    Marker::Esc,
    Marker::Char('['),
    Marker::Char('2'),
    Marker::Char('0'),
    Marker::Char('1'),
    Marker::Char('~'),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Esc,
    Char(char),
}

/// A batch that still needs more keys before the paste can be committed.
#[derive(Debug)]
pub(super) struct HeldPaste {
    pub(super) ready: Vec<TimedInputEvent>,
    pub(super) held: Vec<TimedInputEvent>,
    /// The introducer `ESC [200~` has already been seen. The body is open.
    pub(super) in_body: bool,
}

/// Rewrite complete bracketed-paste spans into `Event::Paste`.
/// `Err` means the suffix is an unfinished introducer or an open paste body.
pub(super) fn rewrite(events: Vec<TimedInputEvent>) -> Result<Vec<TimedInputEvent>, HeldPaste> {
    let mut ready = Vec::with_capacity(events.len());
    let mut index = 0;
    while index < events.len() {
        if let Some(intro_end) = match_marker(&events, index, &INTRO) {
            match take_body(&events, intro_end) {
                Body::Closed { end, text, arrived_at } => {
                    ready.push(TimedInputEvent {
                        event: Event::Paste(text),
                        arrived_at,
                    });
                    index = end;
                }
                Body::Open => {
                    return Err(HeldPaste {
                        ready,
                        held: events[index..].to_vec(),
                        in_body: true,
                    });
                }
            }
        } else if marker_prefix_at_end(&events, index, &INTRO) {
            return Err(HeldPaste {
                ready,
                held: events[index..].to_vec(),
                in_body: false,
            });
        } else {
            ready.push(events[index].clone());
            index += 1;
        }
    }
    Ok(ready)
}

/// Commit an unfinished body after input goes quiet. A partial introducer is
/// released unchanged so a real Escape still cancels.
pub(super) fn finish_timeout(held: HeldPaste) -> Vec<TimedInputEvent> {
    let mut ready = held.ready;
    if !held.in_body {
        ready.extend(held.held);
        return ready;
    }
    let arrived_at = held
        .held
        .first()
        .map(|event| event.arrived_at)
        .unwrap_or_else(std::time::Instant::now);
    let Some(body_at) = match_marker(&held.held, 0, &INTRO) else {
        ready.extend(held.held);
        return ready;
    };
    let text = body_text(&held.held[body_at..]);
    ready.push(TimedInputEvent {
        event: Event::Paste(text),
        arrived_at,
    });
    ready
}

enum Body {
    Closed {
        end: usize,
        text: String,
        arrived_at: std::time::Instant,
    },
    Open,
}

fn take_body(events: &[TimedInputEvent], body_at: usize) -> Body {
    let arrived_at = events
        .get(body_at.saturating_sub(1))
        .map(|event| event.arrived_at)
        .unwrap_or_else(std::time::Instant::now);
    let mut text = String::new();
    let mut index = body_at;
    let mut prev_was_enter = false;
    while index < events.len() {
        if let Some(end) = match_marker(events, index, &END) {
            return Body::Closed {
                end,
                text,
                arrived_at,
            };
        }
        if marker_prefix_at_end(events, index, &END) {
            return Body::Open;
        }
        let event = &events[index];
        if is_key_release(&event.event) {
            index += 1;
            continue;
        }
        if prev_was_enter && is_bare_lf(&event.event) {
            prev_was_enter = false;
            index += 1;
            continue;
        }
        match body_char(&event.event) {
            Some(ch) => {
                text.push(ch);
                prev_was_enter = ch == '\n';
                index += 1;
            }
            None => {
                return Body::Closed {
                    end: index,
                    text,
                    arrived_at,
                };
            }
        }
    }
    Body::Open
}

fn body_text(events: &[TimedInputEvent]) -> String {
    let mut text = String::new();
    let mut index = 0;
    let mut prev_was_enter = false;
    while index < events.len() {
        if marker_prefix_at_end(events, index, &END) {
            break;
        }
        let event = &events[index];
        if is_key_release(&event.event) {
            index += 1;
            continue;
        }
        if prev_was_enter && is_bare_lf(&event.event) {
            prev_was_enter = false;
            index += 1;
            continue;
        }
        match body_char(&event.event) {
            Some(ch) => {
                text.push(ch);
                prev_was_enter = ch == '\n';
                index += 1;
            }
            None => break,
        }
    }
    text
}

fn match_marker(events: &[TimedInputEvent], start: usize, marker: &[Marker]) -> Option<usize> {
    let mut index = start;
    for expected in marker {
        loop {
            let event = events.get(index)?;
            if is_key_release(&event.event) {
                index += 1;
                continue;
            }
            if marker_of(&event.event) == Some(*expected) {
                index += 1;
                break;
            }
            return None;
        }
    }
    Some(index)
}

/// True when `events[start..]` is a non-empty proper prefix of `marker` with
/// nothing after it. A rejected later key is not a prefix.
fn marker_prefix_at_end(events: &[TimedInputEvent], start: usize, marker: &[Marker]) -> bool {
    if start >= events.len() {
        return false;
    }
    let mut index = start;
    let mut matched = 0usize;
    for expected in marker {
        loop {
            let Some(event) = events.get(index) else {
                return matched > 0;
            };
            if is_key_release(&event.event) {
                index += 1;
                continue;
            }
            if marker_of(&event.event) == Some(*expected) {
                matched += 1;
                index += 1;
                break;
            }
            return false;
        }
    }
    false
}

fn marker_of(event: &Event) -> Option<Marker> {
    let Event::Key(key) = event else {
        return None;
    };
    if !is_press(key) {
        return None;
    }
    if is_bare_esc_press(event) {
        return Some(Marker::Esc);
    }
    if let KeyCode::Char(ch) = key.code
        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
    {
        return Some(Marker::Char(ch));
    }
    None
}

fn body_char(event: &Event) -> Option<char> {
    let Event::Key(key) = event else {
        return None;
    };
    if !is_press(key) {
        return None;
    }
    match key.code {
        KeyCode::Char(ch) if is_plain_or_altgr(key.modifiers) => Some(ch),
        KeyCode::Enter if key.modifiers.is_empty() => Some('\n'),
        KeyCode::Tab if key.modifiers.is_empty() => Some('\t'),
        _ => None,
    }
}

fn is_plain_or_altgr(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty()
        || modifiers == KeyModifiers::SHIFT
        || modifiers == KeyModifiers::CONTROL | KeyModifiers::ALT
        || modifiers == KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
}

fn is_bare_lf(event: &Event) -> bool {
    matches!(event, Event::Key(key)
        if is_press(key)
            && key.code == KeyCode::Char('j')
            && key.modifiers == KeyModifiers::CONTROL)
}

fn is_press(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn is_key_release(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.kind == KeyEventKind::Release)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crossterm::event::KeyEventState;

    use super::*;

    fn at(code: KeyCode, kind: KeyEventKind) -> TimedInputEvent {
        TimedInputEvent {
            event: Event::Key(KeyEvent {
                code,
                modifiers: KeyModifiers::NONE,
                kind,
                state: KeyEventState::NONE,
            }),
            arrived_at: Instant::now(),
        }
    }

    fn press(code: KeyCode) -> TimedInputEvent {
        at(code, KeyEventKind::Press)
    }

    fn release(code: KeyCode) -> TimedInputEvent {
        at(code, KeyEventKind::Release)
    }

    fn ch(c: char) -> TimedInputEvent {
        press(KeyCode::Char(c))
    }

    fn marker(end: bool) -> Vec<TimedInputEvent> {
        let flag = if end { '1' } else { '0' };
        vec![
            press(KeyCode::Esc),
            release(KeyCode::Esc),
            ch('['),
            release(KeyCode::Char('[')),
            ch('2'),
            ch('0'),
            ch(flag),
            ch('~'),
        ]
    }

    fn paste_text(events: &[TimedInputEvent]) -> String {
        match &events[0].event {
            Event::Paste(text) => text.clone(),
            other => panic!("expected paste, got {other:?}"),
        }
    }

    #[test]
    fn windows_style_press_and_release_becomes_one_paste() {
        let mut events = marker(false);
        events.push(ch('h'));
        events.push(ch('i'));
        events.push(press(KeyCode::Enter));
        events.push(ch('!'));
        events.extend(marker(true));
        let done = rewrite(events).expect("complete paste");
        assert_eq!(done.len(), 1);
        assert_eq!(paste_text(&done), "hi\n!");
    }

    #[test]
    fn text_around_the_paste_stays_in_order() {
        let mut events = vec![ch('x')];
        events.extend(marker(false));
        events.push(ch('y'));
        events.extend(marker(true));
        events.push(ch('z'));
        let done = rewrite(events).expect("complete paste");
        assert_eq!(done.len(), 3);
        assert!(matches!(done[0].event, Event::Key(_)));
        assert_eq!(paste_text(&done[1..2]), "y");
        assert!(matches!(done[2].event, Event::Key(_)));
    }

    #[test]
    fn partial_introducer_is_held() {
        let events = vec![press(KeyCode::Esc), ch('[')];
        let held = rewrite(events).expect_err("partial");
        assert!(!held.in_body);
        assert!(held.ready.is_empty());
        let released = finish_timeout(held);
        assert!(matches!(released[0].event, Event::Key(key) if key.code == KeyCode::Esc));
    }

    #[test]
    fn open_body_without_terminator_becomes_paste_on_timeout() {
        let mut events = marker(false);
        events.push(ch('a'));
        events.push(ch('b'));
        let held = rewrite(events).expect_err("open body");
        assert!(held.in_body);
        let done = finish_timeout(held);
        assert_eq!(paste_text(&done), "ab");
    }

    #[test]
    fn escape_then_unrelated_key_is_not_a_paste() {
        let events = vec![press(KeyCode::Esc), ch('a')];
        let done = rewrite(events).expect("not a paste");
        assert_eq!(done.len(), 2);
        assert!(matches!(done[0].event, Event::Key(key) if key.code == KeyCode::Esc));
    }
}
