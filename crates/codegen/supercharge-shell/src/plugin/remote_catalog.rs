//! HTTPS catalog packages managed by the owner's Supercharge website.
use super::catalog::CatalogEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
const MAX_BYTES: u64 = 3 * 1024 * 1024;
pub const DEFAULT_CATALOG_URL: &str = "https://www.superchargeai.in/api/plugins/v1";
#[derive(Default, Serialize, Deserialize)]
struct Config {
    url: String,
    token: Option<String>,
    #[serde(default)]
    disabled: bool,
}
fn config_path() -> PathBuf {
    supercharge_config::supercharge_home().join("plugin-catalog.json")
}
fn validate_base(raw: &str) -> anyhow::Result<url::Url> {
    let url = url::Url::parse(raw)?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    );
    anyhow::ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && local),
        "Catalog requires HTTPS; HTTP is allowed only on loopback"
    );
    anyhow::ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Catalog URL must not contain credentials, query, or fragment"
    );
    Ok(url)
}
fn config() -> anyhow::Result<Option<Config>> {
    if let Ok(url) = std::env::var("SUPERCHARGE_PLUGIN_CATALOG_URL") {
        validate_base(&url)?;
        return Ok(Some(Config {
            url,
            token: std::env::var("SUPERCHARGE_PLUGIN_CATALOG_TOKEN").ok(),
            disabled: false,
        }));
    }
    match std::fs::read(config_path()) {
        Ok(bytes) => {
            let config: Config = serde_json::from_slice(&bytes)?;
            if config.disabled {
                return Ok(None);
            }
            validate_base(&config.url)?;
            Ok(Some(config))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(Config {
            url: DEFAULT_CATALOG_URL.to_owned(),
            token: None,
            disabled: false,
        })),
        Err(e) => Err(e.into()),
    }
}
pub fn status() -> anyhow::Result<serde_json::Value> {
    Ok(match config()? {
        Some(config) => {
            serde_json::json!({"url":config.url,"hasToken":config.token.as_ref().is_some_and(|t| !t.is_empty()),"environmentOverride":std::env::var_os("SUPERCHARGE_PLUGIN_CATALOG_URL").is_some()})
        }
        None => serde_json::json!({"url":null,"hasToken":false,"environmentOverride":false}),
    })
}
pub fn configure(url: &str, token: Option<String>) -> anyhow::Result<()> {
    validate_base(url)?;
    anyhow::ensure!(
        std::env::var_os("SUPERCHARGE_PLUGIN_CATALOG_URL").is_none(),
        "Catalog is configured by the agent environment; change or remove that override first"
    );
    let token = token.or_else(|| {
        std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Config>(&b).ok())
            .filter(|c| c.url.trim_end_matches('/') == url.trim_end_matches('/'))
            .and_then(|c| c.token)
    });
    anyhow::ensure!(
        token.as_ref().is_none_or(|s| !s.contains(['\r', '\n'])),
        "Invalid catalog token"
    );
    let path = config_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No Supercharge home"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec(&Config {
        url: url.trim_end_matches('/').to_owned(),
        token: token.filter(|t| !t.is_empty()),
        disabled: false,
    })?)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
/// Disable the hosted catalog for this target and forget any saved token.
pub fn clear() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var_os("SUPERCHARGE_PLUGIN_CATALOG_URL").is_none(),
        "Catalog is configured by the agent environment; remove that override first"
    );
    let path = config_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No Supercharge home"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec(&Config {
        url: String::new(),
        token: None,
        disabled: true,
    })?)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

/// A network failure must not hide built-in plugins on an unconfigured installation.
pub(crate) fn is_implicit_default() -> bool {
    std::env::var_os("SUPERCHARGE_PLUGIN_CATALOG_URL").is_none() && !config_path().exists()
}

