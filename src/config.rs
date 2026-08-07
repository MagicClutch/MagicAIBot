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
    pub vertical_navigation: VerticalNavigationConfig,
    #[serde(default)]
    pub bridging: BridgingConfig,
    #[serde(default)]
    pub block_search: BlockSearchConfig,
    #[serde(default)]
    pub block_navigation: BlockNavigationConfig,
    #[serde(default)]
    pub look: LookConfig,
    #[serde(default)]
    pub interaction: InteractionConfig,
    #[serde(default)]
    pub multitasking: MultitaskingConfig,
    #[serde(default)]
    pub chat_commands: ChatCommandsConfig,
    #[serde(default)]
    pub equipment: EquipmentConfig,
    #[serde(default)]
    pub survival: SurvivalConfig,
    #[serde(default)]
    pub output: OutputConfig,
    #[serde(default)]
    pub killbot: KillbotConfig,
    #[serde(default)]
    pub pathfinding: PathfindingConfig,
}

/// How much of the bot's own status narration (task started/progress/
/// finished, not raw player chat -- see `crate::logging`) reaches a given
/// destination. Each level shows everything the levels below it show, plus
/// more:
///
/// - `none` -- nothing at all.
/// - `light` -- only a task's start and its final outcome (e.g. "Get task
///   started: diamond x5" / "Collected 5 diamond").
/// - `info` -- `light`, plus a running progress report as a task makes
///   headway (e.g. "Collected diamond (3/5)"). The default for both
///   destinations.
/// - `debug` -- `info`, plus every other diagnostic line the bot prints
///   (connection state, per-step narration, retries, ...) -- this is every
///   line that printed unconditionally before this setting existed.
/// - `fulldebug` -- `debug`, but also disables the console's repeat
///   collapsing (see `crate::logging`'s `RepeatGuard`) so a stuck retry
///   loop prints every single repetition instead of being summarized.
///   Intended for troubleshooting only, not everyday use.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    None,
    Light,
    #[default]
    Info,
    Debug,
    FullDebug,
}

impl OutputMode {
    /// Parses a mode name the way config/console/chat all spell it --
    /// case-insensitively, and accepting `full-debug`/`full_debug` as
    /// spelling variants of `fulldebug`. `None` (not an error) when `text`
    /// doesn't name a mode, so callers can report their own usage message.
    pub fn parse(text: &str) -> Option<Self> {
        match text
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_'], "")
            .as_str()
        {
            "none" => Some(Self::None),
            "light" => Some(Self::Light),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "fulldebug" => Some(Self::FullDebug),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Light => "light",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::FullDebug => "fulldebug",
        }
    }
}

/// Where the bot's own status narration is allowed to reach -- the local
/// console/run window, or Minecraft chat -- each independently levelled by
/// [`OutputMode`]. Defaults to `info`/`info`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OutputConfig {
    #[serde(default)]
    pub console: OutputMode,
    #[serde(default)]
    pub chat: OutputMode,
}

/// Access control and rate limiting for the `#`-prefixed direct console
/// command feature in Minecraft chat (see `App::handle_chat_console_command`).
/// Typing `#goto 100 64 20` in chat runs exactly what `/goto 100 64 20`
/// would in the local console -- a strictly more powerful (and more directly
/// abusable) surface than anything else reachable from chat, so it is never
/// less guarded than this section allows.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ChatCommandsConfig {
    #[serde(default)]
    pub access: ChatCommandsAccessConfig,
    #[serde(default)]
    pub rate_limit: ChatCommandsRateLimitConfig,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ChatCommandsAccessConfig {
    #[serde(default)]
    pub operators_only: bool,
    #[serde(default)]
    pub allowed_players: Vec<String>,
    #[serde(default)]
    pub blocked_players: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ChatCommandsRateLimitConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_chat_command_rate_limit_requests")]
    pub requests: usize,
    #[serde(default = "default_chat_command_rate_limit_window")]
    pub window_seconds: u64,
}
impl Default for ChatCommandsRateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests: 3,
            window_seconds: 30,
        }
    }
}
fn default_chat_command_rate_limit_requests() -> usize {
    3
}
fn default_chat_command_rate_limit_window() -> u64 {
    30
}

/// Everything the fully automatic equipment system needs -- see
/// `crate::equipment` for the scoring/selection logic this configures.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EquipmentConfig {
    #[serde(default)]
    pub armor: EquipmentArmorConfig,
    #[serde(default)]
    pub offhand: EquipmentOffhandConfig,
    #[serde(default)]
    pub hotbar: HotbarEquipmentConfig,
    #[serde(default)]
    pub tools: ToolEquipmentConfig,
    #[serde(default)]
    pub autodrop: AutoDropConfig,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EquipmentArmorConfig {
    #[serde(default)]
    pub mode: ArmorMode,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EquipmentOffhandConfig {
    #[serde(default)]
    pub priority: OffhandPriority,
}
/// `score` weighs material and durability together
/// (`crate::equipment::armor::score`); `rarity` ignores durability entirely
/// and ranks by material alone.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ArmorMode {
    #[default]
    Score,
    Rarity,
}
/// Which of a Totem of Undying or a Shield the offhand prefers when both
/// are available (see `crate::equipment::offhand::desired_item`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OffhandPriority {
    #[default]
    Totem,
    Shield,
}

/// Automatic hotbar equipment: keeps the best sword/pickaxe/axe/shovel/
/// building block/water bucket (plus any configured utility items) in a
/// designated hotbar slot, swapping in a better one automatically -- the
/// same idea as `armor`/`offhand` above, just for the hotbar. See
/// `crate::equipment::hotbar::HotbarEquipmentService`.
#[derive(Clone, Debug, Deserialize)]
pub struct HotbarEquipmentConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub slots: HotbarSlotsConfig,
    /// Extra items to keep stocked in a specific hotbar slot by exact item
    /// id -- not ranked against anything else, just kept there whenever
    /// held anywhere in the inventory (e.g. a Totem of Undying pinned to a
    /// slot the offhand-priority logic doesn't already cover).
    #[serde(default)]
    pub utility_items: Vec<UtilityItemConfig>,
    /// Real re-evaluation is event-driven (keyed off
    /// `InventorySnapshot::revision`, which only changes when slot contents
    /// actually do -- see `item pickup`/`inventory changes` in the feature
    /// spec) -- this is only the periodic fallback safety net in case a
    /// revision bump is ever missed.
    #[serde(default = "default_hotbar_periodic_scan_seconds")]
    pub periodic_scan_seconds: u64,
}
impl Default for HotbarEquipmentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            slots: HotbarSlotsConfig::default(),
            utility_items: Vec::new(),
            periodic_scan_seconds: default_hotbar_periodic_scan_seconds(),
        }
    }
}
fn default_hotbar_periodic_scan_seconds() -> u64 {
    10
}

/// 1-9 hotbar slot (as typed/shown in-game) each built-in category is kept
/// in. `None` (or the field omitted) leaves that category entirely
/// unmanaged.
#[derive(Clone, Debug, Deserialize)]
pub struct HotbarSlotsConfig {
    #[serde(default = "default_slot_1")]
    pub sword: Option<u8>,
    #[serde(default = "default_slot_2")]
    pub pickaxe: Option<u8>,
    #[serde(default = "default_slot_3")]
    pub axe: Option<u8>,
    #[serde(default = "default_slot_4")]
    pub shovel: Option<u8>,
    #[serde(default = "default_slot_5")]
    pub blocks: Option<u8>,
    #[serde(default = "default_slot_6")]
    pub water_bucket: Option<u8>,
}
impl Default for HotbarSlotsConfig {
    fn default() -> Self {
        Self {
            sword: default_slot_1(),
            pickaxe: default_slot_2(),
            axe: default_slot_3(),
            shovel: default_slot_4(),
            blocks: default_slot_5(),
            water_bucket: default_slot_6(),
        }
    }
}
fn default_slot_1() -> Option<u8> {
    Some(1)
}
fn default_slot_2() -> Option<u8> {
    Some(2)
}
fn default_slot_3() -> Option<u8> {
    Some(3)
}
fn default_slot_4() -> Option<u8> {
    Some(4)
}
fn default_slot_5() -> Option<u8> {
    Some(5)
}
fn default_slot_6() -> Option<u8> {
    Some(6)
}

#[derive(Clone, Debug, Deserialize)]
pub struct UtilityItemConfig {
    pub item_id: String,
    pub slot: u8,
}

/// How `pickaxe`/`axe`/`shovel`/`sword` candidates are ranked against each
/// other and against whatever is currently held -- mirrors `ArmorMode`
/// exactly, just applied to `interaction::tool_selection::tier` (material
/// tier) instead of `equipment::armor::ArmorMaterial`, since tools don't
/// have their own separate ranking system (see
/// `equipment::tools::rank_score`, which reuses the same durability-penalty
/// formula as `equipment::armor::score`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ToolRankingMode {
    #[default]
    Rarity,
    Score,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ToolEquipmentConfig {
    #[serde(default)]
    pub ranking_mode: ToolRankingMode,
}

