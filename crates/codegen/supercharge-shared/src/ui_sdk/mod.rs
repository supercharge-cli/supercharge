//! Built-in Supercharge UI skill: teaches the agent to build interfaces from the UI SDK
//! behavior library (`@ui-sdk/*`), plus the tagged system-prompt blocks that switch it on.
//!
//! The skill text is original Supercharge documentation describing the public `@ui-sdk/*`
//! packages; nothing is copied from the library's own skill files.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

/// Skill name, also the folder name under the built-in skills root.
pub const SKILL_NAME: &str = "ui-sdk";

const SKILL_MD: &str = include_str!("assets/SKILL.md");

/// Every bundled file, as (path relative to the skill folder, contents).
pub const FILES: &[(&str, &str)] = &[
    ("SKILL.md", SKILL_MD),
    (
        "references/components.md",
        include_str!("assets/references/components.md"),
    ),
    (
        "references/behaviors.md",
        include_str!("assets/references/behaviors.md"),
    ),
    (
        "references/frameworks.md",
        include_str!("assets/references/frameworks.md"),
    ),
    (
        "references/agent-ready.md",
        include_str!("assets/references/agent-ready.md"),
    ),
    (
        "references/verify.md",
        include_str!("assets/references/verify.md"),
    ),
];

/// Folder the UI skill is written to under `home`.
pub fn skill_dir(home: &Path) -> PathBuf {
    crate::rea::builtin_skills_root(home).join(SKILL_NAME)
}

/// Write the bundled skill under `home`, touching only files whose contents differ.
/// Returns the skill folder.
pub fn materialize(home: &Path) -> io::Result<PathBuf> {
    let dir = skill_dir(home);
    for (rel, contents) in FILES {
        let path = dir.join(rel);
        if std::fs::read(&path).is_ok_and(|existing| existing == contents.as_bytes()) {
            continue;
        }
        let parent = path.parent().unwrap_or(&dir);
        std::fs::create_dir_all(parent)?;
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::fs::write(tmp.path(), contents)?;
        tmp.persist(&path).map_err(|e| e.error)?;
    }
    Ok(dir)
}

/// The skill body without its YAML frontmatter.
pub fn skill_body() -> &'static str {
    let Some(rest) = SKILL_MD.strip_prefix("---") else {
        return SKILL_MD;
    };
    match rest.find("\n---") {
        Some(end) => rest
            .get(end + 4..)
            .unwrap_or(SKILL_MD)
            .trim_start_matches(['\r', '\n']),
        None => SKILL_MD,
    }
}

/// Open/close tags of a system-prompt block owned by Supercharge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockTags {
    pub open: &'static str,
    pub close: &'static str,
}

/// Tags of the block added automatically in frontend workspaces.
pub const AUTO_TAGS: BlockTags = BlockTags {
    open: "<supercharge-ui-skill>",
    close: "</supercharge-ui-skill>",
};

/// Tags of the design-mode block.
pub const DESIGN_TAGS: BlockTags = BlockTags {
    open: "<supercharge-design-mode>",
    close: "</supercharge-design-mode>",
};

/// Remove every block with `tags` from `prompt`, including blocks written by older versions,
/// together with the blank line that separated it from the preceding text.
pub fn strip_block(prompt: &str, tags: BlockTags) -> Cow<'_, str> {
    if !prompt.contains(tags.open) {
        return Cow::Borrowed(prompt);
    }
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(start) = rest.find(tags.open) {
        let before = rest.get(..start).unwrap_or_default();
        out.push_str(before.strip_suffix("\n\n").unwrap_or(before));
        let block = rest.get(start..).unwrap_or_default();
        rest = match block.find(tags.close) {
            Some(end) => block.get(end + tags.close.len()..).unwrap_or_default(),
            None => "",
        };
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// `prompt` with exactly one copy of `block` when it is `Some`, and no block with `tags` otherwise.
/// Returns `prompt` unchanged when it already has the right shape, so the cached prefix survives.
pub fn apply_block<'a>(prompt: &'a str, tags: BlockTags, block: Option<&str>) -> Cow<'a, str> {
    if let Some(block) = block
        && prompt.matches(tags.open).count() == 1
        && prompt.contains(block)
    {
        return Cow::Borrowed(prompt);
    }
    let stripped = strip_block(prompt, tags);
    match block {
        None => stripped,
        Some(block) => Cow::Owned(format!("{stripped}\n\n{block}")),
    }
}

/// What the shell knows about the frontend workspace, already rendered for the prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectFacts {
    /// Human list of detected frameworks, e.g. `React`.
    pub frameworks: String,
    /// Package manager name, e.g. `pnpm`.
    pub package_manager: String,
    /// Command that installs the needed `@ui-sdk/*` packages.
    pub install_command: String,
    /// Verification commands (typecheck, build), in run order.
    pub verify_commands: Vec<String>,
    /// Project folder relative to the workspace, when it is not the workspace itself.
    pub relative_root: Option<String>,
}

fn where_line(facts: &ProjectFacts) -> String {
    match &facts.relative_root {
        Some(rel) => format!(" in `{rel}`"),
        None => String::new(),
    }
}

