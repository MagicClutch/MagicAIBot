//! Pure attack-timing and critical-hit decisions.
//!
//! Vanilla awards a critical hit (1.5x damage, the little star particles)
//! when the attacker hits while falling (not on the ground, moving
//! downward) and isn't otherwise disqualified (sprinting *into* the hit,
//! climbing, blind, etc. -- none of which this bot ever does mid-swing, so
//! those exclusions don't need modeling here). The only lever available is
//! *when* to attack relative to a jump: jump, let gravity take over for a
//! couple of ticks, then swing while still airborne and descending.
//!
//! # Maximum damage means swinging at exactly 100% charge
//!
//! Java scales every hit by the attack-strength cooldown:
//! `0.2 + 0.8 * (t/T)^2`, where `T` is the weapon's own recharge time
//! (`1 / attack_speed`). That curve is why spam-clicking loses: at half
//! charge a hit does 40% damage at twice the rate -- 0.8x the DPS of
//! waiting -- and partial-charge hits also forfeit the sweep and land
//! weaker knockback. Damage per second is maximized by swinging the instant
//! the bar refills and not one tick before, which is exactly what
//! [`attack_ready`] does.
//!
//! `T` depends on the weapon actually in hand, so it is read from the held
//! item ([`weapon_cooldown`]) rather than assumed: an axe recharges in 1.0s
//! against a sword's 0.625s, and swinging an axe on a sword's clock would
//! land every hit at ~62% charge -- the single biggest damage loss available
//! to a bot that switches to an axe to break shields.
//!
//! # A sword hit that isn't a crit is a wasted hit
//!
//! With `always_crit_with_sword` on (the default), a sword swing is never
//! taken flat-footed: if the cooldown comes up while the bot is standing on
//! the ground with no jump in flight, it jumps *and holds the swing* until
//! it is falling ([`should_force_crit_jump`]). That costs ~0.3s on that one
//! hit and buys 1.5x damage, which is a small net DPS gain on its own and a
//! large one against armor.
//!
//! It is bounded by [`CRIT_HOLD_TIMEOUT`] rather than unconditional, because
//! "jump" is a request the world can refuse: under a low ceiling, in a
//! one-block tunnel, or in a cobweb the bot never leaves the ground, and an
//! unconditional hold would mean it never swings again. After the timeout it
//! takes the ordinary hit.
//!
//! # The jump is timed to finish *inside* the cooldown, not after it
//!
//! The obvious way to land crits -- wait for the cooldown, then jump, then
//! swing on the way down -- costs the whole rise of the jump (~0.3s) on top
//! of every cooldown, so the bot's real hit rate is always slower than its
//! configured cadence, and the harder you tune the cadence the more it
//! drifts. Instead, [`should_prejump_for_crit`] fires the jump
//! [`CRIT_JUMP_LEAD`] *before* the cooldown expires, so the bot is already
//! falling at the exact moment it becomes ready to swing. Crits then cost
//! nothing: hits land on the configured cadence, and they land as crits.

use std::time::{Duration, Instant};

/// Recharge time of a sword (attack speed 1.6/s) -- the fastest real melee
/// weapon in Java, and the fallback whenever the held item is unknown.
pub const SWORD_COOLDOWN: Duration = Duration::from_millis(625);

