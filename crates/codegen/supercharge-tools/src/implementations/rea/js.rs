//! Cheap, bounded markers for JavaScript and Electron sources.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct JsMarkers {
    pub markers: Vec<String>,
    pub source_map: Option<String>,
    pub minified_likely: bool,
}

const PATTERNS: &[(&str, &str)] = &[
    ("require('child_process')", "spawns processes (child_process)"),
    ("require(\"child_process\")", "spawns processes (child_process)"),
    ("nodeIntegration: true", "renderer has Node integration"),
    ("nodeIntegration:!0", "renderer has Node integration"),
    ("contextIsolation: false", "context isolation disabled"),
    ("contextIsolation:!1", "context isolation disabled"),
    ("ipcMain", "main-process IPC handlers"),
    ("ipcRenderer", "renderer IPC calls"),
    ("contextBridge", "exposes APIs through contextBridge"),
    ("webSecurity: false", "web security disabled"),
    ("eval(", "dynamic eval"),
    ("new Function(", "dynamic Function constructor"),
    ("process.dlopen", "loads native code"),
    ("fetch(", "network fetch"),
    ("XMLHttpRequest", "network XHR"),
    ("WebSocket", "WebSocket use"),
];

pub fn js_markers(text: &str) -> JsMarkers {
    let mut out = JsMarkers::default();
    for (needle, label) in PATTERNS {
        if text.contains(needle) && !out.markers.iter().any(|m| m == label) {
            out.markers.push((*label).to_owned());
        }
    }
    out.source_map = text
        .rfind("sourceMappingURL=")
        .map(|i| text[i + 17..].lines().next().unwrap_or("").chars().take(256).collect());
    let longest = text.lines().map(str::len).max().unwrap_or(0);
    out.minified_likely = longest > 2_000;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_markers_and_source_map() {
        let m = js_markers("ipcMain.on('x');\n//# sourceMappingURL=app.js.map\n");
        assert!(m.markers.iter().any(|s| s.contains("IPC")));
        assert_eq!(m.source_map.as_deref(), Some("app.js.map"));
        assert!(!m.minified_likely);
    }
}
