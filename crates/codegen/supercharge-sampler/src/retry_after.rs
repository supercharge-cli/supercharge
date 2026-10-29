//! Server wait hints on rate-limited and overloaded responses.
//!
//! Recognised, in priority order:
//! - `retry-after-ms`: milliseconds (integer or fractional)
//! - `Retry-After`: delta seconds or an HTTP-date
//! - `x-ratelimit-reset*`: a duration such as `1m30s`, `250ms`, `6s`, plain seconds, or a Unix epoch (seconds or milliseconds)
//!
//! Garbage, negative, and non-finite values are ignored. Dates in the past mean "retry now".
//! Very large values are clamped to [`MAX_HEADER_WAIT`]; the retry policy applies its own (smaller) per-wait cap.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Upper bound on any single parsed hint so arithmetic never overflows.
pub const MAX_HEADER_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// Values at or above this are treated as a Unix epoch in seconds rather than a delta (2001-09-09).
const EPOCH_SECS_THRESHOLD: f64 = 1_000_000_000.0;
/// Values at or above this are treated as a Unix epoch in milliseconds.
const EPOCH_MILLIS_THRESHOLD: f64 = 1_000_000_000_000.0;

/// Parse every recognised wait hint from `lookup` (case-insensitive header name → value).
pub fn parse_wait_hint<'a>(
    lookup: impl Fn(&str) -> Option<&'a str>,
    now: SystemTime,
) -> Option<Duration> {
    if let Some(d) = lookup("retry-after-ms").and_then(parse_millis) {
        return Some(d);
    }
    if let Some(d) = lookup("retry-after").and_then(|v| parse_retry_after(v, now)) {
        return Some(d);
    }
    // Several reset headers may be present (requests vs tokens); the longest one is the binding limit.
    [
        "x-ratelimit-reset",
        "x-ratelimit-reset-requests",
        "x-ratelimit-reset-tokens",
        "x-ratelimit-reset-input-tokens",
        "x-ratelimit-reset-output-tokens",
    ]
    .iter()
    .filter_map(|name| lookup(name).and_then(|v| parse_reset(v, now)))
    .max()
}

/// Parse from a `reqwest` header map.
pub fn wait_hint_from_headers(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    parse_wait_hint(
        |name| headers.get(name).and_then(|v| v.to_str().ok()),
        SystemTime::now(),
    )
}

fn clamp(d: Duration) -> Duration {
    d.min(MAX_HEADER_WAIT)
}

fn finite_non_negative(v: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0).then_some(v)
}

fn secs_f64(secs: f64) -> Duration {
    clamp(Duration::from_secs_f64(
        secs.min(MAX_HEADER_WAIT.as_secs_f64()),
    ))
}

fn parse_millis(value: &str) -> Option<Duration> {
    let ms = finite_non_negative(value.trim().parse::<f64>().ok()?)?;
    Some(secs_f64(ms / 1000.0))
}

/// `Retry-After`: delta seconds (fractional tolerated) or an HTTP-date.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(secs) = value.parse::<f64>() {
        return finite_non_negative(secs).map(secs_f64);
    }
    let date = chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .or_else(|| chrono::DateTime::parse_from_rfc3339(value).ok())?;
    let target_ms = date.timestamp_millis();
    Some(until_epoch_millis(target_ms, now))
}

fn until_epoch_millis(target_ms: i64, now: SystemTime) -> Duration {
    let now_ms = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let delta = target_ms.saturating_sub(now_ms);
    if delta <= 0 {
        Duration::ZERO
    } else {
        clamp(Duration::from_millis(delta as u64))
    }
}

/// `x-ratelimit-reset*`: Go-style durations (`1h2m3.5s`, `250ms`, `6s`), bare seconds, or a Unix epoch.
pub fn parse_reset(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(n) = value.parse::<f64>() {
        let n = finite_non_negative(n)?;
        if n >= EPOCH_MILLIS_THRESHOLD {
            return Some(until_epoch_millis(n as i64, now));
        }
        if n >= EPOCH_SECS_THRESHOLD {
            return Some(until_epoch_millis((n * 1000.0) as i64, now));
        }
        return Some(secs_f64(n));
    }
    if let Some(d) = parse_go_duration(value) {
        return Some(d);
    }
    // Some providers send an RFC 3339 timestamp.
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| until_epoch_millis(date.timestamp_millis(), now))
}