/// Recharge time for `item_id`, from Java's per-item attack-speed attribute:
/// `1000ms / attack_speed`.
///
/// Anything not listed -- an empty hand, a block, a bow -- uses vanilla's
/// default attack speed of 4.0/s. That is genuinely correct for a bare fist,
/// and for the odd case of the bot swinging something that isn't a weapon it
/// is also the right answer: there is no damage bonus to wait for.
pub fn weapon_cooldown(item_id: Option<&str>) -> Duration {
    let Some(id) = item_id else {
        return Duration::from_millis(250);
    };
    let speed = match id.trim_start_matches("minecraft:") {
        id if id.ends_with("_sword") => 1.6,
        "trident" => 1.1,
        "wooden_axe" | "stone_axe" => 0.8,
        "iron_axe" => 0.9,
        "golden_axe" | "diamond_axe" | "netherite_axe" => 1.0,
        id if id.ends_with("_pickaxe") => 1.2,
        id if id.ends_with("_shovel") => 1.0,
        "wooden_hoe" | "golden_hoe" => 1.0,
        "stone_hoe" => 2.0,
        "iron_hoe" => 3.0,
        "diamond_hoe" | "netherite_hoe" => 4.0,
        // Default attack speed for everything else, including an empty hand.
        _ => 4.0,
    };
    Duration::from_secs_f64(1.0 / speed)
}

/// Whether `item_id` is a sword -- the weapon `always_crit_with_sword`
/// applies to. Deliberately narrow: an axe's 1.0s cooldown already leaves
/// room for the ordinary pre-jump to line up, whereas a sword's 0.625s is
/// tight enough that hits routinely come up while the bot is grounded.
pub fn is_sword(item_id: Option<&str>) -> bool {
    item_id.is_some_and(|id| id.trim_start_matches("minecraft:").ends_with("_sword"))
}

/// Whether to jump *now* and hold a ready swing until it can land as a crit.
///
/// Only when the cooldown is already up, the bot is on the ground in range,
/// and nothing has been held back longer than [`CRIT_HOLD_TIMEOUT`]. The
/// caller passes how long it has been holding (`held_for`); `None` means
/// this tick is the first.
pub fn should_force_crit_jump(
    on_ground: bool,
    within_attack_range: bool,
    last_jump: Option<Instant>,
    now: Instant,
    held_for: Option<Duration>,
) -> bool {
    if !on_ground || !within_attack_range {
        return false;
    }
    if held_for.is_some_and(|held| held >= CRIT_HOLD_TIMEOUT) {
        // Given up on this one: something is stopping the bot leaving the
        // ground, so take the ordinary hit rather than never hitting.
        return false;
    }
    last_jump.is_none_or(|last| now.saturating_duration_since(last) >= FORCED_CRIT_JUMP_RETRY)
}

/// How long before the attack cooldown expires the crit jump is fired.
///
/// A vanilla jump takes roughly six ticks (~0.3s) to reach its apex and
/// start descending, which is exactly the window
/// [`is_critical_window`] needs -- so jumping this far ahead means the bot
/// is falling right as the cooldown opens. Slightly generous rather than
/// exact: being a tick early only means falling a tick longer (still a
/// crit), while being a tick late means no crit at all.
pub const CRIT_JUMP_LEAD: Duration = Duration::from_millis(320);

/// Minimum spacing between two crit-seeking jumps. Mostly redundant -- a
/// jump can only start from the ground, and the arc itself is ~0.75s -- but
/// it stops a very fast cadence from turning into a bunny-hop. Comfortably
/// under a sword's 625ms cycle so it can never be the reason a sword hit
/// misses its crit.
pub const JUMP_COOLDOWN: Duration = Duration::from_millis(400);

/// How long a ready swing is held back waiting to become a crit before the
/// bot gives up and hits anyway -- see this module's doc comment. Roughly
/// one jump arc's rise plus slack: long enough for a jump that works, short
/// enough that a bot which physically cannot jump loses one hit's worth of
/// tempo rather than the fight.
pub const CRIT_HOLD_TIMEOUT: Duration = Duration::from_millis(400);

/// Retry spacing for the forced crit jump while a swing is being held. Much
/// tighter than [`JUMP_COOLDOWN`]: the first jump may have been eaten by a
/// ceiling or a mid-air moment, and there is only [`CRIT_HOLD_TIMEOUT`] of
/// room to try again -- but not so tight that a blocked bot floods the
/// server with jump packets every tick.
pub const FORCED_CRIT_JUMP_RETRY: Duration = Duration::from_millis(150);