fn get(config: &Config, suffix: &str) -> anyhow::Result<Vec<u8>> {
    let base = validate_base(&config.url)?;
    let url = format!("{}{suffix}", base.as_str().trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(25))
        .build()?;
    let mut request = client.get(url);
    if let Some(token) = config.token.as_ref().filter(|t| !t.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|_| anyhow::anyhow!("Could not reach plugin catalog"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "Plugin catalog returned HTTP {}; check access or publication status",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    response.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Catalog response exceeds size limit"
    );
    Ok(bytes)
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 100
        && version.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && version.contains('.')
        && !version.contains("..")
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
}

#[derive(Deserialize)]
struct List {
    plugins: Vec<CatalogEntry>,
}
pub fn list() -> anyhow::Result<Vec<CatalogEntry>> {
    let Some(config) = config()? else {
        return Ok(Vec::new());
    };
    let list: List = serde_json::from_slice(&get(&config, "/plugins")?)?;
    anyhow::ensure!(
        list.plugins.len() <= 1000,
        "Catalog contains too many entries"
    );
    let mut entries = list.plugins;
    for entry in &mut entries {
        anyhow::ensure!(
            valid_name(&entry.name) && valid_version(&entry.version),
            "Invalid catalog plugin identity"
        );
        entry.source = format!("catalog:{}@{}", entry.name, entry.version);
    }
    Ok(entries)
}
#[derive(Deserialize)]
struct Release {
    name: String,
    version: String,
    sha256: String,
}
#[derive(Deserialize)]
struct Package {
    format: String,
    name: String,
    version: String,
    files: std::collections::BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
struct Provenance {
    url: String,
    name: String,
    version: String,
    sha256: String,
}
fn source_root(config: &Config, name: &str) -> PathBuf {
    let namespace = format!(
        "{:x}",
        Sha256::digest(config.url.trim_end_matches('/').as_bytes())
    );
    supercharge_config::supercharge_home()
        .join("catalog-sources")
        .join(namespace)
        .join(name)
}
fn validate_package(bytes: &[u8], release: &Release) -> anyhow::Result<Package> {
    anyhow::ensure!(
        release.sha256.len() == 64 && release.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid package digest"
    );
    anyhow::ensure!(
        format!("{:x}", Sha256::digest(bytes)) == release.sha256.to_ascii_lowercase(),
        "Plugin package integrity check failed"
    );
    let package: Package = serde_json::from_slice(bytes)?;
    anyhow::ensure!(
        package.format == "supercharge-plugin-package-v1"
            && package.name == release.name
            && package.version == release.version,
        "Plugin package identity mismatch"
    );
    anyhow::ensure!(
        package.files.len() <= 64 && package.files.contains_key("plugin.json"),
        "Invalid plugin package file set"
    );
    for path in package.files.keys() {
        let parts: Vec<_> = path.split('/').collect();
        anyhow::ensure!(
            path == "plugin.json"
                || path == "mcp.json"
                || (parts.len() == 3
                    && parts.first() == Some(&"skills")
                    && parts.last() == Some(&"SKILL.md")
                    && parts.get(1).is_some_and(|name| valid_name(name))),
            "Package contains an unsupported or escaping path"
        );
    }
    let manifest: supercharge_agent::plugins::manifest::PluginManifest = serde_json::from_str(
        package
            .files
            .get("plugin.json")
            .ok_or_else(|| anyhow::anyhow!("Missing plugin manifest"))?,
    )?;
    manifest.validate()?;
    anyhow::ensure!(
        manifest.name == package.name
            && manifest.version.as_deref() == Some(package.version.as_str()),
        "Manifest identity does not match the release"
    );
    Ok(package)
}
fn fetch_into(config: &Config, name: &str, version: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        valid_name(name) && valid_version(version),
        "Invalid catalog plugin reference"
    );
    let suffix = format!("/plugins/{name}/releases/{version}");
    let release: Release = serde_json::from_slice(&get(config, &suffix)?)?;
    anyhow::ensure!(
        release.name == name && release.version == version,
        "Catalog release identity mismatch"
    );
    let bytes = get(config, &format!("{suffix}/package"))?;
    let package = validate_package(&bytes, &release)?;
    let root = source_root(config, name);
    std::fs::create_dir_all(&root)?;
    // Native acquisition serializes install/update. This source is separate from the live installed snapshot.
    let skills = root.join("skills");
    if skills.exists() {
        std::fs::remove_dir_all(&skills)?;
    }
    for (relative, content) in package.files {
        let path = root.join(relative);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid package path"))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(content.as_bytes())?;
        file.persist(path)?;
    }
    let provenance = Provenance {
        url: config.url.clone(),
        name: name.to_owned(),
        version: version.to_owned(),
        sha256: release.sha256,
    };
    let mut file = tempfile::NamedTempFile::new_in(&root)?;
    file.write_all(&serde_json::to_vec(&provenance)?)?;
    file.persist(root.join(".catalog-source.json"))?;
    Ok(root)
}
pub fn materialize(reference: &str) -> anyhow::Result<PathBuf> {
    let config = config()?.ok_or_else(|| anyhow::anyhow!("Configure a plugin catalog first"))?;
    let (name, version) = reference
        .rsplit_once('@')
        .ok_or_else(|| anyhow::anyhow!("Use catalog:name@version"))?;
    fetch_into(&config, name, version)
}
/// Refresh catalog-owned sources before the existing native local-snapshot updater runs.
pub(crate) fn refresh_source(path: &Path) -> anyhow::Result<bool> {
    let file = path.join(".catalog-source.json");
    if !file.exists() {
        return Ok(false);
    }
    let provenance: Provenance = serde_json::from_slice(&std::fs::read(file)?)?;
    let config = config()?.ok_or_else(|| {
        anyhow::anyhow!("Restore the original catalog configuration before updating")
    })?;
    anyhow::ensure!(
        config.url.trim_end_matches('/') == provenance.url.trim_end_matches('/')
            && source_root(&config, &provenance.name) == path,
        "Catalog source does not match this target configuration"
    );
    let entries = list()?;
    let latest = entries
        .iter()
        .find(|p| p.name == provenance.name)
        .ok_or_else(|| {
            anyhow::anyhow!("Plugin is archived, revoked, private, or unavailable in this catalog")
        })?;
    fetch_into(&config, &latest.name, &latest.version)?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    #[test]
    fn catalog_urls_and_package_paths_fail_closed() {
        for url in [
            "http://remote.example/api",
            "https://user:secret@example/api",
            "https://example/api?token=secret",
        ] {
            assert!(super::validate_base(url).is_err());
        }
        assert!(super::validate_base("http://127.0.0.1:3000/api/plugins/v1").is_ok());
        let bytes=br#"{"format":"supercharge-plugin-package-v1","name":"test","version":"1.0.0","files":{"plugin.json":"{}","../escape":"bad"}}"#;
        use sha2::{Digest, Sha256};
        let release = super::Release {
            name: "test".into(),
            version: "1.0.0".into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        };
        assert!(super::validate_package(bytes, &release).is_err());
    }
}
