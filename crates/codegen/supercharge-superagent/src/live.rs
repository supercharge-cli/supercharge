//! Live gateway protocol shared by the Vectra app inbox, Telegram, and other
//! channels. Event shapes match the XibeCode app adapter so the same mobile
//! client can render Supercharge turns as they happen.

use serde_json::{Value, json};

const COMMAND_MAX: usize = 400;
const STDOUT_MAX: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    TextDelta(String),
    Thought(String),
    ToolStart {
        id: String,
        name: String,
        title: String,
    },
    ToolDone {
        id: String,
        name: String,
        command: String,
        stdout: String,
        success: bool,
        exit_code: Option<i64>,
    },
    Usage {
        used: Option<u64>,
        max: Option<u64>,
    },
}

fn cap_text(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= max {
        return trimmed.to_owned();
    }
    trimmed
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn bytes_array_to_string(items: &[Value]) -> Option<String> {
    if items.is_empty()
        || !items
            .iter()
            .all(|value| value.as_u64().is_some_and(|n| n <= 255))
    {
        return None;
    }
    let bytes: Vec<u8> = items
        .iter()
        .filter_map(Value::as_u64)
        .map(|n| n as u8)
        .collect();
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn extract_command(line: &Value) -> Option<String> {
    let input = line.get("rawInput").unwrap_or(line);
    let from_output = line
        .get("rawOutput")
        .and_then(|value| value.get("command").or_else(|| value.get("cmd")))
        .and_then(Value::as_str);
    let raw = input
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| input.get("cmd").and_then(Value::as_str))
        .or_else(|| input.as_str())
        .or(from_output)
        .or_else(|| line.get("command").and_then(Value::as_str))
        .or_else(|| line.get("title").and_then(Value::as_str))?;
    let cleaned = raw.replace('\n', " ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cap_text(cleaned, COMMAND_MAX))
    }
}

fn push_output_text(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
        Value::Array(items) => {
            if let Some(text) = bytes_array_to_string(items) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&text);
                return;
            }
            for item in items {
                push_output_text(item, out);
            }
        }
        Value::Object(map) => {
            for key in [
                "output_for_prompt",
                "stdout",
                "stderr",
                "output",
                "text",
                "content",
                "message",
            ] {
                if let Some(item) = map.get(key) {
                    push_output_text(item, out);
                    if key == "output_for_prompt" && !out.is_empty() {
                        return;
                    }
                }
            }
            if out.is_empty() {
                for (key, item) in map {
                    if key == "type" || key == "command" {
                        continue;
                    }
                    push_output_text(item, out);
                    if !out.is_empty() {
                        break;
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn extract_tool_output(line: &Value) -> String {
    let mut out = String::new();
    for key in ["rawOutput", "content", "output", "stdout"] {
        if let Some(value) = line.get(key) {
            push_output_text(value, &mut out);
        }
    }
    cap_text(&out, STDOUT_MAX)
}

fn extract_exit_code(line: &Value) -> Option<i64> {
    line.get("exitCode")
        .or_else(|| line.get("exit_code"))
        .and_then(Value::as_i64)
        .or_else(|| {
            line.get("rawOutput")
                .and_then(|value| {
                    value
                        .get("exitCode")
                        .or_else(|| value.get("exit_code"))
                        .or_else(|| value.get("exit_status"))
                })
                .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|n| n as i64)))
        })
}

fn screenshot_path(command: &str, stdout: &str) -> Option<String> {
    let hay = format!("{command}\n{stdout}");
    hay.split_whitespace().find_map(|token| {
        let token = token.trim_matches(|c: char| c == '`' || c == '"' || c == '\'');
        let lower = token.to_ascii_lowercase();
        if lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".webp")
        {
            Some(token.to_owned())
        } else {
            None
        }
    })
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            if text.trim().is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(part) = json_text(item) {
                    out.push_str(&part);
                }
            }
            if out.trim().is_empty() {
                None
            } else {
                Some(out)
            }
        }
        Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("data"))
            .or_else(|| map.get("content"))
            .or_else(|| map.get("message"))
            .and_then(json_text),
        _ => None,
    }
}