/// How long until the next attack may be sent. Zero once ready.
///
/// This, rather than a 0.0-1.0 "cooldown progress" fraction, is what every
/// decision here needs: the bot no longer swings at a percentage of the
/// cooldown (`attack_cooldown_ms` is honored exactly), and the crit jump is
/// timed by how much of the cooldown is *left*.
pub fn time_until_ready(
    last_attack: Option<Instant>,
    now: Instant,
    cooldown: Duration,
) -> Duration {
    match last_attack {
        None => Duration::ZERO,
        Some(last) => cooldown.saturating_sub(now.saturating_duration_since(last)),
    }
}

/// Whether the cooldown has fully elapsed.
///
/// Deliberately the *whole* configured cooldown, with no early-swing
/// allowance: `attack_cooldown_ms` is a user-facing "this many milliseconds
/// between hits" setting, so shaving a percentage off it would quietly make
/// the bot hit faster than the number the user typed.
pub fn attack_ready(last_attack: Option<Instant>, now: Instant, cooldown: Duration) -> bool {
    time_until_ready(last_attack, now, cooldown).is_zero()
}

/// Whether attacking *right now* would land as a critical hit: airborne
/// and already descending. `velocity_y` is Minecraft's raw vertical
/// velocity (negative while falling).
pub fn is_critical_window(on_ground: bool, velocity_y: f64) -> bool {
    !on_ground && velocity_y < 0.0
}