fn skill_line(skill_dir: Option<&Path>) -> String {
    match skill_dir {
        Some(dir) => format!(
            "Read `{}` before building or changing UI; its `references/` folder has the component table, behavior guide, per-framework rendering, agent-readiness and verification steps.",
            dir.join("SKILL.md").display()
        ),
        None => format!("Load the built-in `{SKILL_NAME}` skill before building or changing UI."),
    }
}

/// Short block added automatically when the workspace is a frontend project.
pub fn auto_block(facts: &ProjectFacts, skill_dir: Option<&Path>) -> String {
    format!(
        "{open}\n\
This workspace is a frontend project{root} ({frameworks}, {pm}). The built-in Supercharge UI skill (`{SKILL_NAME}`) is active for UI work: build interfaces from UI SDK behaviors and components instead of hand-rolling keyboard, focus, popup and ARIA logic.\n\
{skill}\n\
If `node_modules/@ui-sdk/` is missing, install with the project's package manager: `{install}`.\n\
{close}",
        open = AUTO_TAGS.open,
        close = AUTO_TAGS.close,
        root = where_line(facts),
        frameworks = facts.frameworks,
        pm = facts.package_manager,
        skill = skill_line(skill_dir),
        install = facts.install_command,
    )
}

/// The design-focused instructions shared by design mode and `/design <task>`.
pub fn design_brief(facts: Option<&ProjectFacts>, skill_dir: Option<&Path>) -> String {
    let project = match facts {
        Some(facts) => {
            let verify = if facts.verify_commands.is_empty() {
                "no typecheck or build script was found; add one or run the TypeScript checker directly".to_string()
            } else {
                facts
                    .verify_commands
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(", then ")
            };
            format!(
                "Project{root}: {frameworks}, package manager {pm}. Install UI SDK with `{install}` when `node_modules/@ui-sdk/` is missing. Verification: {verify}.",
                root = where_line(facts),
                frameworks = facts.frameworks,
                pm = facts.package_manager,
                install = facts.install_command,
            )
        }
        None => "No frontend project was detected in the workspace; if you create one, pick the framework the user asked for and install `@ui-sdk/core` plus its adapter with the chosen package manager.".to_string(),
    };
    format!(
        "You are working as a senior product designer and frontend engineer.\n\
{skill}\n\
{project}\n\n\
Design principles:\n\
- Start from the person's job: the task, the states (empty, loading, error, success, disabled) and the keyboard path, before markup.\n\
- Compose UI SDK components and behaviors (createPopup, createSelection, createTypeahead, createRovingFocus, createListbox, definePrimitive, createParts); never hand-roll focus, key handling, popups or ARIA that the library covers.\n\
- Clear hierarchy, consistent spacing scale, readable type, 4.5:1 text contrast and 3:1 for controls and focus rings, visible focus, motion that respects prefers-reduced-motion, layouts that hold from 320px wide to desktop and in right-to-left.\n\
- Match the project's existing design system, tokens and styling approach (Tailwind, CSS modules, etc.) instead of inventing a new one.\n\
- Name every control so people using screen readers and agents can find it.\n\n\
Verification loop (required):\n\
1. After every component you create or change, call `ui_verify` (or run the verification commands yourself when the tool is unavailable).\n\
2. If anything fails, read the first real error, fix the cause, and run it again. Repeat until every check passes. Never silence a check, loosen tsconfig, or add ts-ignore to get green.\n\
3. When a dev server can run and the `ui_browser` tool is available, start the server in the background and use the tool to open the page, read the accessibility snapshot, and work the new controls with clicks and keys.\n\
4. Only then report done, listing each command with its exit code and what you checked in the browser.",
        skill = skill_line(skill_dir),
    )
}

/// System-prompt block while design mode is on.
pub fn design_block(facts: Option<&ProjectFacts>, skill_dir: Option<&Path>) -> String {
    format!(
        "{open}\nDesign mode is on.\n{brief}\n{close}",
        open = DESIGN_TAGS.open,
        close = DESIGN_TAGS.close,
        brief = design_brief(facts, skill_dir),
    )
}

