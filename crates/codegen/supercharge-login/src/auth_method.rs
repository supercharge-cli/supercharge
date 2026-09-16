//! First-party API-key environment primitives.
//!
//! Only the env-key checks the auth subsystem itself needs live here. The ACP
//! `auth_methods` list-building surface (`build_auth_methods`,
//! `AuthMethodsBuildInputs`, `should_advertise_xai_api_key`, ...) stays in
//! `supercharge_shell::agent::auth_method`, which depends on shell's `ModelEntry`.

/// Conventional first-party env var name, retained for compatibility checks.
/// Supercharge does not use ambient first-party keys to bypass `/setup`.
pub const XAI_API_KEY_ENV_VAR: &str = "XAI_API_KEY";

/// Legacy first-party env var name, likewise not consumed implicitly.
pub const LEGACY_XAI_API_KEY_ENV_VAR: &str = "SUPERCHARGE_CODE_XAI_API_KEY";

/// Compatibility entry point: ambient first-party keys are intentionally absent.
/// Explicit saved or process provider credentials are handled by the manager.
pub fn read_xai_api_key_env() -> Result<String, std::env::VarError> {
    // Saved, validated provider credentials from /setup are authoritative.
    // Do not silently activate ambient first-party credentials.
    Err(std::env::VarError::NotPresent)
}

/// Whether ambient first-party authentication is available (always false).
pub fn has_xai_api_key_env() -> bool {
    read_xai_api_key_env().is_ok()
}