/// Whether to fire the crit jump *now*, while the attack cooldown is still
/// running, so the fall coincides with the cooldown expiring -- see this
/// module's doc comment.
///
/// Only from solid ground, only within striking distance (jumping while
/// still closing distance just wastes the arc), only inside the
/// [`CRIT_JUMP_LEAD`] window before the cooldown opens, and never again
/// within [`JUMP_COOLDOWN`] of the last one.
pub fn should_prejump_for_crit(
    on_ground: bool,
    within_attack_range: bool,
    last_attack: Option<Instant>,
    last_jump: Option<Instant>,
    now: Instant,
    cooldown: Duration,
) -> bool {
    if !on_ground || !within_attack_range {
        return false;
    }
    if last_jump.is_some_and(|last| now.saturating_duration_since(last) < JUMP_COOLDOWN) {
        return false;
    }
    // A cooldown shorter than the jump itself can never be pre-empted this
    // way -- the bot would be permanently airborne. Those cadences simply
    // don't get crits, which is the right trade: the user asked for the hit
    // rate, not for the crits.
    if cooldown <= CRIT_JUMP_LEAD {
        return false;
    }
    time_until_ready(last_attack, now, cooldown) <= CRIT_JUMP_LEAD
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_millis(1650);

    #[test]
    fn attack_is_ready_immediately_with_no_prior_attack() {
        assert!(attack_ready(None, Instant::now(), COOLDOWN));
        assert_eq!(
            time_until_ready(None, Instant::now(), COOLDOWN),
            Duration::ZERO
        );
    }

    #[test]
    fn attack_is_not_ready_before_the_full_configured_cooldown() {
        let now = Instant::now();
        assert!(!attack_ready(Some(now), now + COOLDOWN / 2, COOLDOWN));
        assert!(!attack_ready(
            Some(now),
            now + COOLDOWN - Duration::from_millis(1),
            COOLDOWN
        ));
    }

    #[test]
    fn the_configured_cooldown_is_honored_exactly_rather_than_shaved() {
        // The whole point of the setting: "1650ms between hits" must mean
        // 1650, not 90% of it.
        let now = Instant::now();
        assert!(attack_ready(Some(now), now + COOLDOWN, COOLDOWN));
        assert!(!attack_ready(
            Some(now),
            now + Duration::from_millis(1485),
            COOLDOWN
        ));
    }

    #[test]
    fn a_different_configured_cadence_changes_the_spacing() {
        let now = Instant::now();
        let fast = Duration::from_millis(300);
        assert!(attack_ready(Some(now), now + fast, fast));
        assert!(!attack_ready(Some(now), now + fast, COOLDOWN));
    }

    #[test]
    fn time_until_ready_counts_down_and_saturates_at_zero() {
        let now = Instant::now();
        assert_eq!(
            time_until_ready(Some(now), now, COOLDOWN),
            COOLDOWN,
            "nothing elapsed yet"
        );
        assert_eq!(
            time_until_ready(Some(now), now + Duration::from_millis(650), COOLDOWN),
            Duration::from_millis(1000)
        );
        assert_eq!(
            time_until_ready(Some(now), now + COOLDOWN * 3, COOLDOWN),
            Duration::ZERO
        );
    }

    #[test]
    fn every_sword_recharges_on_the_vanilla_sword_clock() {
        for sword in [
            "minecraft:wooden_sword",
            "minecraft:stone_sword",
            "minecraft:iron_sword",
            "minecraft:golden_sword",
            "minecraft:diamond_sword",
            "minecraft:netherite_sword",
        ] {
            assert_eq!(weapon_cooldown(Some(sword)), SWORD_COOLDOWN, "{sword}");
        }
    }

    #[test]
    fn axes_recharge_slower_than_swords_and_differ_by_material() {
        // The case that actually matters: `#kill` swaps to an axe to break a
        // shield, and swinging it on a sword's clock would land every hit at
        // roughly 62% charge.
        assert_eq!(
            weapon_cooldown(Some("minecraft:netherite_axe")),
            Duration::from_millis(1000)
        );
        assert_eq!(
            weapon_cooldown(Some("minecraft:iron_axe")),
            Duration::from_secs_f64(1.0 / 0.9)
        );
        assert_eq!(
            weapon_cooldown(Some("minecraft:stone_axe")),
            Duration::from_millis(1250)
        );
        assert!(weapon_cooldown(Some("minecraft:diamond_axe")) > SWORD_COOLDOWN);
    }

    #[test]
    fn an_unknown_item_or_empty_hand_uses_the_default_attack_speed() {
        assert_eq!(weapon_cooldown(None), Duration::from_millis(250));
        assert_eq!(
            weapon_cooldown(Some("minecraft:cooked_beef")),
            Duration::from_millis(250)
        );
        assert_eq!(
            weapon_cooldown(Some("minecraft:diamond_hoe")),
            Duration::from_millis(250),
            "a diamond hoe genuinely does swing at 4.0/s"
        );
    }

    #[test]
    fn critical_window_requires_airborne_and_descending() {
        assert!(is_critical_window(false, -0.1));
        assert!(!is_critical_window(true, -0.1), "on the ground");
        assert!(!is_critical_window(false, 0.5), "still rising");
        assert!(!is_critical_window(false, 0.0), "apex, not yet falling");
    }

    #[test]
    fn the_crit_jump_fires_shortly_before_the_cooldown_opens() {
        let now = Instant::now();
        let attacked_at = Some(now);
        // Far from ready: no jump yet, or the bot would land before it could
        // swing.
        assert!(!should_prejump_for_crit(
            true,
            true,
            attacked_at,
            None,
            now + Duration::from_millis(500),
            COOLDOWN
        ));
        // Inside the lead window: jump now so the fall lines up with the
        // cooldown expiring.
        assert!(should_prejump_for_crit(
            true,
            true,
            attacked_at,
            None,
            now + COOLDOWN - Duration::from_millis(200),
            COOLDOWN
        ));
    }

    #[test]
    fn the_crit_jump_still_fires_once_the_cooldown_is_already_up() {
        // The bot may have been out of range (or airborne) through the whole
        // lead window; catching up late is better than never jumping.
        let now = Instant::now();
        assert!(should_prejump_for_crit(
            true,
            true,
            Some(now),
            None,
            now + COOLDOWN * 2,
            COOLDOWN
        ));
    }

    #[test]
    fn the_crit_jump_needs_ground_and_range_and_respects_its_own_cooldown() {
        let now = Instant::now();
        let ready_moment = now + COOLDOWN;
        assert!(should_prejump_for_crit(
            true,
            true,
            Some(now),
            None,
            ready_moment,
            COOLDOWN
        ));
        assert!(
            !should_prejump_for_crit(false, true, Some(now), None, ready_moment, COOLDOWN),
            "already airborne"
        );
        assert!(
            !should_prejump_for_crit(true, false, Some(now), None, ready_moment, COOLDOWN),
            "out of range"
        );
        assert!(
            !should_prejump_for_crit(
                true,
                true,
                Some(now),
                Some(ready_moment - Duration::from_millis(1)),
                ready_moment,
                COOLDOWN
            ),
            "jumped a moment ago"
        );
    }

    #[test]
    fn swords_are_recognized_and_other_weapons_are_not() {
        assert!(is_sword(Some("minecraft:netherite_sword")));
        assert!(is_sword(Some("wooden_sword")));
        assert!(!is_sword(Some("minecraft:diamond_axe")));
        assert!(!is_sword(Some("minecraft:trident")));
        assert!(!is_sword(None));
    }

    #[test]
    fn a_ready_sword_swing_on_the_ground_jumps_instead_of_hitting_flat() {
        let now = Instant::now();
        assert!(should_force_crit_jump(true, true, None, now, None));
    }

    #[test]
    fn the_forced_crit_jump_needs_ground_and_range() {
        let now = Instant::now();
        assert!(
            !should_force_crit_jump(false, true, None, now, None),
            "already airborne -- the swing is about to land as a crit anyway"
        );
        assert!(!should_force_crit_jump(true, false, None, now, None));
    }

    #[test]
    fn a_held_swing_is_released_once_the_hold_times_out() {
        // The case that matters: a bot in a one-block tunnel can never leave
        // the ground, and must not stop attacking because of it.
        let now = Instant::now();
        assert!(should_force_crit_jump(
            true,
            true,
            None,
            now,
            Some(CRIT_HOLD_TIMEOUT - Duration::from_millis(1))
        ));
        assert!(!should_force_crit_jump(
            true,
            true,
            None,
            now,
            Some(CRIT_HOLD_TIMEOUT)
        ));
        assert!(!should_force_crit_jump(
            true,
            true,
            None,
            now,
            Some(CRIT_HOLD_TIMEOUT * 10)
        ));
    }

    #[test]
    fn the_forced_jump_retries_quickly_but_does_not_spam_every_tick() {
        let now = Instant::now();
        assert!(
            !should_force_crit_jump(
                true,
                true,
                Some(now),
                now + Duration::from_millis(50),
                Some(Duration::from_millis(50))
            ),
            "one tick later is too soon to re-send a jump"
        );
        assert!(should_force_crit_jump(
            true,
            true,
            Some(now),
            now + FORCED_CRIT_JUMP_RETRY,
            Some(FORCED_CRIT_JUMP_RETRY)
        ));
    }

    #[test]
    fn the_jump_cooldown_never_blocks_a_sword_crit() {
        // A sword recharges in 625ms; the crit jump has to be available
        // every cycle or hits land flat.
        assert!(JUMP_COOLDOWN < SWORD_COOLDOWN);
    }

    #[test]
    fn a_cadence_shorter_than_the_jump_arc_never_pre_jumps() {
        // Nothing sensible to do here: the bot would be airborne
        // permanently, so it just swings on the ground at the requested rate.
        let now = Instant::now();
        let fast = CRIT_JUMP_LEAD - Duration::from_millis(50);
        assert!(!should_prejump_for_crit(
            true,
            true,
            Some(now),
            None,
            now + fast,
            fast
        ));
    }
}
