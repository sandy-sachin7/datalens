use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeltalensConfig {
    pub telemetry: bool,
}

impl DeltalensConfig {
    pub fn load() -> Self {
        let path = Self::path();
        if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Resolve cross-platform config path respecting DELTALENS_CONFIG_DIR, XDG_CONFIG_HOME,
    /// HOME, APPDATA, and USERPROFILE.
    pub fn path() -> PathBuf {
        if let Ok(dir) = std::env::var("DELTALENS_CONFIG_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir).join("config.json");
            }
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("deltalens").join("config.json");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                return PathBuf::from(home)
                    .join(".config")
                    .join("deltalens")
                    .join("config.json");
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join("deltalens").join("config.json");
            }
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            if !profile.is_empty() {
                return PathBuf::from(profile)
                    .join(".config")
                    .join("deltalens")
                    .join("config.json");
            }
        }
        PathBuf::from(".deltalens").join("config.json")
    }
}
