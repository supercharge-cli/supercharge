use indexmap::IndexMap;

/// Configuration for the web search tool. Use `Disabled` when no API key is available or web search should be turned off. The `Enabled` variant
/// is inherently large (credentials, headers, domain filters) while `Disabled` is empty, but this config is built once per session and never
/// stored in bulk collections, so boxing would add indirection for no real benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WebSearchConfig {
    #[default]
    Disabled,
    Enabled {
        api_key: String,
        base_url: String,
        model: String,
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        extra_headers: IndexMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alpha_test_key: Option<String>,
        /// Authoritative domain allowlist from `[toolset.web_search] allowed_domains`. When set, it governs the client-side
        /// web_search tool and the model's own per-call `allowed_domains` is ignored (see `resolve_filters`), so a configured
        /// policy cannot be bypassed. Mutually exclusive with `excluded_domains`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allowed_domains: Option<Vec<String>>,
        /// Authoritative domain blocklist from `[toolset.web_search] excluded_domains`. Like `allowed_domains` it governs
        /// outright: the model cannot un-block a domain by naming it in its own per-call `allowed_domains`, which is what makes
        /// this a real block. Mutually exclusive with `allowed_domains`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        excluded_domains: Option<Vec<String>>,
        /// Exa key from env or `[toolset.web_search]`. Absent means Exa is not used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exa_api_key: Option<String>,
        /// Linkup key from env or `[toolset.web_search]`. Used when Exa is unset or fails.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linkup_api_key: Option<String>,
    },
    /// Responses API web search is off, but Exa and/or Linkup is configured.
    Providers {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exa_api_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linkup_api_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allowed_domains: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        excluded_domains: Option<Vec<String>>,
    },
}

impl std::fmt::Debug for WebSearchConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.redacted() {
            Self::Disabled => f.write_str("Disabled"),
            Self::Enabled {
                base_url,
                model,
                allowed_domains,
                excluded_domains,
                exa_api_key,
                linkup_api_key,
                ..
            } => f
                .debug_struct("Enabled")
                .field("base_url", &base_url)
                .field("model", &model)
                .field("allowed_domains", &allowed_domains)
                .field("excluded_domains", &excluded_domains)
                .field("has_exa_api_key", &exa_api_key.is_some())
                .field("has_linkup_api_key", &linkup_api_key.is_some())
                .finish(),
            Self::Providers {
                allowed_domains,
                excluded_domains,
                exa_api_key,
                linkup_api_key,
            } => f
                .debug_struct("Providers")
                .field("allowed_domains", &allowed_domains)
                .field("excluded_domains", &excluded_domains)
                .field("has_exa_api_key", &exa_api_key.is_some())
                .field("has_linkup_api_key", &linkup_api_key.is_some())
                .finish(),
        }
    }
}

