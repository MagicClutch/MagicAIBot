//! The per-tick combat movement controller: reads the fight, produces key
//! presses. Pure -- it takes a snapshot of the world and returns a
//! [`MovementCommand`]; `crate::combat::executor` is what turns that into
//! client calls.
//!
//! Keeping it pure is what makes the interesting behavior testable at all:
//! an approach curve, an orbit, a sprint reset and a ledge stop are all just
//! sequences of `update` calls against hand-built inputs, with no live
//! server anywhere.
//!
//! # Order of operations, once per tick
//!
//! 1. Record where the target is, and work out where it is *going*
//!    ([`prediction`]).
//! 2. Pick which way to circle ([`strafing`]).
//! 3. Build the continuous desired-velocity vector ([`steering`]).
//! 4. Bend the previous vector toward it, rather than replacing it
//!    ([`smoothing`]) -- this is the step that produces curves.
//! 5. Project onto the eight keys Minecraft accepts, and decide sprint,
//!    jump and sneak.
//!
//! [`prediction`]: crate::combat::movement::prediction
//! [`strafing`]: crate::combat::movement::strafing
//! [`steering`]: crate::combat::movement::steering
//! [`smoothing`]: crate::combat::movement::smoothing

use std::time::{Duration, Instant};

use crate::{
    combat::movement::{
        prediction::TargetTracker,
        smoothing::{SmoothingLimits, SteeringSmoother},
        steering::{
            CombatWalk, DistanceBand, SteeringInput, SteeringWeights, Vec2, desired_velocity,
            project_to_walk,
        },
        strafing::{OrbitDirection, StrafePlanner},
    },
    look::aim_point::SeededRng,
};

/// How long sprint is released after a hit -- the "sprint reset"/w-tap real
/// players use to regain the knockback edge of a freshly-started sprint.
/// Deliberately short: the keys stay down, only the sprint flag drops, so
/// the bot keeps its footing and its direction through the reset.
pub const SPRINT_RESET: Duration = Duration::from_millis(120);

/// Minimum gap between jumps, so obstacle clearing and crit hops don't turn
/// into continuous bunny-hopping.
const JUMP_COOLDOWN: Duration = Duration::from_millis(350);

/// How far the projection has to be beaten by before the keys change --
/// see `steering::project_to_walk`.
const DIRECTION_HYSTERESIS: f64 = 12.0;

/// Steering magnitude below which the bot holds position instead of
/// shuffling.
const THROTTLE_FLOOR: f64 = 0.12;

/// Everything the controller is allowed to do, and the distances it works
/// to. Assembled from `KillbotConfig` by the caller.
#[derive(Clone, Copy, Debug)]
pub struct MovementTuning {
    pub band: DistanceBand,
    pub weights: SteeringWeights,
    pub limits: SmoothingLimits,
    /// Full prediction lead for perfectly steady target motion, in seconds.
    pub lead_seconds: f64,
    pub strafe_enabled: bool,
    pub sprint_reset_enabled: bool,
}

impl Default for MovementTuning {
    fn default() -> Self {
        Self {
            band: DistanceBand::default(),
            weights: SteeringWeights::default(),
            limits: SmoothingLimits::default(),
            lead_seconds: 0.25,
            strafe_enabled: true,
            sprint_reset_enabled: true,
        }
    }
}

/// What the local terrain probe found around the bot. Supplied by
/// `crate::combat::executor` from a small sampled region; the controller
/// never reads the world itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalHazards {
    /// Direction to push away from, already summed over everything nearby
    /// worth avoiding. Zero when the surroundings are clear.
    pub avoidance: Vec2,
    /// Whether continuing in the current direction runs off a drop or into
    /// something lethal. Stops the bot rather than merely nudging it.
    pub blocked_ahead: bool,
    /// Whether a one-block step up is in the way -- worth a jump rather than
    /// a detour.
    pub step_ahead: bool,
}

/// One tick of fight state.
#[derive(Clone, Copy, Debug)]
pub struct MovementSnapshot {
    pub bot: Vec2,
    pub target: Vec2,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    /// The camera's yaw. The look controller owns this; movement only reads
    /// it, which is what keeps the two decoupled -- the bot can circle left
    /// while the camera stays locked on the target.
    pub camera_yaw: f32,
    pub hazards: LocalHazards,
    pub now: Instant,
}

