use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

fn endpoint(base_url: &str) -> TranscriptionEndpoint {
    TranscriptionEndpoint {
        base_url: base_url.to_owned(),
        api_key: "test-key".to_owned(),
        model: "stt-model".to_owned(),
    }
}

fn clip() -> AudioClip {
    AudioClip {
        wav: b"RIFF0000WAVEfake".to_vec(),
        language: "en".to_owned(),
    }
}

/// One-connection HTTP server: records the raw request and answers with `status` and `body`.
async fn serve_once(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<u8>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        // Read headers, then exactly Content-Length bytes of body.
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(buf.get(..n).unwrap());
            let text = String::from_utf8_lossy(&request).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + len {
                    break;
                }
            }
        }
        *log.lock().unwrap() = request;
        let reply = format!(
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(reply.as_bytes()).await.unwrap();
        socket.shutdown().await.ok();
    });
    (format!("http://{addr}/v1"), seen)
}

#[tokio::test]
async fn sends_multipart_with_model_language_and_bearer() {
    let (base, seen) = serve_once(200, r#"{"text":" hello world "}"#).await;
    let transcriber = HttpClipTranscriber::new(endpoint(&base), "supercharge-test");
    let text = transcriber.transcribe(clip()).await.unwrap();
    assert_eq!("hello world", text);
    let request = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    assert!(
        request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-key")
    );
    assert!(request.contains("name=\"model\"\r\n\r\nstt-model"));
    assert!(request.contains("name=\"language\"\r\n\r\nen"));
    assert!(request.contains("name=\"response_format\"\r\n\r\njson"));
    assert!(request.contains("filename=\"clip.wav\""));
    assert!(request.contains("RIFF0000WAVEfake"));
}

#[tokio::test]
async fn auto_language_is_not_sent() {
    let (base, seen) = serve_once(200, r#"{"text":"hi"}"#).await;
    let transcriber = HttpClipTranscriber::new(endpoint(&base), "");
    let mut auto = clip();
    auto.language = crate::STT_LANGUAGE_AUTO.to_owned();
    assert_eq!("hi", transcriber.transcribe(auto).await.unwrap());
    let request = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    assert!(!request.contains("name=\"language\""));
}

#[tokio::test]
async fn missing_endpoint_is_a_fatal_neutral_message() {
    let (base, _) = serve_once(404, r#"{"error":{"message":"not found"}}"#).await;
    let error = HttpClipTranscriber::new(endpoint(&base), "")
        .transcribe(clip())
        .await
        .unwrap_err();
    assert!(error.fatal);
    assert!(
        error.message.contains("/audio/transcriptions"),
        "{}",
        error.message
    );
    assert!(error.message.contains("[voice] base_url"));
}

#[tokio::test]
async fn missing_key_fails_without_a_request() {
    let mut no_key = endpoint("https://provider.example/v1");
    no_key.api_key = "  ".to_owned();
    let error = HttpClipTranscriber::new(no_key, "")
        .transcribe(clip())
        .await
        .unwrap_err();
    assert!(error.fatal);
    assert!(
        error
            .message
            .contains("needs an API key for provider.example")
    );
}

#[tokio::test]
async fn unreachable_host_is_transient() {
    // Bind then drop so the port refuses connections.
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let error = HttpClipTranscriber::new(endpoint(&format!("http://{addr}")), "")
        .transcribe(clip())
        .await
        .unwrap_err();
    assert!(!error.fatal);
    assert!(
        error.message.contains("could not reach"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn garbage_reply_is_an_error_not_a_panic() {
    let (base, _) = serve_once(200, "<html>gateway</html>").await;
    let error = HttpClipTranscriber::new(endpoint(&base), "")
        .transcribe(clip())
        .await
        .unwrap_err();
    assert!(!error.fatal);
    assert!(error.message.contains("without a transcript"));
}

#[test]
fn url_joins_and_validates() {
    assert_eq!(
        "https://p.example/v1/audio/transcriptions",
        transcriptions_url("https://p.example/v1/")
            .unwrap()
            .as_str()
    );
    assert_eq!(
        "https://p.example/v1/audio/transcriptions",
        transcriptions_url(" https://p.example/v1/audio/transcriptions ")
            .unwrap()
            .as_str()
    );
    assert!(transcriptions_url("http://127.0.0.1:8080/v1").is_ok());
    assert!(transcriptions_url("http://localhost/v1").is_ok());
    let clear = transcriptions_url("http://p.example/v1").unwrap_err();
    assert!(clear.fatal && clear.message.contains("plain http"));
    for bad in ["", "   ", "not a url", "ftp://p.example", "file:///tmp"] {
        assert!(transcriptions_url(bad).unwrap_err().fatal, "{bad:?}");
    }
}

#[test]
fn bad_url_reports_on_use() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = rt
        .block_on(HttpClipTranscriber::new(endpoint("nope"), "").transcribe(clip()))
        .unwrap_err();
    assert!(error.fatal && error.message.contains("[voice] base_url"));
}

#[test]
fn success_shapes() {
    let ok = |body: &str| interpret_reply(200, body.as_bytes(), "m", "h");
    assert_eq!("a b", ok(r#"{"text":"a b"}"#).unwrap());
    assert_eq!("", ok("").unwrap());
    assert_eq!("", ok("  \n").unwrap());
    assert_eq!("", ok(r#"{"text":""}"#).unwrap());
    assert_eq!("plain words", ok("plain words\n").unwrap());
    assert_eq!("quoted", ok(r#""quoted""#).unwrap());
    assert!(ok(r#"{"text":5}"#).is_err());
    assert!(ok(r#"{"other":1}"#).is_err());
    assert!(ok("[1,2]").is_err());
    assert!(ok("{broken").is_err());
    let error = ok(r#"{"error":{"message":"quota"}}"#).unwrap_err();
    assert!(error.message.contains("quota"));
    // Invalid UTF-8 never panics.
    assert!(interpret_reply(200, &[0xff, 0xfe, b'{'], "m", "h").is_err());
}

#[test]
fn error_statuses() {
    let err = |status, body: &str| {
        interpret_reply(status, body.as_bytes(), "stt-model", "h").unwrap_err()
    };
    let auth = err(401, r#"{"error":{"message":"Invalid key"}}"#);
    assert!(
        auth.fatal
            && auth.message.contains("rejected the API key")
            && auth.message.contains("Invalid key")
    );
    assert!(err(403, "").fatal);
    assert!(err(405, "").fatal);
    let model = err(
        400,
        r#"{"error":{"message":"model `stt-model` does not exist"}}"#,
    );
    assert!(
        model.fatal && model.message.contains("[voice] model"),
        "{}",
        model.message
    );
    let other_400 = err(400, r#"{"error":{"message":"audio too short"}}"#);
    assert!(!other_400.fatal && other_400.message.contains("audio too short"));
    assert!(!err(413, "").fatal);
    let limited = err(429, "");
    assert!(!limited.fatal && limited.message.contains("rate limiting"));
    let server = err(503, "upstream down");
    assert!(!server.fatal && server.message.contains("HTTP 503"));
    assert!(!err(418, "").fatal);
}

#[test]
fn error_detail_is_single_line_and_bounded() {
    let long = "x".repeat(500);
    let body = format!(r#"{{"error":{{"message":"line1\nline2\t{long}"}}}}"#);
    let detail = error_detail(&body).unwrap();
    assert!(!detail.contains('\n') && !detail.contains('\t'));
    assert!(detail.starts_with("line1 line2 "));
    assert_eq!(MAX_DETAIL_CHARS + 1, detail.chars().count());
    assert_eq!(
        Some("plain".to_owned()),
        error_detail(r#"{"error":"plain"}"#)
    );
    assert_eq!(Some("d".to_owned()), error_detail(r#"{"detail":"d"}"#));
    assert_eq!(None, error_detail("not json"));
    assert_eq!(None, error_detail(r#"{"error":{"message":"  "}}"#));
}

#[test]
fn timeout_scales_and_caps() {
    assert_eq!(Duration::from_secs(20), request_timeout(0));
    assert_eq!(Duration::from_secs(30), request_timeout(1_000_000));
    assert_eq!(Duration::from_secs(220), request_timeout(usize::MAX));
}

#[test]
fn debug_never_shows_the_key() {
    let shown = format!(
        "{:?}",
        HttpClipTranscriber::new(endpoint("https://p.example"), "")
    );
    assert!(!shown.contains("test-key"));
    assert!(shown.contains("<set>"));
}

#[tokio::test]
async fn empty_key_falls_back_to_the_agent_key() {
    let (base, seen) = serve_once(200, r#"{"text":"ok"}"#).await;
    let mut no_key = endpoint(&base);
    no_key.api_key = String::new();
    let transcriber = HttpClipTranscriber::new(no_key, "")
        .with_fallback_key(crate::StaticVoiceAuth::shared("agent-key").unwrap());
    assert_eq!("ok", transcriber.transcribe(clip()).await.unwrap());
    let request = String::from_utf8_lossy(&seen.lock().unwrap()).to_ascii_lowercase();
    assert!(request.contains("authorization: bearer agent-key"));
}

#[test]
fn interims_are_opt_in() {
    let transcriber = HttpClipTranscriber::new(endpoint("https://p.example"), "");
    assert!(!transcriber.interims());
    assert!(transcriber.with_interims(true).interims());
}
