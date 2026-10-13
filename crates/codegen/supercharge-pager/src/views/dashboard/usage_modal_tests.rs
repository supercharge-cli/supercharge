use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use indexmap::IndexMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::app::app_view::InputOutcome;
use crate::views::dashboard::state::DashboardState;
use crate::views::usage_modal::{UsageInfoContext, UsageInfoModalState, UsageInfoTab};

fn session_less_modal(tab: UsageInfoTab) -> Box<UsageInfoModalState> {
    Box::new(UsageInfoModalState::new(
        tab,
        UsageInfoContext { session_id: None },
    ))
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn render_with_modal(state: &mut DashboardState, area: Rect) -> String {
    let mut buf = Buffer::empty(area);
    let mut agents = IndexMap::new();
    let registry = crate::actions::ActionRegistry::defaults();
    let cursor = crate::views::dashboard::render_dashboard(
        &mut buf,
        area,
        state,
        &mut agents,
        &registry,
        None,
        &[],
        false,
        crate::views::dashboard::WorkspaceRowInputs {
            workspace: None,
            provisional: &[],
        },
        None,
        false,
        None,
        None,
    );
    assert!(
        cursor.is_none(),
        "the dispatch caret must hide while the modal owns input"
    );
    let mut content = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                content.push_str(cell.symbol());
            }
        }
        content.push('\n');
    }
    content
}

#[test]
fn esc_closes_usage_modal() {
    let mut state = DashboardState::new();
    state.usage_modal = Some(session_less_modal(UsageInfoTab::UsageLimit));
    let reg = crate::actions::ActionRegistry::defaults();
    assert!(matches!(
        state.handle_input(&key(KeyCode::Esc), &reg),
        InputOutcome::Changed
    ));
    assert!(state.usage_modal.is_none());
}

/// Tab must reach the modal (tab cycle), not the dashboard's focus toggle.
#[test]
fn usage_modal_owns_keys_while_open() {
    let mut state = DashboardState::new();
    state.usage_modal = Some(session_less_modal(UsageInfoTab::UsageLimit));
    let reg = crate::actions::ActionRegistry::defaults();

    state.handle_input(&key(KeyCode::Char('x')), &reg);
    assert_eq!(state.dispatch.text(), "");

    assert!(matches!(
        state.handle_input(&key(KeyCode::Tab), &reg),
        InputOutcome::Changed
    ));
    assert_eq!(
        state.usage_modal.as_ref().unwrap().active_tab,
        UsageInfoTab::SessionInfo
    );
    assert!(state.usage_modal.is_some(), "Tab must not close the modal");
}

#[test]
fn close_button_click_closes_usage_modal() {
    let mut state = DashboardState::new();
    let mut modal = session_less_modal(UsageInfoTab::UsageLimit);
    modal.window.close_button_rect = Some(Rect::new(70, 2, 5, 1));
    state.usage_modal = Some(modal);
    let reg = crate::actions::ActionRegistry::defaults();
    let click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 72,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(
        state.handle_input(&click, &reg),
        InputOutcome::Changed
    ));
    assert!(state.usage_modal.is_none());
}

/// Mouse on a tab header switches tabs through the chrome (the router's `TabChanged` arm).
#[test]
fn tab_header_click_switches_tab() {
    let mut state = DashboardState::new();
    let mut modal = session_less_modal(UsageInfoTab::UsageLimit);
    modal.window.tab_rects = vec![
        Some(Rect::new(10, 2, 13, 1)),
        Some(Rect::new(25, 2, 11, 1)),
        Some(Rect::new(38, 2, 12, 1)),
    ];
    state.usage_modal = Some(modal);
    let reg = crate::actions::ActionRegistry::defaults();
    let click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 40,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(
        state.handle_input(&click, &reg),
        InputOutcome::Changed
    ));
    let modal = state.usage_modal.as_ref().unwrap();
    assert_eq!(modal.active_tab, UsageInfoTab::SessionInfo);
}

#[test]
fn usage_modal_renders_session_usage_not_a_plan() {
    let area = Rect::new(0, 0, 100, 30);
    let mut state = DashboardState::new();
    let mut modal = session_less_modal(UsageInfoTab::UsageLimit);
    modal.ctx.session_id = Some("sid-123".to_string());
    modal.session_usage_text = Some("Session usage: 3 input tokens".to_string());
    state.usage_modal = Some(modal);

    let content = render_with_modal(&mut state, area);
    assert!(content.contains("Usage"), "{content}");
    assert!(
        content.contains("Session usage: 3 input tokens"),
        "{content}"
    );
    assert!(!content.contains("SuperGrok"), "{content}");
    assert!(!content.contains("42%"), "{content}");

    state
        .usage_modal
        .as_mut()
        .unwrap()
        .set_tab(UsageInfoTab::ContextUsage);
    let content = render_with_modal(&mut state, area);
    assert!(content.contains("Loading context usage"), "{content}");
}

#[test]
fn usage_modal_renders_loading_until_session_usage_arrives() {
    let area = Rect::new(0, 0, 100, 30);
    let mut state = DashboardState::new();
    let mut modal = session_less_modal(UsageInfoTab::UsageLimit);
    modal.ctx.session_id = Some("sid-123".to_string());
    state.usage_modal = Some(modal);

    let content = render_with_modal(&mut state, area);
    assert!(content.contains("Loading session usage"), "{content}");
}

#[test]
fn usage_modal_renders_no_session_without_billing_error() {
    let area = Rect::new(0, 0, 100, 30);
    let mut state = DashboardState::new();
    state.usage_modal = Some(session_less_modal(UsageInfoTab::UsageLimit));

    let content = render_with_modal(&mut state, area);
    assert!(content.contains("No active session"), "{content}");
    assert!(!content.contains("Couldn't load usage"), "{content}");
}
