use crate::anthropic::AnthropicClient;
use crate::openai::OpenAiClient;
use crate::LlmClient;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A named provider entry in the config file.
/// Supports OpenAI-compatible and Anthropic protocol endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    #[default]
    OpenAi,
    LiteLlm,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Display name, e.g. "openai", "ollama", "deepseek"
    pub name: String,
    /// Provider API protocol.
    #[serde(default)]
    pub protocol: ProviderProtocol,
    /// Base URL. None = OpenAI official endpoint.
    pub base_url: Option<String>,
    /// API key. Can also be set via env var named in `api_key_env`.
    pub api_key: Option<String>,
    /// Name of env var to read the API key from (e.g. "OPENAI_API_KEY")
    pub api_key_env: Option<String>,
    /// Default model for this provider
    pub model: String,
}

/// Top-level config file (~/.config/bs/config.toml or .bs/config.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BsConfig {
    /// Which provider to use by default
    pub default_provider: Option<String>,
    pub providers: Vec<ProviderConfig>,
}

impl BsConfig {
    /// Load config: project-local `.bs/config.toml` overrides global `~/.config/bs/config.toml`
    pub fn load(project_root: &Path) -> Result<Self> {
        let global = dirs_path().map(|d| d.join("config.toml"));
        let local = project_root.join(".bs/config.toml");

        let mut cfg = BsConfig::default();

        if let Some(g) = global.filter(|p| p.exists()) {
            let s =
                std::fs::read_to_string(&g).with_context(|| format!("reading {}", g.display()))?;
            cfg = toml::from_str(&s)?;
        }
        if local.exists() {
            let s = std::fs::read_to_string(&local)
                .with_context(|| format!("reading {}", local.display()))?;
            // local overrides global
            cfg = toml::from_str(&s)?;
        }

        Ok(cfg)
    }

    pub fn save_project(&self, project_root: &Path) -> Result<std::path::PathBuf> {
        let dir = project_root.join(".bs");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("config.toml");
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Build a client for the named provider (or default provider).
    pub fn build_client(&self, provider_name: Option<&str>) -> Result<Box<dyn LlmClient>> {
        let name = provider_name
            .or(self.default_provider.as_deref())
            .unwrap_or("default");

        // Find matching provider, or fall back to env-var based default
        let provider = self.providers.iter().find(|p| p.name == name);

        match provider {
            Some(p) => {
                let api_key = resolve_api_key(p)?;
                match p.protocol {
                    ProviderProtocol::OpenAi | ProviderProtocol::LiteLlm => Ok(Box::new(
                        OpenAiClient::new(&api_key, p.base_url.as_deref(), &p.model),
                    )),
                    ProviderProtocol::Anthropic => Ok(Box::new(AnthropicClient::new(
                        &api_key,
                        p.base_url.as_deref(),
                        &p.model,
                    ))),
                }
            }
            None => {
                // No config — fall back to env vars (backward compat)
                let api_key =
                    std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "no-key".to_string());
                let base_url = std::env::var("LLM_BASE_URL").ok();
                let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
                Ok(Box::new(OpenAiClient::new(
                    &api_key,
                    base_url.as_deref(),
                    &model,
                )))
            }
        }
    }

    /// List all configured provider names
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name.as_str()).collect()
    }
}

fn resolve_api_key(p: &ProviderConfig) -> Result<String> {
    // 1. Explicit key in config
    if let Some(key) = &p.api_key {
        return Ok(key.clone());
    }
    // 2. Named env var
    if let Some(env_name) = &p.api_key_env {
        return std::env::var(env_name)
            .with_context(|| format!("env var {} not set for provider {}", env_name, p.name));
    }
    // 3. Guess common env var names
    let guesses = [
        format!("{}_API_KEY", p.name.to_uppercase()),
        "LITELLM_API_KEY".to_string(),
        "ANTHROPIC_API_KEY".to_string(),
        "OPENAI_API_KEY".to_string(),
    ];
    for g in &guesses {
        if let Ok(v) = std::env::var(g) {
            return Ok(v);
        }
    }
    anyhow::bail!(
        "no API key found for provider '{}'. Set api_key or api_key_env in config.",
        p.name
    )
}

fn dirs_path() -> Option<std::path::PathBuf> {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(std::path::PathBuf::from(appdata).join("bs"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Some(std::path::PathBuf::from(home).join(".config/bs"));
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        return Some(std::path::PathBuf::from(user_profile).join(".config/bs"));
    }
    match (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        (Ok(drive), Ok(path)) => Some(std::path::PathBuf::from(format!("{drive}{path}")).join(".config/bs")),
        _ => None,
    }
}
