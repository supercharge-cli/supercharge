//! Headless browser checks for the UI workflow: opens a page in the locally installed Chrome or
//! Chromium, reads its accessibility tree, and works controls with real mouse and keyboard events.
//! No screenshots: the agent reads roles, names and states the way assistive technology does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::browser::cdp::{CdpError, CdpSession};

/// Environment variable naming the browser executable to use.
pub const BROWSER_ENV: &str = "SUPERCHARGE_CHROME";
/// Most nodes a snapshot prints.
const MAX_SNAPSHOT_LINES: usize = 400;
/// Longest accessible name printed.
const MAX_NAME_CHARS: usize = 120;
/// How long the browser gets to write its debugging port.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a page gets to finish loading after open or an action.
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Message when no usable browser is installed.
pub fn no_browser_message() -> String {
    format!(
        "No Chrome or Chromium browser was found, so the page can't be opened. Install Chrome or Chromium, \
set {BROWSER_ENV}=/path/to/browser, or run `supercharge browser install`. ui_verify still checks typecheck and build."
    )
}

/// Find a Chrome or Chromium executable: [`BROWSER_ENV`], then `CHROME_PATH`, then the usual install
/// locations for this OS, then the browser `supercharge browser install` downloads.
pub fn find_browser() -> Option<PathBuf> {
    for var in [BROWSER_ENV, "CHROME_PATH"] {
        if let Some(path) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            let path = PathBuf::from(path);
            return path.is_file().then_some(path);
        }
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
    }
    if let Some(path) = platform_candidates().into_iter().find(|p| p.is_file()) {
        return Some(path);
    }
    crate::browser::is_installed().then(crate::browser::binary_path)
}

fn platform_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        let apps = [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ];
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = supercharge_dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        for root in &roots {
            for app in apps {
                out.push(root.join(app));
            }
        }
    } else if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                out.push(base.join("Google/Chrome/Application/chrome.exe"));
                out.push(base.join("Chromium/Application/chrome.exe"));
            }
        }
    } else {
        for path in [
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/opt/google/chrome/chrome",
        ] {
            out.push(PathBuf::from(path));
        }
    }
    out
}

/// Parse the `DevToolsActivePort` file the browser writes: the port, then the browser socket path.
pub fn parse_devtools_active_port(contents: &str) -> Option<String> {
    let mut lines = contents.lines().map(str::trim);
    let port: u16 = lines.next()?.parse().ok()?;
    let path = lines.next().filter(|p| p.starts_with('/'))?;
    (port != 0).then(|| format!("ws://127.0.0.1:{port}{path}"))
}

/// Accept `localhost:5173`-style addresses as http URLs.
pub fn normalize_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("`url` is required for open".into());
    }
    let lower = url.to_ascii_lowercase();
    if ["http://", "https://", "file://", "about:", "data:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return Ok(url.to_string());
    }
    if url.contains("://") {
        return Err(format!("unsupported URL scheme in `{url}`"));
    }
    Ok(format!("http://{url}"))
}

/// One key press as CDP `Input.dispatchKeyEvent` fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    pub text: Option<String>,
    pub modifiers: u32,
}

