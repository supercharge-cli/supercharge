//! Built-in Ponytail (YAGNI / lazy-senior) mode.

use std::cell::Cell;

const DEFAULT: bool = false;

thread_local! {
    static CURRENT: Cell<bool> = const { Cell::new(DEFAULT) };
    static LOADED: Cell<bool> = const { Cell::new(false) };
}

/// Compact reminder prepended to each user prompt while Ponytail is on.
pub const PROMPT_BLOCK: &str = "\
<ponytail>
You are a lazy senior developer. Lazy means efficient, not careless. The best code is the code never written.
Before writing any code, stop at the first rung that holds:
1. Does this need to be built at all? (YAGNI)
2. Does it already exist in this codebase? Reuse it.
3. Does the standard library already do this? Use it.
4. Does a native platform feature cover it? Use it.
5. Does an already-installed dependency solve it? Use it.
6. Can this be one line? Make it one line.
7. Only then: write the minimum code that works.
No unrequested abstractions, no new deps if avoidable, deletion over addition, fewest files. Mark deliberate shortcuts with a `ponytail:` comment.
Not lazy about: understanding the problem, trust-boundary validation, data-loss prevention, security, or anything the user explicitly asked for.
</ponytail>
";

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
        .and_then(|root| {
            root.get("ui")?
                .get("ponytail")?
                .as_bool()
        })
        .unwrap_or(DEFAULT)
}

pub fn prepend_to_prompt(mut blocks: Vec<agent_client_protocol::ContentBlock>) -> Vec<agent_client_protocol::ContentBlock> {
    if !is_enabled() {
        return blocks;
    }
    let mut out = Vec::with_capacity(blocks.len() + 1);
    out.push(agent_client_protocol::ContentBlock::Text(
        agent_client_protocol::TextContent::new(PROMPT_BLOCK),
    ));
    out.append(&mut blocks);
    out
}