fn stream_text(line: &Value) -> Option<String> {
    line.get("data")
        .and_then(json_text)
        .or_else(|| line.get("text").and_then(json_text))
        .or_else(|| line.get("content").and_then(json_text))
        .or_else(|| line.get("message").and_then(json_text))
}

pub fn parse_cli_stream_line(line: &Value) -> Vec<LiveEvent> {
    let kind = line.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text" | "assistant" | "agent_message" | "message" => stream_text(line)
            .map(|text| vec![LiveEvent::TextDelta(text)])
            .unwrap_or_default(),
        "thought" | "thinking" | "agent_thought" | "reasoning" => stream_text(line)
            .map(|text| vec![LiveEvent::Thought(text)])
            .unwrap_or_default(),
        "tool_call" => {
            let id = line
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            let name = line
                .get("toolName")
                .and_then(Value::as_str)
                .or_else(|| line.get("title").and_then(Value::as_str))
                .unwrap_or("tool")
                .to_owned();
            let title = extract_command(line).unwrap_or_else(|| name.clone());
            vec![LiveEvent::ToolStart { id, name, title }]
        }
        "tool_call_update" => {
            let status = line
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if status != "completed" && status != "failed" {
                return Vec::new();
            }
            let id = line
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            let name = line
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            vec![LiveEvent::ToolDone {
                id,
                name,
                command: extract_command(line).unwrap_or_default(),
                stdout: extract_tool_output(line),
                success: status == "completed",
                exit_code: extract_exit_code(line),
            }]
        }
        "usage" => {
            let usage = line.get("usage").cloned().unwrap_or(json!({}));
            vec![LiveEvent::Usage {
                used: usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .or_else(|| usage.get("prompt_tokens").and_then(Value::as_u64)),
                max: usage.get("total_tokens").and_then(Value::as_u64),
            }]
        }
        _ => Vec::new(),
    }
}

pub fn tool_computer_kind(name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    if name.contains("desktop_observe")
        || name.contains("desktop_action")
        || name.contains("browser")
        || name.contains("navigate")
        || name.contains("screenshot")
    {
        "browser"
    } else if name.contains("read") || name.contains("write") || name.contains("edit") {
        "file"
    } else if name.contains("shell")
        || name.contains("bash")
        || name.contains("exec")
        || name.contains("command")
    {
        "shell"
    } else {
        "other"
    }
}

/// Injected on every daemon turn so E2B app sessions use the native browser
/// tools and the same secure browser-tab handoff contract as the runtime.
pub const E2B_BROWSER_RULES: &str = r#"Browser (E2B / native Playwright):
- Proactively use the shared visible browser when the task depends on a live website: web research, current public facts, comparing pages or products, inspecting a URL, reproducing a browser issue, or validating anything the user sees in a web UI. The authenticated Vectra app user can watch the same browser live.
- Do not open the browser for work fully answerable from local code, files, tests, or a non-visual API response. Browser use must advance the task, not perform a token demonstration.
- Use `browser_navigate`, then `browser_snapshot` for current accessible element references. Use `browser_click`, `browser_fill`, and `browser_press` only with current snapshot references. Prefer snapshots to screenshots, but use `browser_screenshot` when layout, rendering, images, charts, canvas, or other visual evidence matters.
- The agent controls ordinary browsing and non-sensitive forms. Do not ask the user to take control merely because a page is complex. Keep using the same browser tab when practical so the live view and automation stay aligned.
- For passwords, passkeys, MFA/OTP, CAPTCHA, OAuth/consent, payments, private messages, or other sensitive user-only input, do not enter or request the value. Call `browser_handoff` with `action: "start"`, explain the step, and wait for the authenticated user to complete it in the Browser tab.
- When available, `desktop_observe` and `desktop_action` control the SAME Ubuntu E2B desktop the user watches, never their local machine. You CAN move the pointer, click, type, drag, focus windows, and launch installed GUI apps, not just Chromium. `desktop_observe` lists app IDs; use `desktop_action` launch with a listed `.desktop` ID (for example `discord.desktop` when installed), or install a missing app in this VM before retrying. Observe first, use image-pixel coordinates and its frameId, and observe again after actions. Prefer DOM tools for ordinary web elements.
- Browser and desktop input share an owner lease. Wait for another bot to release it; do not bypass with shell input, direct VNC, a second display or a second VM. Same-user shell access is not isolated by these coordination locks.
- Human handoff uses private, ticketed noVNC. `browser_handoff` start requests help; the authenticated user explicitly takes over and returns control. Stop only cancels your pending request, never active user control. Do not capture or interact during takeover. After return, inspect a fresh desktop screenshot/current window and browser snapshot without replaying manual actions. Never expose VNC, CDP, sidecar or gateway credentials.
- Never ask the user to paste passwords, MFA codes, cookies, or other secrets into chat.
"#;

