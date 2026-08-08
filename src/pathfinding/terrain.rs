//! Block-id -> navigation meaning. Pure: no Azalea types, no I/O -- the
//! sampler (`crate::pathfinding::sampler`) is what reads real block states
//! out of the world and turns them into these classes, and everything above
//! it (grid, moves, cost, A*) only ever sees a [`TerrainClass`].
//!
//! Deliberately a *small* closed set rather than one variant per block:
//! pathfinding only ever asks four questions of a block -- can I stand in
//! it, can I stand on it, will it hurt me, and what would it cost to remove
//! it -- so anything that answers those identically is the same class here.
//! Reuses `crate::interaction::placement_rules` for the air/support/
//! unbreakable predicates rather than restating them, so the pathfinder and
//! the block-interaction system can never disagree about what "solid" means.

use crate::interaction::placement_rules::{has_support, is_air, is_replaceable, is_unbreakable};

/// What a single block cell means to the pathfinder.
///
/// Ordered from "nothing there" to "never enter this" only for readability;
/// nothing depends on the ordering, and [`Self::Unknown`] is deliberately
/// the `Default` so an unsampled cell in a [`crate::pathfinding::grid::
/// TerrainGrid`] is never mistaken for open air.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum TerrainClass {
    /// Never sampled, or outside the sampled region: the chunk isn't loaded
    /// from this side. Treated as impassable by [`Self::passable`] -- the
    /// spec's "avoid calculating paths through unknown terrain" -- and as
    /// the frontier the bot walks toward so the server streams it in (see
    /// `crate::pathfinding::route`).
    #[default]
    Unknown,
    /// Air, cave air, void air: walk straight through, nothing to stand on.
    Air,
    /// Passable but not air and not worth breaking: grass, ferns, snow
    /// layers, the replaceable clutter that costs nothing to walk into.
    Replaceable,
    /// Water. Passable, slower, and no fall damage -- see
    /// `crate::pathfinding::cost`.
    Water,
    /// Lava. Passable in the physics sense and lethal in every other sense;
    /// the cost model prices it high enough that a route only crosses it if
    /// there is genuinely no alternative, and [`Self::lethal`] lets callers
    /// forbid it outright.
    Lava,
    /// Passable (or standable) but damaging: fire, magma, cactus, sweet
    /// berries, powder snow, campfires. Routed around unless the detour is
    /// long.
    Hazard,
    /// An ordinary full block: stand on top of it, or pay a break cost to
    /// pass through it.
    Solid,
    /// Solid and impossible to remove (bedrock, barrier, ...): never a
    /// break-through candidate at any cost.
    Unbreakable,
    /// Ladders, vines, scaffolding: passable, and climbable straight up or
    /// down, which is the one way the bot gains height without a jump.
    Climbable,
}

impl TerrainClass {
    /// Whether the bot's body can occupy this cell without first changing
    /// the world. `Unknown` is *not* passable: an unloaded chunk could be
    /// anything, and pathing optimistically through it is exactly how a bot
    /// ends up walking into a wall (or off a cliff) it never saw.
    #[must_use]
    pub fn passable(self) -> bool {
        matches!(
            self,
            Self::Air
                | Self::Replaceable
                | Self::Water
                | Self::Lava
                | Self::Hazard
                | Self::Climbable
        )
    }

    /// Whether standing on top of this cell is supported -- the block under
    /// the bot's feet. Water deliberately isn't: floating in water is
    /// handled as a swim move, not as standing on a floor.
    #[must_use]
    pub fn supports_standing(self) -> bool {
        matches!(self, Self::Solid | Self::Unbreakable | Self::Hazard)
    }

    /// Whether the bot can climb this cell vertically.
    #[must_use]
    pub fn climbable(self) -> bool {
        matches!(self, Self::Climbable)
    }

    /// Whether entering this cell is expected to kill or badly hurt the bot,
    /// regardless of cost weighting.
    #[must_use]
    pub fn lethal(self) -> bool {
        matches!(self, Self::Lava)
    }

