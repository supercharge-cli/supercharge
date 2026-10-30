//! `/isetup` / `/imodels` persistence for `[image_provider]`.

use std::path::Path;

use super::provider_setup::{ProviderSetup, ProviderSetupError, normalize_provider_base_url};
use supercharge_tools::implementations::supercharge_build::image_gen::ImageProviderFormat;

const MAX_API_KEY_LEN: usize = 8192;
const MAX_MODEL_ID_LEN: usize = 200;

fn image_format_from_setup(setup: &ProviderSetup) -> ImageProviderFormat {
    if setup.provider_id.eq_ignore_ascii_case("pollinations") {
        ImageProviderFormat::Pollinations
    } else if setup.provider_id.eq_ignore_ascii_case("custom") {
        ImageProviderFormat::Custom
    } else {
        ImageProviderFormat::OpenaiImages
    }
}

pub fn persist_image_provider_setup(setup: &ProviderSetup) -> Result<(), ProviderSetupError> {
    persist_image_provider_setup_at(&crate::util::config::user_config_path(), setup)
}

pub fn persist_image_provider_setup_at(
    path: &Path,
    setup: &ProviderSetup,
) -> Result<(), ProviderSetupError> {
    let base_url = normalize_provider_base_url(&setup.base_url)?;
    let api_key = setup.api_key.trim();
    if api_key.len() > MAX_API_KEY_LEN {
        return Err(ProviderSetupError::ApiKeyTooLong);
    }
    let model_id = setup.model_id.trim();
    if model_id.is_empty() || model_id.len() > MAX_MODEL_ID_LEN {
        return Err(ProviderSetupError::InvalidModel);
    }
    let format = image_format_from_setup(setup);
    let provider_id = if setup.provider_id.trim().is_empty() {
        "custom"
    } else {
        setup.provider_id.trim()
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut document = match crate::util::config::read_to_string_or_empty(path) {
        Ok(content) if !content.trim().is_empty() => content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| ProviderSetupError::Io(error.to_string()))?,
        Ok(_) => toml_edit::DocumentMut::new(),
        Err(error) => return Err(error.into()),
    };

    let image_provider = document
        .entry("image_provider")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| ProviderSetupError::Io("[image_provider] is not a table".to_owned()))?;
    image_provider.insert("id", toml_edit::value(provider_id));
    image_provider.insert("base_url", toml_edit::value(base_url.as_str()));
    image_provider.insert("model", toml_edit::value(model_id));
    image_provider.insert("format", toml_edit::value(format.as_str()));
    if api_key.is_empty() {
        image_provider.remove("api_key");
    } else {
        image_provider.insert("api_key", toml_edit::value(api_key));
    }
    let features = document
        .entry("features")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| ProviderSetupError::Io("[features] is not a table".to_owned()))?;
    features.insert("image_gen", toml_edit::value(true));

    crate::util::config::atomic_write_string(path, &document.to_string())?;
    Ok(())
}

pub fn persist_image_model(model_id: &str) -> Result<(), ProviderSetupError> {
    persist_image_model_at(&crate::util::config::user_config_path(), model_id)
}

pub fn persist_image_model_at(path: &Path, model_id: &str) -> Result<(), ProviderSetupError> {
    let model_id = model_id.trim();
    if model_id.is_empty() || model_id.len() > MAX_MODEL_ID_LEN {
        return Err(ProviderSetupError::InvalidModel);
    }
    let mut document = match crate::util::config::read_to_string_or_empty(path) {
        Ok(content) if !content.trim().is_empty() => content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| ProviderSetupError::Io(error.to_string()))?,
        Ok(_) => toml_edit::DocumentMut::new(),
        Err(error) => return Err(error.into()),
    };
    let image_provider = document
        .entry("image_provider")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| ProviderSetupError::Io("[image_provider] is not a table".to_owned()))?;
    image_provider.insert("model", toml_edit::value(model_id));
    crate::util::config::atomic_write_string(path, &document.to_string())?;
    Ok(())
}

pub fn load_existing_image_provider_setup() -> Option<(String, String, String)> {
    let path = crate::util::config::user_config_path();
    let content = crate::util::config::read_to_string_or_empty(&path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    let parsed = content.parse::<toml::Value>().ok()?;
    let table = parsed.get("image_provider")?;
    let base_url = table
        .get("base_url")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)?;
    let api_key = table
        .get("api_key")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    let model = table
        .get("model")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    Some((base_url, api_key, model))
}

#[cfg(test)]
mod tests {
    use super::super::provider_setup::SetupWireFormat;
    use super::*;

    #[test]
    fn persist_writes_image_provider_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        persist_image_provider_setup_at(
            &path,
            &ProviderSetup {
                base_url: "https://gen.pollinations.ai/v1".into(),
                api_key: String::new(),
                model_id: "flux".into(),
                model_name: "flux".into(),
                format: SetupWireFormat::Openai,
                provider_id: "pollinations".into(),
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[image_provider]"));
        assert!(text.contains("model = \"flux\""));
        assert!(text.contains("format = \"pollinations\""));
        assert!(!text.contains("api_key"));
    }

    #[test]
    fn persist_image_model_updates_only_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[image_provider]\nbase_url = \"https://api.openai.com/v1\"\nmodel = \"dall-e-3\"\n",
        )
        .unwrap();
        persist_image_model_at(&path, "gpt-image-1").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("model = \"gpt-image-1\""));
        assert!(text.contains("https://api.openai.com/v1"));
    }
}