/// Parse `Enter`, `Shift+Tab`, `ArrowDown`, `Control+a`, `Space` or a single character.
pub fn parse_key(spec: &str) -> Result<KeySpec, String> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    let (key_part, modifier_parts) = match parts.split_last() {
        Some((last, rest)) if !last.is_empty() => (*last, rest),
        // "Control++" or "+" means the plus key.
        _ if spec.trim().ends_with('+') => (
            "+",
            parts
                .get(..parts.len().saturating_sub(2))
                .unwrap_or_default(),
        ),
        _ => return Err("`key` is required for press".into()),
    };
    let mut modifiers = 0;
    for modifier in modifier_parts {
        modifiers |= match modifier.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "ctrl" | "control" => 2,
            "meta" | "cmd" | "command" | "super" => 4,
            "shift" => 8,
            other => return Err(format!("unknown modifier `{other}` in `{spec}`")),
        };
    }
    let named = |key: &str, code: &str, key_code: u32, text: Option<&str>| KeySpec {
        key: key.into(),
        code: code.into(),
        key_code,
        text: text.map(Into::into),
        modifiers,
    };
    let spec = match key_part.to_ascii_lowercase().as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" | "del" => named("Delete", "Delete", 46, None),
        "space" | " " => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" => named("PageUp", "PageUp", 33, None),
        "pagedown" => named("PageDown", "PageDown", 34, None),
        _ => {
            let mut chars = key_part.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(format!(
                    "unknown key `{key_part}`; use Enter, Tab, Escape, Space, Backspace, Delete, \
Arrow keys, Home, End, PageUp, PageDown or one character"
                ));
            };
            let upper = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() {
                format!("Key{upper}")
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            let key_code = if c.is_ascii_alphanumeric() {
                u32::from(upper)
            } else {
                0
            };
            // A character typed with Control/Alt/Meta is a shortcut, not text.
            let text = (modifiers & 0b0111 == 0).then(|| c.to_string());
            KeySpec {
                key: c.to_string(),
                code,
                key_code,
                text,
                modifiers,
            }
        }
    };
    Ok(spec)
}

/// States printed after a node's name, in this order.
const STATE_PROPERTIES: &[&str] = &[
    "focused",
    "disabled",
    "expanded",
    "checked",
    "pressed",
    "selected",
    "required",
    "invalid",
    "readonly",
    "haspopup",
    "level",
    "modal",
    "multiselectable",
];
/// States worth printing even when false, because "not expanded" is meaningful.
const STATES_SHOWN_WHEN_FALSE: &[&str] = &["expanded", "checked", "pressed", "selected"];
/// Roles that only group other nodes; printed only when they carry a name.
const STRUCTURAL_ROLES: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "InlineTextBox",
    "LineBreak",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
];

fn ax_str<'a>(node: &'a Value, field: &str) -> &'a str {
    node.get(field)
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn clip(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > MAX_NAME_CHARS {
        let kept: String = text.chars().take(MAX_NAME_CHARS).collect();
        format!("{kept}…")
    } else {
        text
    }
}

fn states(node: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let props = node
        .get("properties")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for wanted in STATE_PROPERTIES {
        let Some(value) = props
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(wanted))
            .and_then(|p| p.get("value"))
            .and_then(|v| v.get("value"))
        else {
            continue;
        };
        match value {
            Value::Bool(true) => out.push((*wanted).to_string()),
            Value::Bool(false) if STATES_SHOWN_WHEN_FALSE.contains(wanted) => {
                out.push(format!("{wanted}=false"))
            }
            Value::String(s) if s == "true" => out.push((*wanted).to_string()),
            Value::String(s) if s == "false" => {
                if STATES_SHOWN_WHEN_FALSE.contains(wanted) {
                    out.push(format!("{wanted}=false"));
                }
            }
            Value::String(s) if !s.is_empty() => out.push(format!("{wanted}={s}")),
            Value::Number(n) => out.push(format!("{wanted}={n}")),
            _ => {}
        }
    }
    let value = ax_str(node, "value");
    if !value.is_empty() {
        out.push(format!("value=\"{}\"", clip(value)));
    }
    out
}

/// A rendered accessibility snapshot and the element refs it hands out.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub text: String,
    /// `e12` → backend DOM node id.
    pub refs: HashMap<String, i64>,
}