/// Configurable automatic dropping of whatever a hotbar/armor replacement
/// just displaced. Never touches the currently-equipped item itself -- only
/// ever considers the item a *successful* swap just displaced, and only
/// after that swap is confirmed (see `equipment::autodrop`).
#[derive(Clone, Debug, Deserialize)]
pub struct AutoDropConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Item ids never auto-dropped regardless of any scoring comparison --
    /// e.g. keep a spare diamond pickaxe even after finding a netherite one.
    #[serde(default)]
    pub protected_items: Vec<String>,
    /// Off by default: players often want to keep backup armor.
    #[serde(default = "default_autodrop_armor")]
    pub armor: AutoDropCategoryConfig,
    #[serde(default = "default_autodrop_enabled")]
    pub tools: AutoDropCategoryConfig,
    #[serde(default = "default_autodrop_enabled")]
    pub weapons: AutoDropCategoryConfig,
}
impl Default for AutoDropConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            protected_items: Vec::new(),
            armor: default_autodrop_armor(),
            tools: default_autodrop_enabled(),
            weapons: default_autodrop_enabled(),
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct AutoDropCategoryConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Currently the only drop trigger this system has (see this module's
    /// `AutoDropConfig` doc comment) -- kept as its own field, rather than
    /// folded into `enabled`, to match the category's config shape 1:1 and
    /// leave room for a future trigger that isn't "a better item was found"
    /// without a breaking config change.
    #[serde(default = "default_true")]
    pub drop_when_better_available: bool,
}
fn default_autodrop_armor() -> AutoDropCategoryConfig {
    AutoDropCategoryConfig {
        enabled: false,
        drop_when_better_available: true,
    }
}
fn default_autodrop_enabled() -> AutoDropCategoryConfig {
    AutoDropCategoryConfig {
        enabled: true,
        drop_when_better_available: true,
    }
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
            send_plain_input_to_chat: false,
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

/// Automatic water-bucket MLG (falling-water clutch), driven from
/// `crate::survival::SurvivalController`. Watched continuously alongside
/// every other tick-driven controller (movement, look, interaction, combat)
/// rather than as a command -- see that module's doc comment for the
/// detection/execution pipeline.
#[derive(Clone, Debug, Deserialize)]
pub struct SurvivalConfig {
    #[serde(default = "default_true")]
    pub water_mlg_enabled: bool,
    /// Predicted total fall distance (blocks) at which the bot arms the
    /// clutch. Vanilla's own safe-fall distance is 3 blocks; this defaults
    /// a bit higher to leave margin for prediction error.
    #[serde(default = "default_min_fall_distance")]
    pub min_fall_distance: f64,
    /// How many blocks above the predicted landing surface water is placed
    /// -- "approximately 2-3 blocks before the player reaches the ground",
    /// not the instant a landing block is first detected. Widened
    /// automatically for the current fall speed
    /// (`placement_latency_compensation_ms`) and for an unstable/still-
    /// drifting prediction -- see
    /// `survival::prediction::effective_placement_offset`.
    #[serde(default = "default_placement_offset_blocks")]
    pub placement_offset_blocks: f64,
    /// Round-trip network latency + server tick-processing delay to
    /// compensate for, in milliseconds: converted into extra placement lead
    /// distance scaled by the bot's current fall speed (see
    /// `survival::prediction::latency_compensation_blocks`), so a placement
    /// packet sent now has time to actually reach and take effect on the
    /// server before impact.
    #[serde(default = "default_placement_latency_compensation_ms")]
    pub placement_latency_compensation_ms: u64,
    #[serde(default = "default_true")]
    pub pickup_after_landing: bool,
    /// Water evaporates in the Nether, so placing it there is pointless (and
    /// wastes the bucket slot mid-fall) -- disabled by default.
    #[serde(default = "default_true")]
    pub disable_in_nether: bool,
}
fn default_min_fall_distance() -> f64 {
    4.0
}
fn default_placement_offset_blocks() -> f64 {
    2.5
}
fn default_placement_latency_compensation_ms() -> u64 {
    100
}
impl Default for SurvivalConfig {
    fn default() -> Self {
        Self {
            water_mlg_enabled: true,
            min_fall_distance: default_min_fall_distance(),
            placement_offset_blocks: default_placement_offset_blocks(),
            placement_latency_compensation_ms: default_placement_latency_compensation_ms(),
            pickup_after_landing: true,
            disable_in_nether: true,
        }
    }
}

/// Standalone PvP (`#kill`/`/kill`, `crate::combat`) tuning -- entirely
/// independent of `MovementConfig`/`BlockNavigationConfig` above, since
/// combat drives its own raw movement rather than the pathfinder (see
/// `crate::combat`'s module doc comment).
#[derive(Clone, Debug, Deserialize)]
pub struct KillbotConfig {
    /// Vanilla melee reach: past this, the bot doesn't even attempt to
    /// swing regardless of cooldown.
    #[serde(default = "default_killbot_attack_range")]
    pub attack_range: f64,
    /// Milliseconds between two attacks, or **0 for automatic** (the
    /// default): swing exactly when the weapon actually in hand finishes
    /// recharging -- 625ms for any sword, 1000ms for a netherite axe, and so
    /// on (see `combat::crits::weapon_cooldown`).
    ///
    /// Automatic is the maximum-damage setting, and there is real arithmetic
    /// behind that rather than a preference: Java scales each hit by
    /// `0.2 + 0.8 * (t/T)^2`, so hitting at half charge deals 40% damage at
    /// twice the rate -- 0.8x the damage per second of simply waiting. An
    /// explicit value here overrides that curve in both directions, and any
    /// value below the held weapon's real recharge time costs damage.
    ///
    /// Governs both `#kill` (PvP) and `#get <mob>` (mob combat): it is the
    /// same swing, on the same weapon, against the same vanilla cooldown, so
    /// there is one knob rather than two that would only ever be set to the
    /// same number.
    #[serde(default = "default_killbot_attack_cooldown_ms")]
    pub attack_cooldown_ms: u64,
    /// The distance the bot holds and circles at once it has closed in
    /// (see `combat::movement`'s doc comment for the full distance policy
    /// this anchors -- it only ever closes toward this, never backs out to
    /// it).
    #[serde(default = "default_killbot_preferred_range")]
    pub preferred_range: f64,
    #[serde(default = "default_true")]
    pub crit_enabled: bool,
    /// Never take a sword swing flat-footed: if the attack cooldown comes up
    /// while the bot is standing on the ground, jump and hold the hit until
    /// it lands as a critical (1.5x damage) instead of swinging immediately.
    ///
    /// Costs about a third of a second on the hits where it applies and pays
    /// for it several times over in damage. Bounded, so a bot that cannot
    /// jump (a low ceiling, a one-block tunnel) swings normally after a
    /// moment rather than freezing -- see `combat::crits::CRIT_HOLD_TIMEOUT`.
    /// Sword-only: an axe's slower cooldown already leaves room for the
    /// ordinary crit jump. Requires `crit_enabled`.
    #[serde(default = "default_true")]
    pub always_crit_with_sword: bool,
    #[serde(default = "default_true")]
    pub shield_break_enabled: bool,
    #[serde(default = "default_true")]
    pub shield_use_enabled: bool,
    /// Health (HP, 0-20) below which the bot eats -- *without* disengaging:
    /// it keeps chasing and hitting the target the whole time (see
    /// `combat::executor`'s module doc comment). Nothing about this
    /// threshold ever makes the bot retreat or stop fighting.
    ///
    /// Exactly one bite is taken per trip below the threshold; the bot then
    /// commits to the fight for `eat_cooldown_ms` before it will even
    /// consider another. Defaults to four hearts.
    #[serde(default = "default_killbot_heal_threshold")]
    pub heal_threshold: f64,
    /// How long after finishing one bite before the bot will eat again, in
    /// milliseconds. **One bite at a time**: the bot swallows, goes straight
    /// back to fighting, and only reaches for more food once this has
    /// elapsed and it is still hurt.
    ///
    /// Defaults to a golden apple's own Regeneration II duration (5s),
    /// because that is the thing being waited for: a gap heals 4 HP
    /// *gradually*, so health has barely moved the instant it lands.
    /// Without this wait the bot re-checks its health, still sees it low,
    /// and eats again -- burning five apples in ten seconds without
    /// throwing a punch. Set to 0 to chain bites back to back.
    #[serde(default = "default_killbot_eat_cooldown_ms")]
    pub eat_cooldown_ms: u64,
    /// Target health (HP, 0-20) at or below which the bot stops caring about
    /// its own: no eating, just damage until the target is down.
    ///
    /// A finishing rule, and the one case where ignoring `heal_threshold` is
    /// correct -- a target four hearts from death dies to the next couple of
    /// hits, and a bot that stops to eat instead hands them the fight. Set
    /// to 0 to disable and always respect `heal_threshold`.
    #[serde(default = "default_killbot_finisher_health")]
    pub finisher_health: f64,
    #[serde(default = "default_true")]
    pub sprint_reset_enabled: bool,
    #[serde(default = "default_true")]
    pub strafe_enabled: bool,
    #[serde(default = "default_true")]
    pub prediction_enabled: bool,
    /// Safety ceiling on chasing one target, mirroring
    /// `mobs::combat::TARGET_TIMEOUT`'s "don't chase forever" purpose but
    /// expressed as a distance rather than a duration -- past this, the
    /// bot gives up rather than following a fleeing/mounted player across
    /// the whole map.
    #[serde(default = "default_killbot_max_chase_distance")]
    pub max_chase_distance: f64,
}
fn default_killbot_attack_range() -> f64 {
    2.0
}
fn default_killbot_attack_cooldown_ms() -> u64 {
    0
}
fn default_killbot_preferred_range() -> f64 {
    1.8
}
fn default_killbot_heal_threshold() -> f64 {
    8.0
}
fn default_killbot_eat_cooldown_ms() -> u64 {
    5000
}
fn default_killbot_finisher_health() -> f64 {
    8.0
}
fn default_killbot_max_chase_distance() -> f64 {
    128.0
}
impl KillbotConfig {
    /// [`Self::eat_cooldown_ms`] as a `Duration`.
    #[must_use]
    pub fn eat_cooldown(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.eat_cooldown_ms)
    }

    /// [`Self::attack_cooldown_ms`] as an explicit override, or `None` for
    /// the automatic weapon-accurate cadence -- see that field's doc
    /// comment. Callers that have no idea what the bot is holding fall back
    /// to `crate::combat::crits::SWORD_COOLDOWN`.
    #[must_use]
    pub fn attack_cooldown(&self) -> Option<std::time::Duration> {
        (self.attack_cooldown_ms > 0)
            .then(|| std::time::Duration::from_millis(self.attack_cooldown_ms))
    }
}

