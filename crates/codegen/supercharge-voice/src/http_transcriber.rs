//! Clip transcriber for any provider with a standard `POST {base}/audio/transcriptions` endpoint
//! (multipart `file` + `model`, JSON `{"text": ...}` reply). The key is the user's own (BYOK); nothing here
//! falls back to another credential. Every failure becomes a [`TranscribeError`] whose message names the fix.

use std::time::Duration;

use crate::transcriber::{AudioClip, ClipTranscriber, TranscribeError};

/// Used when `[voice] model` is unset. Providers that name their models differently answer with a model error,
/// which is reported with the setting to change.
pub const DEFAULT_TRANSCRIPTION_MODEL: &str = "whisper-1";

/// Path appended to the provider base URL.
pub const TRANSCRIPTIONS_PATH: &str = "/audio/transcriptions";

/// Largest reply body read; a transcript of the longest clip is far smaller.
const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// Longest provider error detail shown in a toast.
const MAX_DETAIL_CHARS: usize = 160;

/// Where and how to transcribe.
#[derive(Clone, PartialEq, Eq)]
pub struct TranscriptionEndpoint {
    /// Provider base URL, e.g. `https://provider.example/v1`. A URL already ending in the transcriptions path is used as is.
    pub base_url: String,
    /// The user's key for that provider; empty when none is configured (reported on use, never sent).
    pub api_key: String,
    pub model: String,
}

impl std::fmt::Debug for TranscriptionEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptionEndpoint")
            .field("base_url", &self.base_url)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<none>"
                } else {
                    "<set>"
                },
            )
            .field("model", &self.model)
            .finish()
    }
}

/// The full transcriptions URL for `base_url`.
///
/// # Errors
/// A message naming the setting when the URL is not absolute http(s), or is plain http to a non-loopback host
/// (the key would travel in the clear).
pub fn transcriptions_url(base_url: &str) -> Result<url::Url, TranscribeError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let invalid = || {
        TranscribeError::fatal(format!(
            "Speech-to-text base URL {trimmed:?} is not a valid http(s) URL; set [voice] base_url or run /setup"
        ))
    };
    if trimmed.is_empty() {
        return Err(invalid());
    }
    let joined = if trimmed.ends_with(TRANSCRIPTIONS_PATH) {
        trimmed.to_owned()
    } else {
        format!("{trimmed}{TRANSCRIPTIONS_PATH}")
    };
    let url = url::Url::parse(&joined).map_err(|_| invalid())?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&url) => {}
        "http" => {
            return Err(TranscribeError::fatal(format!(
                "Speech-to-text base URL {trimmed:?} uses plain http; use https so the API key is not sent in the clear"
            )));
        }
        _ => return Err(invalid()),
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid());
    }
    Ok(url)
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Per-request bound that grows with the upload: a short clip fails fast, the largest still has time on a slow link.
pub(crate) fn request_timeout(wav_len: usize) -> Duration {
    const BASE: Duration = Duration::from_secs(20);
    const MAX: Duration = Duration::from_secs(220);
    // Budget 100 KB/s of upload on top of the base.
    let upload = Duration::from_secs(u64::try_from(wav_len / 100_000).unwrap_or(u64::MAX));
    BASE.saturating_add(upload).min(MAX)
}

/// Transcribes clips against one provider endpoint.
pub struct HttpClipTranscriber {
    endpoint: TranscriptionEndpoint,
    /// `Err` holds the configuration problem, reported on every use instead of failing construction.
    url: Result<url::Url, TranscribeError>,
    client: Result<reqwest::Client, String>,
    /// Consulted per request when `endpoint.api_key` is empty: the key the agent itself uses for this provider.
    fallback_key: Option<crate::auth::SharedVoiceAuth>,
    /// Progressive interims re-upload the growing clip; off by default because the user pays per request.
    interims: bool,
}

