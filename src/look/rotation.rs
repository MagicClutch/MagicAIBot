use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
}

pub fn rotation_towards(eye: [f64; 3], target: [f64; 3]) -> Rotation {
    let dx = target[0] - eye[0];
    let dy = target[1] - eye[1];
    let dz = target[2] - eye[2];
    let horizontal = (dx * dx + dz * dz).sqrt();
    if !dx.is_finite() || !dy.is_finite() || !dz.is_finite() {
        return Rotation::default();
    }
    let yaw = normalize_yaw((PI - (-dx).atan2(-dz)).to_degrees()) as f32;
    let pitch = (-(dy.atan2(horizontal).to_degrees())).clamp(-90.0, 90.0) as f32;
    Rotation { yaw, pitch }
}

pub fn normalize_yaw(yaw: f64) -> f64 {
    (yaw + 180.0).rem_euclid(360.0) - 180.0
}

pub fn shortest_angle_delta(current: f32, target: f32) -> f32 {
    normalize_yaw(f64::from(target - current)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_cardinal_and_negative_coordinate_yaw() {
        let north = rotation_towards([0.0, 1.62, 0.0], [0.0, 1.62, -10.0]);
        assert!((north.yaw + 180.0).abs() < 0.01);
        let east = rotation_towards([0.0, 1.62, 0.0], [10.0, 1.62, 0.0]);
        assert!((east.yaw + 90.0).abs() < 0.01);
        let negative = rotation_towards([10.0, 1.62, 10.0], [-10.0, 1.62, -10.0]);
        assert!(negative.yaw.is_finite());
    }

    #[test]
    fn calculates_pitch_up_down_and_large_distance() {
        let up = rotation_towards([0.0, 1.62, 0.0], [0.0, 20.0, 0.0]);
        let down = rotation_towards([0.0, 20.0, 0.0], [0.0, 1.0, 0.0]);
        assert_eq!(up.pitch, -90.0);
        assert_eq!(down.pitch, 90.0);
        assert!(
            rotation_towards([0.0, 1.62, 0.0], [1e12, 1.62, -1e12])
                .yaw
                .is_finite()
        );
    }

    #[test]
    fn normalizes_angles_and_shortest_delta() {
        assert_eq!(normalize_yaw(540.0), -180.0);
        assert!((shortest_angle_delta(179.0, -179.0) - 2.0).abs() < 0.01);
    }
}