fn read_trimmed_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_or_file(keys: &[&str], paths: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    for path in paths {
        if let Some(value) = read_trimmed_file(path) {
            return Some(value);
        }
    }
    None
}

pub fn sandbox_id() -> Option<String> {
    env_or_file(
        &["E2B_SANDBOX_ID", "SUPERCHARGE_SANDBOX_ID", "SANDBOX_ID"],
        &["/run/e2b/.E2B_SANDBOX_ID", "/run/e2b/E2B_SANDBOX_ID"],
    )
}

fn preview_domain() -> String {
    env_or_file(&["E2B_DOMAIN", "XIBECODE_E2B_PREVIEW_DOMAIN"], &[])
        .unwrap_or_else(|| "e2b.app".to_owned())
        .trim()
        .trim_start_matches('.')
        .to_owned()
}

fn template_id() -> Option<String> {
    env_or_file(
        &[
            "E2B_TEMPLATE_ID",
            "SUPERCHARGE_E2B_TEMPLATE_ID",
            "SUPERCHARGE_E2B_TEMPLATE",
        ],
        &["/run/e2b/.E2B_TEMPLATE_ID", "/run/e2b/E2B_TEMPLATE_ID"],
    )
}

pub fn preview_url(port: u16, sandbox: &str, domain: &str) -> String {
    format!("https://{port}-{sandbox}.{domain}")
}

pub fn channel_from_session_key(session_key: &str) -> &'static str {
    let key = session_key.to_ascii_lowercase();
    if key.starts_with("app:") || key.contains(":app:") {
        "vectra-app"
    } else if key.contains("telegram") {
        "telegram"
    } else if key.contains("discord") {
        "discord"
    } else if key.contains("slack") {
        "slack"
    } else if key.contains("whatsapp") {
        "whatsapp"
    } else if key.contains("signal") {
        "signal"
    } else if key.contains("matrix") {
        "matrix"
    } else if key.contains("email") {
        "email"
    } else {
        "vectra-app"
    }
}

fn channel_label(channel: &str) -> &'static str {
    match channel {
        "vectra-app" => "Vectra Cloud app (phone / web chat)",
        "telegram" => "Telegram",
        "discord" => "Discord",
        "slack" => "Slack",
        "whatsapp" => "WhatsApp",
        "signal" => "Signal",
        "matrix" => "Matrix",
        "email" => "email",
        _ => "Vectra Cloud",
    }
}

/// Full E2B / hosted-sandbox system prompt (identity, previews, MEDIA, tools).
pub fn e2b_agent_rules() -> String {
    e2b_agent_rules_for("app:owner")
}

