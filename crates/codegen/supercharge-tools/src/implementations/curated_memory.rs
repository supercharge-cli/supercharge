//! Bounded curated memory, Hermes-style.
//!
//! Two files under `$SUPERCHARGE_HOME/memories/`:
//! `MEMORY.md` (environment notes, 2,200 chars) and `USER.md` (who the user is, 1,375 chars).
//! Entries are joined by `§`. The same files are injected into the system prompt and updated
//! by the `memory` tool, so local and E2B sessions share whatever home the process uses.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::ToolOutput;
use crate::types::tool::{ToolKind, ToolNamespace};

const ENTRY_DELIMITER: &str = "\n§\n";
const MEMORY_CHAR_LIMIT: usize = 2_200;
const USER_CHAR_LIMIT: usize = 1_375;
const MAX_FAILURES: u8 = 3;

static CONSOLIDATION_FAILURES: Mutex<u8> = Mutex::new(0);

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryOp {
    /// add, replace, or remove.
    pub action: String,
    /// Complete entry text for add and replace.
    #[serde(default)]
    pub content: Option<String>,
    /// Substring that identifies the existing entry for replace and remove.
    #[serde(default)]
    pub old_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CuratedMemoryInput {
    /// `memory` for environment notes, `user` for who the user is.
    pub target: String,
    /// Single-change action. Omit when `operations` is set.
    #[serde(default)]
    pub action: Option<String>,
    /// Complete entry text for add and replace.
    #[serde(default)]
    pub content: Option<String>,
    /// Substring that identifies the existing entry for replace and remove.
    #[serde(default)]
    pub old_text: Option<String>,
    /// Atomic batch. The character cap is checked on the final list only.
    #[serde(default)]
    pub operations: Option<Vec<MemoryOp>>,
}

impl From<CuratedMemoryInput> for crate::types::tool_io::ToolInput {
    fn from(value: CuratedMemoryInput) -> Self {
        Self::Dynamic(serde_json::to_value(value).expect("memory input serializes"))
    }
}

#[derive(Debug, Default)]
pub struct CuratedMemoryTool;

impl crate::types::tool_metadata::ToolMetadata for CuratedMemoryTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Save a durable fact that should be in every future session. The current notes are already \
         in the system prompt under <curated_memory>; do not read the files to recall them. Call \
         this tool only to change them.\n\n\
         WHEN: facts that apply to every session. Target `user` is shared by every bot: who the user \
         is, and anything every bot needs (name, role, preferences, style). Target `memory` is this \
         bot only: its environment, conventions, and tool quirks. Skip task progress, secrets, raw \
         dumps, and anything easy to re-discover.\n\n\
         HOW: one call. Prefer `operations` for several edits; the character cap is checked on the \
         final list, so one batch can remove a stale entry and add a new one. `content` on replace \
         is the COMPLETE new entry. `old_text` only identifies which entry, it is not spliced. \
         If an add is rejected because the store is full, reissue one batch that frees room and \
         adds the fact. After repeated failures, stop and answer the user."
    }
}

impl supercharge_tool_runtime::Tool for CuratedMemoryTool {
    type Args = CuratedMemoryInput;
    type Output = ToolOutput;

