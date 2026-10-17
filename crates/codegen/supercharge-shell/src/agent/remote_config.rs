//! Remote settings and model-catalog fetching, caching, and startup wiring.

/// True for the built-in `cli-chat-proxy.grok.com` origin.
///
/// That host serves Grok account announcements and campaign default models.
/// Supercharge does not connect to it. A provider configured with `/setup`
/// uses that provider's own base URL.
pub(crate) fn is_builtin_grok_proxy(url: &str) -> bool {
    fn norm(s: &str) -> String {
        s.trim().trim_end_matches('/').to_ascii_lowercase()
    }
    norm(url) == norm(crate::env::PROD_CLI_CHAT_PROXY_BASE_URL)
}

#[cfg(test)]
mod builtin_proxy_tests {
    use super::is_builtin_grok_proxy;

    #[test]
    fn builtin_grok_proxy_is_only_the_prod_host() {
        assert!(is_builtin_grok_proxy(
            crate::env::PROD_CLI_CHAT_PROXY_BASE_URL
        ));
        assert!(is_builtin_grok_proxy("https://cli-chat-proxy.grok.com/v1/"));
        assert!(!is_builtin_grok_proxy("https://api.x.ai/v1"));
        assert!(!is_builtin_grok_proxy("https://api.zenllm.org/v1"));
    }
}

mod cache;
mod cache_file;
mod endpoint;
mod fetch;
mod manager;
mod metrics;
mod model_fetch_auth;
mod prefetch;
mod resolution;
mod scope;
mod settings_cache;
pub mod settings_get;
mod settings_refresh;

pub(in crate::agent::remote_config) use cache::ModelsCacheManager;
pub(crate) use endpoint::{HttpModelsEndpoint, ModelsEndpoint};
pub(crate) use fetch::prefetch_models_blocking;
pub(in crate::agent::remote_config) use fetch::{ModelsPrefetch, fetch_models_uncommitted};
pub(crate) use manager::ModelsManager;
pub(crate) use metrics::{DegradedStartCause, record_degraded_start};
pub(in crate::agent::remote_config) use model_fetch_auth::ModelsCacheScope;
pub(crate) use model_fetch_auth::{CacheAuthMethod, ModelFetchAuth, task_model_error_for_catalog};
pub(in crate::agent::remote_config) use prefetch::resolve_startup_endpoints;
pub(crate) use prefetch::{
    ResolvedModels, fetch_initial_models_blocking, start_initial_models_load,
};
pub(crate) use resolution::{
    MODEL_NOT_CONFIGURED_ERROR_CODE, ModelGlobSet, allowlist_denied_message,
    allowlist_excludes_all_message, allowlist_matches_nothing, available_models,
    is_campaign_only_flip, model_not_configured_error, models_list_response, resolve_catalog_key,
    resolve_default_model, resolve_model_catalog, selectable_catalog_key_for_persisted,
    is_studio_model_id, stale_user_allowlist_reason, validate_selectable,
};
pub(in crate::agent::remote_config) use scope::{
    Commit, evaluate_commit, evaluate_models_commit, resolve_disk_auth,
};
pub(crate) use settings_cache::SettingsCacheManager;
pub(in crate::agent::remote_config) use settings_cache::settings_cache_disabled;
pub(in crate::agent) use settings_refresh::SettingsRefresh;

// Re-exports reached only through the manager test module.
#[cfg(test)]
pub(crate) use cache::{CACHE_TTL, MODELS_CACHE_FILE, ModelsCache};
#[cfg(test)]
pub(crate) use endpoint::ModelsFetchFuture;
#[cfg(test)]
pub(crate) use fetch::build_prefetched_map;
#[cfg(test)]
pub(crate) use metrics::degraded_log_level;
#[cfg(test)]
pub(crate) use prefetch::resolve_prefetch_inputs_from_parts;
