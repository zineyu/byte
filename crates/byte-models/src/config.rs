use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default configuration file name.
const DEFAULT_FILE_NAME: &str = "config.toml";
/// Environment variable that overrides the configuration file path.
const OVERRIDE_ENV_VAR: &str = "BYTE_CONFIG_PATH";
/// Environment variable for the XDG configuration home directory.
const XDG_CONFIG_HOME_VAR: &str = "XDG_CONFIG_HOME";
/// Environment variable for the user's home directory.
const HOME_VAR: &str = "HOME";

/// Configuration for a model provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    /// Provider identifier, e.g. `openai`, `openai-compatible`, or `echo`.
    pub provider: String,
    /// Base URL for the provider's API endpoint.
    pub base_url: String,
    /// API key used to authenticate with the provider.
    pub api_key: String,
    /// Model name to use for completions.
    pub model: String,
    /// Optional chunk size for the `echo` provider's text splitting.
    pub echo_chunk_size: Option<usize>,
    /// Optional delay in milliseconds between `echo` provider chunks.
    pub echo_delay_ms: Option<u64>,
}

impl ModelProviderConfig {
    /// Returns the configured echo chunk size or a sensible default.
    #[must_use]
    pub fn echo_chunk_size_or_default(&self) -> usize {
        self.echo_chunk_size.unwrap_or(5)
    }

    /// Returns the configured echo delay or zero if unset.
    #[must_use]
    pub fn echo_delay_or_default(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.echo_delay_ms.unwrap_or(0))
    }
}

/// Raw configuration deserialized from TOML before validation and defaults are applied.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    /// Provider identifier, e.g. `openai`, `openai-compatible`, or `echo`.
    provider: Option<String>,
    /// Base URL for the provider's API endpoint.
    base_url: Option<String>,
    /// API key used to authenticate with the provider.
    api_key: Option<String>,
    /// Model name to use for completions.
    model: Option<String>,
    /// Optional chunk size for the `echo` provider's text splitting.
    echo_chunk_size: Option<usize>,
    /// Optional delay in milliseconds between `echo` provider chunks.
    echo_delay_ms: Option<u64>,
    /// OpenAI-specific overrides for `openai-compatible` providers.
    openai: Option<OpenAiSection>,
}

/// OpenAI-specific section used to override `base_url`, `api_key`, and `model`.
#[derive(Debug, Default, Deserialize)]
struct OpenAiSection {
    /// Base URL for the OpenAI-compatible API endpoint.
    base_url: Option<String>,
    /// API key used to authenticate with the OpenAI-compatible provider.
    api_key: Option<String>,
    /// Model name to use for completions.
    model: Option<String>,
}

/// Errors that can occur while loading or parsing provider configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The configuration file was not found at the expected path.
    #[error("provider config not found at {0}")]
    NotFound(PathBuf),
    /// The configuration file could not be read.
    #[error("failed to read provider config at {path}: {source}")]
    Read {
        /// Path to the file that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The configuration file contains invalid TOML.
    #[error("provider config is invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    /// The configured provider is not supported.
    #[error("unsupported provider '{provider}' in config")]
    UnsupportedProvider {
        /// The provider name read from configuration.
        provider: String,
    },
    /// A required field is missing from the configuration.
    #[error("missing required field '{field}' in provider config")]
    MissingField {
        /// Name of the missing field.
        field: String,
    },
}

/// Resolve the configuration file path from the given environment values.
///
/// Prefers `override_path` if present, otherwise falls back to
/// `$XDG_CONFIG_HOME/byte/config.toml` or `$HOME/.config/byte/config.toml`.
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

/// # Errors
///
/// Returns an error if any of the required environment variables are invalid.
#[must_use]
pub fn resolve_config_path() -> PathBuf {
    resolve_config_path_with_env(
        std::env::var(OVERRIDE_ENV_VAR).ok(),
        std::env::var(XDG_CONFIG_HOME_VAR).ok(),
        std::env::var(HOME_VAR).ok(),
    )
}

/// # Errors
///
/// Returns an error if the config file does not exist or cannot be parsed.
pub async fn load_config() -> Result<ModelProviderConfig, ConfigError> {
    load_config_at_path(resolve_config_path()).await
}

/// # Errors
///
/// Returns an error if the file does not exist, cannot be read, or is invalid.
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

/// # Errors
///
/// Returns an error if the TOML is invalid or a required field is missing.
fn parse_config(contents: &str) -> Result<ModelProviderConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(contents)?;

    let provider = raw.provider.unwrap_or_else(|| "openai".to_owned());
    if provider != "openai" && provider != "openai-compatible" && provider != "echo" {
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

/// # Errors
///
/// Returns an error if `value` is `None`.
fn require_field(name: &str, value: Option<&str>) -> Result<String, ConfigError> {
    value
        .map(ToOwned::to_owned)
        .ok_or_else(|| ConfigError::MissingField {
            field: name.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

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
    async fn openai_compatible_alias_loads() {
        let path = temp_config_file(
            "provider = 'openai-compatible'\nbase_url = 'https://api.example.com/v1/'\napi_key = 'sk-test'\nmodel = 'gpt-test'",
        );

        let config = load_config_at_path(&path)
            .await
            .expect("openai-compatible alias should load");
        assert_eq!(config.provider, "openai-compatible");
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
}