/// Render `Accessibility.getFullAXTree` nodes as an indented outline with `[ref=eN]` handles.
pub fn render_ax_tree(nodes: &[Value]) -> Snapshot {
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| Some((n.get("nodeId")?.as_str()?, n)))
        .collect();
    let roots: Vec<&Value> = nodes
        .iter()
        .filter(|n| {
            n.get("parentId")
                .and_then(Value::as_str)
                .is_none_or(|parent| !by_id.contains_key(parent))
        })
        .collect();
    let mut snapshot = Snapshot::default();
    let mut lines: Vec<String> = Vec::new();
    let mut hidden = 0usize;
    // (node, depth, nearest printed ancestor name)
    let mut stack: Vec<(&Value, usize, String)> = roots
        .into_iter()
        .rev()
        .map(|n| (n, 0, String::new()))
        .collect();
    let mut seen = std::collections::HashSet::new();
    while let Some((node, depth, parent_name)) = stack.pop() {
        let id = node.get("nodeId").and_then(Value::as_str).unwrap_or("");
        if !seen.insert(id.to_string()) {
            continue;
        }
        let role = ax_str(node, "role");
        let name = clip(ax_str(node, "name"));
        let ignored = node
            .get("ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let skip = ignored
            || role.is_empty()
            || (STRUCTURAL_ROLES.contains(&role) && name.is_empty())
            || (role == "StaticText" && (name.is_empty() || name == parent_name));
        let (child_depth, child_parent_name) = if skip {
            (depth, parent_name.clone())
        } else if lines.len() >= MAX_SNAPSHOT_LINES {
            hidden += 1;
            (depth + 1, name.clone())
        } else {
            let indent = "  ".repeat(depth);
            let mut line = if role == "StaticText" {
                format!("{indent}- text \"{name}\"")
            } else {
                let shown_role = if role == "RootWebArea" {
                    "document"
                } else {
                    role
                };
                let mut line = format!("{indent}- {shown_role}");
                if !name.is_empty() {
                    line.push_str(&format!(" \"{name}\""));
                }
                if let Some(backend) = node.get("backendDOMNodeId").and_then(Value::as_i64) {
                    let r = format!("e{}", snapshot.refs.len() + 1);
                    line.push_str(&format!(" [ref={r}]"));
                    snapshot.refs.insert(r, backend);
                }
                line
            };
            let states = states(node);
            if !states.is_empty() {
                line.push_str(&format!(" ({})", states.join(", ")));
            }
            lines.push(line);
            (depth + 1, name.clone())
        };
        if let Some(children) = node.get("childIds").and_then(Value::as_array) {
            for child in children.iter().rev() {
                if let Some(child) = child.as_str().and_then(|c| by_id.get(c)) {
                    stack.push((child, child_depth, child_parent_name.clone()));
                }
            }
        }
    }
    snapshot.text = lines.join("\n");
    if hidden > 0 {
        snapshot
            .text
            .push_str(&format!("\n… {hidden} more nodes not shown"));
    }
    snapshot
}

/// Collects page errors so snapshots can report them; runs before any page script.
const ERROR_CAPTURE_SCRIPT: &str = r#"(() => {
  if (window.__superchargeErrors) return;
  const errors = [];
  const keep = (message) => { errors.push(String(message).slice(0, 500)); if (errors.length > 50) errors.shift(); };
  Object.defineProperty(window, '__superchargeErrors', { value: errors });
  addEventListener('error', (e) => keep(e.message || e));
  addEventListener('unhandledrejection', (e) => keep('Unhandled rejection: ' + ((e.reason && e.reason.message) || e.reason)));
  const original = console.error;
  console.error = function (...args) { keep(args.map(String).join(' ')); return original.apply(this, args); };
})()"#;

/// A running headless browser with one page.
pub struct UiBrowser {
    session: CdpSession,
    child: tokio::process::Child,
    group: std::sync::Arc<crate::util::ProcessGroup>,
    refs: HashMap<String, i64>,
    executable: PathBuf,
    _profile: tempfile::TempDir,
}

impl Drop for UiBrowser {
    fn drop(&mut self) {
        let _ = self.group.kill();
        let _ = self.child.start_kill();
    }
}

fn cdp(err: CdpError) -> String {
    err.to_string()
}