impl Default for KillbotConfig {
    fn default() -> Self {
        Self {
            attack_range: default_killbot_attack_range(),
            attack_cooldown_ms: default_killbot_attack_cooldown_ms(),
            preferred_range: default_killbot_preferred_range(),
            crit_enabled: true,
            always_crit_with_sword: true,
            shield_break_enabled: true,
            shield_use_enabled: true,
            heal_threshold: default_killbot_heal_threshold(),
            eat_cooldown_ms: default_killbot_eat_cooldown_ms(),
            finisher_health: default_killbot_finisher_health(),
            sprint_reset_enabled: true,
            strafe_enabled: true,
            prediction_enabled: true,
            max_chase_distance: default_killbot_max_chase_distance(),
        }
    }
}

/// Baritone-style hierarchical long-distance navigation -- see
/// `crate::pathfinding`'s module doc comment. This section governs the
/// *planner* (route, segments, searching, caching); the physical execution
/// of each segment still runs through `[movement]` and
/// `[vertical_navigation]`.
#[derive(Clone, Debug, Deserialize)]
pub struct PathfindingConfig {
    /// Whether `/goto <x> <y> <z>` uses the segmented planner for distant
    /// destinations at all. With this off, every `/goto` behaves exactly as
    /// it did before this system existed (one goal, straight to Azalea's
    /// pathfinder) -- the escape hatch if the planner ever misbehaves.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The `[Pathfinder]` narration: route plans, per-segment progress, and
    /// every replan with its reason. Off by default; this is a lot of
    /// output for a long trip.
    #[serde(default)]
    pub debug_pathfinding: bool,
    /// Destinations closer than this (in blocks) skip the planner entirely
    /// and go straight to the movement layer -- segmenting a 40-block walk
    /// only adds latency, and Azalea's own pathfinder is reliable at that
    /// range.
    #[serde(default = "default_long_distance_threshold")]
    pub long_distance_threshold: f64,
    /// Target length of one path slice, in blocks. Shorter means more
    /// frequent (cheaper) searches and more responsive replanning; longer
    /// means fewer, larger searches.
    #[serde(default = "default_segment_length")]
    pub segment_length: f64,
    /// Horizontal half-extent of the terrain sampled around a segment
    /// before searching it. Must comfortably exceed `segment_length` or the
    /// search has no room to route around anything.
    #[serde(default = "default_sample_margin")]
    pub sample_margin: i32,
    /// Vertical half-extent sampled around a segment. The dominant term in
    /// how much memory one sample costs.
    #[serde(default = "default_vertical_window")]
    pub vertical_window: i32,
    /// Node budget for one block-level search.
    #[serde(default = "default_max_search_nodes")]
    pub max_search_nodes: usize,
    /// Wall-clock budget for one block-level search. Runs on a blocking
    /// thread, so this never stalls the bot's own tick loop -- it only
    /// bounds how stale a computed segment can be by the time it lands.
    #[serde(default = "default_search_timeout_ms")]
    pub search_timeout_ms: u64,
    /// How close (in blocks) the bot must get to the final destination to
    /// count as arrived.
    #[serde(default = "default_arrival_radius")]
    pub arrival_radius: f64,
    /// How close the bot must get to a segment's end before moving on to
    /// the next one. Looser than `arrival_radius`: a segment boundary is an
    /// arbitrary point in open terrain, not a place the user asked to be.
    #[serde(default = "default_segment_arrival_radius")]
    pub segment_arrival_radius: f64,
    /// Seconds without measurable progress toward the current segment's end
    /// before it is treated as blocked and recalculated.
    #[serde(default = "default_segment_stuck_seconds")]
    pub segment_stuck_seconds: u64,
    /// How many consecutive failed replans end the task. Each replan
    /// re-samples the world, so a few retries genuinely help (terrain
    /// streams in); an unbounded number just spins.
    #[serde(default = "default_max_consecutive_replans")]
    pub max_consecutive_replans: u32,
    /// Chunks the navigation cache holds before evicting the oldest.
    #[serde(default = "default_cache_capacity")]
    pub cache_capacity: usize,
    /// How many cache writes a chunk summary stays trustworthy for -- an
    /// age in "chunks recorded since", not in seconds, so it scales with how
    /// much the bot is actually moving.
    #[serde(default = "default_cache_max_age")]
    pub cache_max_age: u64,
    /// Whether the planner may route through blocks it would have to mine.
    /// Independent of `[vertical_navigation]`'s build permissions, which
    /// govern what the *executor* does.
    #[serde(default = "default_true")]
    pub allow_breaking: bool,
    /// Whether the planner may route across a gap by placing blocks.
    /// Intersected with `[vertical_navigation]`'s own build permissions
    /// (see `pathfinding::planner::move_policy`), so turning building off
    /// there also stops the planner routing bridges -- these two can't
    /// drift apart into "plans a bridge it will never build".
    #[serde(default = "default_true")]
    pub allow_bridging: bool,
    /// Whether the planner may route through water.
    #[serde(default = "default_true")]
    pub allow_swimming: bool,
    /// Deepest drop the planner will route down.
    #[serde(default = "default_max_drop")]
    pub max_drop: i32,
    #[serde(default)]
    pub costs: PathfindingCostConfig,
}

/// The weights `crate::pathfinding::cost::CostProfile` scores with, in units
/// of "one block of easy walking". See that type's doc comment for what each
/// one means -- they are deliberately one-to-one so tuning is a config edit.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct PathfindingCostConfig {
    #[serde(default = "default_cost_walk")]
    pub walk: f64,
    #[serde(default = "default_cost_diagonal")]
    pub diagonal: f64,
    #[serde(default = "default_cost_jump_up")]
    pub jump_up: f64,
    #[serde(default = "default_cost_drop_per_block")]
    pub drop_per_block: f64,
    #[serde(default = "default_cost_max_safe_drop")]
    pub max_safe_drop: i32,
    #[serde(default = "default_cost_fall_damage_penalty")]
    pub fall_damage_penalty: f64,
    #[serde(default = "default_cost_swim")]
    pub swim: f64,
    #[serde(default = "default_cost_break_block")]
    pub break_block: f64,
    #[serde(default = "default_cost_bridge_block")]
    pub bridge_block: f64,
    #[serde(default = "default_cost_climb")]
    pub climb: f64,
    #[serde(default = "default_cost_hazard")]
    pub hazard: f64,
    #[serde(default = "default_cost_lava_avoidance")]
    pub lava_avoidance: f64,
    #[serde(default = "default_cost_hazard_proximity")]
    pub hazard_proximity: f64,
    #[serde(default = "default_cost_ledge_proximity")]
    pub ledge_proximity: f64,
    #[serde(default = "default_cost_entity_hazard")]
    pub entity_hazard: f64,
}

