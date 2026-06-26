use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_FILE_NAME: &str = "config.toml";
const OVERRIDE_ENV_VAR: &str = "BYTE_CONFIG_PATH";
const XDG_CONFIG_HOME_VAR: &str = "XDG_CONFIG_HOME";
const HOME_VAR: &str = "HOME";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub echo_chunk_size: Option<usize>,
    pub echo_delay_ms: Option<u64>,
}

impl ModelProviderConfig {
    pub fn echo_chunk_size_or_default(&self) -> usize {
        self.echo_chunk_size.unwrap_or(5)
    }

    pub fn echo_delay_or_default(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.echo_delay_ms.unwrap_or(0))
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    provider: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    echo_chunk_size: Option<usize>,
    echo_delay_ms: Option<u64>,
    openai: Option<OpenAiSection>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiSection {
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("provider config not found at {0}")]
    NotFound(PathBuf),
    #[error("failed to read provider config at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("provider config is invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported provider '{provider}' in config")]
    UnsupportedProvider { provider: String },
    #[error("missing required field '{field}' in provider config")]
    MissingField { field: String },
}

fn resolve_config_path_with_env(
    override_path: Option<String>,
    xdg_config_home: Option<String>,
    home: Option<String>,
) -> PathBuf {
    if let Some(path) = override_path {
        return PathBuf::from(path);
    }

    let config_dir = xdg_config_home.map_or_else(
        || {
            let home = home.unwrap_or_else(|| String::from("."));
            PathBuf::from(home).join(".config")
        },
        PathBuf::from,
    );

    config_dir.join("byte").join(DEFAULT_FILE_NAME)
}

pub fn resolve_config_path() -> PathBuf {
    resolve_config_path_with_env(
        std::env::var(OVERRIDE_ENV_VAR).ok(),
        std::env::var(XDG_CONFIG_HOME_VAR).ok(),
        std::env::var(HOME_VAR).ok(),
    )
}

pub async fn load_config() -> Result<ModelProviderConfig, ConfigError> {
    load_config_at_path(resolve_config_path()).await
}

pub async fn load_config_at_path(
    path: impl AsRef<Path>,
) -> Result<ModelProviderConfig, ConfigError> {
    let path = path.as_ref();

    if !path.exists() {
        return Err(ConfigError::NotFound(path.to_path_buf()));
    }

    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;

    parse_config(&contents)
}

fn parse_config(contents: &str) -> Result<ModelProviderConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(contents)?;

    let provider = raw.provider.unwrap_or_else(|| "openai".to_owned());
    if provider != "openai" && provider != "echo" {
        return Err(ConfigError::UnsupportedProvider { provider });
    }
    let section = raw.openai.unwrap_or_default();

    let base_url = require_field("base_url", raw.base_url.or(section.base_url).as_deref())?;
    let api_key = require_field("api_key", raw.api_key.or(section.api_key).as_deref())?;
    let model = require_field("model", raw.model.or(section.model).as_deref())?;

    Ok(ModelProviderConfig {
        provider,
        base_url,
        api_key,
        model,
        echo_chunk_size: raw.echo_chunk_size,
        echo_delay_ms: raw.echo_delay_ms,
    })
}

fn require_field(name: &str, value: Option<&str>) -> Result<String, ConfigError> {
    value
        .map(std::borrow::ToOwned::to_owned)
        .ok_or_else(|| ConfigError::MissingField {
            field: name.to_owned(),
        })
}

pub fn normalize_base_url(url: &str) -> String {
    url.trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_config_file(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "byte-models-config-test-{}.toml",
            uuid::Uuid::new_v4()
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn env_override_takes_precedence() {
        let path = "/override/config.toml";
        let resolved = resolve_config_path_with_env(
            Some(path.into()),
            Some("/xdg/config".into()),
            Some("/home/user".into()),
        );
        assert_eq!(resolved, PathBuf::from(path));
    }

    #[test]
    fn xdg_config_home_is_used_when_env_set() {
        let resolved = resolve_config_path_with_env(
            None,
            Some("/xdg/config".into()),
            Some("/home/user".into()),
        );
        assert_eq!(resolved, PathBuf::from("/xdg/config/byte/config.toml"));
    }

    #[test]
    fn falls_back_to_home_dot_config() {
        let resolved = resolve_config_path_with_env(None, None, Some("/home/user".into()));
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.config/byte/config.toml")
        );
    }

    #[tokio::test]
    async fn missing_config_returns_not_found() {
        let err = load_config_at_path("/nonexistent/byte/config.toml")
            .await
            .expect_err("missing config should fail");
        assert!(
            matches!(err, ConfigError::NotFound(_)),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn flat_config_loads() {
        let path = temp_config_file(
            "provider = 'openai'\nbase_url = 'https://api.openai.com/v1/'\napi_key = 'sk-test'\nmodel = 'gpt-4o'",
        );

        let config = load_config_at_path(&path)
            .await
            .expect("valid config loads");
        assert_eq!(config.provider, "openai");
        assert_eq!(config.base_url, "https://api.openai.com/v1/");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "gpt-4o");
    }

    #[tokio::test]
    async fn section_config_loads() {
        let path = temp_config_file(
            "[openai]\nbase_url = 'https://api.openai.com/v1/'\napi_key = 'sk-test'\nmodel = 'gpt-4o'",
        );

        let config = load_config_at_path(&path)
            .await
            .expect("valid section config loads");
        assert_eq!(config.provider, "openai");
        assert_eq!(config.base_url, "https://api.openai.com/v1/");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "gpt-4o");
    }

    #[tokio::test]
    async fn flat_fields_override_section_fields() {
        let path = temp_config_file(
            "base_url = 'https://flat.example.com/v1/'\napi_key = 'sk-flat'\nmodel = 'gpt-flat'\n\n[openai]\nbase_url = 'https://section.example.com/v1/'\napi_key = 'sk-section'\nmodel = 'gpt-section'",
        );

        let config = load_config_at_path(&path)
            .await
            .expect("valid config loads");
        assert_eq!(config.base_url, "https://flat.example.com/v1/");
        assert_eq!(config.api_key, "sk-flat");
        assert_eq!(config.model, "gpt-flat");
    }

    #[tokio::test]
    async fn unsupported_provider_fails() {
        let path = temp_config_file(
            "provider = 'anthropic'\nbase_url = 'https://api.anthropic.com/'\napi_key = 'sk-test'\nmodel = 'claude'",
        );

        let err = load_config_at_path(&path)
            .await
            .expect_err("unsupported provider should fail");
        assert!(
            matches!(err, ConfigError::UnsupportedProvider { ref provider } if provider == "anthropic"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn missing_required_field_fails() {
        let path = temp_config_file(
            "[openai]\nbase_url = 'https://api.openai.com/v1/'\napi_key = 'sk-test'",
        );

        let err = load_config_at_path(&path)
            .await
            .expect_err("missing model should fail");
        assert!(
            matches!(err, ConfigError::MissingField { ref field } if field == "model"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn normalize_base_url_removes_trailing_slash() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
    }
}
