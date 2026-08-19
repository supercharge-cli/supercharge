//! Connection configuration and environment-variable resolution.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::logs::Logger;
use crate::utils::build_user_agent;

/// Default per-request timeout in milliseconds (60s).
pub const REQUEST_TIMEOUT_MS: u64 = 60_000;
/// Default sandbox lifetime in milliseconds (5 minutes).
pub const DEFAULT_SANDBOX_TIMEOUT_MS: u64 = 300_000;
/// Keepalive ping interval for streaming RPCs, in seconds.
pub const KEEPALIVE_PING_INTERVAL_SEC: u64 = 50;
/// Header carrying the keepalive ping interval.
pub const KEEPALIVE_PING_HEADER: &str = "Keepalive-Ping-Interval";
/// Default sandbox user.
pub const DEFAULT_USERNAME: &str = "user";
/// Port the envd daemon listens on inside a sandbox.
pub const ENVD_PORT: u16 = 49983;

/// Domains for which the stable `sandbox.<domain>` host is guaranteed.
const SUPPORTED_DOMAINS: [&str; 4] = ["e2b.app", "e2b.dev", "e2b.pro", "e2b-staging.dev"];

/// Options for constructing a [`ConnectionConfig`]. Unset (`None`/empty) fields
/// fall back to environment variables and then documented defaults.
#[derive(Default, Clone)]
pub struct ConnectionConfigOpts {
    /// API key; falls back to `E2B_API_KEY`.
    pub api_key: Option<String>,
    /// Whether to validate the API key format; falls back to `E2B_VALIDATE_API_KEY` (default `true`).
    pub validate_api_key: Option<bool>,
    /// Deprecated access token; falls back to `E2B_ACCESS_TOKEN`.
    pub access_token: Option<String>,
    /// Domain; falls back to `E2B_DOMAIN` (default `e2b.app`).
    pub domain: Option<String>,
    /// API base URL; falls back to `E2B_API_URL`.
    pub api_url: Option<String>,
    /// Sandbox base URL override; falls back to `E2B_SANDBOX_URL`.
    pub sandbox_url: Option<String>,
    /// Debug mode; falls back to `E2B_DEBUG` (default `false`).
    pub debug: Option<bool>,
    /// Per-request timeout in milliseconds (default [`REQUEST_TIMEOUT_MS`]).
    pub request_timeout_ms: Option<u64>,
    /// Optional logger.
    pub logger: Option<Arc<dyn Logger>>,
    /// Extra request headers.
    pub headers: BTreeMap<String, String>,
    /// Optional proxy URL.
    pub proxy: Option<String>,
    /// Integration name appended to the `User-Agent`.
    pub integration: Option<String>,
}

/// Resolved connection configuration.
#[derive(Clone)]
pub struct ConnectionConfig {
    /// Debug mode.
    pub debug: bool,
    /// Resolved domain.
    pub domain: String,
    /// Resolved API base URL.
    pub api_url: String,
    /// Optional sandbox base URL override.
    pub sandbox_url: Option<String>,
    /// Optional logger.
    pub logger: Option<Arc<dyn Logger>>,
    /// Per-request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Resolved API key.
    pub api_key: Option<String>,
    /// Whether to validate the API key format.
    pub validate_api_key: bool,
    /// Deprecated access token.
    pub access_token: Option<String>,
    /// Integration name.
    pub integration: Option<String>,
    /// Request headers, including the computed `User-Agent`.
    pub headers: BTreeMap<String, String>,
    /// Optional proxy URL.
    pub proxy: Option<String>,
    /// Max concurrent in-flight control-plane requests (`0` = unlimited).
    pub api_inflight_requests: usize,
}

/// Return `primary` if it is a non-empty string, else the first non-empty
/// `fallback`. Mirrors JS truthiness for `a || b` where `""` is falsy.
fn first_non_empty(
    primary: Option<String>,
    fallback: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match primary {
        Some(s) if !s.is_empty() => Some(s),
        _ => fallback().filter(|s| !s.is_empty()),
    }
}

impl ConnectionConfig {
    /// Build a configuration from `opts`, reading any unset values from the
    /// process environment.
    pub fn new(opts: ConnectionConfigOpts) -> Self {
        Self::from_env(opts, |key| std::env::var(key).ok())
    }