    /// Whether entering this cell costs health but is survivable.
    #[must_use]
    pub fn damaging(self) -> bool {
        matches!(self, Self::Hazard)
    }

    /// Whether this cell could be mined out of the way, given the bot is
    /// allowed to mine at all.
    #[must_use]
    pub fn breakable(self) -> bool {
        matches!(self, Self::Solid)
    }

    /// Whether anything is actually known about this cell.
    #[must_use]
    pub fn known(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Compact form for [`crate::pathfinding::grid::TerrainGrid`]'s storage
    /// -- one byte per block is what keeps a multi-chunk sample cheap enough
    /// to snapshot and hand to a background thread.
    #[must_use]
    pub fn as_byte(self) -> u8 {
        self as u8
    }

    /// Inverse of [`Self::as_byte`]. Any unrecognized byte decodes to
    /// `Unknown`, which is the safe direction: a corrupt cell is never
    /// mistaken for walkable ground.
    #[must_use]
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Air,
            2 => Self::Replaceable,
            3 => Self::Water,
            4 => Self::Lava,
            5 => Self::Hazard,
            6 => Self::Solid,
            7 => Self::Unbreakable,
            8 => Self::Climbable,
            _ => Self::Unknown,
        }
    }
}

/// Blocks the bot can climb straight up and down.
const CLIMBABLE_BLOCKS: &[&str] = &[
    "minecraft:ladder",
    "minecraft:vine",
    "minecraft:scaffolding",
    "minecraft:twisting_vines",
    "minecraft:weeping_vines",
    "minecraft:cave_vines",
];

/// Blocks that hurt on contact but don't stop movement. Not exhaustive by
/// design -- anything missed here simply classifies as `Solid`/`Air` and is
/// routed through normally, which is the same behavior the bot had before
/// this module existed; entries are added as they prove to matter.
const HAZARD_BLOCKS: &[&str] = &[
    "minecraft:fire",
    "minecraft:soul_fire",
    "minecraft:magma_block",
    "minecraft:cactus",
    "minecraft:sweet_berry_bush",
    "minecraft:powder_snow",
    "minecraft:wither_rose",
    "minecraft:campfire",
    "minecraft:soul_campfire",
    "minecraft:lava_cauldron",
    "minecraft:pointed_dripstone",
];

/// Blocks that are solid but can never be mined, on top of
/// `placement_rules::is_unbreakable`'s own list. Kept separate from that
/// function because "the bot must not try to mine this" (its concern) and
/// "the pathfinder must not plan a route through this" (this one's) are
/// different questions that happen to share most of their answer: a barrier
/// or portal frame isn't a *mining* target the bot would ever pick, but it
/// absolutely is something a route could otherwise try to tunnel through.
const UNBREAKABLE_BLOCKS: &[&str] = &[
    "minecraft:barrier",
    "minecraft:end_portal_frame",
    "minecraft:end_portal",
    "minecraft:nether_portal",
    "minecraft:command_block",
    "minecraft:structure_block",
    "minecraft:jigsaw",
    "minecraft:light",
    "minecraft:reinforced_deepslate",
];

