use std::env;
use std::fs;
use std::path::Path;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct YamlConfig {
    #[serde(default)]
    pub runtime: Option<RuntimeConfig>,
    #[serde(default)]
    pub playback: Option<PlaybackConfig>,
    #[serde(default)]
    pub api: Option<ApiConfigYaml>,
    #[serde(default)]
    pub storage: Option<StorageConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
pub struct StorageConfig {
    #[serde(default)]
    pub log_level: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PlaybackConfig {
    #[serde(default)]
    pub default_volume: Option<f32>,
    #[serde(default)]
    pub normalize_audio: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ApiConfigYaml {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub frontend_url: Option<String>,
    #[serde(default)]
    pub cors_origins: Option<Vec<String>>,
    #[serde(default)]
    pub discord_redirect_uri: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AppConfig {
    pub discord_token: String,
    pub discord_client_id: String,
    pub discord_client_secret: String,
    pub discord_profile: String,
    pub admin_user_id: String,
    pub host: String,
    pub port: u16,
    pub frontend_url: String,
    pub cors_origins: Vec<String>,
    pub discord_redirect_uri: String,
    pub database_url: String,
    pub default_volume: f32,
    pub normalize_audio: bool,
    pub log_level: String,
}

impl AppConfig {
    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Load .env
        let _ = dotenvy::dotenv();

        let discord_profile = env::var("DISCORD_PROFILE")
            .or_else(|_| env::var("PROFILE"))
            .unwrap_or_else(|_| "DEV".to_string())
            .split('#')
            .next()
            .unwrap_or("DEV")
            .trim()
            .to_uppercase();

        // Select profile-specific config file: config.dev.yaml or config.prod.yaml
        let config_file = if let Ok(custom) = env::var("CONFIG_PATH") {
            custom
        } else if discord_profile == "PROD" {
            if Path::new("config.prod.yaml").exists() {
                "config.prod.yaml".to_string()
            } else {
                "config.yaml".to_string()
            }
        } else {
            if Path::new("config.dev.yaml").exists() {
                "config.dev.yaml".to_string()
            } else {
                "config.yaml".to_string()
            }
        };

        let yaml_config: Option<YamlConfig> = if Path::new(&config_file).exists() {
            let content = fs::read_to_string(&config_file)?;
            serde_yaml::from_str(&content).ok()
        } else {
            None
        };

        let (discord_token, discord_client_id, discord_client_secret) = if discord_profile == "DEV" {
            let token = env::var("DEV_DISCORD_TOKEN")
                .or_else(|_| env::var("DISCORD_TOKEN"))
                .unwrap_or_default();
            let id = env::var("DEV_DISCORD_CLIENT_ID")
                .or_else(|_| env::var("DISCORD_CLIENT_ID"))
                .unwrap_or_default();
            let secret = env::var("DEV_DISCORD_CLIENT_SECRET")
                .or_else(|_| env::var("DISCORD_CLIENT_SECRET"))
                .unwrap_or_default();
            (token, id, secret)
        } else {
            let token = env::var("DISCORD_TOKEN")
                .or_else(|_| env::var("DEV_DISCORD_TOKEN"))
                .unwrap_or_default();
            let id = env::var("DISCORD_CLIENT_ID")
                .or_else(|_| env::var("DEV_DISCORD_CLIENT_ID"))
                .unwrap_or_default();
            let secret = env::var("DISCORD_CLIENT_SECRET")
                .or_else(|_| env::var("DEV_DISCORD_CLIENT_SECRET"))
                .unwrap_or_default();
            (token, id, secret)
        };

        let admin_user_id = env::var("USER_ID").unwrap_or_else(|_| "368435858950324235".to_string());

        let host = env::var("HOST")
            .ok()
            .or_else(|| yaml_config.as_ref().and_then(|y| y.api.as_ref()).and_then(|a| a.host.clone()))
            .unwrap_or_else(|| "0.0.0.0".to_string());

        let port: u16 = env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| yaml_config.as_ref().and_then(|y| y.api.as_ref()).and_then(|a| a.port))
            .unwrap_or(8000);

        let frontend_url = env::var("FRONTEND_URL")
            .ok()
            .or_else(|| yaml_config.as_ref().and_then(|y| y.api.as_ref()).and_then(|a| a.frontend_url.clone()))
            .unwrap_or_else(|| {
                if discord_profile == "PROD" {
                    "https://musicbotui.pages.dev".to_string()
                } else {
                    "http://localhost:3000".to_string()
                }
            });

        let mut cors_origins = vec![
            "http://localhost:3000".to_string(),
            "http://127.0.0.1:3000".to_string(),
            "http://localhost:8080".to_string(),
            "http://127.0.0.1:8080".to_string(),
            "https://musicbotui.pages.dev".to_string(),
            "https://anhntse183225.github.io".to_string(),
        ];

        if let Some(yaml) = &yaml_config {
            if let Some(api) = &yaml.api {
                if let Some(origins) = &api.cors_origins {
                    for o in origins {
                        if !cors_origins.contains(o) {
                            cors_origins.push(o.clone());
                        }
                    }
                }
            }
        }

        let discord_redirect_uri = env::var("DISCORD_REDIRECT_URI")
            .ok()
            .or_else(|| yaml_config.as_ref().and_then(|y| y.api.as_ref()).and_then(|a| a.discord_redirect_uri.clone()))
            .unwrap_or_else(|| {
                if discord_profile == "PROD" {
                    "https://theanh.tail38dac5.ts.net/callback".to_string()
                } else {
                    "http://localhost:8000/callback".to_string()
                }
            });

        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://data/musicbot.db?mode=rwc".to_string());

        let default_volume = yaml_config.as_ref()
            .and_then(|y| y.playback.as_ref())
            .and_then(|p| p.default_volume)
            .unwrap_or(1.0);

        let normalize_audio = yaml_config.as_ref()
            .and_then(|y| y.playback.as_ref())
            .and_then(|p| p.normalize_audio)
            .unwrap_or(true);

        let log_level = env::var("LOG_LEVEL")
            .ok()
            .or_else(|| yaml_config.as_ref().and_then(|y| y.storage.as_ref()).and_then(|s| s.log_level.clone()))
            .unwrap_or_else(|| {
                if discord_profile == "DEV" {
                    "DEBUG".to_string()
                } else {
                    "INFO".to_string()
                }
            });

        Ok(Self {
            discord_token,
            discord_client_id,
            discord_client_secret,
            discord_profile,
            admin_user_id,
            host,
            port,
            frontend_url,
            cors_origins,
            discord_redirect_uri,
            database_url,
            default_volume,
            normalize_audio,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_dev_and_prod_split() {
        // Test DEV config loading
        let dev_content = fs::read_to_string("config.dev.yaml").expect("config.dev.yaml must exist");
        let dev_yaml: YamlConfig = serde_yaml::from_str(&dev_content).expect("Invalid config.dev.yaml");
        let dev_api = dev_yaml.api.expect("api section missing in config.dev.yaml");
        assert_eq!(dev_api.frontend_url.as_deref(), Some("http://localhost:3000"));
        assert_eq!(dev_api.discord_redirect_uri.as_deref(), Some("http://localhost:8000/callback"));

        // Test PROD config loading
        let prod_content = fs::read_to_string("config.prod.yaml").expect("config.prod.yaml must exist");
        let prod_yaml: YamlConfig = serde_yaml::from_str(&prod_content).expect("Invalid config.prod.yaml");
        let prod_api = prod_yaml.api.expect("api section missing in config.prod.yaml");
        assert_eq!(prod_api.frontend_url.as_deref(), Some("https://musicbotui.pages.dev"));
        assert_eq!(prod_api.discord_redirect_uri.as_deref(), Some("https://theanh.tail38dac5.ts.net/callback"));

        // Test AppConfig::load() with current env (DEV profile)
        let config = AppConfig::load().expect("Failed to load config");
        if config.discord_profile == "DEV" {
            assert_eq!(config.discord_redirect_uri, "http://localhost:8000/callback");
            assert_eq!(config.frontend_url, "http://localhost:3000");
        } else {
            assert_eq!(config.discord_redirect_uri, "https://theanh.tail38dac5.ts.net/callback");
            assert_eq!(config.frontend_url, "https://musicbotui.pages.dev");
        }
    }
}