impl std::fmt::Debug for HttpClipTranscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClipTranscriber")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl HttpClipTranscriber {
    /// Never fails: a bad URL or a client that cannot be built is reported when a clip is transcribed, so a
    /// misconfiguration surfaces as a toast instead of disabling voice silently.
    pub fn new(endpoint: TranscriptionEndpoint, user_agent: &str) -> Self {
        let url = transcriptions_url(&endpoint.base_url);
        let user_agent = user_agent.to_owned();
        let client = supercharge_extra_ca::build_reqwest_client(move |builder| {
            let builder = builder.connect_timeout(Duration::from_secs(15));
            if user_agent.is_empty() {
                builder
            } else {
                builder.user_agent(user_agent.clone())
            }
        })
        .map_err(|error| error.to_string());
        HttpClipTranscriber {
            endpoint,
            url,
            client,
            fallback_key: None,
            interims: false,
        }
    }

    /// Resolve the key per request from `auth` whenever the endpoint carries none.
    #[must_use]
    pub fn with_fallback_key(mut self, auth: crate::auth::SharedVoiceAuth) -> Self {
        self.fallback_key = Some(auth);
        self
    }

    /// Allow progressive interim transcripts (`[voice] interim_transcripts = true`).
    #[must_use]
    pub fn with_interims(mut self, interims: bool) -> Self {
        self.interims = interims;
        self
    }

    async fn api_key(&self) -> Option<String> {
        let own = self.endpoint.api_key.trim();
        if !own.is_empty() {
            return Some(own.to_owned());
        }
        let fallback = self.fallback_key.as_ref()?.bearer().await.ok()?;
        let fallback = fallback.trim();
        (!fallback.is_empty()).then(|| fallback.to_owned())
    }

    pub fn endpoint(&self) -> &TranscriptionEndpoint {
        &self.endpoint
    }

    async fn transcribe_clip(&self, clip: AudioClip) -> Result<String, TranscribeError> {
        let url = self.url.clone()?;
        let host = url.host_str().unwrap_or_default().to_owned();
        let Some(api_key) = self.api_key().await else {
            return Err(TranscribeError::fatal(format!(
                "Speech-to-text needs an API key for {host}; set [voice] api_key (or env_key) or run /setup"
            )));
        };
        let model = self.endpoint.model.trim();
        if model.is_empty() {
            return Err(TranscribeError::fatal(
                "Speech-to-text model is empty; set [voice] model",
            ));
        }
        let client = self.client.as_ref().map_err(|error| {
            TranscribeError::fatal(format!(
                "Speech-to-text could not start its HTTP client: {error}"
            ))
        })?;
        let timeout = request_timeout(clip.wav.len());
        let file = reqwest::multipart::Part::bytes(clip.wav)
            .file_name("clip.wav")
            .mime_str("audio/wav")
            .map_err(|error| {
                TranscribeError::new(format!("Speech-to-text upload failed: {error}"))
            })?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("model", model.to_owned())
            .text("response_format", "json");
        let language = clip.language.trim();
        if !language.is_empty() && language != crate::STT_LANGUAGE_AUTO {
            form = form.text("language", language.to_owned());
        }
        let response = client
            .post(url)
            .bearer_auth(api_key)
            .timeout(timeout)
            .multipart(form)
            .send()
            .await
            .map_err(|error| transport_error(&error, &host))?;
        let status = response.status().as_u16();
        let body = read_capped(response)
            .await
            .map_err(|error| transport_error(&error, &host))?;
        interpret_reply(status, &body, model, &host)
    }
}

impl ClipTranscriber for HttpClipTranscriber {
    fn interims(&self) -> bool {
        self.interims
    }

    fn transcribe(
        &self,
        clip: AudioClip,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, TranscribeError>> + Send + '_>,
    > {
        Box::pin(self.transcribe_clip(clip))
    }
}

async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let room = MAX_REPLY_BYTES.saturating_sub(body.len());
        body.extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or_default());
        if body.len() >= MAX_REPLY_BYTES {
            break;
        }
    }
    Ok(body)
}

fn transport_error(error: &reqwest::Error, host: &str) -> TranscribeError {
    if error.is_timeout() {
        TranscribeError::new(format!(
            "Speech-to-text request to {host} timed out; try again"
        ))
    } else if error.is_connect() {
        TranscribeError::new(format!(
            "Speech-to-text could not reach {host}; check the connection"
        ))
    } else {
        TranscribeError::new(format!(
            "Speech-to-text request to {host} failed; try again"
        ))
    }
}