pub fn e2b_agent_rules_for(session_key: &str) -> String {
    let sandbox = sandbox_id().unwrap_or_else(|| "unknown".to_owned());
    let domain = preview_domain();
    let preview_3000 = preview_url(3000, &sandbox, &domain);
    let preview_5173 = preview_url(5173, &sandbox, &domain);
    let preview_8080 = preview_url(8080, &sandbox, &domain);
    let preview_8788 = preview_url(8788, &sandbox, &domain);
    let share = format!("{preview_8788}/f/{{token}}/{{name}}");
    let template = template_id().unwrap_or_else(|| "unknown".to_owned());
    let id_hint = if sandbox == "unknown" {
        "\n- Sandbox id is missing from env — run `cat /run/e2b/.E2B_SANDBOX_ID` and use that value."
    } else {
        ""
    };
    let channel = channel_from_session_key(session_key);
    let channel_name = channel_label(channel);
    format!(
        r#"{E2B_BROWSER_RULES}

## Platform (read this first)
- Stack: **Vectra Cloud** (product the user is talking in) → **Supercharge** (this agent) → **E2B** (the Linux VM you are inside). You are not on the user's phone or laptop.
- This turn arrived through: **{channel_name}** (`{channel}`).
- To refresh Supercharge **without a new E2B template**, the user can run `/update` then `/update yes` (downloads daemon+CLI, restarts, keeps chats). XibeCode is not updated this way.
- Share so that client can actually open things:
  - Running servers → markdown link to `https://PORT-{sandbox}.{domain}` (never localhost).
  - Files, zips, screenshots → a `MEDIA:relative/path` line in the **final** reply (native card/attachment) **and** the public `{share}` style URL.
  - Vectra app also has a Computer pane (live browser JPEG on 9223 + terminal). Telegram/Discord/Slack get attachments + links, not the live pane.

## Reply format (required)
- Write every user-visible answer in GitHub-flavored **Markdown**. The Vectra app renders it (headings, lists, fenced code, links, tables).
- Use `##` / `###` headings, bullets, **bold** for key facts, `inline code` for commands/ids, and fenced ```lang blocks for code.
- Put every URL as a markdown link: `[Open preview]({preview_3000})` — never dump `http://localhost`.
- Keep `MEDIA:path` on its **own raw line** (not inside a code fence) so the gateway can parse it.
- Do not wrap the entire reply in one giant code fence. Prefer short sections the app can render.

## Who you are (this VM)
- You are **Supercharge** running **inside an E2B sandbox**, reached via **Vectra Cloud** / `{channel_name}`. Not a local laptop.
- Workspace: `/home/user/workspace` (cwd for every command). A generated copy of this identity is also at `.superagent/identity.md`.
- **Sandbox id:** `{sandbox}` — if the user asks “what sandbox / instance / vm is this?”, reply with this exact id. Do not invent one.
- **Template:** `{template}`
- Preview domain: `{domain}`
- Formula for any listening port N: `https://N-{sandbox}.{domain}`
- Concrete URLs for **this** sandbox (already filled — not placeholders):
  - Vite / typical frontend 5173 → `{preview_5173}`
  - Next / Node 3000 → `{preview_3000}`
  - Generic 8080 → `{preview_8080}`
  - File shares (wake-http) 8788 → `{preview_8788}` and `{share}`
- After you start a server, curl `http://127.0.0.1:PORT` **inside** the VM to confirm it is up, then send the user the matching `https://PORT-{sandbox}.{domain}` link. Never send localhost.
- Installed: Node 20, npm/pnpm/bun, Python 3 + uv, git, GitHub CLI, ripgrep, fd, tmux, Xfce desktop, Chromium, a terminal and a file manager.
- Services in this VM: app inbox `:8790` (phone/Telegram), wake-http `:8788`, and private loopback browser-sidecar/VNC services. Do not bind or expose those browser services.
- Tools: shell (`run_command` / bash), file read/write/edit, native `browser_*` tools, and when the runtime reports desktop capability, native `desktop_observe` and `desktop_action`. Call tools in the same turn; do not only describe them.{id_hint}

## Files and links for the user
- The user **cannot** open sandbox disk paths. Never say a file is “at /home/user/workspace/…”.
- To deliver **any** artifact (zip, tar.gz, pdf, png, video, code, csv) put `MEDIA:relative/path` in your **final** reply. The gateway then does both:
  1. Native upload (phone app file/photo card, Telegram/Discord/Slack attachment)
  2. A public download link `{share}` (wake-http — works after pause)
- Always MEDIA: zips and screenshots. Do not only describe the path.
- Bind servers to `0.0.0.0`.

### Vite / dev-server host allowlist
- E2B opens the app as `https://{{port}}-{sandbox}.{domain}` (example: `{preview_3000}`). Vite blocks unknown Host headers → "This host is not allowed".
- Whenever you create or run a Vite app, set in vite.config before sharing the link:
```js
export default defineConfig({{
  server: {{
    host: '0.0.0.0',
    allowedHosts: true,
  }},
}})
```
- Equivalent: `allowedHosts: ['.{domain}', '{sandbox}.{domain}']`. Bind with `--host 0.0.0.0`. Next.js: `next dev -H 0.0.0.0`. webpack-dev-server may need `allowedHosts: "all"`.
- If the user already sees "host is not allowed", edit vite.config immediately, restart, then re-send `{preview_5173}` or the port you actually used.

### sudo
- You may use passwordless `sudo -n <cmd>` for apt/global npm. Never interactive sudo. Destructive wipes are forbidden.
"#
    )
}