impl UiBrowser {
    /// Launch `executable` headless with a throwaway profile and attach to a blank page.
    pub async fn launch(executable: &Path) -> Result<Self, String> {
        let profile = tempfile::Builder::new()
            .prefix("supercharge-ui-browser-")
            .tempdir()
            .map_err(|e| format!("could not create a browser profile folder: {e}"))?;
        let mut cmd = tokio::process::Command::new(executable);
        cmd.arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-sync",
                "--mute-audio",
                "--window-size=1280,800",
            ]);
        if cfg!(target_os = "linux") {
            cmd.arg("--disable-dev-shm-usage");
            if running_as_root() {
                cmd.arg("--no-sandbox");
            }
        }
        cmd.arg("about:blank")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // Enrolled in the process-wide scope so the browser never outlives Supercharge.
        let (mut child, group) = crate::util::spawn::global_process_scope()
            .spawn(cmd)
            .map_err(|e| {
                format!(
                    "could not start the browser at {}: {e}",
                    executable.display()
                )
            })?;
        let port_file = profile.path().join("DevToolsActivePort");
        let started = Instant::now();
        let ws = loop {
            if let Some(ws) = tokio::fs::read_to_string(&port_file)
                .await
                .ok()
                .and_then(|c| parse_devtools_active_port(&c))
            {
                break ws;
            }
            if let Ok(Some(status)) = child.try_wait() {
                let _ = group.kill();
                return Err(format!(
                    "the browser at {} exited during startup ({status}); set {BROWSER_ENV} to a working Chrome or Chromium",
                    executable.display()
                ));
            }
            if started.elapsed() > LAUNCH_TIMEOUT {
                let _ = group.kill();
                let _ = child.start_kill();
                return Err(format!(
                    "the browser at {} did not open its debugging port within {}s",
                    executable.display(),
                    LAUNCH_TIMEOUT.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let connected = async {
            let mut session = CdpSession::connect(&ws).await?;
            session.attach_page("about:blank").await?;
            session.call("DOM.enable", json!({})).await?;
            session.call("Accessibility.enable", json!({})).await?;
            session
                .call(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": ERROR_CAPTURE_SCRIPT }),
                )
                .await?;
            Ok::<_, CdpError>(session)
        }
        .await;
        match connected {
            Ok(session) => Ok(Self {
                session,
                child,
                group,
                refs: HashMap::new(),
                executable: executable.to_path_buf(),
                _profile: profile,
            }),
            Err(e) => {
                let _ = group.kill();
                let _ = child.start_kill();
                Err(format!("could not attach to the browser: {e}"))
            }
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub async fn open(&mut self, url: &str) -> Result<(), String> {
        self.refs.clear();
        self.session.navigate(url).await.map_err(cdp)?;
        self.wait_for_load().await;
        Ok(())
    }

    async fn wait_for_load(&mut self) {
        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(150)).await;
        while started.elapsed() < LOAD_TIMEOUT {
            let state = self.session.evaluate("document.readyState").await;
            if matches!(&state, Ok(Value::String(s)) if s == "complete") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Give client-side frameworks a moment to hydrate and render.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    /// The page outline with fresh refs, plus title, URL and any page errors.
    pub async fn snapshot(&mut self) -> Result<String, String> {
        let tree = self
            .session
            .call("Accessibility.getFullAXTree", json!({}))
            .await
            .map_err(cdp)?;
        let nodes = tree
            .get("nodes")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let snapshot = render_ax_tree(nodes);
        self.refs = snapshot.refs;
        let info = self
            .session
            .evaluate(
                "({ title: document.title, url: location.href, \
focused: document.activeElement && document.activeElement !== document.body \
? (document.activeElement.getAttribute('aria-label') || document.activeElement.textContent || document.activeElement.tagName).trim().slice(0, 80) : null, \
errors: (window.__superchargeErrors || []).slice(-10) })",
            )
            .await
            .unwrap_or(Value::Null);
        let mut out = format!(
            "Page: {} ({})\n",
            info.get("title").and_then(Value::as_str).unwrap_or(""),
            info.get("url").and_then(Value::as_str).unwrap_or("")
        );
        if let Some(focused) = info.get("focused").and_then(Value::as_str) {
            out.push_str(&format!("Focus: {}\n", clip(focused)));
        }
        out.push_str(&snapshot.text);
        let errors: Vec<&str> = info
            .get("errors")
            .and_then(Value::as_array)
            .map(|e| e.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !errors.is_empty() {
            out.push_str("\n\nPage errors:\n");
            for error in errors {
                out.push_str(&format!("- {}\n", clip(error)));
            }
        }
        Ok(out)
    }

    fn backend_id(&self, element: &str) -> Result<i64, String> {
        let element = element.trim().trim_start_matches("ref=");
        self.refs.get(element).copied().ok_or_else(|| {
            format!("unknown ref `{element}`; take a fresh snapshot and use a ref from it")
        })
    }

    pub async fn click(&mut self, element: &str) -> Result<(), String> {
        let backend = self.backend_id(element)?;
        let _ = self
            .session
            .call(
                "DOM.scrollIntoViewIfNeeded",
                json!({ "backendNodeId": backend }),
            )
            .await;
        let model = self
            .session
            .call("DOM.getBoxModel", json!({ "backendNodeId": backend }))
            .await;
        let center = model.ok().and_then(|m| {
            let quad: Vec<f64> = m
                .pointer("/model/content")?
                .as_array()?
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            (quad.len() == 8).then(|| {
                let xs = quad.iter().step_by(2).sum::<f64>() / 4.0;
                let ys = quad.iter().skip(1).step_by(2).sum::<f64>() / 4.0;
                (xs, ys)
            })
        });
        match center {
            Some((x, y)) => {
                for (kind, buttons) in
                    [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)]
                {
                    let params = if kind == "mouseMoved" {
                        json!({ "type": kind, "x": x, "y": y, "buttons": buttons })
                    } else {
                        json!({
                            "type": kind, "x": x, "y": y, "buttons": buttons,
                            "button": "left", "clickCount": 1,
                        })
                    };
                    self.session
                        .call("Input.dispatchMouseEvent", params)
                        .await
                        .map_err(cdp)?;
                }
            }
            // Hidden or zero-size elements have no box; fall back to a DOM click.
            None => {
                self.call_on_element(backend, "function () { this.click(); }")
                    .await?;
            }
        }
        self.wait_for_load().await;
        Ok(())
    }

    async fn call_on_element(&mut self, backend: i64, function: &str) -> Result<(), String> {
        let resolved = self
            .session
            .call("DOM.resolveNode", json!({ "backendNodeId": backend }))
            .await
            .map_err(cdp)?;
        let object_id = resolved
            .pointer("/object/objectId")
            .and_then(Value::as_str)
            .ok_or("the element is no longer in the page; take a fresh snapshot")?
            .to_string();
        self.session
            .call(
                "Runtime.callFunctionOn",
                json!({ "objectId": object_id, "functionDeclaration": function }),
            )
            .await
            .map_err(cdp)?;
        Ok(())
    }

    pub async fn focus(&mut self, element: &str) -> Result<(), String> {
        let backend = self.backend_id(element)?;
        self.session
            .call("DOM.focus", json!({ "backendNodeId": backend }))
            .await
            .map_err(cdp)?;
        Ok(())
    }

    pub async fn type_text(
        &mut self,
        element: &str,
        text: &str,
        clear: bool,
    ) -> Result<(), String> {
        self.focus(element).await?;
        if clear {
            let backend = self.backend_id(element)?;
            self.call_on_element(
                backend,
                "function () { if ('value' in this) { this.value = ''; this.dispatchEvent(new Event('input', { bubbles: true })); } else if (this.isContentEditable) { this.textContent = ''; } }",
            )
            .await?;
        }
        self.session
            .call("Input.insertText", json!({ "text": text }))
            .await
            .map_err(cdp)?;
        self.wait_for_load().await;
        Ok(())
    }

    pub async fn press(&mut self, element: Option<&str>, key: &KeySpec) -> Result<(), String> {
        if let Some(element) = element.filter(|e| !e.trim().is_empty()) {
            self.focus(element).await?;
        }
        let mut down = json!({
            "type": if key.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key.key,
            "code": key.code,
            "windowsVirtualKeyCode": key.key_code,
            "nativeVirtualKeyCode": key.key_code,
            "modifiers": key.modifiers,
        });
        if let (Some(text), Some(fields)) = (&key.text, down.as_object_mut()) {
            fields.insert("text".into(), json!(text));
            fields.insert("unmodifiedText".into(), json!(text));
        }
        self.session
            .call("Input.dispatchKeyEvent", down)
            .await
            .map_err(cdp)?;
        self.session
            .call(
                "Input.dispatchKeyEvent",
                json!({
                    "type": "keyUp",
                    "key": key.key,
                    "code": key.code,
                    "windowsVirtualKeyCode": key.key_code,
                    "nativeVirtualKeyCode": key.key_code,
                    "modifiers": key.modifiers,
                }),
            )
            .await
            .map_err(cdp)?;
        self.wait_for_load().await;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

#[cfg(not(target_os = "linux"))]
fn running_as_root() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent: Option<&str>, role: &str, name: &str, children: &[&str]) -> Value {
        let mut n = json!({
            "nodeId": id,
            "ignored": false,
            "role": { "type": "role", "value": role },
            "name": { "type": "computedString", "value": name },
            "childIds": children,
            "backendDOMNodeId": id.parse::<i64>().unwrap() + 100,
        });
        if let Some(parent) = parent {
            n["parentId"] = json!(parent);
        }
        n
    }

    #[test]
    fn devtools_port_file_parses() {
        assert_eq!(
            parse_devtools_active_port("9333\n/devtools/browser/abc\n").as_deref(),
            Some("ws://127.0.0.1:9333/devtools/browser/abc")
        );
        assert_eq!(parse_devtools_active_port("9333\n"), None);
        assert_eq!(parse_devtools_active_port("x\n/devtools"), None);
        assert_eq!(parse_devtools_active_port(""), None);
    }

    #[test]
    fn urls_normalize() {
        assert_eq!(
            normalize_url("localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(
            normalize_url(" http://127.0.0.1:3000/a ").unwrap(),
            "http://127.0.0.1:3000/a"
        );
        assert_eq!(
            normalize_url("file:///tmp/x.html").unwrap(),
            "file:///tmp/x.html"
        );
        assert!(normalize_url("").is_err());
        assert!(normalize_url("ftp://host").is_err());
    }

    #[test]
    fn keys_parse_with_modifiers() {
        let enter = parse_key("Enter").unwrap();
        assert_eq!((enter.key.as_str(), enter.key_code), ("Enter", 13));
        assert_eq!(enter.text.as_deref(), Some("\r"));
        let back = parse_key("Shift+Tab").unwrap();
        assert_eq!((back.key.as_str(), back.modifiers), ("Tab", 8));
        let select_all = parse_key("Control+a").unwrap();
        assert_eq!(select_all.code, "KeyA");
        assert_eq!(select_all.key_code, 65);
        assert_eq!(select_all.text, None);
        assert_eq!(parse_key("x").unwrap().text.as_deref(), Some("x"));
        assert_eq!(parse_key("space").unwrap().code, "Space");
        assert_eq!(parse_key("ArrowDown").unwrap().key_code, 40);
        assert!(parse_key("Hyper+x").is_err());
        assert!(parse_key("F13").is_err());
        assert!(parse_key("").is_err());
    }

    #[test]
    fn ax_tree_renders_roles_names_states_and_refs() {
        let mut button = node("3", Some("2"), "button", "Open menu", &["4"]);
        button["properties"] = json!([
            { "name": "expanded", "value": { "type": "booleanOrUndefined", "value": false } },
            { "name": "haspopup", "value": { "type": "token", "value": "menu" } },
            { "name": "focused", "value": { "type": "boolean", "value": true } }
        ]);
        let mut textbox = node("5", Some("2"), "textbox", "Email", &[]);
        textbox["value"] = json!({ "type": "string", "value": "a@b.c" });
        textbox["properties"] = json!([
            { "name": "invalid", "value": { "type": "token", "value": "false" } },
            { "name": "required", "value": { "type": "boolean", "value": true } }
        ]);
        let mut hidden = node("6", Some("2"), "button", "Hidden", &[]);
        hidden["ignored"] = json!(true);
        let nodes = vec![
            node("1", None, "RootWebArea", "Settings", &["2"]),
            node("2", Some("1"), "generic", "", &["3", "5", "6", "7"]),
            button,
            node("4", Some("3"), "StaticText", "Open menu", &[]),
            textbox,
            hidden,
            node("7", Some("2"), "StaticText", "Saved", &[]),
        ];
        let snap = render_ax_tree(&nodes);
        let expected = "- document \"Settings\" [ref=e1]\n  \
- button \"Open menu\" [ref=e2] (focused, expanded=false, haspopup=menu)\n  \
- textbox \"Email\" [ref=e3] (required, value=\"a@b.c\")\n  \
- text \"Saved\"";
        assert_eq!(snap.text, expected);
        assert_eq!(snap.refs.get("e2"), Some(&103));
        assert_eq!(snap.refs.get("e3"), Some(&105));
        assert_eq!(snap.refs.len(), 3);
    }

    #[test]
    fn huge_trees_are_capped() {
        let ids: Vec<String> = (2..2 + MAX_SNAPSHOT_LINES as i64 + 50)
            .map(|i| i.to_string())
            .collect();
        let child_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut nodes = vec![node("1", None, "RootWebArea", "Big", &child_refs)];
        for id in &ids {
            nodes.push(node(id, Some("1"), "link", &format!("Link {id}"), &[]));
        }
        let snap = render_ax_tree(&nodes);
        assert_eq!(snap.text.lines().count(), MAX_SNAPSHOT_LINES + 1);
        assert!(
            snap.text.ends_with("… 51 more nodes not shown"),
            "{}",
            snap.text.lines().last().unwrap()
        );
    }

    #[test]
    fn platform_candidates_are_absolute_and_the_hint_names_the_override() {
        assert!(no_browser_message().contains(BROWSER_ENV));
        assert!(platform_candidates().iter().all(|p| p.is_absolute()));
    }

    /// Drives a real headless Chrome or Chromium when one is installed; skipped otherwise.
    #[tokio::test]
    async fn real_browser_reads_and_works_controls() {
        let Some(executable) = find_browser() else {
            eprintln!("skipping: no Chrome or Chromium installed");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("index.html");
        std::fs::write(
            &page,
            r#"<!doctype html><title>Form</title>
<label>Name <input id="name"></label>
<button id="save" aria-expanded="false" onclick="this.setAttribute('aria-expanded','true');document.getElementById('out').textContent='Saved ' + document.getElementById('name').value">Save</button>
<p id="out">Not saved</p>
<script>document.getElementById('name').addEventListener('keydown', e => { if (e.key === 'Enter') document.getElementById('out').textContent = 'Submitted'; });</script>"#,
        )
        .unwrap();
        let mut browser = match UiBrowser::launch(&executable).await {
            Ok(browser) => browser,
            Err(e) => {
                eprintln!("skipping: browser did not start: {e}");
                return;
            }
        };
        let url = format!("file://{}", page.display());
        browser.open(&url).await.unwrap();
        let snap = browser.snapshot().await.unwrap();
        assert!(snap.contains("Page: Form"), "{snap}");
        assert!(snap.contains("button \"Save\""), "{snap}");
        assert!(snap.contains("expanded=false"), "{snap}");
        let field_ref = ref_for(&snap, "textbox \"Name\"");
        let save_ref = ref_for(&snap, "button \"Save\"");

        browser.type_text(&field_ref, "Ada", false).await.unwrap();
        browser.click(&save_ref).await.unwrap();
        let snap = browser.snapshot().await.unwrap();
        assert!(snap.contains("Saved Ada"), "{snap}");
        assert!(
            snap.contains("(expanded)")
                || snap.contains("expanded,")
                || snap.contains(", expanded"),
            "{snap}"
        );

        let field_ref = ref_for(&snap, "textbox \"Name\"");
        browser
            .press(Some(&field_ref), &parse_key("Enter").unwrap())
            .await
            .unwrap();
        let snap = browser.snapshot().await.unwrap();
        assert!(snap.contains("Submitted"), "{snap}");
        assert!(
            browser
                .click("e999")
                .await
                .unwrap_err()
                .contains("unknown ref")
        );
    }

    fn ref_for(snapshot: &str, needle: &str) -> String {
        let line = snapshot
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not in {snapshot}"));
        let start = line.find("[ref=").unwrap() + 5;
        let end = start + line[start..].find(']').unwrap();
        line[start..end].to_string()
    }
}