fn default_long_distance_threshold() -> f64 {
    96.0
}
fn default_segment_length() -> f64 {
    48.0
}
fn default_sample_margin() -> i32 {
    24
}
fn default_vertical_window() -> i32 {
    32
}
fn default_max_search_nodes() -> usize {
    20_000
}
fn default_search_timeout_ms() -> u64 {
    1_500
}
fn default_arrival_radius() -> f64 {
    1.5
}
fn default_segment_arrival_radius() -> f64 {
    3.0
}
fn default_segment_stuck_seconds() -> u64 {
    12
}
fn default_max_consecutive_replans() -> u32 {
    8
}
fn default_cache_capacity() -> usize {
    4_096
}
fn default_cache_max_age() -> u64 {
    8_192
}
fn default_max_drop() -> i32 {
    8
}
fn default_cost_walk() -> f64 {
    1.0
}
fn default_cost_diagonal() -> f64 {
    1.41
}
fn default_cost_jump_up() -> f64 {
    1.8
}
fn default_cost_drop_per_block() -> f64 {
    1.1
}
fn default_cost_max_safe_drop() -> i32 {
    3
}
fn default_cost_fall_damage_penalty() -> f64 {
    20.0
}
fn default_cost_swim() -> f64 {
    3.0
}
fn default_cost_break_block() -> f64 {
    6.0
}
fn default_cost_bridge_block() -> f64 {
    8.0
}
fn default_cost_climb() -> f64 {
    2.5
}
fn default_cost_hazard() -> f64 {
    40.0
}
fn default_cost_lava_avoidance() -> f64 {
    200.0
}
fn default_cost_hazard_proximity() -> f64 {
    6.0
}
fn default_cost_ledge_proximity() -> f64 {
    1.0
}
fn default_cost_entity_hazard() -> f64 {
    25.0
}

impl Default for PathfindingCostConfig {
    fn default() -> Self {
        Self {
            walk: default_cost_walk(),
            diagonal: default_cost_diagonal(),
            jump_up: default_cost_jump_up(),
            drop_per_block: default_cost_drop_per_block(),
            max_safe_drop: default_cost_max_safe_drop(),
            fall_damage_penalty: default_cost_fall_damage_penalty(),
            swim: default_cost_swim(),
            break_block: default_cost_break_block(),
            bridge_block: default_cost_bridge_block(),
            climb: default_cost_climb(),
            hazard: default_cost_hazard(),
            lava_avoidance: default_cost_lava_avoidance(),
            hazard_proximity: default_cost_hazard_proximity(),
            ledge_proximity: default_cost_ledge_proximity(),
            entity_hazard: default_cost_entity_hazard(),
        }
    }
}

impl Default for PathfindingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debug_pathfinding: false,
            long_distance_threshold: default_long_distance_threshold(),
            segment_length: default_segment_length(),
            sample_margin: default_sample_margin(),
            vertical_window: default_vertical_window(),
            max_search_nodes: default_max_search_nodes(),
            search_timeout_ms: default_search_timeout_ms(),
            arrival_radius: default_arrival_radius(),
            segment_arrival_radius: default_segment_arrival_radius(),
            segment_stuck_seconds: default_segment_stuck_seconds(),
            max_consecutive_replans: default_max_consecutive_replans(),
            cache_capacity: default_cache_capacity(),
            cache_max_age: default_cache_max_age(),
            allow_breaking: true,
            allow_bridging: true,
            allow_swimming: true,
            max_drop: default_max_drop(),
            costs: PathfindingCostConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovementConfig {
    #[serde(default = "default_follow_distance")]
    pub follow_distance: f64,
    #[serde(default = "default_repath_interval_ms")]
    pub repath_interval_ms: u64,
    #[serde(default = "default_arrival_distance")]
    pub arrival_distance: f64,
    /// How long `App::await_movement_terminal` (backing `/goto`,
    /// `/goto-mine`, `/follow`, and every other direct caller awaiting
    /// movement) will tolerate the bot's distance to its destination not
    /// improving before giving up -- much shorter than
    /// `maximum_navigation_seconds` below, the same way
    /// `BlockNavigationConfig::stuck_timeout_seconds` relates to that
    /// config's own `maximum_navigation_seconds`. This is what actually
    /// catches a genuinely unreachable goal (Azalea's pathfinder is
    /// submitted with `retry_on_no_path(true)`, so it retries forever
    /// without ever reporting failure on its own -- but a goal it truly
    /// cannot reach also never gets any closer, so this notices quickly);
    /// a bot that keeps making real progress, however slowly, never trips
    /// this at all, no matter how long the whole trip ends up taking.
    #[serde(default = "default_stuck_timeout_seconds")]
    pub stuck_timeout_seconds: u64,
    /// Absolute backstop on top of `stuck_timeout_seconds` above, for the
    /// pathological case of a route that keeps inching closer forever
    /// without ever actually arriving. Deliberately generous -- unlike
    /// `stuck_timeout_seconds`, this is not expected to fire during normal
    /// use (a long-distance `/goto` or a multi-minute bridge build is
    /// still making progress the whole time, so `stuck_timeout_seconds`
    /// alone governs those) -- it exists only so movement commands run on
    /// the same single-threaded console loop as every other command can
    /// never freeze the whole app forever.
    #[serde(default = "default_movement_maximum_navigation_seconds")]
    pub maximum_navigation_seconds: u64,
}

/// Policy for the pathfinding engine's terrain-modifying movement
/// primitives (pillaring, bridging, staircasing, safe digging-down). These
/// are real A* graph edges inside the pathfinder itself (see
/// `azalea::pathfinder::moves::build` and `azalea::pathfinder::policy`), not
/// command-layer logic -- every `/goto`, `/follow`, and task automatically
/// inherits them. Enabled by default. Building material is a deny-list, not
/// an allow-list (see `crate::bridging`): the bot may use any placeable
/// block it holds except `crate::bridging::SCAFFOLD_BLACKLIST`,
/// `denied_building_blocks` below, and a built-in hard denylist inside the
/// pathfinder engine itself (tools, ores, valuables) -- so it never
/// consumes or places anything unexpected.
#[derive(Clone, Debug, Deserialize)]
pub struct VerticalNavigationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub allow_pillaring: bool,
    #[serde(default = "default_true")]
    pub allow_digging_down: bool,
    #[serde(default = "default_true")]
    pub allow_bridging: bool,
    #[serde(default = "default_true")]
    pub prefer_staircase_descent: bool,
    #[serde(default = "default_vertical_pillar_height")]
    pub max_pillar_height: u32,
    #[serde(default = "default_vertical_dig_depth")]
    pub max_dig_depth: u32,
    #[serde(default = "default_minimum_building_blocks")]
    pub minimum_building_blocks: u32,
    /// Lowest Y coordinate the bot will ever deliberately target: block
    /// search results below this are dropped before they can ever become a
    /// mining/navigation target (`MinecraftClient::scan_loaded_blocks`), and
    /// `minecraft:bedrock` is never a valid target regardless of Y (see
    /// `interaction::placement_rules::is_unbreakable`). Defaults to -59,
    /// just above the Overworld's uneven natural-bedrock band
    /// (Y -64 to roughly -60) in 1.18+ -- low enough to reach ore right down
    /// to the floor without the bot wasting time trying to dig or path
    /// through terrain that's frequently unbreakable bedrock.
    #[serde(default = "default_minimum_y")]
    pub minimum_y: i32,
    /// Extra blocks to deny beyond `crate::bridging::SCAFFOLD_BLACKLIST`
    /// (the built-in blacklist covering containers, valuables, redstone,
    /// decorative blocks, crops, etc.). Building material is otherwise a
    /// deny-list, not an allow-list: the bot may use any placeable block it
    /// holds except what's blacklisted here or in the built-in list.
    #[serde(default = "default_denied_building_blocks")]
    pub denied_building_blocks: Vec<String>,
}
fn default_vertical_pillar_height() -> u32 {
    32
}
fn default_vertical_dig_depth() -> u32 {
    64
}
fn default_minimum_building_blocks() -> u32 {
    4
}
fn default_minimum_y() -> i32 {
    -59
}
fn default_denied_building_blocks() -> Vec<String> {
    [
        "minecraft:diamond_block",
        "minecraft:chest",
        "minecraft:crafting_table",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
impl Default for VerticalNavigationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_pillaring: true,
            allow_digging_down: true,
            allow_bridging: true,
            prefer_staircase_descent: true,
            max_pillar_height: default_vertical_pillar_height(),
            max_dig_depth: default_vertical_dig_depth(),
            minimum_building_blocks: default_minimum_building_blocks(),
            minimum_y: default_minimum_y(),
            denied_building_blocks: default_denied_building_blocks(),
        }
    }
}

