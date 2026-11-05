//! Live CDP smoke against a real browser binary on Linux.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::path::PathBuf;

#[tokio::test]
async fn open_local_page_read_text_and_screenshot() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser_hello.html");
    assert!(fixture.is_file(), "missing fixture {}", fixture.display());

    // Prefer a pre-fetched binary from the agent workspace to avoid network in CI.
    let probe = PathBuf::from("/tmp/obscura-probe/obscura");
    let home = tempfile::tempdir().unwrap();
    let browser_dir = home.path().join(".supercharge/browser");
    std::fs::create_dir_all(&browser_dir).unwrap();
    let dest = browser_dir.join("obscura");
    if probe.is_file() {
        std::fs::copy(&probe, &dest).unwrap();
    } else {
        // Fall back to install_latest (needs network).
        unsafe {
            std::env::set_var("SUPERCHARGE_HOME", home.path().join(".supercharge"));
            std::env::set_var("HOME", home.path());
        }
        supercharge_tools::browser::install_latest()
            .await
            .expect("install browser binary");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&dest).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&dest, p).unwrap();
    }
    unsafe {
        std::env::set_var("SUPERCHARGE_HOME", home.path().join(".supercharge"));
        std::env::set_var("HOME", home.path());
    }
    assert!(supercharge_tools::browser::is_installed());

    let url = format!("file://{}", fixture.display());
    let runtime = supercharge_tools::browser::BrowserRuntime::global();
    let mut session = runtime.session(&url).await.expect("cdp session");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let text = session.page_text().await.expect("page text");
    assert!(
        text.contains("Hello from Supercharge browser fixture"),
        "unexpected text: {text:?}"
    );
    let png = session.screenshot_png().await.expect("screenshot");
    assert!(png.starts_with(b"\x89PNG"), "not a png ({} bytes)", png.len());
    runtime.shutdown();
}