/// Replace localhost / 127.0.0.1 links in user-facing text with E2B preview URLs.
pub fn rewrite_localhost_for_user(text: &str) -> String {
    let Some(sandbox) = sandbox_id() else {
        return text.to_owned();
    };
    let domain = preview_domain();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest
        .find("http://localhost")
        .into_iter()
        .chain(rest.find("https://localhost"))
        .chain(rest.find("http://127.0.0.1"))
        .chain(rest.find("https://127.0.0.1"))
        .min()
    {
        out.push_str(&rest[..idx]);
        let after_scheme = rest[idx..].find("://").map(|n| idx + n + 3).unwrap_or(idx);
        let host = if rest[after_scheme..].starts_with("localhost") {
            "localhost"
        } else {
            "127.0.0.1"
        };
        let mut cursor = after_scheme + host.len();
        let mut port = 3000u16;
        if rest[cursor..].starts_with(':') {
            let digits: String = rest[cursor + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(parsed) = digits.parse::<u16>() {
                port = parsed;
            }
            cursor += 1 + digits.len();
        }
        let path_end = rest[cursor..]
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\''))
            .map(|n| cursor + n)
            .unwrap_or(rest.len());
        let path = &rest[cursor..path_end];
        out.push_str(&format!("https://{port}-{sandbox}.{domain}{path}"));
        rest = &rest[path_end..];
    }
    out.push_str(rest);
    out
}

/// App-inbox JSON events (XibeCode `AppAdapter` compatible).
pub fn app_events_for(
    event: &LiveEvent,
    tool_count: u32,
    live_text: &str,
    live_thought: &str,
) -> Vec<Value> {
    match event {
        LiveEvent::TextDelta(_) => {
            if live_text.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "text",
                    "text": live_text,
                    "final": false,
                    "format": "markdown"
                })]
            }
        }
        LiveEvent::Thought(_) => {
            let text = if live_thought.is_empty() {
                return Vec::new();
            } else {
                live_thought
            };
            vec![json!({
                "type": "progress",
                "kind": "thinking",
                "text": text,
                "tools": tool_count
            })]
        }
        LiveEvent::ToolStart { name, title, .. } => {
            let computer_kind = tool_computer_kind(&format!("{name} {title}"));
            let tab = if computer_kind == "browser" {
                "browser"
            } else {
                "terminal"
            };
            let focus = computer_kind == "browser"
                && title.to_ascii_lowercase().contains("browser_navigate");
            vec![
                json!({
                    "type": "tool",
                    "name": name,
                    "state": "start",
                    "summary": title,
                    "text": format!("$ {title}"),
                    "command": title
                }),
                json!({
                    "type": "computer",
                    "tab": tab,
                    "kind": computer_kind,
                    "name": name,
                    "state": "start",
                    "label": title,
                    "command": title,
                    "text": title,
                    "focus": focus
                }),
                json!({"type":"status","busy":true,"tools":tool_count,"lastTool":name}),
            ]
        }
        LiveEvent::ToolDone {
            name,
            command,
            stdout,
            success,
            exit_code,
            ..
        } => {
            let computer_kind = tool_computer_kind(&format!("{name} {command}"));
            let tab = if computer_kind == "browser" {
                "browser"
            } else {
                "terminal"
            };
            let summary = if stdout.is_empty() {
                if *success {
                    "done".to_owned()
                } else {
                    "failed".to_owned()
                }
            } else {
                cap_text(stdout, 240)
            };
            let mut events = vec![
                json!({
                    "type": "tool",
                    "name": name,
                    "state": "done",
                    "summary": summary,
                    "text": if stdout.is_empty() {
                        format!("$ {command}")
                    } else {
                        format!("$ {command}\n{stdout}")
                    },
                    "command": command,
                    "stdout": stdout
                }),
                json!({
                    "type": "computer",
                    "tab": tab,
                    "kind": computer_kind,
                    "name": name,
                    "state": "done",
                    "label": command,
                    "command": command,
                    "stdout": stdout,
                    "success": success,
                    "exitCode": exit_code,
                    "text": stdout
                }),
            ];
            if let Some(path) = screenshot_path(command, stdout) {
                let file_name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("screenshot.png");
                events.push(json!({
                    "type": "file",
                    "kind": "photo",
                    "name": file_name,
                    "caption": command,
                    "text": path,
                    "mime": "image/png"
                }));
                events.push(json!({
                    "type": "computer",
                    "tab": "browser",
                    "kind": "focus",
                    "action": "screenshot",
                    "state": "done",
                    "label": "Screenshot",
                    "focus": true
                }));
            }
            events
        }
        LiveEvent::Usage { used, max } => vec![json!({
            "type": "usage",
            "used": used,
            "max": max
        })],
    }
}

