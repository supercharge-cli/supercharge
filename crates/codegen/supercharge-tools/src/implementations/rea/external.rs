//! Optional heavy engines on PATH. Detect only; never crash if missing.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ExternalToolStatus {
    pub name: &'static str,
    pub on_path: bool,
    pub resolved: Option<String>,
    pub note: String,
}

/// Tools that are optional. Built-in native analysis does not require them.
pub fn probe_optional_engines() -> Vec<ExternalToolStatus> {
    vec![
        probe(
            "ghidraRun",
            "Ghidra headless/UI launcher; used only when present for deep decompilation.",
        ),
        probe("analyzeHeadless", "Ghidra headless analyzer; optional."),
        probe(
            "tshark",
            "Wireshark CLI; optional for PCAP field extraction.",
        ),
        probe(
            "jadx",
            "Android decompiler; optional for APK method bodies.",
        ),
        probe("objdump", "GNU binutils; optional disassembly fallback."),
    ]
}

fn probe(name: &'static str, note: &str) -> ExternalToolStatus {
    let resolved = which(name);
    ExternalToolStatus {
        name,
        on_path: resolved.is_some(),
        resolved,
        note: note.to_owned(),
    }
}

fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe.display().to_string());
            }
        }
    }
    None
}

pub fn status_json() -> String {
    let tools = probe_optional_engines();
    serde_json::json!({
        "operation": "rea_external_tools",
        "normalized_result": { "tools": tools },
        "limitations": [
            "Optional engines are never installed automatically. When absent, native tools still work for triage, hashes, strings, and static headers."
        ],
        "untrusted_content": false
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_never_panics() {
        let s = status_json();
        assert!(s.contains("rea_external_tools"));
        assert!(s.contains("ghidraRun") || s.contains("tshark"));
    }
}

/// `rea_pcap`: protocol hierarchy and conversations through tshark, when installed.
pub async fn pcap_summary(path: &str) -> String {
    const OP: &str = "rea_pcap";
    let Some(tshark) = which("tshark") else {
        return super::evidence::EvidenceError::new(
            OP,
            Some(path.into()),
            "tshark is not installed or not on PATH. Install Wireshark's command-line tools to summarize captures; rea_identify, rea_strings and rea_hash still work on the file.",
        )
        .to_json_text();
    };
    if !std::path::Path::new(path).is_file() {
        return super::evidence::EvidenceError::new(OP, Some(path.into()), "file not found").to_json_text();
    }
    let run = tokio::process::Command::new(&tshark)
        .args(["-n", "-r", path, "-q", "-z", "io,phs", "-z", "conv,ip"])
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(std::time::Duration::from_secs(120), run).await {
        Err(_) => return super::evidence::EvidenceError::new(OP, Some(path.into()), "tshark timed out after 120s").to_json_text(),
        Ok(Err(e)) => return super::evidence::EvidenceError::new(OP, Some(path.into()), format!("tshark failed to start: {e}")).to_json_text(),
        Ok(Ok(o)) => o,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    let cap = |s: &str| s.chars().take(256 * 1024).collect::<String>();
    let mut reply = super::evidence::EvidenceReply::ok(
        OP,
        path,
        serde_json::json!({ "exit_code": out.status.code(), "summary": cap(&text), "stderr": cap(&err) }),
    );
    if !out.status.success() {
        reply = reply.with_limitation("tshark exited with an error; see stderr");
    }
    reply.to_json_text()
}