/// The user message sent for `/design <task>`.
pub fn design_task_prompt(
    task: &str,
    facts: Option<&ProjectFacts>,
    skill_dir: Option<&Path>,
) -> String {
    format!(
        "Design task: {task}\n\n{brief}",
        task = task.trim(),
        brief = design_brief(facts, skill_dir),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> ProjectFacts {
        ProjectFacts {
            frameworks: "React".into(),
            package_manager: "pnpm".into(),
            install_command: "pnpm add @ui-sdk/core @ui-sdk/react".into(),
            verify_commands: vec!["pnpm run typecheck".into(), "pnpm run build".into()],
            relative_root: None,
        }
    }

    #[test]
    fn bundled_skill_has_frontmatter_and_every_reference_it_links() {
        assert!(SKILL_MD.starts_with("---"));
        assert!(SKILL_MD.contains(&format!("name: {SKILL_NAME}\n")));
        for (path, contents) in FILES {
            assert!(!contents.trim().is_empty(), "{path} is empty");
        }
        for (path, contents) in FILES {
            for link in contents.split("](").skip(1) {
                let target = link.split(')').next().unwrap_or_default();
                let target = target.split('#').next().unwrap_or_default();
                if target.starts_with("http") || !target.ends_with(".md") {
                    continue;
                }
                let base = Path::new(path).parent().unwrap_or(Path::new(""));
                let resolved = base.join(target);
                let resolved = resolved.to_string_lossy().replace("references/../", "");
                assert!(
                    FILES.iter().any(|(p, _)| *p == resolved),
                    "{path} links to missing {resolved}"
                );
            }
        }
    }

    #[test]
    fn skill_teaches_the_core_behaviors_and_every_framework() {
        let all: String = FILES.iter().map(|(_, c)| *c).collect();
        for needle in [
            "createPopup",
            "createSelection",
            "createTypeahead",
            "createRovingFocus",
            "createListbox",
            "definePrimitive",
            "createParts",
            "@ui-sdk/react",
            "@ui-sdk/vue",
            "@ui-sdk/svelte",
            "@ui-sdk/solid",
            "@ui-sdk/web",
            "pnpm add",
            "bun add",
            "yarn add",
            "npm install",
        ] {
            assert!(all.contains(needle), "skill is missing {needle}");
        }
    }

    #[test]
    fn body_drops_frontmatter() {
        let body = skill_body();
        assert!(!body.starts_with("---"));
        assert!(!body.contains("description:"));
        assert!(body.contains("createPopup"));
    }

    #[test]
    fn materialize_writes_every_file_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let dir = materialize(home.path()).unwrap();
        assert_eq!(dir, home.path().join("builtin-skills").join(SKILL_NAME));
        for (rel, contents) in FILES {
            assert_eq!(std::fs::read_to_string(dir.join(rel)).unwrap(), *contents);
        }
        let skill = dir.join("SKILL.md");
        let before = std::fs::metadata(&skill).unwrap().modified().unwrap();
        materialize(home.path()).unwrap();
        let after = std::fs::metadata(&skill).unwrap().modified().unwrap();
        assert_eq!(before, after, "unchanged files must not be rewritten");
        std::fs::write(&skill, "stale").unwrap();
        materialize(home.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), SKILL_MD);
    }

    #[test]
    fn auto_block_names_framework_package_manager_and_install_command() {
        let block = auto_block(&facts(), Some(Path::new("/h/builtin-skills/ui-sdk")));
        assert!(block.starts_with(AUTO_TAGS.open));
        assert!(block.ends_with(AUTO_TAGS.close));
        assert!(block.contains("React, pnpm"));
        assert!(block.contains("`pnpm add @ui-sdk/core @ui-sdk/react`"));
        assert!(block.contains("SKILL.md"));
    }

    #[test]
    fn design_block_carries_the_verification_loop() {
        let block = design_block(Some(&facts()), None);
        assert!(block.starts_with(DESIGN_TAGS.open));
        assert!(block.contains("`pnpm run typecheck`, then `pnpm run build`"));
        assert!(block.contains("ui_verify"));
        assert!(block.contains("ui_browser"));
        assert!(design_block(None, None).contains("No frontend project"));
        let task = design_task_prompt("  a teammate picker ", Some(&facts()), None);
        assert!(task.starts_with("Design task: a teammate picker\n"));
    }

    #[test]
    fn apply_block_adds_once_keeps_current_and_strips_when_off() {
        let base = "You are helpful.";
        let block = auto_block(&facts(), None);
        let on = apply_block(base, AUTO_TAGS, Some(&block)).into_owned();
        assert_eq!(on, format!("{base}\n\n{block}"));
        assert!(matches!(
            apply_block(&on, AUTO_TAGS, Some(&block)),
            Cow::Borrowed(_)
        ));
        let other = auto_block(
            &ProjectFacts {
                frameworks: "Vue".into(),
                ..facts()
            },
            None,
        );
        let swapped = apply_block(&on, AUTO_TAGS, Some(&other)).into_owned();
        assert_eq!(swapped.matches(AUTO_TAGS.open).count(), 1);
        assert!(swapped.contains("Vue"));
        assert_eq!(apply_block(&on, AUTO_TAGS, None), base);
        assert!(matches!(
            apply_block(base, AUTO_TAGS, None),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn blocks_with_different_tags_are_independent() {
        let base = "base";
        let auto = auto_block(&facts(), None);
        let design = design_block(Some(&facts()), None);
        let both = apply_block(base, AUTO_TAGS, Some(&auto)).into_owned();
        let both = apply_block(&both, DESIGN_TAGS, Some(&design)).into_owned();
        assert!(both.contains(&auto) && both.contains(&design));
        let only_design = apply_block(&both, AUTO_TAGS, None).into_owned();
        assert!(!only_design.contains(AUTO_TAGS.open));
        assert!(only_design.contains(&design));
    }

    #[test]
    fn strip_handles_an_unclosed_block() {
        let prompt = format!("keep\n\n{}dangling", AUTO_TAGS.open);
        assert_eq!(strip_block(&prompt, AUTO_TAGS), "keep");
    }
}