/// Compact Telegram/Discord/Slack progress line (XibeCode `formatToolProgress`).
pub fn messenger_progress(event: &LiveEvent, tool_count: u32) -> Option<String> {
    match event {
        LiveEvent::Thought(text) => Some(text.chars().take(350).collect()),
        LiveEvent::ToolStart { name, title, .. } => Some(format!("[{tool_count}] {name}: {title}")),
        LiveEvent::ToolDone {
            command,
            success,
            stdout,
            ..
        } => {
            let mark = if *success { "ok" } else { "fail" };
            let cmd = if command.is_empty() {
                "tool"
            } else {
                command.as_str()
            };
            if stdout.is_empty() {
                Some(format!("[{tool_count}] {cmd} {mark}"))
            } else {
                Some(format!(
                    "[{tool_count}] {cmd} {mark}\n{}",
                    cap_text(stdout, 280)
                ))
            }
        }
        LiveEvent::TextDelta(_) | LiveEvent::Usage { .. } => None,
    }
}

/// Advance tool count / thought buffer and return the latest messenger progress line.
pub fn next_messenger_progress(
    line: &Value,
    tool_count: &mut u32,
    thought: &mut String,
) -> Option<String> {
    let mut latest = None;
    for event in parse_cli_stream_line(line) {
        if let LiveEvent::ToolStart { .. } = &event {
            *tool_count = tool_count.saturating_add(1);
        }
        if let LiveEvent::Thought(chunk) = &event {
            thought.push_str(chunk);
            latest = Some(tail_chars(thought, 800));
            continue;
        }
        if let Some(text) = messenger_progress(&event, *tool_count) {
            latest = Some(text);
        }
    }
    latest
}

fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    text.chars().skip(count - max).collect()
}

#[cfg(test)]
mod tests {
    use super::{LiveEvent, app_events_for, parse_cli_stream_line};
    use serde_json::json;