    fn id(&self) -> supercharge_tool_protocol::ToolId {
        supercharge_tool_protocol::ToolId::new("memory").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::supercharge_tool_runtime::ListToolsContext,
    ) -> supercharge_tool_types::ToolDescription {
        supercharge_tool_types::ToolDescription::new(
            "memory",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
        supercharge_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(supercharge_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    async fn run(
        &self,
        _ctx: supercharge_tool_runtime::ToolCallContext,
        input: CuratedMemoryInput,
    ) -> Result<ToolOutput, supercharge_tool_runtime::ToolError> {
        let root = memories_dir();
        let message = match apply(&root, &input) {
            Ok(message) => {
                if let Ok(mut n) = CONSOLIDATION_FAILURES.lock() {
                    *n = 0;
                }
                message
            }
            Err(message) => {
                let failures = CONSOLIDATION_FAILURES
                    .lock()
                    .map(|mut n| {
                        *n = n.saturating_add(1);
                        *n
                    })
                    .unwrap_or(1);
                if failures > MAX_FAILURES {
                    format!(
                        "{message}\nMemory updates failed {failures} times this process. Stop retrying and answer the user. The fact can be saved in a later turn."
                    )
                } else {
                    message
                }
            }
        };
        Ok(ToolOutput::Text(message.into()))
    }
}

/// System-prompt block. Empty stores still name the tool so the model knows it can save.
pub fn prompt_block() -> String {
    prompt_block_at(&supercharge_dirs::grok_home())
}

pub fn memories_dir() -> PathBuf {
    supercharge_dirs::grok_home().join("memories")
}

/// Bot notes live in this process home. User facts live one level above `bots/<id>` when this
/// home is a per-bot runtime, so every bot in the sandbox reads the same profile.
fn store_file(home: &Path, target: &str) -> PathBuf {
    if target == "user" {
        universal_memories_dir(home).join("USER.md")
    } else {
        home.join("memories").join("MEMORY.md")
    }
}

fn universal_memories_dir(home: &Path) -> PathBuf {
    match home.parent() {
        Some(parent) if parent.file_name().is_some_and(|name| name == "bots") => {
            parent.parent().unwrap_or(parent).join("memories")
        }
        _ => home.join("memories"),
    }
}

fn prompt_block_at(home: &Path) -> String {
    let memory = read_entries(&store_file(home, "memory")).unwrap_or_default();
    let user = read_entries(&store_file(home, "user")).unwrap_or_default();
    let mut block = String::from(
        "\n\n<curated_memory>\n\
         These notes are already loaded. Refer to them when the task touches the user or this \
         bot. Do not re-read the files. Save with the `memory` tool when a fact will still matter \
         in a later session: `user` for who the user is and anything every bot needs (one shared \
         profile), `memory` for notes that belong only to this bot. Do not store secrets, task \
         progress, or one-off details.\n",
    );
    push_section(
        &mut block,
        "MEMORY (environment notes)",
        &memory,
        MEMORY_CHAR_LIMIT,
    );
    push_section(&mut block, "USER PROFILE", &user, USER_CHAR_LIMIT);
    block.push_str("</curated_memory>\n");
    block
}

fn push_section(block: &mut String, title: &str, entries: &[String], limit: usize) {
    let body = entries.join(ENTRY_DELIMITER);
    block.push_str(&format!(
        "\n{title} [{}/{} chars]\n",
        body.chars().count(),
        limit
    ));
    if entries.is_empty() {
        block.push_str("(empty)\n");
    } else {
        block.push_str(&body);
        block.push('\n');
    }
}

fn apply(home: &Path, input: &CuratedMemoryInput) -> Result<String, String> {
    let target = input.target.trim();
    if target != "memory" && target != "user" {
        return Err(format!(
            "Invalid target '{target}'. Use 'memory' or 'user'."
        ));
    }
    let ops: Vec<MemoryOp> = if let Some(ops) = &input.operations {
        if ops.is_empty() {
            return Err("operations was empty".into());
        }
        ops.clone()
    } else {
        let action = input
            .action
            .clone()
            .filter(|a| !a.trim().is_empty())
            .ok_or_else(|| "action is required when operations is omitted".to_string())?;
        vec![MemoryOp {
            action,
            content: input.content.clone(),
            old_text: input.old_text.clone(),
        }]
    };
    let path = store_file(home, target);
    let scope = if target == "user" && universal_memories_dir(home) != home.join("memories") {
        "shared by every bot"
    } else if target == "user" {
        "this agent"
    } else {
        "this bot only"
    };
    let limit = if target == "user" {
        USER_CHAR_LIMIT
    } else {
        MEMORY_CHAR_LIMIT
    };
    let _lock = Lock::acquire(&path)?;
    let existing = read_entries(&path)?;
    let mut entries = existing;
    for op in &ops {
        apply_one(&mut entries, op)?;
    }
    let count = entries.join(ENTRY_DELIMITER).chars().count();
    if count > limit {
        return Err(format!(
            "{target} would be {count}/{limit} chars. Remove or shorten an entry in the same batch, then retry. Current entries:\n{}",
            entries.join(ENTRY_DELIMITER)
        ));
    }
    write_entries(&path, &entries)?;
    Ok(format!(
        "Saved {target} ({scope}, {count}/{limit} chars, {} entries). Later sessions see this. This session's prompt copy updates the next time the prompt is built.",
        entries.len()
    ))
}

fn apply_one(entries: &mut Vec<String>, op: &MemoryOp) -> Result<(), String> {
    let action = op.action.trim();
    match action {
        "add" => {
            let content = clean_content(op.content.as_deref())?;
            if entries.iter().any(|e| e == &content) {
                return Ok(());
            }
            entries.push(content);
            Ok(())
        }
        "replace" => {
            let content = clean_content(op.content.as_deref())?;
            let idx = locate(entries, op.old_text.as_deref(), "replace")?;
            let entry = entries
                .get_mut(idx)
                .ok_or_else(|| "matched memory entry disappeared before replacement".to_owned())?;
            *entry = content;
            Ok(())
        }
        "remove" => {
            let idx = locate(entries, op.old_text.as_deref(), "remove")?;
            entries.remove(idx);
            Ok(())
        }
        other => Err(format!(
            "Unknown action '{other}'. Use add, replace, or remove."
        )),
    }
}

fn clean_content(content: Option<&str>) -> Result<String, String> {
    let content = content.unwrap_or("").trim();
    if content.is_empty() {
        return Err("content is required".into());
    }
    if content.contains('§') {
        return Err("content cannot contain the § entry separator".into());
    }
    let lower = content.to_ascii_lowercase();
    if lower.contains("ignore previous")
        || lower.contains("ignore all previous")
        || lower.contains("you are now")
        || lower.contains("<system")
    {
        return Err("content looks like a prompt injection and was not saved".into());
    }
    if let Some(kind) = supercharge_secrets::find_secret(content) {
        return Err(format!(
            "content looks like it contains a secret ({kind}) and was not saved. Memory never \
             stores API keys, tokens, passwords, or other credentials; save only a description \
             of where the user keeps it."
        ));
    }
    Ok(content.to_string())
}

fn locate(entries: &[String], old_text: Option<&str>, verb: &str) -> Result<usize, String> {
    let old = old_text.unwrap_or("").trim();
    if old.is_empty() {
        return Err(format!("old_text is required for {verb}"));
    }
    let exact: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.as_str() == old)
        .map(|(i, _)| i)
        .collect();
    let matches = if !exact.is_empty() {
        exact
    } else {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.contains(old))
            .map(|(i, _)| i)
            .collect()
    };
    let distinct: std::collections::BTreeSet<&str> = matches
        .iter()
        .filter_map(|index| entries.get(*index).map(String::as_str))
        .collect();
    match distinct.len() {
        1 => matches
            .first()
            .copied()
            .ok_or_else(|| format!("No entry matched old_text for {verb}.")),
        0 => Err(format!("No entry matched old_text for {verb}.")),
        _ => Err(format!(
            "old_text matched more than one entry for {verb}. Quote a longer unique substring."
        )),
    }
}

fn read_entries(path: &Path) -> Result<Vec<String>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).map_err(|e| {
        format!(
            "Refusing to write {}: the file exists but could not be read ({e}). Nothing was changed.",
            path.display()
        )
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if !trimmed.contains('§') && trimmed.contains('\n') {
        let bak = path.with_extension("md.bak");
        let _ = fs::write(&bak, &raw);
        return Err(format!(
            "Refusing to write {}: the file is free-form prose, not §-separated entries. A copy is at {}.",
            path.display(),
            bak.display()
        ));
    }
    let entries = parse_entries(trimmed);
    let rendered = entries.join(ENTRY_DELIMITER);
    if rendered != trimmed {
        let bak = path.with_extension("md.bak");
        let _ = fs::write(&bak, &raw);
        return Err(format!(
            "Refusing to write {}: the file would not round-trip (hand edit or extra prose). A copy is at {}. Rewrite it as §-separated entries, then retry.",
            path.display(),
            bak.display()
        ));
    }
    Ok(entries)
}

fn parse_entries(raw: &str) -> Vec<String> {
    raw.split(ENTRY_DELIMITER)
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

fn write_entries(path: &Path, entries: &[String]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        restrict_mode(parent, 0o700);
    }
    let body = if entries.is_empty() {
        String::new()
    } else {
        format!("{}\n", entries.join(ENTRY_DELIMITER))
    };
    let tmp = path.with_extension("md.tmp");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        file.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().ok();
    }
    restrict_mode(&tmp, 0o600);
    fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Memory notes are private to the owner: 0700 directories, 0600 files on unix.
#[cfg(unix)]
fn restrict_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict_mode(_path: &Path, _mode: u32) {}

struct Lock {
    path: PathBuf,
}

impl Lock {
    fn acquire(memory_file: &Path) -> Result<Self, String> {
        let path = memory_file.with_extension("md.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        for _ in 0..50 {
            if let Ok(meta) = fs::metadata(&path)
                && meta
                    .modified()
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > Duration::from_secs(30))
            {
                let _ = fs::remove_file(&path);
            }
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        Err(format!("Could not lock {}", memory_file.display()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_then_prompt_contains_the_fact() {
        let home = std::env::temp_dir().join(format!("sc-mem-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        let msg = apply(
            &home,
            &CuratedMemoryInput {
                target: "user".into(),
                action: Some("add".into()),
                content: Some("User prefers short replies.".into()),
                old_text: None,
                operations: None,
            },
        )
        .unwrap();
        assert!(msg.contains("Saved user"));
        let block = prompt_block_at(&home);
        assert!(block.contains("User prefers short replies."));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn drift_is_refused() {
        let home = std::env::temp_dir().join(format!("sc-mem-drift-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join("memories")).unwrap();
        fs::write(
            home.join("memories/MEMORY.md"),
            "# free form notes\nstill prose\n",
        )
        .unwrap();
        let err = apply(
            &home,
            &CuratedMemoryInput {
                target: "memory".into(),
                action: Some("add".into()),
                content: Some("A clean entry.".into()),
                old_text: None,
                operations: None,
            },
        )
        .unwrap_err();
        assert!(err.contains("free-form"));
        assert!(home.join("memories/MEMORY.md.bak").exists());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn user_profile_is_shared_across_bots() {
        let runtime = std::env::temp_dir().join(format!("sc-mem-bots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&runtime);
        let main = runtime.join("bots/main");
        let coder = runtime.join("bots/coder");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(&coder).unwrap();
        apply(
            &main,
            &CuratedMemoryInput {
                target: "user".into(),
                action: Some("add".into()),
                content: Some("User prefers short replies.".into()),
                old_text: None,
                operations: None,
            },
        )
        .unwrap();
        apply(
            &coder,
            &CuratedMemoryInput {
                target: "memory".into(),
                action: Some("add".into()),
                content: Some("Coder bot owns the API folder.".into()),
                old_text: None,
                operations: None,
            },
        )
        .unwrap();
        let shared = runtime.join("memories/USER.md");
        assert!(shared.exists());
        assert!(!main.join("memories/USER.md").exists());
        let coder_view = prompt_block_at(&coder);
        assert!(coder_view.contains("User prefers short replies."));
        assert!(coder_view.contains("Coder bot owns the API folder."));
        let main_view = prompt_block_at(&main);
        assert!(main_view.contains("User prefers short replies."));
        assert!(!main_view.contains("Coder bot owns the API folder."));
        let _ = fs::remove_dir_all(&runtime);
    }

    #[test]
    fn secrets_are_never_saved_and_files_are_owner_only() {
        let home = std::env::temp_dir().join(format!("sc-mem-secret-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let add = |content: &str| {
            apply(
                &home,
                &CuratedMemoryInput {
                    target: "memory".into(),
                    action: Some("add".into()),
                    content: Some(content.into()),
                    old_text: None,
                    operations: None,
                },
            )
        };
        for secret in [
            "Pollinations key is sk-abcdefghijklmnopqrstuvwxyz123456",
            "Use password = correct-horse-battery",
            "Deploy token Zx9QmR7tLp2VbN8cKd4WsHy6FgJ3aE1u",
        ] {
            let err = add(secret).unwrap_err();
            assert!(err.contains("secret"), "{err}");
        }
        assert!(!prompt_block_at(&home).contains("abcdefghijklmnop"));
        add("The user keeps provider keys in a password manager.").unwrap();
        assert!(prompt_block_at(&home).contains("password manager"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.join("memories")), 0o700);
            assert_eq!(mode(&home.join("memories/MEMORY.md")), 0o600);
        }
        let _ = fs::remove_dir_all(&home);
    }
}
