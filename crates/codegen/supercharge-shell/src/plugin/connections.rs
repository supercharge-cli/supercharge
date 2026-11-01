//! Target-local account settings. Only redacted metadata crosses ACP.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use supercharge_config_types::{McpJsonOAuthBlock, McpServerConfig, McpServerTransportConfig};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub plugin: String,
    pub server: String,
    pub origin: String,
    pub account: String,
    pub client_id: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub secret_kind: Option<String>,
    pub secret_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) secret: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Store {
    accounts: BTreeMap<String, Account>,
    active: BTreeMap<String, String>,
}
fn path() -> std::path::PathBuf {
    supercharge_config::supercharge_home().join("plugin-connections.json")
}
fn load() -> anyhow::Result<Store> {
    load_at(&path())
}
fn load_at(path: &std::path::Path) -> anyhow::Result<Store> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn namespaced(plugin: &str, name: &str) -> String {
    let id = key(plugin, name, "namespace", "");
    format!("plugin-{plugin}-{}", &id[..16])
}
fn key(plugin: &str, server: &str, origin: &str, account: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(plugin, server, origin, account)).unwrap_or_default())
    )
}
fn mutate<T>(f: impl FnOnce(&mut Store) -> anyhow::Result<T>) -> anyhow::Result<T> {
    mutate_at(&path(), f)
}
fn mutate_at<T>(
    p: &std::path::Path,
    f: impl FnOnce(&mut Store) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let parent = p
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No credential directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(p.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut store = load_at(p)?;
    let value = f(&mut store)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec(&store)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(p)?;
    Ok(value)
}
fn secret(id: &str, account: &Account) -> anyhow::Result<Option<String>> {
    if account.secret_backend.as_deref() == Some("keychain") {
        return Ok(Some(
            keyring::Entry::new("in.superchargeai.plugins", id)?.get_password()?,
        ));
    }
    Ok(account.secret.clone())
}
/// Never serializes token values or OAuth application secrets.
pub fn list() -> anyhow::Result<serde_json::Value> {
    redacted(load()?)
}
fn redacted(store: Store) -> anyhow::Result<serde_json::Value> {
    Ok(
        serde_json::json!({"accounts": store.accounts.iter().map(|(id, a)| serde_json::json!({
        "id": id, "plugin": a.plugin, "server": a.server, "origin": a.origin,
        "account": a.account, "clientId": a.client_id, "secretKind": a.secret_kind,
        "secretBackend": a.secret_backend, "active": store.active.get(&key(&a.plugin, &a.server, &a.origin, "")).is_some_and(|v| v == id)
    })).collect::<Vec<_>>()}),
    )
}
/// Origin is a normalized full resource URL, not a client-supplied credential destination.
pub fn configure(mut account: Account, value: Option<String>) -> anyhow::Result<()> {
    anyhow::ensure!(
        !account.account.is_empty()
            && account.account.len() <= 64
            && account
                .account
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
        "Invalid account name"
    );
    anyhow::ensure!(
        matches!(
            account.secret_kind.as_deref(),
            None | Some("oauth") | Some("bearer") | Some("client_secret")
        ),
        "Unsupported credential type"
    );
    anyhow::ensure!(
        !matches!(
            account.secret_kind.as_deref(),
            Some("oauth") | Some("client_secret")
        ) || account
            .client_id
            .as_ref()
            .is_some_and(|id| !id.trim().is_empty()),
        "OAuth client ID is required"
    );
    let id = key(
        &account.plugin,
        &account.server,
        &account.origin,
        &account.account,
    );
    let auth_identity = account.clone();
    let auth_id = id.clone();
    mutate(|store| {
        if value.is_none() {
            if let Some(old) = store.accounts.get(&id).filter(|old| {
                old.secret_kind != account.secret_kind
                    && old.secret_backend.as_deref() == Some("keychain")
            }) {
                let _ = old;
                match keyring::Entry::new("in.superchargeai.plugins", &id)?.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        if let Some(value) = value {
            anyhow::ensure!(
                !value.is_empty() && !value.contains(['\r', '\n']),
                "Invalid secret value"
            );
            let entry = keyring::Entry::new("in.superchargeai.plugins", &id);
            if entry.and_then(|e| e.set_password(&value)).is_ok() {
                account.secret_backend = Some("keychain".into());
                account.secret = None;
            } else {
                account.secret_backend = Some("file".into());
                account.secret = Some(value);
            }
        } else if let Some(old) = store
            .accounts
            .get(&id)
            .filter(|old| old.secret_kind == account.secret_kind)
        {
            account.secret_backend = old.secret_backend.clone();
            account.secret = old.secret.clone();
        }
        anyhow::ensure!(
            account.secret_kind.as_deref() != Some("bearer") || account.secret_backend.is_some(),
            "An access token is required"
        );
        anyhow::ensure!(
            !matches!(
                account.secret_kind.as_deref(),
                Some("oauth") | Some("client_secret")
            ) || account
                .client_id
                .as_ref()
                .is_some_and(|id| !id.trim().is_empty()),
            "OAuth client ID is required"
        );
        store.active.insert(
            key(&account.plugin, &account.server, &account.origin, ""),
            id.clone(),
        );
        store.accounts.insert(id, account);
        Ok(())
    })?;
    forget_oauth(&auth_identity, &auth_id)?;
    Ok(())
}
fn forget_oauth(account: &Account, id: &str) -> anyhow::Result<()> {
    let resource = url::Url::parse(&account.origin)?;
    let base = format!(
        "{}--{}",
        account.server,
        id.get(..16)
            .ok_or_else(|| anyhow::anyhow!("Invalid account identity"))?
    );
    let mut store = supercharge_mcp::credentials::McpCredentialStore::load_default()?;
    store.remove_and_save(&base, &resource)?;
    store.remove_and_save(&namespaced(&account.plugin, &base), &resource)?;
    Ok(())
}
pub fn select(id: &str) -> anyhow::Result<()> {
    mutate(|store| {
        let a = store
            .accounts
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Account not found"))?;
        store
            .active
            .insert(key(&a.plugin, &a.server, &a.origin, ""), id.to_owned());
        Ok(())
    })
}
pub fn disconnect(id: &str) -> anyhow::Result<()> {
    mutate(|store| {
        let a = store
            .accounts
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Account not found"))?;
        if a.secret_backend.as_deref() == Some("keychain") {
            match keyring::Entry::new("in.superchargeai.plugins", id)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(e.into()),
            }
        }
        forget_oauth(a, id)?;
        store.accounts.remove(id);
        store.active.retain(|_, value| value != id);
        Ok(())
    })
}
/// Applied only to plugin servers, after package expansion, before constructing private ACP transports.
pub(crate) fn apply(
    plugin: &str,
    name: &str,
    config: &mut McpServerConfig,
) -> anyhow::Result<(String, Option<String>)> {
    let origin = match &config.transport {
        McpServerTransportConfig::StreamableHttp { url, .. } => url::Url::parse(url)?.to_string(),
        _ => return Ok((name.to_owned(), None)),
    };
    let store = load()?;
    let Some(id) = store.active.get(&key(plugin, name, &origin, "")) else {
        return Ok((name.to_owned(), None));
    };
    let a = store
        .accounts
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("Active account missing"))?;
    let value = secret(id, a)?;
    apply_account(name, config, a, value, id)
}
fn apply_account(
    name: &str,
    config: &mut McpServerConfig,
    a: &Account,
    value: Option<String>,
    id: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let McpServerTransportConfig::StreamableHttp {
        headers,
        bearer_token_env_var,
        bearer_token_file,
        oauth_client_id,
        oauth_client_secret_env_var,
        oauth_scopes,
        ..
    } = &mut config.transport
    else {
        anyhow::bail!("Account requires an HTTP server");
    };
    // An explicitly selected account must never inherit another credential from package/env defaults.
    *bearer_token_env_var = None;
    *bearer_token_file = None;
    *oauth_client_id = None;
    *oauth_client_secret_env_var = None;
    *oauth_scopes = None;
    if let Some(headers) = headers.as_mut() {
        headers.retain(|name, _| !name.eq_ignore_ascii_case("authorization"));
    }
    if let Some(value) = value
        .as_ref()
        .filter(|_| a.secret_kind.as_deref() == Some("bearer"))
    {
        headers
            .get_or_insert_with(Default::default)
            .insert("Authorization".into(), format!("Bearer {value}"));
        config.oauth = None;
    } else if let Some(client_id) = &a.client_id {
        config.oauth = Some(McpJsonOAuthBlock {
            client_id: Some(client_id.clone()),
            scopes: a.scopes.clone(),
            ..Default::default()
        });
    }
    // Isolate OAuth token persistence and MCP connection pools by selected account and endpoint.
    let client_secret = value.filter(|_| a.secret_kind.as_deref() == Some("client_secret"));
    Ok((format!("{name}--{}", &id[..16]), client_secret))
}

#[cfg(test)]
mod tests {
    #[test]
    fn selected_accounts_replace_inherited_auth_without_serializing_client_secret() {
        let original = serde_json::json!({"url":"https://example/mcp","headers":{"authorization":"old-token","X-Test":"preserved"},"bearer_token_env_var":"OLD_ACCOUNT_TOKEN","bearer_token_file":"/old/token","oauth_client_id":"old-client"});
        let id = "0123456789abcdef0123456789abcdef";
        let mut config = serde_json::from_value(original.clone()).unwrap();
        let account = super::Account {
            secret_kind: Some("bearer".into()),
            ..Default::default()
        };
        super::apply_account(
            "server",
            &mut config,
            &account,
            Some("new-token".into()),
            id,
        )
        .unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["headers"]["Authorization"], "Bearer new-token");
        assert_eq!(value["headers"]["X-Test"], "preserved");
        assert!(value.get("bearer_token_env_var").is_none());
        assert!(value.get("bearer_token_file").is_none());
        assert!(config.oauth_config().is_none());
        let mut config = serde_json::from_value(original).unwrap();
        let account = super::Account {
            secret_kind: Some("client_secret".into()),
            client_id: Some("new-client".into()),
            ..Default::default()
        };
        let (_, secret) = super::apply_account(
            "server",
            &mut config,
            &account,
            Some("private-app-secret".into()),
            id,
        )
        .unwrap();
        assert_eq!(secret.as_deref(), Some("private-app-secret"));
        assert_eq!(
            config.oauth_config().unwrap().client_id.as_deref(),
            Some("new-client")
        );
        let value = serde_json::to_string(&config).unwrap();
        assert!(!value.contains("old-token"));
        assert!(!value.contains("private-app-secret"));
    }
    #[test]
    fn atomic_store_is_private_redacted_and_preserves_concurrent_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    super::mutate_at(&path, |store| {
                        store.accounts.insert(
                            i.to_string(),
                            super::Account {
                                account: i.to_string(),
                                secret: Some("must-never-be-returned".into()),
                                secret_backend: Some("file".into()),
                                ..Default::default()
                            },
                        );
                        Ok(())
                    })
                    .unwrap()
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let store = super::load_at(&path).unwrap();
        assert_eq!(store.accounts.len(), 8);
        let view = super::redacted(store).unwrap().to_string();
        assert!(!view.contains("must-never-be-returned"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&path, b"broken").unwrap();
        assert!(super::mutate_at(&path, |_| Ok(())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    }
    #[test]
    fn identity_includes_plugin_endpoint_and_account() {
        let base = super::key("a", "s", "https://example/mcp", "work");
        for (p, s, u, a) in [
            ("b", "s", "https://example/mcp", "work"),
            ("a", "t", "https://example/mcp", "work"),
            ("a", "s", "https://other/mcp", "work"),
            ("a", "s", "https://example/mcp", "personal"),
        ] {
            assert_ne!(base, super::key(p, s, u, a));
        }
    }
}