/// Controls Baritone-style "fast bridging" (speed bridging): walking
/// normally across each placed block and only sneaking for the brief
/// moment needed to place the next one near the edge, instead of sneaking
/// for the entire approach. This is the default technique the pathfinder's
/// bridge move primitive uses whenever it decides a bridge must be built --
/// see `azalea::pathfinder::moves::build::execute_fast_bridge_move` -- for
/// every `/goto`, `/follow`, and task that crosses a gap, not a
/// command-specific implementation.
#[derive(Clone, Debug, Deserialize)]
pub struct BridgingConfig {
    /// `false` falls back to the original permanent-sneak bridging
    /// technique.
    #[serde(default = "default_true")]
    pub fast_bridge_enabled: bool,
    /// How close to a block's edge (in blocks) the bot must be before it
    /// starts sneaking to place the next bridge block. Must be wide enough
    /// that a single tick's normal-speed walking step can't skip clean over
    /// the detection window (see
    /// `azalea::pathfinder::moves::build::FAST_BRIDGE_EDGE_THRESHOLD_FALLBACK`'s
    /// doc comment) -- 0.15-0.25 is a reasonable range.
    #[serde(default = "default_edge_threshold")]
    pub edge_threshold: f64,
}
fn default_edge_threshold() -> f64 {
    0.18
}
impl Default for BridgingConfig {
    fn default() -> Self {
        Self {
            fast_bridge_enabled: true,
            edge_threshold: default_edge_threshold(),
        }
    }
}

/// Angles used by the movement adapter when pathfinding direction and camera
/// direction differ.  Keeping these in configuration prevents command code
/// from hardcoding a particular movement style.
#[derive(Clone, Debug, Deserialize)]
pub struct MultitaskingConfig {
    #[serde(default = "default_normal_forward_angle")]
    pub normal_forward_angle: f32,
    #[serde(default = "default_strafe_angle")]
    pub strafe_angle: f32,
    #[serde(default = "default_backward_angle")]
    pub backward_angle: f32,
    #[serde(default = "default_extreme_angle")]
    pub extreme_angle: f32,
}

fn default_normal_forward_angle() -> f32 {
    35.0
}
fn default_strafe_angle() -> f32 {
    75.0
}
fn default_backward_angle() -> f32 {
    130.0
}
fn default_extreme_angle() -> f32 {
    160.0
}

impl Default for MultitaskingConfig {
    fn default() -> Self {
        Self {
            normal_forward_angle: default_normal_forward_angle(),
            strafe_angle: default_strafe_angle(),
            backward_angle: default_backward_angle(),
            extreme_angle: default_extreme_angle(),
        }
    }
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
    10
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
            maximum_target_attempts: 10,
            stuck_timeout_seconds: 12,
            maximum_navigation_seconds: 120,
            repath_interval_ms: 1000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LookConfig {
    #[serde(default = "default_look_update_rate")]
    pub update_rate: u32,
    #[serde(default = "default_reaction_delay_min_ms")]
    pub reaction_delay_min_ms: u64,
    #[serde(default = "default_reaction_delay_max_ms")]
    pub reaction_delay_max_ms: u64,
    #[serde(default = "default_true")]
    pub moving_target_prediction: bool,
    #[serde(default = "default_prediction_strength")]
    pub prediction_strength: f64,
    #[serde(default = "default_minimum_target_movement")]
    pub minimum_target_movement: f64,
    #[serde(default)]
    pub randomization: LookRandomizationConfig,
    #[serde(default)]
    pub motion: LookMotionConfig,
}

fn default_look_update_rate() -> u32 {
    30
}
fn default_reaction_delay_min_ms() -> u64 {
    10
}
fn default_reaction_delay_max_ms() -> u64 {
    28
}
fn default_prediction_strength() -> f64 {
    0.35
}
fn default_minimum_target_movement() -> f64 {
    0.03
}

#[derive(Clone, Debug, Deserialize)]
pub struct LookRandomizationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub block_randomization: bool,
    #[serde(default = "default_true")]
    pub entity_randomization: bool,
    #[serde(default = "default_true")]
    pub player_randomization: bool,
    #[serde(default = "default_horizontal_strength")]
    pub horizontal_strength: f64,
    #[serde(default = "default_vertical_strength")]
    pub vertical_strength: f64,
    #[serde(default = "default_minimum_hold_time_ms")]
    pub minimum_hold_time_ms: u64,
    #[serde(default = "default_maximum_hold_time_ms")]
    pub maximum_hold_time_ms: u64,
    #[serde(default = "default_retarget_chance")]
    pub retarget_chance_per_second: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LookMotionConfig {
    #[serde(default = "default_minimum_yaw_speed")]
    pub minimum_yaw_speed: f64,
    #[serde(default = "default_maximum_yaw_speed")]
    pub maximum_yaw_speed: f64,
    #[serde(default = "default_minimum_pitch_speed")]
    pub minimum_pitch_speed: f64,
    #[serde(default = "default_maximum_pitch_speed")]
    pub maximum_pitch_speed: f64,
    #[serde(default = "default_yaw_acceleration")]
    pub yaw_acceleration: f64,
    #[serde(default = "default_pitch_acceleration")]
    pub pitch_acceleration: f64,
    #[serde(default = "default_yaw_deceleration")]
    pub yaw_deceleration: f64,
    #[serde(default = "default_pitch_deceleration")]
    pub pitch_deceleration: f64,
    #[serde(default = "default_slowdown_angle")]
    pub slowdown_angle: f64,
    #[serde(default = "default_look_arrival_tolerance")]
    pub arrival_tolerance: f64,
    #[serde(default = "default_true")]
    pub micro_correction_enabled: bool,
    #[serde(default = "default_micro_correction_strength")]
    pub micro_correction_strength: f64,
    #[serde(default = "default_speed_variation")]
    pub speed_variation: f64,
    #[serde(default = "default_overshoot_chance")]
    pub overshoot_chance: f64,
    #[serde(default = "default_maximum_overshoot_degrees")]
    pub maximum_overshoot_degrees: f64,
}

fn default_true() -> bool {
    true
}
fn default_horizontal_strength() -> f64 {
    0.35
}
fn default_vertical_strength() -> f64 {
    0.30
}
fn default_minimum_hold_time_ms() -> u64 {
    350
}
fn default_maximum_hold_time_ms() -> u64 {
    1200
}
fn default_retarget_chance() -> f64 {
    0.35
}
// Aim speeds are doubled relative to the original "human-like" profile, and
// the accelerations are quadrupled to match. That pairing is what makes the
// aim genuinely twice as fast rather than only twice as fast on long sweeps:
// a turn short enough to never reach top speed is limited by acceleration
// alone (`d = a*t^2/2`), so halving *its* time needs 4x the acceleration,
// while a long turn is limited by top speed and needs 2x. Scaling both
// together halves the time in either regime.
fn default_minimum_yaw_speed() -> f64 {
    120.0
}
fn default_maximum_yaw_speed() -> f64 {
    960.0
}
fn default_minimum_pitch_speed() -> f64 {
    100.0
}
fn default_maximum_pitch_speed() -> f64 {
    760.0
}
fn default_yaw_acceleration() -> f64 {
    8800.0
}
fn default_pitch_acceleration() -> f64 {
    7200.0
}
fn default_yaw_deceleration() -> f64 {
    10400.0
}
fn default_pitch_deceleration() -> f64 {
    8400.0
}
fn default_slowdown_angle() -> f64 {
    14.0
}
fn default_look_arrival_tolerance() -> f64 {
    1.2
}
fn default_micro_correction_strength() -> f64 {
    0.35
}
fn default_speed_variation() -> f64 {
    0.12
}
fn default_overshoot_chance() -> f64 {
    0.10
}
fn default_maximum_overshoot_degrees() -> f64 {
    2.2
}

impl Default for LookConfig {
    fn default() -> Self {
        // Every field defers to its `default_*` function rather than
        // restating the number: the two had already drifted apart once, so a
        // `LookConfig::default()` could disagree with the same config
        // deserialized from an empty table.
        Self {
            update_rate: default_look_update_rate(),
            reaction_delay_min_ms: default_reaction_delay_min_ms(),
            reaction_delay_max_ms: default_reaction_delay_max_ms(),
            moving_target_prediction: true,
            prediction_strength: default_prediction_strength(),
            minimum_target_movement: default_minimum_target_movement(),
            randomization: LookRandomizationConfig::default(),
            motion: LookMotionConfig::default(),
        }
    }
}

impl Default for LookRandomizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_randomization: true,
            entity_randomization: true,
            player_randomization: true,
            horizontal_strength: 0.35,
            vertical_strength: 0.30,
            minimum_hold_time_ms: 350,
            maximum_hold_time_ms: 1200,
            retarget_chance_per_second: 0.35,
        }
    }
}

