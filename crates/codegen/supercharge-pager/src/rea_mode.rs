//! Built-in REA (reverse-engineer-anything) mode: cached `[rea] enabled` flag.
//! The shell adds or removes the guide in the system prompt at the start of each turn.

use std::cell::Cell;

thread_local! {
    static CURRENT: Cell<Option<bool>> = const { Cell::new(None) };
}

pub fn is_enabled() -> bool {
    CURRENT.with(|c| {
        c.get().unwrap_or_else(|| {
            let on = supercharge_shell::util::rea::enabled();
            c.set(Some(on));
            on
        })
    })
}

pub fn set_enabled(on: bool) {
    CURRENT.with(|c| c.set(Some(on)));
}

/// Status line for `/REA-mode` with no argument.
pub fn status_message(on: bool) -> String {
    format!(
        "REA mode is {}. Use /REA-mode on|off. {}",
        if on { "on" } else { "off" },
        setup_hint()
    )
}

/// Hint about registering the REA tools. Checks `PATH` for Node.js without running anything.
pub fn setup_hint() -> String {
    supercharge_shared::rea::setup_hint(supercharge_shared::rea::node_on_path())
}