fn nonempty(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

impl WebSearchConfig {
    /// Returns `true` when the Responses API web-search path is configured.
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    /// Responses API path, or an Exa/Linkup key, so `web_search` can be registered.
    pub fn is_available(&self) -> bool {
        match self {
            Self::Disabled => false,
            Self::Enabled { .. } => true,
            Self::Providers {
                exa_api_key,
                linkup_api_key,
                ..
            } => nonempty(exa_api_key).is_some() || nonempty(linkup_api_key).is_some(),
        }
    }

    pub fn exa_api_key(&self) -> Option<String> {
        match self {
            Self::Enabled { exa_api_key, .. } | Self::Providers { exa_api_key, .. } => {
                nonempty(exa_api_key)
            }
            Self::Disabled => None,
        }
    }

    pub fn linkup_api_key(&self) -> Option<String> {
        match self {
            Self::Enabled { linkup_api_key, .. } | Self::Providers { linkup_api_key, .. } => {
                nonempty(linkup_api_key)
            }
            Self::Disabled => None,
        }
    }

    pub fn domain_defaults(&self) -> (Option<Vec<String>>, Option<Vec<String>>) {
        match self {
            Self::Disabled => (None, None),
            Self::Enabled {
                allowed_domains,
                excluded_domains,
                ..
            }
            | Self::Providers {
                allowed_domains,
                excluded_domains,
                ..
            } => (allowed_domains.clone(), excluded_domains.clone()),
        }
    }

    /// Attach Exa/Linkup keys. `Disabled` with at least one key becomes `Providers`.
    /// An already-`Enabled` Responses config keeps that path and stores the keys beside it.
    /// Empty strings are treated as unset. `disable` leaves the config unchanged so a
    /// kill-switch cannot be overridden by ambient keys.
    pub fn with_search_providers(
        self,
        disable: bool,
        exa_api_key: Option<String>,
        linkup_api_key: Option<String>,
        allowed_domains: Option<Vec<String>>,
        excluded_domains: Option<Vec<String>>,
    ) -> Self {
        if disable {
            return Self::Disabled;
        }
        let exa_api_key = nonempty(&exa_api_key);
        let linkup_api_key = nonempty(&linkup_api_key);
        match self {
            Self::Disabled => {
                if exa_api_key.is_none() && linkup_api_key.is_none() {
                    Self::Disabled
                } else {
                    Self::Providers {
                        exa_api_key,
                        linkup_api_key,
                        allowed_domains,
                        excluded_domains,
                    }
                }
            }
            Self::Enabled {
                api_key,
                base_url,
                model,
                extra_headers,
                alpha_test_key,
                allowed_domains: existing_allowed,
                excluded_domains: existing_excluded,
                ..
            } => Self::Enabled {
                api_key,
                base_url,
                model,
                extra_headers,
                alpha_test_key,
                allowed_domains: existing_allowed.or(allowed_domains),
                excluded_domains: existing_excluded.or(excluded_domains),
                exa_api_key,
                linkup_api_key,
            },
            Self::Providers {
                allowed_domains: existing_allowed,
                excluded_domains: existing_excluded,
                ..
            } => Self::Providers {
                exa_api_key,
                linkup_api_key,
                allowed_domains: existing_allowed.or(allowed_domains),
                excluded_domains: existing_excluded.or(excluded_domains),
            },
        }
    }

    /// Preferred backend name for status output. Never includes key material.
    pub fn backend_name(&self) -> &'static str {
        if !self.is_available() {
            return "none";
        }
        if self.exa_api_key().is_some() {
            "exa"
        } else if self.linkup_api_key().is_some() {
            "linkup"
        } else {
            "default"
        }
    }

    /// Return a copy safe for returning to clients. The `api_key` is replaced with
    /// `"***REDACTED***"` and the optional extra access key field is stripped.
    pub fn redacted(&self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Enabled {
                base_url,
                model,
                extra_headers,
                allowed_domains,
                excluded_domains,
                exa_api_key,
                linkup_api_key,
                ..
            } => Self::Enabled {
                api_key: "***REDACTED***".to_string(),
                base_url: base_url.clone(),
                model: model.clone(),
                extra_headers: extra_headers.clone(),
                alpha_test_key: None,
                allowed_domains: allowed_domains.clone(),
                excluded_domains: excluded_domains.clone(),
                exa_api_key: exa_api_key.as_ref().map(|_| "***REDACTED***".to_string()),
                linkup_api_key: linkup_api_key
                    .as_ref()
                    .map(|_| "***REDACTED***".to_string()),
            },
            Self::Providers {
                exa_api_key,
                linkup_api_key,
                allowed_domains,
                excluded_domains,
            } => Self::Providers {
                exa_api_key: exa_api_key.as_ref().map(|_| "***REDACTED***".to_string()),
                linkup_api_key: linkup_api_key
                    .as_ref()
                    .map(|_| "***REDACTED***".to_string()),
                allowed_domains: allowed_domains.clone(),
                excluded_domains: excluded_domains.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_is_disabled() {
        let config = WebSearchConfig::default();
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_config_redacted() {
        let mut headers = IndexMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());
        let config = WebSearchConfig::Enabled {
            api_key: "secret-key-12345".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-web-search-model".to_string(),
            extra_headers: headers,
            alpha_test_key: Some("alpha-secret".to_string()),
            allowed_domains: Some(vec!["docs.x.ai".to_string()]),
            excluded_domains: None,
            exa_api_key: Some("exa-secret".to_string()),
            linkup_api_key: None,
        };
        let redacted = config.redacted();
        assert!(!format!("{redacted:?}").contains("exa-secret"));
        assert!(!format!("{redacted:?}").contains("secret-key-12345"));
        match redacted {
            WebSearchConfig::Enabled {
                api_key,
                base_url,
                model,
                extra_headers,
                alpha_test_key,
                allowed_domains,
                excluded_domains,
                exa_api_key,
                linkup_api_key,
            } => {
                assert_eq!(api_key, "***REDACTED***");
                assert_eq!(base_url, "https://api.x.ai/v1");
                assert_eq!(model, "test-web-search-model");
                assert_eq!(extra_headers.get("X-Custom").unwrap(), "value");
                assert!(alpha_test_key.is_none());
                // Domain filters survive redaction (not secrets).
                assert_eq!(allowed_domains, Some(vec!["docs.x.ai".to_string()]));
                assert!(excluded_domains.is_none());
                assert_eq!(exa_api_key.as_deref(), Some("***REDACTED***"));
                assert!(linkup_api_key.is_none());
            }
            _ => panic!("Expected Enabled variant"),
        }
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = WebSearchConfig::Enabled {
            api_key: "key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-web-search-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            exa_api_key: None,
            linkup_api_key: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: WebSearchConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_enabled());
    }

    #[test]
    fn test_config_deserialize_from_set_options_payload() {
        let json = r#"{
            "status": "enabled",
            "api_key": "xai-abc123",
            "base_url": "https://api.x.ai/v1",
            "model": "test-web-search-model"
        }"#;
        let config: WebSearchConfig = serde_json::from_str(json).unwrap();
        assert!(config.is_enabled());
    }
}
