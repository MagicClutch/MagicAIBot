use std::{fs, path::Path};

use serde::Deserialize;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter as TargetLevel, Targets},
    fmt,
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::error::AppError;
use crate::minecraft::world_state::{
    DEFAULT_ENTITY_RADIUS, DEFAULT_MAXIMUM_ENTITIES, DEFAULT_STALE_SECONDS, validate_limits,
};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub minecraft: MinecraftConfig,
    pub reconnect: ReconnectConfig,
    #[serde(default)]
    pub console: ConsoleConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub world_state: WorldStateConfig,
    #[serde(default)]
    pub movement: MovementConfig,
    #[serde(default)]
    pub block_search: BlockSearchConfig,
    #[serde(default)]
    pub block_navigation: BlockNavigationConfig,
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
    #[serde(default)]
    pub debug: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorldStateConfig {
    #[serde(default = "default_entity_radius")]
    pub nearby_entity_radius: f64,
    #[serde(default = "default_maximum_entities")]
    pub maximum_tracked_entities: usize,
    #[serde(default = "default_stale_seconds")]
    pub stale_entity_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovementConfig {
    #[serde(default = "default_follow_distance")]
    pub follow_distance: f64,
    #[serde(default = "default_repath_interval_ms")]
    pub repath_interval_ms: u64,
    #[serde(default = "default_arrival_distance")]
    pub arrival_distance: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockSearchConfig {
    #[serde(default = "default_block_search_radius")]
    pub default_radius: u32,
    #[serde(default = "default_block_search_maximum_radius")]
    pub maximum_radius: u32,
    #[serde(default = "default_block_search_result_limit")]
    pub default_result_limit: usize,
    #[serde(default = "default_block_search_maximum_result_limit")]
    pub maximum_result_limit: usize,
    #[serde(default = "default_block_search_vertical_range")]
    pub default_vertical_range: u32,
}

fn default_block_search_radius() -> u32 {
    32
}
fn default_block_search_maximum_radius() -> u32 {
    128
}
fn default_block_search_result_limit() -> usize {
    20
}
fn default_block_search_maximum_result_limit() -> usize {
    256
}
fn default_block_search_vertical_range() -> u32 {
    32
}

impl Default for BlockSearchConfig {
    fn default() -> Self {
        Self {
            default_radius: default_block_search_radius(),
            maximum_radius: default_block_search_maximum_radius(),
            default_result_limit: default_block_search_result_limit(),
            maximum_result_limit: default_block_search_maximum_result_limit(),
            default_vertical_range: default_block_search_vertical_range(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockNavigationConfig {
    #[serde(default = "default_block_search_radius")]
    pub default_search_radius: u32,
    #[serde(default = "default_block_search_maximum_radius")]
    pub maximum_search_radius: u32,
    #[serde(default = "default_block_search_result_limit")]
    pub candidate_limit: usize,
    #[serde(default = "default_interaction_distance")]
    pub interaction_distance: f64,
    #[serde(default = "default_arrival_distance")]
    pub arrival_distance: f64,
    #[serde(default = "default_maximum_target_attempts")]
    pub maximum_target_attempts: usize,
    #[serde(default = "default_stuck_timeout_seconds")]
    pub stuck_timeout_seconds: u64,
    #[serde(default = "default_maximum_navigation_seconds")]
    pub maximum_navigation_seconds: u64,
    #[serde(default = "default_block_repath_interval_ms")]
    pub repath_interval_ms: u64,
}

fn default_interaction_distance() -> f64 {
    4.5
}
fn default_maximum_target_attempts() -> usize {
    8
}
fn default_stuck_timeout_seconds() -> u64 {
    12
}
fn default_maximum_navigation_seconds() -> u64 {
    120
}
fn default_block_repath_interval_ms() -> u64 {
    1000
}

impl Default for BlockNavigationConfig {
    fn default() -> Self {
        Self {
            default_search_radius: 32,
            maximum_search_radius: 128,
            candidate_limit: 20,
            interaction_distance: 4.5,
            arrival_distance: 1.5,
            maximum_target_attempts: 8,
            stuck_timeout_seconds: 12,
            maximum_navigation_seconds: 120,
            repath_interval_ms: 1000,
        }
    }
}
fn default_follow_distance() -> f64 {
    3.0
}
fn default_repath_interval_ms() -> u64 {
    500
}
fn default_arrival_distance() -> f64 {
    1.5
}
impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            follow_distance: 3.0,
            repath_interval_ms: 500,
            arrival_distance: 1.5,
        }
    }
}
fn default_entity_radius() -> f64 {
    DEFAULT_ENTITY_RADIUS
}
fn default_maximum_entities() -> usize {
    DEFAULT_MAXIMUM_ENTITIES
}
fn default_stale_seconds() -> u64 {
    DEFAULT_STALE_SECONDS
}
impl Default for WorldStateConfig {
    fn default() -> Self {
        Self {
            nearby_entity_radius: DEFAULT_ENTITY_RADIUS,
            maximum_tracked_entities: DEFAULT_MAXIMUM_ENTITIES,
            stale_entity_seconds: DEFAULT_STALE_SECONDS,
        }
    }
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
        validate_limits(
            self.world_state.nearby_entity_radius,
            self.world_state.maximum_tracked_entities,
            self.world_state.stale_entity_seconds,
        )?;
        if !(self.movement.follow_distance > 0.0 && self.movement.follow_distance <= 32.0) {
            return Err(AppError::InvalidMovementConfiguration(
                "follow_distance must be greater than zero and at most 32".into(),
            ));
        }
        if !(50..=10_000).contains(&self.movement.repath_interval_ms) {
            return Err(AppError::InvalidMovementConfiguration(
                "repath_interval_ms must be between 50 and 10000".into(),
            ));
        }
        if !(self.movement.arrival_distance > 0.0 && self.movement.arrival_distance <= 16.0) {
            return Err(AppError::InvalidMovementConfiguration(
                "arrival_distance must be greater than zero and at most 16".into(),
            ));
        }
        if self.block_search.default_radius == 0
            || self.block_search.maximum_radius == 0
            || self.block_search.default_radius > self.block_search.maximum_radius
            || self.block_search.maximum_radius > 256
        {
            return Err(AppError::InvalidBlockSearchConfiguration(
                "radius must be positive, default_radius <= maximum_radius, and maximum_radius <= 256".into(),
            ));
        }
        if self.block_search.default_result_limit == 0
            || self.block_search.maximum_result_limit == 0
            || self.block_search.default_result_limit > self.block_search.maximum_result_limit
            || self.block_search.maximum_result_limit > 4096
        {
            return Err(AppError::InvalidBlockSearchConfiguration(
                "result limits must be positive, default_result_limit <= maximum_result_limit, and maximum_result_limit <= 4096".into(),
            ));
        }
        if self.block_search.default_vertical_range == 0
            || self.block_search.default_vertical_range > 384
        {
            return Err(AppError::InvalidBlockSearchConfiguration(
                "default_vertical_range must be between 1 and 384".into(),
            ));
        }
        let navigation = &self.block_navigation;
        if navigation.default_search_radius == 0
            || navigation.maximum_search_radius == 0
            || navigation.default_search_radius > navigation.maximum_search_radius
            || navigation.maximum_search_radius > 256
        {
            return Err(AppError::InvalidBlockNavigationConfiguration(
                "search radius values are invalid".into(),
            ));
        }
        if navigation.candidate_limit == 0 || navigation.candidate_limit > 4096 {
            return Err(AppError::InvalidBlockNavigationConfiguration(
                "candidate_limit must be between 1 and 4096".into(),
            ));
        }
        if !(navigation.interaction_distance > 0.0 && navigation.interaction_distance <= 16.0)
            || !(navigation.arrival_distance > 0.0 && navigation.arrival_distance <= 16.0)
            || navigation.maximum_target_attempts == 0
            || navigation.maximum_target_attempts > 64
            || navigation.stuck_timeout_seconds == 0
            || navigation.stuck_timeout_seconds > 3600
            || navigation.maximum_navigation_seconds == 0
            || navigation.maximum_navigation_seconds > 3600
            || !(100..=10_000).contains(&navigation.repath_interval_ms)
        {
            return Err(AppError::InvalidBlockNavigationConfiguration(
                "navigation distances, attempts, timeouts, or repath interval are invalid".into(),
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

    let mut targets = Targets::new()
        .with_default(TargetLevel::OFF)
        .with_target(env!("CARGO_PKG_NAME"), level);
    if config.debug {
        targets = targets
            .with_target("azalea", TargetLevel::DEBUG)
            .with_target("azalea_client", TargetLevel::DEBUG)
            .with_target("azalea_entity", TargetLevel::DEBUG)
            .with_target("azalea_world", TargetLevel::DEBUG)
            .with_target("bevy", TargetLevel::DEBUG);
    }

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).with_filter(targets))
        .try_init()
        .map_err(|error| AppError::Logging(Box::new(error)))?;
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
        assert_eq!(config.block_search.default_radius, 32);
        assert_eq!(config.block_search.default_result_limit, 20);
        assert_eq!(config.block_navigation.maximum_target_attempts, 8);
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

    #[test]
    fn rejects_invalid_movement_configuration() {
        let config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"
                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1
                [logging]
                level = "info"
                [movement]
                follow_distance = 0
                repath_interval_ms = 500
                arrival_distance = 1.5
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidMovementConfiguration(_))
        ));
    }

    #[test]
    fn rejects_invalid_block_search_configuration() {
        let mut config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"
                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1
                [logging]
                level = "info"
                [block_search]
                default_radius = 129
                maximum_radius = 128
                default_result_limit = 20
                maximum_result_limit = 256
                default_vertical_range = 32
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockSearchConfiguration(_))
        ));
        config.block_search.default_radius = 32;
        config.block_search.maximum_radius = 128;
        config.block_search.default_result_limit = 20;
        config.block_search.maximum_result_limit = 4097;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockSearchConfiguration(_))
        ));
    }

    #[test]
    fn rejects_invalid_block_navigation_configuration() {
        let mut config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"
                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1
                [logging]
                level = "info"
                [block_navigation]
                default_search_radius = 32
                maximum_search_radius = 128
                candidate_limit = 20
                interaction_distance = 4.5
                arrival_distance = 1.5
                maximum_target_attempts = 0
                stuck_timeout_seconds = 12
                maximum_navigation_seconds = 120
                repath_interval_ms = 1000
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockNavigationConfiguration(_))
        ));
        config.block_navigation.maximum_target_attempts = 8;
        config.block_navigation.maximum_search_radius = 0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockNavigationConfiguration(_))
        ));
    }
}