    #[test]
    fn streaming_json_tool_and_text_map_to_xibecode_app_events() {
        let start = parse_cli_stream_line(&json!({
            "type": "tool_call",
            "toolCallId": "t1",
            "toolName": "shell",
            "title": "ls workspace"
        }));
        assert_eq!(
            start,
            vec![LiveEvent::ToolStart {
                id: "t1".into(),
                name: "shell".into(),
                title: "ls workspace".into()
            }]
        );
        let events = app_events_for(&start[0], 1, "", "");
        assert_eq!(events[0]["type"], "tool");
        assert_eq!(events[0]["state"], "start");
        assert_eq!(events[0]["command"], "ls workspace");
        assert_eq!(events[1]["type"], "computer");
        assert_eq!(events[1]["command"], "ls workspace");

        let done = parse_cli_stream_line(&json!({
            "type": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
            "rawOutput": {"stdout": "a.txt\nb.txt"}
        }));
        let mut done_event = done[0].clone();
        if let LiveEvent::ToolDone { command, .. } = &mut done_event {
            *command = "ls workspace".into();
        }
        let finished = app_events_for(&done_event, 1, "", "");
        assert_eq!(finished[0]["stdout"], "a.txt\nb.txt");
        assert_eq!(finished[1]["command"], "ls workspace");
        assert_eq!(finished[1]["stdout"], "a.txt\nb.txt");

        let text = parse_cli_stream_line(&json!({"type":"text","data":"hello "}));
        assert_eq!(text, vec![LiveEvent::TextDelta("hello ".into())]);
        let live = app_events_for(&text[0], 1, "hello ", "");
        assert_eq!(live[0]["final"], false);
        assert_eq!(live[0]["text"], "hello ");

        let thought = parse_cli_stream_line(&json!({"type":"thought","data":"plan "}));
        let shown = app_events_for(&thought[0], 0, "", "plan the fix");
        assert_eq!(shown[0]["type"], "progress");
        assert_eq!(shown[0]["kind"], "thinking");
        assert_eq!(shown[0]["text"], "plan the fix");

        let nested = parse_cli_stream_line(&json!({
            "type": "thinking",
            "content": [{"type": "text", "text": "consider the tests"}]
        }));
        assert_eq!(
            nested,
            vec![LiveEvent::Thought("consider the tests".into())]
        );

        let browser = parse_cli_stream_line(&json!({
            "type": "tool_call",
            "toolName": "browser_navigate",
            "rawInput": {"url": "https://example.com"}
        }));
        let pane = app_events_for(&browser[0], 1, "", "");
        assert_eq!(pane[1]["type"], "computer");
        assert_eq!(pane[1]["kind"], "browser");
        assert_eq!(pane[1]["tab"], "browser");
    }

    #[test]
    fn bash_raw_output_bytes_become_stdout_and_command() {
        let output: Vec<u8> = b"hello.txt\n".to_vec();
        let done = parse_cli_stream_line(&json!({
            "type": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
            "rawOutput": {
                "type": "Bash",
                "command": "ls",
                "exit_code": 0,
                "output": output,
                "output_for_prompt": "hello.txt"
            }
        }));
        match &done[0] {
            LiveEvent::ToolDone {
                command,
                stdout,
                success,
                exit_code,
                ..
            } => {
                assert_eq!(command, "ls");
                assert_eq!(stdout, "hello.txt");
                assert!(*success);
                assert_eq!(*exit_code, Some(0));
            }
            other => panic!("{other:?}"),
        }
        let events = app_events_for(&done[0], 1, "", "");
        assert_eq!(events[1]["command"], "ls");
        assert_eq!(events[1]["stdout"], "hello.txt");
    }

    #[test]
    fn e2b_rules_include_sandbox_vite_sudo_and_media() {
        let block = super::e2b_agent_rules();
        assert!(block.contains("MEDIA:"));
        assert!(block.contains("allowedHosts"));
        assert!(block.contains("sudo -n"));
        assert!(block.contains("0.0.0.0"));
        assert!(block.contains("browser_snapshot"));
        assert!(block.contains("web research, current public facts"));
        assert!(block.contains("fully answerable from local code"));
        assert!(block.contains("same browser tab"));
        assert!(block.contains("browser_handoff"));
        assert!(block.contains("private, ticketed noVNC"));
        assert!(!block.contains("agent-browser"));
        assert!(block.contains("https://N-"));
        assert!(block.contains("https://3000-"));
        assert!(block.contains("https://5173-"));
        assert!(block.contains("Vectra Cloud"));
        assert!(block.contains("E2B"));
        assert!(block.contains("GitHub-flavored"));
        assert!(block.contains("[Open preview]"));
        assert_eq!(super::channel_from_session_key("app:owner"), "vectra-app");
        assert_eq!(
            super::channel_from_session_key("agent:main:telegram:thread:9"),
            "telegram"
        );
        let rewritten = super::rewrite_localhost_for_user("open http://localhost:5173/app");
        assert!(
            rewritten.contains("localhost:5173") || rewritten.contains("https://5173-"),
            "{rewritten}"
        );
    }
}
