//! Converts a pathfinder's world-space travel direction into human-like local
//! controls. Azalea still owns path execution; this adapter is intentionally
//! side-effect free so look control never competes with pathfinding input.

use crate::config::MultitaskingConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LocalMovementInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub sprint: bool,
    pub speed_multiplier: f32,
    pub should_turn_toward_travel: bool,
}

/// Select local movement controls from Minecraft yaw angles. Positive angular
/// error means the travel direction is to the camera's right.
#[must_use]
pub fn local_input_for_direction(
    travel_yaw: f32,
    camera_yaw: f32,
    explicit_look: bool,
    config: &MultitaskingConfig,
) -> LocalMovementInput {
    let error = shortest_yaw_delta(camera_yaw, travel_yaw);
    let magnitude = error.abs();
    let right = error > 0.0;
    let mut input = LocalMovementInput {
        speed_multiplier: 1.0,
        ..LocalMovementInput::default()
    };

    if magnitude <= config.normal_forward_angle {
        input.forward = true;
        input.sprint = true;
    } else if magnitude <= config.strafe_angle {
        input.forward = true;
        input.right = right;
        input.left = !right;
        input.sprint = magnitude <= config.normal_forward_angle * 1.5;
        input.speed_multiplier = 0.88;
    } else if magnitude <= config.backward_angle {
        input.right = right;
        input.left = !right;
        input.speed_multiplier = 0.65;
    } else if explicit_look {
        // Short backwards corrections preserve an explicit gaze without
        // allowing an unrealistic long-distance backwards sprint.
        input.backward = true;
        input.right = right;
        input.left = !right;
        input.speed_multiplier = 0.4;
    } else {
        // Navigation assistance may turn the gaze gradually; it never snaps.
        input.forward = true;
        input.speed_multiplier = 0.35;
        input.should_turn_toward_travel = true;
    }
    input
}

#[must_use]
pub fn shortest_yaw_delta(from: f32, to: f32) -> f32 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_direction_sprints() {
        let input = local_input_for_direction(10.0, 0.0, false, &MultitaskingConfig::default());
        assert!(input.forward && input.sprint);
    }

    #[test]
    fn diagonal_direction_strafes() {
        let input = local_input_for_direction(65.0, 0.0, false, &MultitaskingConfig::default());
        assert!(input.forward && input.right && !input.sprint);
    }

    #[test]
    fn sideways_direction_does_not_sprint() {
        let input = local_input_for_direction(90.0, 0.0, false, &MultitaskingConfig::default());
        assert!(input.right && !input.sprint && input.speed_multiplier < 1.0);
    }

    #[test]
    fn explicit_look_allows_only_slow_backwards_correction() {
        let input = local_input_for_direction(180.0, 0.0, true, &MultitaskingConfig::default());
        assert!(input.backward && !input.sprint && input.speed_multiplier < 0.5);
    }

    #[test]
    fn no_explicit_look_requests_a_gradual_turn_for_extreme_difference() {
        let input = local_input_for_direction(180.0, 0.0, false, &MultitaskingConfig::default());
        assert!(input.should_turn_toward_travel && !input.sprint);
    }
}
