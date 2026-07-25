use crate::{
    config::LookMotionConfig,
    look::{
        aim_point::{LookPrecision, SeededRng},
        rotation::{Rotation, shortest_angle_delta},
    },
};

#[derive(Clone, Copy, Debug, Default)]
pub struct RotationMotion {
    pub yaw_velocity: f32,
    pub pitch_velocity: f32,
    pub overshoot_used: bool,
}

pub fn advance_motion(
    current: Rotation,
    target: Rotation,
    state: &mut RotationMotion,
    config: &LookMotionConfig,
    precision: LookPrecision,
    speed_factor: f32,
    update_rate: u32,
) -> Rotation {
    if precision == LookPrecision::Instant {
        state.yaw_velocity = 0.0;
        state.pitch_velocity = 0.0;
        return target;
    }
    let dt = 1.0 / update_rate.max(1) as f32;
    let tolerance = tolerance(config, precision);
    let yaw_delta = shortest_angle_delta(current.yaw, target.yaw);
    let pitch_delta = target.pitch - current.pitch;
    let yaw_factor =
        if config.micro_correction_enabled && yaw_delta.abs() < config.slowdown_angle as f32 {
            speed_factor * (1.0 - config.micro_correction_strength as f32 * 0.5)
        } else {
            speed_factor
        };
    let pitch_factor =
        if config.micro_correction_enabled && pitch_delta.abs() < config.slowdown_angle as f32 {
            speed_factor * (1.0 - config.micro_correction_strength as f32 * 0.5)
        } else {
            speed_factor
        };
    let yaw_step = axis_step(
        yaw_delta,
        state.yaw_velocity,
        AxisSettings::new(
            config.minimum_yaw_speed,
            config.maximum_yaw_speed,
            config.yaw_acceleration,
            config.yaw_deceleration,
            config.slowdown_angle,
            tolerance,
            dt,
        ),
        yaw_factor,
    );
    let pitch_step = axis_step(
        pitch_delta,
        state.pitch_velocity,
        AxisSettings::new(
            config.minimum_pitch_speed,
            config.maximum_pitch_speed,
            config.pitch_acceleration,
            config.pitch_deceleration,
            config.slowdown_angle,
            tolerance,
            dt,
        ),
        pitch_factor,
    );
    state.yaw_velocity = yaw_step.1;
    state.pitch_velocity = pitch_step.1;
    Rotation {
        yaw: current.yaw + yaw_step.0,
        pitch: (current.pitch + pitch_step.0).clamp(-90.0, 90.0),
    }
}

#[derive(Clone, Copy)]
struct AxisSettings {
    minimum: f32,
    maximum: f32,
    acceleration: f32,
    deceleration: f32,
    slowdown: f32,
    tolerance: f32,
    dt: f32,
}
impl AxisSettings {
    fn new(
        minimum: f64,
        maximum: f64,
        acceleration: f64,
        deceleration: f64,
        slowdown: f64,
        tolerance: f32,
        dt: f32,
    ) -> Self {
        Self {
            minimum: minimum as f32,
            maximum: maximum as f32,
            acceleration: acceleration as f32,
            deceleration: deceleration as f32,
            slowdown: slowdown as f32,
            tolerance,
            dt,
        }
    }
}

fn axis_step(delta: f32, velocity: f32, settings: AxisSettings, speed_factor: f32) -> (f32, f32) {
    if delta.abs() <= settings.tolerance {
        return (delta, 0.0);
    }
    let desired_speed = if delta.abs() < settings.slowdown {
        (settings.minimum
            + (settings.maximum - settings.minimum) * (delta.abs() / settings.slowdown))
            .min(settings.maximum)
    } else {
        settings.maximum
    } * speed_factor;
    let desired_velocity = delta.signum() * desired_speed;
    let rate = if velocity.signum() != desired_velocity.signum()
        || velocity.abs() < desired_velocity.abs()
    {
        settings.acceleration
    } else {
        settings.deceleration
    };
    let next_velocity =
        velocity + (desired_velocity - velocity).clamp(-rate * settings.dt, rate * settings.dt);
    let step = next_velocity * settings.dt;
    if step.abs() >= delta.abs() {
        (delta, 0.0)
    } else {
        (step, next_velocity)
    }
}