    /// Build a configuration using an injected environment lookup. Used by
    /// `new` (with `std::env`) and by tests (with a fixed map).
    pub(crate) fn from_env(
        opts: ConnectionConfigOpts,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let api_key = first_non_empty(opts.api_key, || env("E2B_API_KEY"));
        let access_token = first_non_empty(opts.access_token, || env("E2B_ACCESS_TOKEN"));
        let domain = first_non_empty(opts.domain, || env("E2B_DOMAIN"))
            .unwrap_or_else(|| "e2b.app".to_string());

        let debug = opts.debug.unwrap_or_else(|| {
            env("E2B_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        });

        let validate_api_key = opts.validate_api_key.unwrap_or_else(|| {
            env("E2B_VALIDATE_API_KEY")
                .map(|v| !v.eq_ignore_ascii_case("false"))
                .unwrap_or(true)
        });

        let request_timeout_ms = opts.request_timeout_ms.unwrap_or(REQUEST_TIMEOUT_MS);

        let api_url = first_non_empty(opts.api_url, || env("E2B_API_URL")).unwrap_or_else(|| {
            if debug {
                "http://localhost:3000".to_string()
            } else {
                format!("https://api.{domain}")
            }
        });

        let sandbox_url = first_non_empty(opts.sandbox_url, || env("E2B_SANDBOX_URL"));

        // Inflight cap: allows 0 (disable); non-integer falls back to default and logs a warning.
        let api_inflight_requests = match env("E2B_API_INFLIGHT_REQUESTS") {
            Some(v) => match v.parse::<usize>() {
                Ok(n) => n,
                Err(_) => {
                    if let Some(logger) = &opts.logger {
                        logger.warn(&format!(
                            "invalid E2B_API_INFLIGHT_REQUESTS={v:?}; falling back to default 1000"
                        ));
                    }
                    1000
                }
            },
            None => 1000,
        };

        let mut headers = opts.headers;
        headers.insert(
            "User-Agent".to_string(),
            build_user_agent(opts.integration.as_deref()),
        );

        Self {
            debug,
            domain,
            api_url,
            sandbox_url,
            logger: opts.logger,
            request_timeout_ms,
            api_key,
            validate_api_key,
            access_token,
            integration: opts.integration,
            headers,
            proxy: opts.proxy,
            api_inflight_requests,
        }
    }

    /// External host for a sandbox port, e.g. `49983-<id>.e2b.app`. In debug
    /// mode returns `localhost:<port>`.
    pub fn get_host(&self, sandbox_id: &str, port: u16, sandbox_domain: Option<&str>) -> String {
        if self.debug {
            return format!("localhost:{port}");
        }
        let domain = sandbox_domain.unwrap_or(&self.domain);
        format!("{port}-{sandbox_id}.{domain}")
    }

    /// Base URL for reaching a sandbox: the override if set, the stable
    /// `sandbox.<domain>` host for supported domains, otherwise the direct host.
    pub fn get_sandbox_url(
        &self,
        sandbox_id: &str,
        sandbox_domain: &str,
        envd_port: u16,
    ) -> String {
        if let Some(url) = &self.sandbox_url {
            return url.clone();
        }
        if self.debug {
            return format!(
                "http://{}",
                self.get_host(sandbox_id, envd_port, Some(sandbox_domain))
            );
        }
        if SUPPORTED_DOMAINS.contains(&sandbox_domain) {
            return format!("https://sandbox.{sandbox_domain}");
        }
        format!(
            "https://{}",
            self.get_host(sandbox_id, envd_port, Some(sandbox_domain))
        )
    }

    /// Direct sandbox host URL, never using the stable-domain fallback.
    pub fn get_sandbox_direct_url(
        &self,
        sandbox_id: &str,
        sandbox_domain: &str,
        envd_port: u16,
    ) -> String {
        if let Some(url) = &self.sandbox_url {
            return url.clone();
        }
        let scheme = if self.debug { "http" } else { "https" };
        format!(
            "{scheme}://{}",
            self.get_host(sandbox_id, envd_port, Some(sandbox_domain))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(opts: ConnectionConfigOpts, env: &[(&str, &str)]) -> ConnectionConfig {
        let map: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ConnectionConfig::from_env(opts, move |k| map.get(k).cloned())
    }

    #[test]
    fn defaults_with_empty_env() {
        let c = cfg(ConnectionConfigOpts::default(), &[]);
        assert_eq!(c.domain, "e2b.app");
        assert_eq!(c.api_url, "https://api.e2b.app");
        assert!(!c.debug);
        assert!(c.validate_api_key);
        assert_eq!(c.request_timeout_ms, REQUEST_TIMEOUT_MS);
        assert_eq!(c.api_key, None);
        assert_eq!(
            c.headers.get("User-Agent").map(String::as_str),
            Some("e2b-rs/0.1.0")
        );
    }

    #[test]
    fn env_domain_flows_into_api_url() {
        let c = cfg(
            ConnectionConfigOpts::default(),
            &[("E2B_DOMAIN", "example.com")],
        );
        assert_eq!(c.domain, "example.com");
        assert_eq!(c.api_url, "https://api.example.com");
    }

    #[test]
    fn opt_overrides_env() {
        let opts = ConnectionConfigOpts {
            domain: Some("opt.dev".to_string()),
            ..Default::default()
        };
        let c = cfg(opts, &[("E2B_DOMAIN", "env.dev")]);
        assert_eq!(c.domain, "opt.dev");
    }

    #[test]
    fn empty_string_opt_is_falsy_and_falls_through() {
        // JS uses `||` for domain: an empty-string opt is falsy, so env wins.
        let opts = ConnectionConfigOpts {
            domain: Some(String::new()),
            ..Default::default()
        };
        let c = cfg(opts, &[("E2B_DOMAIN", "env.dev")]);
        assert_eq!(c.domain, "env.dev");
    }

    #[test]
    fn debug_changes_api_url_and_parses_env() {
        let c = cfg(ConnectionConfigOpts::default(), &[("E2B_DEBUG", "true")]);
        assert!(c.debug);
        assert_eq!(c.api_url, "http://localhost:3000");
    }

    #[test]
    fn validate_api_key_env_false_disables() {
        let c = cfg(
            ConnectionConfigOpts::default(),
            &[("E2B_VALIDATE_API_KEY", "false")],
        );
        assert!(!c.validate_api_key);
    }

    #[test]
    fn get_host_production_and_debug() {
        let prod = cfg(ConnectionConfigOpts::default(), &[]);
        assert_eq!(
            prod.get_host("sb1", 49983, Some("e2b.app")),
            "49983-sb1.e2b.app"
        );

        let dbg = cfg(ConnectionConfigOpts::default(), &[("E2B_DEBUG", "true")]);
        assert_eq!(
            dbg.get_host("sb1", 49983, Some("e2b.app")),
            "localhost:49983"
        );
    }

    #[test]
    fn sandbox_url_stable_vs_direct() {
        let c = cfg(ConnectionConfigOpts::default(), &[]);
        // Supported domain → stable host.
        assert_eq!(
            c.get_sandbox_url("sb1", "e2b.app", 49983),
            "https://sandbox.e2b.app"
        );
        // Unsupported domain → direct host.
        assert_eq!(
            c.get_sandbox_url("sb1", "custom.io", 49983),
            "https://49983-sb1.custom.io"
        );
        // Direct URL never uses the stable host, even for supported domains.
        assert_eq!(
            c.get_sandbox_direct_url("sb1", "e2b.app", 49983),
            "https://49983-sb1.e2b.app"
        );
    }

    #[test]
    fn sandbox_url_override_and_debug() {
        let opts = ConnectionConfigOpts {
            sandbox_url: Some("https://my.proxy".to_string()),
            ..Default::default()
        };
        let c = cfg(opts, &[]);
        assert_eq!(
            c.get_sandbox_url("sb1", "e2b.app", 49983),
            "https://my.proxy"
        );

        let dbg = cfg(ConnectionConfigOpts::default(), &[("E2B_DEBUG", "true")]);
        assert_eq!(
            dbg.get_sandbox_url("sb1", "e2b.app", 49983),
            "http://localhost:49983"
        );
    }

    #[test]
    fn resolves_api_inflight_limit() {
        let c = cfg(ConnectionConfigOpts::default(), &[]);
        assert_eq!(c.api_inflight_requests, 1000); // default

        let c2 = cfg(
            ConnectionConfigOpts::default(),
            &[("E2B_API_INFLIGHT_REQUESTS", "50")],
        );
        assert_eq!(c2.api_inflight_requests, 50);

        // 0 is allowed (disables the cap), per parseInflightLimitEnv.
        let c3 = cfg(
            ConnectionConfigOpts::default(),
            &[("E2B_API_INFLIGHT_REQUESTS", "0")],
        );
        assert_eq!(c3.api_inflight_requests, 0);

        // non-integer falls back to default + logs a warning (ConnectionConfig::new is
        // infallible, so unlike JS it cannot throw).
        let c4 = cfg(
            ConnectionConfigOpts::default(),
            &[("E2B_API_INFLIGHT_REQUESTS", "nope")],
        );
        assert_eq!(c4.api_inflight_requests, 1000);
    }
}
