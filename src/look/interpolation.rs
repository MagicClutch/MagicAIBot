use super::rotation::{Rotation, shortest_angle_delta};

pub fn interpolate(
    current: Rotation,
    target: Rotation,
    yaw_speed: f32,
    pitch_speed: f32,
    update_rate: f32,
) -> Rotation {
    let seconds = if update_rate > 0.0 {
        1.0 / update_rate
    } else {
        0.0
    };
    let max_yaw = yaw_speed.max(0.0) * seconds;
    let max_pitch = pitch_speed.max(0.0) * seconds;
    let yaw_delta = shortest_angle_delta(current.yaw, target.yaw);
    let pitch_delta = target.pitch - current.pitch;
    Rotation {
        yaw: current.yaw + yaw_delta.clamp(-max_yaw, max_yaw),
        pitch: (current.pitch + pitch_delta.clamp(-max_pitch, max_pitch)).clamp(-90.0, 90.0),
    }
}

pub fn within_tolerance(current: Rotation, target: Rotation, tolerance: f32) -> bool {
    shortest_angle_delta(current.yaw, target.yaw).abs() <= tolerance
        && (target.pitch - current.pitch).abs() <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_over_multiple_steps_without_snapping() {
        let current = Rotation {
            yaw: 0.0,
            pitch: 0.0,
        };
        let target = Rotation {
            yaw: 90.0,
            pitch: -45.0,
        };
        let next = interpolate(current, target, 180.0, 180.0, 20.0);
        assert_eq!(next.yaw, 9.0);
        assert_eq!(next.pitch, -9.0);
        assert!(!within_tolerance(next, target, 1.0));
    }
}