pub fn tolerance(config: &LookMotionConfig, precision: LookPrecision) -> f32 {
    match precision {
        LookPrecision::Natural => config.arrival_tolerance as f32,
        LookPrecision::Precise => (config.arrival_tolerance * 0.35).max(0.1) as f32,
        LookPrecision::Instant => 0.0,
    }
}

pub fn should_overshoot(
    current: Rotation,
    target: Rotation,
    precision: LookPrecision,
    config: &LookMotionConfig,
    rng: &mut SeededRng,
) -> bool {
    precision == LookPrecision::Natural
        && config.maximum_overshoot_degrees > 0.0
        && (shortest_angle_delta(current.yaw, target.yaw).abs()
            > config.slowdown_angle as f32 * 2.0
            || (target.pitch - current.pitch).abs() > config.slowdown_angle as f32 * 2.0)
        && rng.chance(config.overshoot_chance)
}

pub fn overshoot_rotation(
    current: Rotation,
    target: Rotation,
    config: &LookMotionConfig,
    rng: &mut SeededRng,
) -> Rotation {
    let maximum = config.maximum_overshoot_degrees as f32;
    let yaw = shortest_angle_delta(current.yaw, target.yaw).signum()
        * (maximum * (0.4 + rng.next_unit() as f32 * 0.6));
    let pitch = (target.pitch - current.pitch).signum() * (maximum * 0.5);
    Rotation {
        yaw: target.yaw + yaw,
        pitch: (target.pitch + pitch).clamp(-90.0, 90.0),
    }
}

pub fn within_tolerance(current: Rotation, target: Rotation, tolerance: f32) -> bool {
    shortest_angle_delta(current.yaw, target.yaw).abs() <= tolerance
        && (target.pitch - current.pitch).abs() <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> LookMotionConfig {
        LookMotionConfig::default()
    }
    #[test]
    fn accelerates_then_decelerates_and_caps_speed() {
        let mut motion = RotationMotion::default();
        let current = Rotation::default();
        let target = Rotation {
            yaw: 120.0,
            pitch: 0.0,
        };
        let next = advance_motion(
            current,
            target,
            &mut motion,
            &config(),
            LookPrecision::Natural,
            1.0,
            20,
        );
        assert!(next.yaw > 0.0 && motion.yaw_velocity <= config().maximum_yaw_speed as f32);
        let near = advance_motion(
            Rotation {
                yaw: 115.0,
                pitch: 0.0,
            },
            target,
            &mut motion,
            &config(),
            LookPrecision::Natural,
            1.0,
            20,
        );
        assert!(near.yaw <= 120.0);
    }
    #[test]
    fn instant_is_immediate_and_precise_disables_overshoot() {
        let mut motion = RotationMotion::default();
        let target = Rotation {
            yaw: 90.0,
            pitch: 10.0,
        };
        assert_eq!(
            advance_motion(
                Rotation::default(),
                target,
                &mut motion,
                &config(),
                LookPrecision::Instant,
                1.0,
                20
            ),
            target
        );
        let mut rng = SeededRng::new(1);
        assert!(!should_overshoot(
            Rotation::default(),
            target,
            LookPrecision::Precise,
            &config(),
            &mut rng
        ));
    }

    #[test]
    fn natural_overshoot_is_bounded_and_only_requested_once_by_controller_state() {
        let mut config = config();
        config.overshoot_chance = 1.0;
        let current = Rotation::default();
        let target = Rotation {
            yaw: 90.0,
            pitch: 0.0,
        };
        let mut rng = SeededRng::new(3);
        assert!(should_overshoot(
            current,
            target,
            LookPrecision::Natural,
            &config,
            &mut rng
        ));
        let point = overshoot_rotation(current, target, &config, &mut rng);
        assert!((point.yaw - target.yaw).abs() <= config.maximum_overshoot_degrees as f32);
    }
}
