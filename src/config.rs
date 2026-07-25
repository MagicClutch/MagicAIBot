use std::{fs, path::Path};

use serde::Deserialize;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;

use crate::error::AppError;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub minecraft: MinecraftConfig,
    pub reconnect: ReconnectConfig,
    #[serde(default)]
    pub console: ConsoleConfig,
    pub logging: LoggingConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MinecraftConfig {
    pub server: String,
    pub username: String,
    pub account_mode: AccountMode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountMode {
    Offline,
    Microsoft,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReconnectConfig {
    pub enabled: bool,
    pub delay_seconds: u64,
    pub maximum_attempts: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConsoleConfig {
    pub enabled: bool,
    pub send_plain_input_to_chat: bool,
    pub show_system_messages: bool,
    pub show_action_bar_messages: bool,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            send_plain_input_to_chat: true,
            show_system_messages: true,
            show_action_bar_messages: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let contents = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.minecraft.server.trim().is_empty() {
            return Err(AppError::InvalidConfiguration(
                "minecraft.server must not be empty".to_owned(),
            ));
        }
        if self.minecraft.username.trim().is_empty() {
            return Err(AppError::InvalidConfiguration(
                "minecraft.username must not be empty".to_owned(),
            ));
        }
        if self.reconnect.enabled && self.reconnect.maximum_attempts == 0 {
            return Err(AppError::InvalidConfiguration(
                "reconnect.maximum_attempts must be greater than zero when reconnect is enabled"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

pub fn init_logging(config: &LoggingConfig) -> Result<(), AppError> {
    let level = config
        .level
        .parse::<LevelFilter>()
        .map_err(|_| AppError::InvalidLogLevel(config.level.clone()))?;

    fmt()
        .with_max_level(level)
        .try_init()
        .map_err(AppError::Logging)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_localhost_without_requiring_a_port() {
        let config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"

                [reconnect]
                enabled = false
                delay_seconds = 10
                maximum_attempts = 5

                [logging]
                level = "info"
            "#,
        )
        .expect("test configuration should parse");

        config.validate().expect("localhost should be valid");
        assert_eq!(config.minecraft.server, "localhost");
    }

    #[test]
    fn rejects_enabled_reconnect_without_attempts() {
        let config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost:25565"
                username = "MagicBot"
                account_mode = "offline"

                [reconnect]
                enabled = true
                delay_seconds = 1
                maximum_attempts = 0

                [logging]
                level = "info"
            "#,
        )
        .expect("test configuration should parse");

        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidConfiguration(message))
                if message.contains("maximum_attempts")
        ));
    }
}