/// Turn a provider reply into the transcript or an actionable error. Pure, so every shape is unit-tested.
pub(crate) fn interpret_reply(
    status: u16,
    body: &[u8],
    model: &str,
    host: &str,
) -> Result<String, TranscribeError> {
    let text = String::from_utf8_lossy(body);
    if (200..300).contains(&status) {
        // A transcript is always UTF-8 text; anything else is a misrouted or corrupt reply, never words to insert.
        if std::str::from_utf8(body).is_err() {
            return Err(TranscribeError::new(format!(
                "Speech-to-text: {host} sent a reply without a transcript"
            )));
        }
        return transcript_from_success(&text, host);
    }
    let detail = error_detail(&text);
    let with_detail = |message: String| match &detail {
        Some(detail) => format!("{message} ({detail})"),
        None => message,
    };
    match status {
        401 | 403 => Err(TranscribeError::fatal(with_detail(format!(
            "Speech-to-text: {host} rejected the API key (HTTP {status}); check [voice] api_key or run /setup"
        )))),
        404 | 405 | 501 => Err(TranscribeError::fatal(format!(
            "Speech-to-text: {host} has no {TRANSCRIPTIONS_PATH} endpoint (HTTP {status}); set [voice] base_url to a provider that offers transcription"
        ))),
        400 | 422 if mentions_model(&text) => Err(TranscribeError::fatal(with_detail(format!(
            "Speech-to-text: {host} does not offer the model {model:?}; set [voice] model to one it supports"
        )))),
        413 => Err(TranscribeError::new(format!(
            "Speech-to-text: {host} refused the recording as too large; try a shorter one"
        ))),
        429 => Err(TranscribeError::new(format!(
            "Speech-to-text: {host} is rate limiting requests; try again shortly"
        ))),
        500..=599 => Err(TranscribeError::new(with_detail(format!(
            "Speech-to-text: {host} failed (HTTP {status}); try again"
        )))),
        _ => Err(TranscribeError::new(with_detail(format!(
            "Speech-to-text: {host} refused the recording (HTTP {status})"
        )))),
    }
}

fn transcript_from_success(text: &str, host: &str) -> Result<String, TranscribeError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(map)) => match map.get("text") {
            Some(serde_json::Value::String(transcript)) => Ok(transcript.trim().to_owned()),
            Some(serde_json::Value::Null) | None if map.contains_key("error") => {
                Err(TranscribeError::new(format!(
                    "Speech-to-text: {host} returned an error{}",
                    error_detail(trimmed)
                        .map(|detail| format!(" ({detail})"))
                        .unwrap_or_default()
                )))
            }
            _ => Err(unexpected_reply(host)),
        },
        // A bare JSON string is a transcript too.
        Ok(serde_json::Value::String(transcript)) => Ok(transcript.trim().to_owned()),
        Ok(_) => Err(unexpected_reply(host)),
        // `response_format=text` style replies: plain words, never markup.
        Err(_) if !trimmed.starts_with('<') && !trimmed.starts_with('{') => Ok(trimmed.to_owned()),
        Err(_) => Err(unexpected_reply(host)),
    }
}

fn unexpected_reply(host: &str) -> TranscribeError {
    TranscribeError::new(format!(
        "Speech-to-text: {host} sent a reply without a transcript; check [voice] base_url points at a standard API"
    ))
}

fn mentions_model(body: &str) -> bool {
    body.to_ascii_lowercase().contains("model")
}

/// Short, single-line provider error text: `error.message`, `error` (string), `message`, or `detail`.
fn error_detail(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let raw = value
        .pointer("/error/message")
        .or_else(|| value.get("error").filter(|e| e.is_string()))
        .or_else(|| value.get("message"))
        .or_else(|| value.get("detail"))
        .and_then(serde_json::Value::as_str)?;
    let single_line: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let single_line = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.is_empty() {
        return None;
    }
    let mut detail: String = single_line.chars().take(MAX_DETAIL_CHARS).collect();
    if single_line.chars().count() > MAX_DETAIL_CHARS {
        detail.push('…');
    }
    Some(detail)
}

#[cfg(test)]
#[path = "http_transcriber_tests.rs"]
mod tests;