impl Default for LookMotionConfig {
    fn default() -> Self {
        Self {
            minimum_yaw_speed: default_minimum_yaw_speed(),
            maximum_yaw_speed: default_maximum_yaw_speed(),
            minimum_pitch_speed: default_minimum_pitch_speed(),
            maximum_pitch_speed: default_maximum_pitch_speed(),
            yaw_acceleration: default_yaw_acceleration(),
            pitch_acceleration: default_pitch_acceleration(),
            yaw_deceleration: default_yaw_deceleration(),
            pitch_deceleration: default_pitch_deceleration(),
            slowdown_angle: 14.0,
            arrival_tolerance: 1.2,
            micro_correction_enabled: true,
            micro_correction_strength: 0.35,
            speed_variation: 0.12,
            overshoot_chance: 0.10,
            maximum_overshoot_degrees: 2.2,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct InteractionConfig {
    #[serde(default = "default_interaction_reach")]
    pub maximum_reach: f64,
    #[serde(default = "default_interaction_reach")]
    pub placement_reach: f64,
    #[serde(default = "default_interaction_reach")]
    pub breaking_reach: f64,
    #[serde(default = "default_interaction_retry_limit")]
    pub retry_limit: u32,
    #[serde(default = "default_interaction_retry_delay")]
    pub retry_delay_ms: u64,
    #[serde(default = "default_interaction_verification_timeout")]
    pub verification_timeout_ms: u64,
    #[serde(default = "default_true")]
    pub auto_navigate: bool,
    #[serde(default = "default_true")]
    pub auto_precise_look: bool,
    /// Select the fastest suitable tool already present in the hotbar before breaking.
    #[serde(default = "default_true")]
    pub auto_tool_switch: bool,
    /// Never intentionally spend the last uses of a damageable tool.
    #[serde(default = "default_minimum_tool_durability")]
    pub minimum_tool_durability: u32,
    /// Permit the held item/hand when the block does not require a tool.
    #[serde(default = "default_true")]
    pub allow_hand_fallback: bool,
    /// Fractional speed difference within which the held tool is preferred.
    #[serde(default = "default_held_tool_equivalence")]
    pub held_tool_equivalence: f32,
    /// Item identifiers excluded from automatic tool selection.
    #[serde(default)]
    pub protected_tools: Vec<String>,
    /// Item identifiers temporarily reserved for another purpose.
    #[serde(default)]
    pub reserved_tools: Vec<String>,
    #[serde(default)]
    pub face_targeting: FaceTargetingConfig,
}
#[derive(Clone, Debug, Deserialize)]
pub struct FaceTargetingConfig {
    #[serde(default = "default_face_inset")]
    pub face_inset: f64,
    #[serde(default = "default_edge_margin")]
    pub edge_margin: f64,
    /// How many of a block's (up to 6) faces `InteractionController` tries
    /// before giving up and asking `BlockNavigationService` for a different
    /// approach position entirely (`InteractionController::recover_break_raycast`).
    /// Each face attempt re-aims the camera, so
    /// `maximum_face_attempts * maximum_hit_points_per_face` is roughly how
    /// many times the bot's look direction can visibly change while working
    /// out a clear line of sight on one block -- kept low by default so an
    /// obstructed view is resolved (or handed off to repositioning) quickly
    /// rather than exhaustively.
    #[serde(default = "default_face_attempts")]
    pub maximum_face_attempts: usize,
    #[serde(default = "default_hit_points_per_face")]
    pub maximum_hit_points_per_face: usize,
}
fn default_face_inset() -> f64 {
    0.001
}
fn default_edge_margin() -> f64 {
    0.12
}
fn default_face_attempts() -> usize {
    3
}
fn default_hit_points_per_face() -> usize {
    2
}
impl Default for FaceTargetingConfig {
    fn default() -> Self {
        Self {
            face_inset: default_face_inset(),
            edge_margin: default_edge_margin(),
            maximum_face_attempts: default_face_attempts(),
            maximum_hit_points_per_face: default_hit_points_per_face(),
        }
    }
}
fn default_interaction_reach() -> f64 {
    4.5
}
fn default_interaction_retry_limit() -> u32 {
    3
}
fn default_interaction_retry_delay() -> u64 {
    150
}
fn default_interaction_verification_timeout() -> u64 {
    1500
}
fn default_minimum_tool_durability() -> u32 {
    2
}
fn default_held_tool_equivalence() -> f32 {
    0.10
}
impl Default for InteractionConfig {
    fn default() -> Self {
        Self {
            maximum_reach: 4.5,
            placement_reach: 4.5,
            breaking_reach: 4.5,
            retry_limit: 3,
            retry_delay_ms: 150,
            verification_timeout_ms: 1500,
            auto_navigate: true,
            auto_precise_look: true,
            auto_tool_switch: true,
            minimum_tool_durability: default_minimum_tool_durability(),
            allow_hand_fallback: true,
            held_tool_equivalence: default_held_tool_equivalence(),
            protected_tools: Vec::new(),
            reserved_tools: Vec::new(),
            face_targeting: FaceTargetingConfig::default(),
        }
    }
}
fn default_follow_distance() -> f64 {
    3.0
}
fn default_repath_interval_ms() -> u64 {
    150
}
fn default_arrival_distance() -> f64 {
    1.5
}
impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            follow_distance: 3.0,
            repath_interval_ms: 150,
            arrival_distance: 1.5,
            stuck_timeout_seconds: default_stuck_timeout_seconds(),
            maximum_navigation_seconds: default_movement_maximum_navigation_seconds(),
        }
    }
}
fn default_movement_maximum_navigation_seconds() -> u64 {
    900
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
        if self.movement.stuck_timeout_seconds == 0 || self.movement.stuck_timeout_seconds > 3600 {
            return Err(AppError::InvalidMovementConfiguration(
                "stuck_timeout_seconds must be between 1 and 3600".into(),
            ));
        }
        if self.movement.maximum_navigation_seconds == 0
            || self.movement.maximum_navigation_seconds > 3600
            || self.movement.maximum_navigation_seconds < self.movement.stuck_timeout_seconds
        {
            return Err(AppError::InvalidMovementConfiguration(
                "maximum_navigation_seconds must be between 1 and 3600, and at least stuck_timeout_seconds".into(),
            ));
        }
        let multitasking = &self.multitasking;
        if !(0.0 < multitasking.normal_forward_angle
            && multitasking.normal_forward_angle <= multitasking.strafe_angle
            && multitasking.strafe_angle <= multitasking.backward_angle
            && multitasking.backward_angle <= multitasking.extreme_angle
            && multitasking.extreme_angle <= 180.0)
        {
            return Err(AppError::InvalidMovementConfiguration(
                "multitasking angles must be ordered between 0 and 180 degrees".into(),
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
        let randomization = &self.look.randomization;
        let motion = &self.look.motion;
        if !(1..=120).contains(&self.look.update_rate)
            || self.look.reaction_delay_max_ms < self.look.reaction_delay_min_ms
            || !(0.0..=1.0).contains(&self.look.prediction_strength)
            || !(self.look.minimum_target_movement >= 0.0
                && self.look.minimum_target_movement <= 4.0)
            || !(0.0..=1.0).contains(&randomization.horizontal_strength)
            || !(0.0..=1.0).contains(&randomization.vertical_strength)
            || randomization.minimum_hold_time_ms == 0
            || randomization.maximum_hold_time_ms < randomization.minimum_hold_time_ms
            || !(0.0..=1.0).contains(&randomization.retarget_chance_per_second)
            // Ceilings sized for the doubled aim profile and a little
            // headroom above it: 1440 deg/s is four full turns a second,
            // past anything a hand on a mouse produces for longer than a
            // flick, and 20000 deg/s^2 reaches that top speed in ~70ms.
            || !(motion.minimum_yaw_speed > 0.0
                && motion.minimum_yaw_speed <= motion.maximum_yaw_speed
                && motion.maximum_yaw_speed <= 1440.0)
            || !(motion.minimum_pitch_speed > 0.0
                && motion.minimum_pitch_speed <= motion.maximum_pitch_speed
                && motion.maximum_pitch_speed <= 1440.0)
            || !(motion.yaw_acceleration > 0.0 && motion.yaw_acceleration <= 20_000.0)
            || !(motion.pitch_acceleration > 0.0 && motion.pitch_acceleration <= 20_000.0)
            || !(motion.yaw_deceleration > 0.0 && motion.yaw_deceleration <= 20_000.0)
            || !(motion.pitch_deceleration > 0.0 && motion.pitch_deceleration <= 20_000.0)
            || !(motion.slowdown_angle > 0.0 && motion.slowdown_angle <= 180.0)
            || !(motion.arrival_tolerance > 0.0 && motion.arrival_tolerance <= 45.0)
            || !(0.0..=1.0).contains(&motion.micro_correction_strength)
            || !(0.0..=0.5).contains(&motion.speed_variation)
            || !(0.0..=1.0).contains(&motion.overshoot_chance)
            || !(0.0..=10.0).contains(&motion.maximum_overshoot_degrees)
        {
            return Err(AppError::InvalidLookConfiguration(
                "look randomization or motion configuration is invalid".into(),
            ));
        }
        let interaction = &self.interaction;
        if !(interaction.maximum_reach > 0.0 && interaction.maximum_reach <= 10.0)
            || !(interaction.placement_reach > 0.0
                && interaction.placement_reach <= interaction.maximum_reach)
            || !(interaction.breaking_reach > 0.0
                && interaction.breaking_reach <= interaction.maximum_reach)
            || interaction.retry_limit > 10
            || interaction.retry_delay_ms == 0
            || interaction.retry_delay_ms > 10_000
            || interaction.verification_timeout_ms == 0
            || interaction.verification_timeout_ms > 30_000
            || !(interaction.face_targeting.face_inset > 0.0
                && interaction.face_targeting.face_inset < 0.5)
            || !(interaction.face_targeting.edge_margin > interaction.face_targeting.face_inset
                && interaction.face_targeting.edge_margin < 0.5)
            || interaction.face_targeting.maximum_face_attempts == 0
            // A block cube only has 6 faces; anything beyond that just
            // wraps around and re-tries an already-failed face for no
            // benefit (see `break_face_order`'s fixed 6-entry table).
            || interaction.face_targeting.maximum_face_attempts > 6
            || interaction.face_targeting.maximum_hit_points_per_face == 0
            || interaction.face_targeting.maximum_hit_points_per_face > 5
            || !interaction.held_tool_equivalence.is_finite()
            || !(0.0..=1.0).contains(&interaction.held_tool_equivalence)
        {
            return Err(AppError::InvalidInteractionConfiguration(
                "reach, retry, or verification values are invalid".into(),
            ));
        }
        let survival = &self.survival;
        if !(survival.min_fall_distance > 0.0 && survival.min_fall_distance <= 200.0)
            || !(survival.placement_offset_blocks > 0.0 && survival.placement_offset_blocks <= 10.0)
            || survival.placement_latency_compensation_ms > 2000
        {
            return Err(AppError::InvalidSurvivalConfiguration(
                "min_fall_distance, placement_offset_blocks, or placement_latency_compensation_ms is out of range".into(),
            ));
        }
        let killbot = &self.killbot;
        if !(killbot.attack_range > 0.0 && killbot.attack_range <= 6.0)
            || !(killbot.preferred_range > 0.0 && killbot.preferred_range <= killbot.attack_range)
            || !(0.0..=20.0).contains(&killbot.heal_threshold)
            || !(0.0..=20.0).contains(&killbot.finisher_health)
            || !(killbot.max_chase_distance > 0.0 && killbot.max_chase_distance <= 1024.0)
            || !(killbot.attack_cooldown_ms == 0
                || (50..=10_000).contains(&killbot.attack_cooldown_ms))
            || killbot.eat_cooldown_ms > 60_000
        {
            return Err(AppError::InvalidKillbotConfiguration(
                "attack_range, attack_cooldown_ms, preferred_range, heal_threshold, finisher_health, or max_chase_distance is out of range".into(),
            ));
        }
        let pathfinding = &self.pathfinding;
        if !(pathfinding.segment_length >= 8.0 && pathfinding.segment_length <= 512.0)
            || !(pathfinding.long_distance_threshold >= 0.0
                && pathfinding.long_distance_threshold <= 4096.0)
            || !(1..=256).contains(&pathfinding.sample_margin)
            || !(1..=384).contains(&pathfinding.vertical_window)
            || !(64..=5_000_000).contains(&pathfinding.max_search_nodes)
            || !(10..=60_000).contains(&pathfinding.search_timeout_ms)
            || !(pathfinding.arrival_radius > 0.0 && pathfinding.arrival_radius <= 64.0)
            || !(pathfinding.segment_arrival_radius > 0.0
                && pathfinding.segment_arrival_radius <= 64.0)
            || !(1..=3600).contains(&pathfinding.segment_stuck_seconds)
            || !(0..=1024).contains(&pathfinding.max_drop)
            || pathfinding.cache_capacity == 0
            || pathfinding.cache_max_age == 0
        {
            return Err(AppError::InvalidPathfindingConfiguration(
                "segment_length, long_distance_threshold, sample_margin, vertical_window, \
                 max_search_nodes, search_timeout_ms, arrival_radius, segment_arrival_radius, \
                 segment_stuck_seconds, max_drop, cache_capacity, or cache_max_age is out of range"
                    .into(),
            ));
        }
        // The sampled corridor has to be wider than the segment it contains,
        // or every search starts hard against the edge of what was sampled
        // and can never route around anything.
        if f64::from(pathfinding.sample_margin) < pathfinding.segment_length * 0.25 {
            return Err(AppError::InvalidPathfindingConfiguration(
                "sample_margin must be at least a quarter of segment_length".into(),
            ));
        }
        let costs = &pathfinding.costs;
        let weights = [
            costs.walk,
            costs.diagonal,
            costs.jump_up,
            costs.drop_per_block,
            costs.fall_damage_penalty,
            costs.swim,
            costs.break_block,
            costs.bridge_block,
            costs.climb,
            costs.hazard,
            costs.lava_avoidance,
            costs.hazard_proximity,
            costs.ledge_proximity,
            costs.entity_hazard,
        ];
        // A non-positive or non-finite weight breaks A*'s termination
        // guarantee outright (a zero-cost cycle is an infinite loop), so this
        // is a hard error rather than a clamp.
        if weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight <= 0.0)
            || !(0..=1024).contains(&costs.max_safe_drop)
        {
            return Err(AppError::InvalidPathfindingConfiguration(
                "every pathfinding cost weight must be finite and greater than zero".into(),
            ));
        }
        if !(-2032..=2032).contains(&self.vertical_navigation.minimum_y) {
            return Err(AppError::InvalidVerticalNavigationConfiguration(
                "minimum_y must be within the technical world height limits (-2032..=2032)".into(),
            ));
        }
        let hotbar = &self.equipment.hotbar;
        if hotbar.periodic_scan_seconds == 0 || hotbar.periodic_scan_seconds > 3600 {
            return Err(AppError::InvalidEquipmentConfiguration(
                "hotbar.periodic_scan_seconds must be between 1 and 3600".into(),
            ));
        }
        let configured_slots: Vec<u8> = [
            hotbar.slots.sword,
            hotbar.slots.pickaxe,
            hotbar.slots.axe,
            hotbar.slots.shovel,
            hotbar.slots.blocks,
            hotbar.slots.water_bucket,
        ]
        .into_iter()
        .flatten()
        .chain(hotbar.utility_items.iter().map(|item| item.slot))
        .collect();
        if configured_slots.iter().any(|slot| !(1..=9).contains(slot)) {
            return Err(AppError::InvalidEquipmentConfiguration(
                "hotbar slots must be between 1 and 9".into(),
            ));
        }
        let mut seen_slots = std::collections::HashSet::new();
        if !configured_slots.iter().all(|slot| seen_slots.insert(*slot)) {
            return Err(AppError::InvalidEquipmentConfiguration(
                "hotbar slots must not overlap".into(),
            ));
        }
        for item in &hotbar.utility_items {
            crate::items::validate_item_id(&item.item_id).map_err(|_| {
                AppError::InvalidEquipmentConfiguration(format!(
                    "hotbar.utility_items has an unknown item id: {}",
                    item.item_id
                ))
            })?;
        }
        for protected in &self.equipment.autodrop.protected_items {
            crate::items::validate_item_id(protected).map_err(|_| {
                AppError::InvalidEquipmentConfiguration(format!(
                    "autodrop.protected_items has an unknown item id: {protected}"
                ))
            })?;
        }
        let chat_commands = &self.chat_commands;
        if (chat_commands.rate_limit.enabled
            && (chat_commands.rate_limit.requests == 0
                || chat_commands.rate_limit.window_seconds == 0))
            || chat_commands.access.allowed_players.iter().any(|allowed| {
                chat_commands
                    .access
                    .blocked_players
                    .iter()
                    .any(|blocked| allowed.eq_ignore_ascii_case(blocked))
            })
        {
            return Err(AppError::InvalidConfiguration(
                "chat_commands rate limit or access lists are invalid".into(),
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
        assert_eq!(config.block_navigation.maximum_target_attempts, 10);
        assert_eq!(config.look.motion.maximum_yaw_speed, 960.0);
        assert_eq!(config.look.update_rate, 30);
        assert!(config.look.randomization.enabled);
        assert_eq!(config.look.reaction_delay_min_ms, 10);
        assert!(config.look.moving_target_prediction);
        assert_eq!(config.interaction.maximum_reach, 4.5);
        assert_eq!(config.multitasking.normal_forward_angle, 35.0);
        assert_eq!(config.multitasking.extreme_angle, 160.0);
        assert_eq!(config.equipment.armor.mode, ArmorMode::Score);
        assert_eq!(config.equipment.offhand.priority, OffhandPriority::Totem);
        assert_eq!(config.output.console, OutputMode::Info);
        assert_eq!(config.output.chat, OutputMode::Info);
        assert_eq!(config.killbot.attack_range, 2.0);
        assert_eq!(config.killbot.preferred_range, 1.8);
        assert_eq!(config.killbot.heal_threshold, 8.0);
        assert_eq!(config.killbot.finisher_health, 8.0);
        assert!(config.killbot.crit_enabled);
    }

    /// Every default in `[pathfinding]` must itself pass validation --
    /// otherwise a config file that simply omits the section (which serde
    /// fills in from these defaults) would fail to load.
    #[test]
    fn the_pathfinding_defaults_are_themselves_valid() {
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
            "#,
        )
        .unwrap();
        config
            .validate()
            .expect("a config without a [pathfinding] section must still be valid");
        assert!(config.pathfinding.enabled);
        assert!(!config.pathfinding.debug_pathfinding);
        assert_eq!(config.pathfinding.segment_length, 48.0);
        assert_eq!(config.pathfinding.long_distance_threshold, 96.0);
        assert_eq!(config.pathfinding.costs.walk, 1.0);
        assert_eq!(config.pathfinding.costs.lava_avoidance, 200.0);
    }

    fn pathfinding_config() -> Config {
        toml::from_str(
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
            "#,
        )
        .unwrap()
    }

    #[test]
    fn rejects_a_zero_or_negative_pathfinding_cost_weight() {
        // A zero-cost move makes A* non-terminating (a free cycle is an
        // infinite loop), so this has to be rejected rather than clamped.
        for weight in [0.0, -1.0, f64::NAN] {
            let mut config = pathfinding_config();
            config.pathfinding.costs.walk = weight;
            assert!(
                matches!(
                    config.validate(),
                    Err(AppError::InvalidPathfindingConfiguration(_))
                ),
                "a walk cost of {weight} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_sample_margin_too_narrow_for_the_segment_length() {
        let mut config = pathfinding_config();
        config.pathfinding.segment_length = 256.0;
        config.pathfinding.sample_margin = 8;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidPathfindingConfiguration(_))
        ));
    }

    #[test]
    fn rejects_an_out_of_range_pathfinding_search_budget() {
        let mut config = pathfinding_config();
        config.pathfinding.max_search_nodes = 1;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidPathfindingConfiguration(_))
        ));

        let mut config = pathfinding_config();
        config.pathfinding.search_timeout_ms = 0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidPathfindingConfiguration(_))
        ));
    }

    #[test]
    fn rejects_health_thresholds_outside_the_vanilla_scale() {
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
            "#,
        )
        .unwrap();
        assert_eq!(
            config.killbot.heal_threshold, 8.0,
            "eat below four hearts by default"
        );
        assert_eq!(
            config.killbot.finisher_health, 8.0,
            "ignore own health once the target is under four hearts"
        );
        for out_of_range in [-1.0, 20.1] {
            config.killbot.finisher_health = out_of_range;
            assert!(matches!(
                config.validate(),
                Err(AppError::InvalidKillbotConfiguration(_))
            ));
        }
        config.killbot.finisher_health = 0.0;
        config
            .validate()
            .expect("0 is valid -- it disables the finisher rule");
    }

    #[test]
    fn rejects_an_out_of_range_attack_cooldown() {
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
            "#,
        )
        .unwrap();
        assert_eq!(
            config.killbot.attack_cooldown_ms, 0,
            "the shipped default is the automatic, weapon-accurate cadence"
        );
        assert_eq!(
            config.killbot.attack_cooldown(),
            None,
            "0 means automatic, not an instant cooldown"
        );
        config.validate().expect("the default must be valid");

        assert!(
            config.killbot.always_crit_with_sword,
            "sword swings default to holding out for a critical"
        );
        assert_eq!(
            config.killbot.eat_cooldown_ms, 5000,
            "one bite, then back to the fight while the regeneration works"
        );
        assert_eq!(
            config.killbot.eat_cooldown(),
            std::time::Duration::from_millis(5000)
        );
        config.killbot.eat_cooldown_ms = 60_001;
        assert!(
            matches!(
                config.validate(),
                Err(AppError::InvalidKillbotConfiguration(_))
            ),
            "an absurd eat cooldown is rejected"
        );
        config.killbot.eat_cooldown_ms = 0;
        config
            .validate()
            .expect("0 is valid -- it means chain bites back to back");
        config.killbot.attack_cooldown_ms = 625;
        assert_eq!(
            config.killbot.attack_cooldown(),
            Some(std::time::Duration::from_millis(625))
        );
        config
            .validate()
            .expect("an explicit sword-speed cadence is valid");

        for cooldown in [49, 10_001] {
            config.killbot.attack_cooldown_ms = cooldown;
            assert!(
                matches!(
                    config.validate(),
                    Err(AppError::InvalidKillbotConfiguration(_))
                ),
                "an attack_cooldown_ms of {cooldown} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_killbot_preferred_range_beyond_its_attack_range() {
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
            "#,
        )
        .unwrap();
        config.killbot.preferred_range = config.killbot.attack_range + 1.0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidKillbotConfiguration(_))
        ));
    }

    #[test]
    fn output_mode_parses_every_spelling_case_insensitively() {
        assert_eq!(OutputMode::parse("none"), Some(OutputMode::None));
        assert_eq!(OutputMode::parse("LIGHT"), Some(OutputMode::Light));
        assert_eq!(OutputMode::parse("Info"), Some(OutputMode::Info));
        assert_eq!(OutputMode::parse("debug"), Some(OutputMode::Debug));
        assert_eq!(OutputMode::parse("fulldebug"), Some(OutputMode::FullDebug));
        assert_eq!(OutputMode::parse("full-debug"), Some(OutputMode::FullDebug));
        assert_eq!(OutputMode::parse("full_debug"), Some(OutputMode::FullDebug));
        assert_eq!(OutputMode::parse("nonsense"), None);
    }

    #[test]
    fn output_mode_orders_from_least_to_most_verbose() {
        assert!(OutputMode::None < OutputMode::Light);
        assert!(OutputMode::Light < OutputMode::Info);
        assert!(OutputMode::Info < OutputMode::Debug);
        assert!(OutputMode::Debug < OutputMode::FullDebug);
    }

    #[test]
    fn parses_equipment_settings_from_one_section() {
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

                [equipment.armor]
                mode = "rarity"

                [equipment.offhand]
                priority = "shield"
            "#,
        )
        .expect("test configuration should parse");

        assert_eq!(config.equipment.armor.mode, ArmorMode::Rarity);
        assert_eq!(config.equipment.offhand.priority, OffhandPriority::Shield);
    }

    #[test]
    fn example_config_file_parses_and_validates() {
        let config: Config = toml::from_str(include_str!("../config.toml.example"))
            .expect("config.toml.example should parse");
        config
            .validate()
            .expect("config.toml.example should be a valid configuration");
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
        config.movement.follow_distance = 3.0;
        config.multitasking.strafe_angle = 20.0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidMovementConfiguration(_))
        ));
    }

    #[test]
    fn movement_stuck_timeout_defaults_lower_than_the_absolute_backstop() {
        let default = MovementConfig::default();
        assert!(default.stuck_timeout_seconds < default.maximum_navigation_seconds);

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
            "#,
        )
        .unwrap();
        config
            .validate()
            .expect("defaults must be internally valid");
        assert_eq!(
            config.movement.stuck_timeout_seconds,
            default.stuck_timeout_seconds
        );
        assert_eq!(
            config.movement.maximum_navigation_seconds,
            default.maximum_navigation_seconds
        );
    }

    #[test]
    fn rejects_a_movement_stuck_timeout_that_exceeds_the_absolute_backstop() {
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
                [movement]
                follow_distance = 3.0
                repath_interval_ms = 150
                arrival_distance = 1.5
                stuck_timeout_seconds = 100
                maximum_navigation_seconds = 60
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidMovementConfiguration(_))
        ));
        config.movement.stuck_timeout_seconds = 0;
        config.movement.maximum_navigation_seconds = 900;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidMovementConfiguration(_))
        ));
    }

    #[test]
    fn rejects_invalid_look_configuration() {
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

                [look]
                update_rate = 0
            "#,
        )
        .expect("test configuration should parse");

        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.update_rate = 20;
        config
            .validate()
            .expect("restored look configuration is valid");
        config.look.randomization.horizontal_strength = 1.1;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.randomization.horizontal_strength = 0.35;
        config.look.motion.minimum_yaw_speed = config.look.motion.maximum_yaw_speed + 1.0;
        assert!(
            matches!(
                config.validate(),
                Err(AppError::InvalidLookConfiguration(_))
            ),
            "a minimum aim speed above the maximum is nonsense at any scale"
        );
        config.look.motion.minimum_yaw_speed = default_minimum_yaw_speed();
        config.look.reaction_delay_max_ms = 5;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.reaction_delay_max_ms = 55;
        config.interaction.retry_limit = 11;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidInteractionConfiguration(_))
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