/// Parse a duration made of `<number><unit>` segments (`h`, `m`, `s`, `ms`, `us`/`µs`, `ns`).
fn parse_go_duration(value: &str) -> Option<Duration> {
    let mut rest = value;
    let mut total = 0.0f64;
    let mut segments = 0;
    while !rest.is_empty() {
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_len == 0 {
            return None;
        }
        let number: f64 = rest[..num_len].parse().ok()?;
        rest = &rest[num_len..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        let scale = match unit {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 1e-3,
            "us" | "µs" => 1e-6,
            "ns" => 1e-9,
            _ => return None,
        };
        total += number * scale;
        segments += 1;
    }
    if segments == 0 {
        return None;
    }
    finite_non_negative(total).map(secs_f64)
}

/// Round a hint up to whole seconds (the error carries second precision).
pub fn ceil_secs(d: Duration) -> u64 {
    let secs = d.as_secs();
    if d.subsec_nanos() > 0 { secs + 1 } else { secs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn hint(pairs: &[(&str, &str)]) -> Option<Duration> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect();
        parse_wait_hint(|name| map.get(name).map(String::as_str), now())
    }

    #[test]
    fn retry_after_seconds() {
        assert_eq!(
            hint(&[("retry-after", "30")]),
            Some(Duration::from_secs(30))
        );
        assert_eq!(hint(&[("retry-after", " 0 ")]), Some(Duration::ZERO));
        assert_eq!(
            hint(&[("retry-after", "1.5")]),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn retry_after_http_date_future_past_and_garbage() {
        // now() is 2027-01-15T08:00:00Z
        assert_eq!(
            hint(&[("retry-after", "Fri, 15 Jan 2027 08:00:42 GMT")]),
            Some(Duration::from_secs(42))
        );
        assert_eq!(
            hint(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")]),
            Some(Duration::ZERO)
        );
        assert_eq!(hint(&[("retry-after", "Fri, 99 Foo 2027 GMT")]), None);
        assert_eq!(hint(&[("retry-after", "soon")]), None);
        assert_eq!(hint(&[("retry-after", "")]), None);
    }

    #[test]
    fn negative_and_non_finite_ignored() {
        assert_eq!(hint(&[("retry-after", "-5")]), None);
        assert_eq!(hint(&[("retry-after", "NaN")]), None);
        assert_eq!(hint(&[("retry-after", "inf")]), None);
        assert_eq!(hint(&[("retry-after-ms", "-1")]), None);
        assert_eq!(hint(&[("x-ratelimit-reset-requests", "-3s")]), None);
    }

    #[test]
    fn huge_values_clamped() {
        assert_eq!(hint(&[("retry-after", "99999999")]), Some(MAX_HEADER_WAIT));
        assert_eq!(
            hint(&[("retry-after", "1e308")]),
            Some(MAX_HEADER_WAIT),
            "huge float clamps instead of panicking"
        );
        assert_eq!(hint(&[("retry-after-ms", "1e300")]), Some(MAX_HEADER_WAIT));
        assert_eq!(
            hint(&[("x-ratelimit-reset-tokens", "999999h")]),
            Some(MAX_HEADER_WAIT)
        );
    }

    #[test]
    fn retry_after_ms_wins() {
        assert_eq!(
            hint(&[("retry-after-ms", "250"), ("retry-after", "30")]),
            Some(Duration::from_millis(250))
        );
        // garbage ms falls through to Retry-After
        assert_eq!(
            hint(&[("retry-after-ms", "abc"), ("retry-after", "3")]),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn reset_durations() {
        assert_eq!(
            hint(&[("x-ratelimit-reset-requests", "1m30s")]),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset-tokens", "250ms")]),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset-tokens", "6.5s")]),
            Some(Duration::from_millis(6500))
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset-tokens", "1h0m0s")]),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset", "12")]),
            Some(Duration::from_secs(12))
        );
        assert_eq!(hint(&[("x-ratelimit-reset-requests", "1m30x")]), None);
        assert_eq!(hint(&[("x-ratelimit-reset-requests", "m")]), None);
        assert_eq!(hint(&[("x-ratelimit-reset-requests", "..s")]), None);
    }

    #[test]
    fn reset_takes_longest_binding_limit() {
        assert_eq!(
            hint(&[
                ("x-ratelimit-reset-requests", "2s"),
                ("x-ratelimit-reset-tokens", "1m"),
            ]),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn reset_epoch_seconds_and_millis() {
        assert_eq!(
            hint(&[("x-ratelimit-reset", "1800000020")]),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset", "1800000020500")]),
            Some(Duration::from_millis(20_500))
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset", "1700000000")]),
            Some(Duration::ZERO),
            "past epoch means retry now"
        );
        assert_eq!(
            hint(&[("x-ratelimit-reset", "2027-01-15T08:00:05Z")]),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn nothing_present() {
        assert_eq!(hint(&[]), None);
        assert_eq!(hint(&[("x-unrelated", "5")]), None);
    }

    #[test]
    fn ceil_secs_rounds_up() {
        assert_eq!(ceil_secs(Duration::from_millis(250)), 1);
        assert_eq!(ceil_secs(Duration::from_secs(3)), 3);
        assert_eq!(ceil_secs(Duration::ZERO), 0);
    }

    #[test]
    fn real_header_map() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Retry-After-Ms", "1200".parse().unwrap());
        assert_eq!(
            wait_hint_from_headers(&headers),
            Some(Duration::from_millis(1200))
        );
    }
}
