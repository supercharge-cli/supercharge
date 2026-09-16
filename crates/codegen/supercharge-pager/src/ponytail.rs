//! Built-in Ponytail (YAGNI / lazy-senior) mode.

use std::cell::Cell;

const DEFAULT: bool = false;

thread_local! {
    static CURRENT: Cell<bool> = const { Cell::new(DEFAULT) };
    static LOADED: Cell<bool> = const { Cell::new(false) };
}

// The runtime owns the Ponytail system policy. Keep user prompt text untouched
// so slash-command authority and skill-token byte ranges remain valid.

pub fn is_enabled() -> bool {
    LOADED.with(|loaded| {
        if !loaded.get() {
            CURRENT.with(|c| c.set(load_from_config()));
            loaded.set(true);
        }
    });
    CURRENT.with(|c| c.get())
}

pub fn set_enabled(on: bool) {
    CURRENT.with(|c| c.set(on));
    LOADED.with(|l| l.set(true));
}

fn load_from_config() -> bool {
    supercharge_shell::config::load_effective_config()
        .ok()
        .and_then(|root| root.get("ui")?.get("ponytail")?.as_bool())
        .unwrap_or(DEFAULT)
}