/// Classifies one block id. `None` means "no block state available for this
/// position" -- an unloaded chunk or a position outside the world's height
/// range -- which is [`TerrainClass::Unknown`], not air.
#[must_use]
pub fn classify(block_id: Option<&str>) -> TerrainClass {
    let Some(id) = block_id else {
        return TerrainClass::Unknown;
    };
    if is_air(Some(id)) {
        return TerrainClass::Air;
    }
    match id {
        "minecraft:water" | "minecraft:bubble_column" => return TerrainClass::Water,
        "minecraft:lava" => return TerrainClass::Lava,
        _ => {}
    }
    if HAZARD_BLOCKS.contains(&id) {
        return TerrainClass::Hazard;
    }
    if CLIMBABLE_BLOCKS.contains(&id) {
        return TerrainClass::Climbable;
    }
    if is_unbreakable(Some(id)) || UNBREAKABLE_BLOCKS.contains(&id) {
        return TerrainClass::Unbreakable;
    }
    if is_replaceable(Some(id)) {
        return TerrainClass::Replaceable;
    }
    if has_support(Some(id)) {
        return TerrainClass::Solid;
    }
    // `has_support` already excludes air/replaceable/liquids above, so
    // anything left here is a non-supporting oddity (a sign, a torch, a
    // button). Walkable-through, nothing to stand on.
    TerrainClass::Replaceable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_the_default_and_is_never_passable() {
        assert_eq!(TerrainClass::default(), TerrainClass::Unknown);
        assert!(!TerrainClass::Unknown.passable());
        assert!(!TerrainClass::Unknown.supports_standing());
        assert!(!TerrainClass::Unknown.known());
    }

    #[test]
    fn a_missing_block_state_is_unknown_rather_than_air() {
        assert_eq!(classify(None), TerrainClass::Unknown);
    }

    #[test]
    fn classifies_the_common_cases() {
        assert_eq!(classify(Some("minecraft:air")), TerrainClass::Air);
        assert_eq!(classify(Some("minecraft:cave_air")), TerrainClass::Air);
        assert_eq!(classify(Some("minecraft:stone")), TerrainClass::Solid);
        assert_eq!(classify(Some("minecraft:water")), TerrainClass::Water);
        assert_eq!(classify(Some("minecraft:lava")), TerrainClass::Lava);
        assert_eq!(
            classify(Some("minecraft:bedrock")),
            TerrainClass::Unbreakable
        );
        assert_eq!(
            classify(Some("minecraft:barrier")),
            TerrainClass::Unbreakable
        );
        assert_eq!(classify(Some("minecraft:cactus")), TerrainClass::Hazard);
        assert_eq!(
            classify(Some("minecraft:tall_grass")),
            TerrainClass::Replaceable
        );
    }

    #[test]
    fn solid_blocks_are_standable_and_breakable_but_bedrock_is_not_breakable() {
        assert!(TerrainClass::Solid.supports_standing());
        assert!(TerrainClass::Solid.breakable());
        assert!(TerrainClass::Unbreakable.supports_standing());
        assert!(!TerrainClass::Unbreakable.breakable());
    }

    #[test]
    fn lava_is_passable_but_lethal_and_hazards_are_merely_damaging() {
        assert!(TerrainClass::Lava.passable());
        assert!(TerrainClass::Lava.lethal());
        assert!(!TerrainClass::Hazard.lethal());
        assert!(TerrainClass::Hazard.damaging());
    }

    #[test]
    fn ladders_and_vines_are_climbable_and_passable() {
        assert_eq!(classify(Some("minecraft:ladder")), TerrainClass::Climbable);
        assert_eq!(classify(Some("minecraft:vine")), TerrainClass::Climbable);
        assert!(TerrainClass::Climbable.climbable());
        assert!(TerrainClass::Climbable.passable());
        assert!(!TerrainClass::Solid.climbable());
    }

    #[test]
    fn water_is_passable_but_is_not_a_floor() {
        assert!(TerrainClass::Water.passable());
        assert!(!TerrainClass::Water.supports_standing());
    }

    #[test]
    fn byte_encoding_round_trips_every_class() {
        for class in [
            TerrainClass::Climbable,
            TerrainClass::Unknown,
            TerrainClass::Air,
            TerrainClass::Replaceable,
            TerrainClass::Water,
            TerrainClass::Lava,
            TerrainClass::Hazard,
            TerrainClass::Solid,
            TerrainClass::Unbreakable,
        ] {
            assert_eq!(TerrainClass::from_byte(class.as_byte()), class);
        }
        assert_eq!(TerrainClass::from_byte(200), TerrainClass::Unknown);
    }
}