/// The keys to hold this tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MovementCommand {
    pub walk: CombatWalk,
    pub sprint: bool,
    pub jump: bool,
    pub sneak: bool,
}

/// Per-fight movement state.
pub struct CombatMovementController {
    tracker: TargetTracker,
    strafe: StrafePlanner,
    smoother: SteeringSmoother,
    walk: CombatWalk,
    last_tick: Option<Instant>,
    last_attack: Option<Instant>,
    last_jump: Option<Instant>,
    /// Distance to the target as of the previous tick, for noticing that the
    /// gap is not closing.
    previous_distance: Option<f64>,
}

impl Default for CombatMovementController {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatMovementController {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tracker: TargetTracker::new(),
            strafe: StrafePlanner::new(),
            smoother: SteeringSmoother::new(),
            walk: CombatWalk::None,
            last_tick: None,
            last_attack: None,
            last_jump: None,
            previous_distance: None,
        }
    }

    /// Clears everything carried between fights: momentum, target history,
    /// and the orbit pattern.
    pub fn reset(&mut self, rng: &mut SeededRng) {
        self.tracker.reset();
        self.strafe.reset(rng);
        self.smoother.reset();
        self.walk = CombatWalk::None;
        self.last_tick = None;
        self.last_attack = None;
        self.last_jump = None;
        self.previous_distance = None;
    }

    /// Tells the controller a hit just landed: sprint resets, and the orbit
    /// tightens for the combo.
    pub fn note_attack(&mut self, now: Instant) {
        self.last_attack = Some(now);
        self.strafe.note_attack();
    }

    /// The smoothed steering vector as of the last tick, in world space --
    /// exposed for debugging and for the tests that assert on curvature.
    #[must_use]
    pub fn heading(&self) -> Vec2 {
        self.smoother.current()
    }

    /// Runs one tick and returns the keys to hold.
    pub fn update(
        &mut self,
        snapshot: MovementSnapshot,
        tuning: &MovementTuning,
        rng: &mut SeededRng,
    ) -> MovementCommand {
        let elapsed = self.last_tick.map_or(Duration::from_millis(50), |last| {
            snapshot.now.saturating_duration_since(last)
        });
        self.last_tick = Some(snapshot.now);

        self.tracker.observe(snapshot.target, snapshot.now);
        let aim_point = self
            .tracker
            .predicted_position(snapshot.target, tuning.lead_seconds);

        let orbit = if tuning.strafe_enabled {
            self.strafe.advance(elapsed, rng)
        } else {
            OrbitDirection::Still
        };

        // Circling into a wall just grinds along it, so a blocked direction
        // switches the orbit rather than fighting it.
        if snapshot.hazards.blocked_ahead && orbit.is_circling() {
            self.strafe.force_switch();
        }

        let desired = desired_velocity(
            SteeringInput {
                bot: snapshot.bot,
                target: aim_point,
                orbit_sign: orbit.sign(),
                avoidance: snapshot.hazards.avoidance,
            },
            &tuning.band,
            &tuning.weights,
        );
        let smoothed = self.smoother.advance(desired, &tuning.limits, elapsed);

        let walk = project_to_walk(
            smoothed,
            snapshot.camera_yaw,
            self.walk,
            DIRECTION_HYSTERESIS,
            THROTTLE_FLOOR,
        );
        // A hazard directly ahead overrides the steering outright: the
        // avoidance push bends the route, but a ledge or lava needs the keys
        // released, not merely biased.
        let walk = if snapshot.hazards.blocked_ahead && walk.allows_sprint() {
            CombatWalk::None
        } else {
            walk
        };
        self.walk = walk;

        let distance = snapshot.target.minus(snapshot.bot).length();
        let closing = self
            .previous_distance
            .is_none_or(|previous| distance < previous - 0.01);
        self.previous_distance = Some(distance);

        let sprint_reset = tuning.sprint_reset_enabled
            && self
                .last_attack
                .is_some_and(|at| snapshot.now.saturating_duration_since(at) < SPRINT_RESET);
        // Sprint whenever the bot is actually travelling forward and has
        // ground to cover. Vanilla only sprints on a forward key, so the
        // projection has already decided most of this.
        let sprint = walk.allows_sprint()
            && !sprint_reset
            && !snapshot.hazards.blocked_ahead
            && distance > tuning.band.preferred_max;

        let jump = self.should_jump(&snapshot, walk, closing);
        if jump {
            self.last_jump = Some(snapshot.now);
        }

        MovementCommand {
            walk,
            sprint,
            jump,
            // Sneaking is left to the caller: the only combat use is edge
            // safety, and `blocked_ahead` already stops the bot outright,
            // which is both safer and faster to recover from than creeping.
            sneak: false,
        }
    }

    fn should_jump(&self, snapshot: &MovementSnapshot, walk: CombatWalk, closing: bool) -> bool {
        if !snapshot.on_ground || !walk.is_moving() {
            return false;
        }
        if self
            .last_jump
            .is_some_and(|at| snapshot.now.saturating_duration_since(at) < JUMP_COOLDOWN)
        {
            return false;
        }
        if snapshot.hazards.blocked_ahead {
            return false;
        }
        // A step up in the way, or physics reporting the bot has walked into
        // something while it was supposed to be closing the gap. The second
        // check is what stops it grinding against a fence forever: without
        // terrain knowledge, "I am pushing forward and not getting closer"
        // is the signal that something is there.
        snapshot.hazards.step_ahead || (snapshot.horizontal_collision && !closing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng() -> SeededRng {
        SeededRng::new(5)
    }

    fn snapshot(bot: Vec2, target: Vec2, now: Instant) -> MovementSnapshot {
        MovementSnapshot {
            bot,
            target,
            on_ground: true,
            horizontal_collision: false,
            // Camera locked on the target, which is what the look controller
            // does during a fight.
            camera_yaw: target.minus(bot).yaw_degrees(),
            hazards: LocalHazards::default(),
            now,
        }
    }

    fn tick(index: u64) -> Instant {
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        *ORIGIN.get_or_init(Instant::now) + Duration::from_millis(index * 50)
    }

    /// Runs the controller for `ticks`, moving the bot along its own chosen
    /// direction at a plausible sprint speed, and returns the path.
    fn simulate(
        controller: &mut CombatMovementController,
        mut bot: Vec2,
        target: impl Fn(u64) -> Vec2,
        ticks: u64,
        tuning: &MovementTuning,
    ) -> Vec<(Vec2, MovementCommand)> {
        let mut rng = rng();
        let mut path = Vec::new();
        for index in 0..ticks {
            let target_now = target(index);
            let command =
                controller.update(snapshot(bot, target_now, tick(index)), tuning, &mut rng);
            // Move along the smoothed heading -- the projection to keys is
            // tested separately; here we care about the trajectory.
            let heading = controller.heading();
            let speed = if command.sprint { 5.6 } else { 4.3 };
            bot = bot.plus(heading.scaled(speed * 0.05));
            path.push((bot, command));
        }
        path
    }

    #[test]
    fn a_fresh_controller_is_not_moving() {
        let controller = CombatMovementController::new();
        assert!(controller.heading().is_negligible());
    }

    #[test]
    fn it_closes_distance_on_a_stationary_target() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let target = Vec2::new(0.0, 12.0);
        let path = simulate(&mut controller, Vec2::zero(), |_| target, 40, &tuning);
        let final_distance = target.minus(path.last().unwrap().0).length();
        assert!(
            final_distance <= tuning.band.chase,
            "should have closed to combat range, got {final_distance}"
        );
    }

    #[test]
    fn the_approach_is_a_curve_rather_than_a_straight_line() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let target = Vec2::new(0.0, 12.0);
        let path = simulate(&mut controller, Vec2::zero(), |_| target, 25, &tuning);
        // A straight-line bot would keep x at exactly zero the whole way.
        let max_lateral = path
            .iter()
            .map(|(position, _)| position.x.abs())
            .fold(0.0, f64::max);
        assert!(
            max_lateral > 0.15,
            "the approach never left the straight line: {max_lateral}"
        );
    }

    #[test]
    fn it_sprints_while_closing_and_stops_sprinting_in_the_pocket() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let target = Vec2::new(0.0, 12.0);
        let path = simulate(&mut controller, Vec2::zero(), |_| target, 45, &tuning);
        assert!(
            path.iter().take(10).any(|(_, command)| command.sprint),
            "should sprint the approach"
        );
        assert!(
            !path.last().unwrap().1.sprint,
            "should not sprint once in the preferred band"
        );
    }

    #[test]
    fn it_holds_the_preferred_distance_once_it_arrives() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let target = Vec2::new(0.0, 8.0);
        let path = simulate(&mut controller, Vec2::zero(), |_| target, 90, &tuning);
        for (position, _) in path.iter().skip(45) {
            let distance = target.minus(*position).length();
            assert!(
                (0.9..=3.0).contains(&distance),
                "drifted out of the pocket: {distance}"
            );
        }
    }

    #[test]
    fn it_orbits_the_target_rather_than_standing_still_in_the_pocket() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let target = Vec2::new(0.0, 0.0);
        // Start already in the band.
        let path = simulate(
            &mut controller,
            Vec2::new(0.0, -1.8),
            |_| target,
            60,
            &tuning,
        );
        let travelled: f64 = path
            .windows(2)
            .map(|pair| pair[1].0.minus(pair[0].0).length())
            .sum();
        assert!(
            travelled > 3.0,
            "should be circling, not parked: travelled {travelled}"
        );
    }

    #[test]
    fn it_never_settles_inside_the_target() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let target = Vec2::new(0.0, 0.0);
        // Start standing right on top of them.
        let path = simulate(
            &mut controller,
            Vec2::new(0.0, -0.2),
            |_| target,
            60,
            &tuning,
        );
        let settled = target.minus(path.last().unwrap().0).length();
        assert!(settled > 0.9, "still crowding the target: {settled}");
    }

    #[test]
    fn it_cuts_the_corner_on_a_target_running_across_it() {
        // Prediction earns its keep on *crossing* motion, not a tail chase:
        // a target running directly away is in the same direction whether
        // you lead it or not, but one running across your front is not.
        let tuning = MovementTuning::default();
        let runner = |index: u64| Vec2::new(0.28 * index as f64, 8.0);

        let mut leading = CombatMovementController::new();
        let with_lead = simulate(&mut leading, Vec2::zero(), runner, 40, &tuning);

        let mut trailing = CombatMovementController::new();
        let no_lead = MovementTuning {
            lead_seconds: 0.0,
            ..tuning
        };
        let without_lead = simulate(&mut trailing, Vec2::zero(), runner, 40, &no_lead);

        let gap = |path: &[(Vec2, MovementCommand)]| runner(39).minus(path[39].0).length();
        assert!(
            gap(&with_lead) < gap(&without_lead),
            "leading should intercept sooner: {} vs {}",
            gap(&with_lead),
            gap(&without_lead)
        );
    }

    #[test]
    fn a_sprint_reset_drops_sprint_briefly_without_stopping_the_bot() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let target = Vec2::new(0.0, 12.0);
        for index in 0..10 {
            controller.update(
                snapshot(Vec2::zero(), target, tick(index)),
                &tuning,
                &mut rng,
            );
        }
        controller.note_attack(tick(10));
        let during = controller.update(snapshot(Vec2::zero(), target, tick(10)), &tuning, &mut rng);
        assert!(!during.sprint, "sprint should drop right after a hit");
        assert!(during.walk.is_moving(), "but the bot keeps moving");

        // 150ms later the reset is over.
        let after = controller.update(snapshot(Vec2::zero(), target, tick(13)), &tuning, &mut rng);
        assert!(after.sprint, "sprint should come straight back");
    }

    #[test]
    fn sprint_resets_can_be_switched_off() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning {
            sprint_reset_enabled: false,
            ..MovementTuning::default()
        };
        let mut rng = rng();
        let target = Vec2::new(0.0, 12.0);
        for index in 0..10 {
            controller.update(
                snapshot(Vec2::zero(), target, tick(index)),
                &tuning,
                &mut rng,
            );
        }
        controller.note_attack(tick(10));
        let during = controller.update(snapshot(Vec2::zero(), target, tick(10)), &tuning, &mut rng);
        assert!(during.sprint);
    }

    #[test]
    fn a_ledge_or_lava_ahead_stops_the_bot_rather_than_only_nudging_it() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let target = Vec2::new(0.0, 12.0);
        for index in 0..10 {
            controller.update(
                snapshot(Vec2::zero(), target, tick(index)),
                &tuning,
                &mut rng,
            );
        }
        let mut hazardous = snapshot(Vec2::zero(), target, tick(10));
        hazardous.hazards.blocked_ahead = true;
        let command = controller.update(hazardous, &tuning, &mut rng);
        assert_eq!(command.walk, CombatWalk::None);
        assert!(!command.sprint);
        assert!(!command.jump, "and it does not hop off the edge either");
    }

    #[test]
    fn an_obstacle_push_bends_the_route_without_stopping_it() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let target = Vec2::new(0.0, 12.0);
        let mut with_wall = snapshot(Vec2::zero(), target, tick(0));
        with_wall.hazards.avoidance = Vec2::new(1.0, 0.0);
        controller.update(with_wall, &tuning, &mut rng);
        assert!(
            controller.heading().x > 0.05,
            "should veer around it: {:?}",
            controller.heading()
        );
    }

    #[test]
    fn a_step_up_is_jumped_once_rather_than_hopped_continuously() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let target = Vec2::new(0.0, 12.0);
        let mut stepping = snapshot(Vec2::zero(), target, tick(0));
        stepping.hazards.step_ahead = true;
        assert!(controller.update(stepping, &tuning, &mut rng).jump);

        let mut next = snapshot(Vec2::zero(), target, tick(1));
        next.hazards.step_ahead = true;
        assert!(
            !controller.update(next, &tuning, &mut rng).jump,
            "the jump cooldown should stop bunny-hopping"
        );
    }

    #[test]
    fn walking_into_something_while_failing_to_close_triggers_a_jump() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let target = Vec2::new(0.0, 12.0);
        // Two ticks at the same position: the gap is not closing.
        controller.update(snapshot(Vec2::zero(), target, tick(0)), &tuning, &mut rng);
        let mut stuck = snapshot(Vec2::zero(), target, tick(1));
        stuck.horizontal_collision = true;
        assert!(controller.update(stuck, &tuning, &mut rng).jump);
    }

    #[test]
    fn it_does_not_jump_while_airborne() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let mut airborne = snapshot(Vec2::zero(), Vec2::new(0.0, 12.0), tick(0));
        airborne.on_ground = false;
        airborne.hazards.step_ahead = true;
        assert!(!controller.update(airborne, &tuning, &mut rng).jump);
    }

    #[test]
    fn disabling_strafe_produces_a_direct_approach() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning {
            strafe_enabled: false,
            ..MovementTuning::default()
        };
        let target = Vec2::new(0.0, 12.0);
        let path = simulate(&mut controller, Vec2::zero(), |_| target, 20, &tuning);
        let max_lateral = path
            .iter()
            .map(|(position, _)| position.x.abs())
            .fold(0.0, f64::max);
        assert!(
            max_lateral < 0.05,
            "should be a straight line: {max_lateral}"
        );
    }

    #[test]
    fn a_reset_clears_momentum_and_history_between_fights() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        for index in 0..10 {
            controller.update(
                snapshot(Vec2::zero(), Vec2::new(0.0, 12.0), tick(index)),
                &tuning,
                &mut rng,
            );
        }
        assert!(!controller.heading().is_negligible());
        controller.reset(&mut rng);
        assert!(controller.heading().is_negligible());
        assert!(
            controller.tracker.motion().velocity.is_negligible(),
            "target history is forgotten too"
        );
    }

    #[test]
    fn the_keys_do_not_flap_between_adjacent_directions_every_tick() {
        let mut controller = CombatMovementController::new();
        let tuning = MovementTuning::default();
        let mut rng = rng();
        let target = Vec2::new(0.0, 6.0);
        let mut changes = 0;
        let mut previous = CombatWalk::None;
        for index in 0..60 {
            let command = controller.update(
                snapshot(Vec2::new(0.0, 0.0), target, tick(index)),
                &tuning,
                &mut rng,
            );
            if command.walk != previous {
                changes += 1;
                previous = command.walk;
            }
        }
        assert!(
            changes < 20,
            "the keys changed {changes} times in 3 seconds -- that is chatter, not movement"
        );
    }
}
