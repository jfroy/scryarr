use std::net::SocketAddr;
use std::path::PathBuf;

use url::Url;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub scryer_url: Url,
    pub events_token: String,
    pub data_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}: {1}")]
    Invalid(&'static str, String),
}

impl Config {
    pub fn from_env() -> Result<Config, ConfigError> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(vars: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let listen = vars("SCRYARR_LISTEN").unwrap_or_else(|| "0.0.0.0:8080".to_string());
        let listen = listen.parse().map_err(|e: std::net::AddrParseError| {
            ConfigError::Invalid("SCRYARR_LISTEN", e.to_string())
        })?;

        let scryer_url = vars("SCRYER_URL").ok_or(ConfigError::Missing("SCRYER_URL"))?;
        let scryer_url = Url::parse(&scryer_url)
            .map_err(|e| ConfigError::Invalid("SCRYER_URL", e.to_string()))?;

        let events_token =
            vars("SCRYER_EVENTS_TOKEN").ok_or(ConfigError::Missing("SCRYER_EVENTS_TOKEN"))?;
        if events_token.is_empty() {
            return Err(ConfigError::Invalid(
                "SCRYER_EVENTS_TOKEN",
                "must not be empty".to_string(),
            ));
        }

        let data_dir = vars("SCRYARR_DATA_DIR").unwrap_or_else(|| "/data".to_string());

        Ok(Config {
            listen,
            scryer_url,
            events_token,
            data_dir: PathBuf::from(data_dir),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn config_requires_scryer_url() {
        assert!(Config::from_vars(vars(&[("SCRYER_EVENTS_TOKEN", "t")])).is_err());
    }

    #[test]
    fn config_rejects_empty_token() {
        let v = vars(&[
            ("SCRYER_URL", "http://scryer:8080"),
            ("SCRYER_EVENTS_TOKEN", ""),
        ]);
        assert!(Config::from_vars(v).is_err());
    }

    #[test]
    fn config_defaults() {
        let v = vars(&[
            ("SCRYER_URL", "http://scryer:8080"),
            ("SCRYER_EVENTS_TOKEN", "t"),
        ]);
        let c = Config::from_vars(v).unwrap();
        assert_eq!(c.listen, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(c.data_dir, PathBuf::from("/data"));
    }
}
