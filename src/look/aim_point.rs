use std::time::{Duration, SystemTime};

use crate::{config::LookRandomizationConfig, minecraft::world_state::BlockPosition};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockFace {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAimMode {
    VisibleSurface,
    Center,
    #[allow(dead_code)]
    SpecificFace(BlockFace),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookPrecision {
    Natural,
    #[allow(dead_code)]
    Precise,
    Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hitbox {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RelativeAimPoint {
    Block([f64; 3]),
    Hitbox {
        horizontal_x: f64,
        vertical: f64,
        horizontal_z: f64,
    },
    Fixed,
}

#[derive(Clone, Copy, Debug)]
pub struct AimSelection {
    pub relative: RelativeAimPoint,
    pub hold_until: SystemTime,
    pub speed_factor: f32,
}

#[derive(Clone, Debug)]
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }
    pub fn next_unit(&mut self) -> f64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state as f64 / u64::MAX as f64).clamp(0.0, 1.0)
    }
    pub fn chance(&mut self, chance: f64) -> bool {
        self.next_unit() < chance.clamp(0.0, 1.0)
    }
    pub fn signed(&mut self, strength: f64) -> f64 {
        (self.next_unit() * 2.0 - 1.0) * strength
    }
}

pub fn block_relative_point(
    eye: [f64; 3],
    position: BlockPosition,
    mode: BlockAimMode,
    randomize: bool,
    config: &LookRandomizationConfig,
    rng: &mut SeededRng,
) -> RelativeAimPoint {
    if mode == BlockAimMode::Center {
        return RelativeAimPoint::Block([0.5, 0.5, 0.5]);
    }
    let center = [
        f64::from(position.x) + 0.5,
        f64::from(position.y) + 0.5,
        f64::from(position.z) + 0.5,
    ];
    let face = match mode {
        BlockAimMode::SpecificFace(face) => face,
        BlockAimMode::VisibleSurface => visible_face(eye, center),
        BlockAimMode::Center => unreachable!(),
    };
    let inset = 0.06;
    let h = config.horizontal_strength * 0.40;
    let v = config.vertical_strength * 0.40;
    let (mut x, mut y, mut z) = (0.5, 0.5, 0.5);
    match face {
        BlockFace::North => {
            z = inset;
            if randomize {
                x += rng.signed(h);
                y += rng.signed(v);
            }
        }
        BlockFace::South => {
            z = 1.0 - inset;
            if randomize {
                x += rng.signed(h);
                y += rng.signed(v);
            }
        }
        BlockFace::West => {
            x = inset;
            if randomize {
                z += rng.signed(h);
                y += rng.signed(v);
            }
        }
        BlockFace::East => {
            x = 1.0 - inset;
            if randomize {
                z += rng.signed(h);
                y += rng.signed(v);
            }
        }
        BlockFace::Up => {
            y = 1.0 - inset;
            if randomize {
                x += rng.signed(h);
                z += rng.signed(h);
            }
        }
        BlockFace::Down => {
            y = inset;
            if randomize {
                x += rng.signed(h);
                z += rng.signed(h);
            }
        }
    }
    RelativeAimPoint::Block([
        x.clamp(inset, 1.0 - inset),
        y.clamp(inset, 1.0 - inset),
        z.clamp(inset, 1.0 - inset),
    ])
}

pub fn hitbox_relative_point(
    player: bool,
    randomize: bool,
    config: &LookRandomizationConfig,
    rng: &mut SeededRng,
) -> RelativeAimPoint {
    if !randomize {
        return RelativeAimPoint::Hitbox {
            horizontal_x: 0.0,
            vertical: if player { 0.72 } else { 0.5 },
            horizontal_z: 0.0,
        };
    }
    let h = config.horizontal_strength * 0.42;
    let vertical_center = if player { 0.72 } else { 0.55 };
    let vertical =
        (vertical_center + rng.signed(config.vertical_strength * 0.28)).clamp(0.12, 0.92);
    RelativeAimPoint::Hitbox {
        horizontal_x: rng.signed(h),
        vertical,
        horizontal_z: rng.signed(h),
    }
}

pub fn aim_position(
    relative: RelativeAimPoint,
    base: [f64; 3],
    hitbox: Option<Hitbox>,
) -> [f64; 3] {
    match relative {
        RelativeAimPoint::Block(offset) => [
            base[0] + offset[0],
            base[1] + offset[1],
            base[2] + offset[2],
        ],
        RelativeAimPoint::Hitbox {
            horizontal_x,
            vertical,
            horizontal_z,
        } => {
            let hitbox = hitbox.unwrap_or(Hitbox {
                width: 0.6,
                height: 1.8,
            });
            [
                base[0] + horizontal_x * hitbox.width,
                base[1] + vertical * hitbox.height,
                base[2] + horizontal_z * hitbox.width,
            ]
        }
        RelativeAimPoint::Fixed => base,
    }
}

pub fn new_selection(
    relative: RelativeAimPoint,
    config: &LookRandomizationConfig,
    speed_variation: f64,
    rng: &mut SeededRng,
) -> AimSelection {
    let min = config.minimum_hold_time_ms;
    let max = config.maximum_hold_time_ms;
    let hold = min + ((max - min) as f64 * rng.next_unit()) as u64;
    AimSelection {
        relative,
        hold_until: SystemTime::now() + Duration::from_millis(hold),
        speed_factor: (1.0 + rng.signed(speed_variation)) as f32,
    }
}

fn visible_face(eye: [f64; 3], center: [f64; 3]) -> BlockFace {
    let delta = [eye[0] - center[0], eye[1] - center[1], eye[2] - center[2]];
    let (axis, _) = delta
        .into_iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap_or((2, 0.0));
    match axis {
        0 if delta[0] < 0.0 => BlockFace::West,
        0 => BlockFace::East,
        1 if delta[1] < 0.0 => BlockFace::Down,
        1 => BlockFace::Up,
        _ if delta[2] < 0.0 => BlockFace::North,
        _ => BlockFace::South,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> LookRandomizationConfig {
        LookRandomizationConfig::default()
    }
    #[test]
    fn block_points_stay_inside_and_avoid_edges() {
        let mut rng = SeededRng::new(4);
        let RelativeAimPoint::Block(point) = block_relative_point(
            [0.5, 0.5, -5.0],
            BlockPosition { x: 0, y: 0, z: 0 },
            BlockAimMode::VisibleSurface,
            true,
            &config(),
            &mut rng,
        ) else {
            panic!()
        };
        assert!(point.iter().all(|value| *value >= 0.06 && *value <= 0.94));
        assert!(point[2] <= 0.07);
    }
    #[test]
    fn player_points_prefer_torso_and_are_seeded() {
        let mut first = SeededRng::new(9);
        let mut second = SeededRng::new(9);
        let a = hitbox_relative_point(true, true, &config(), &mut first);
        let b = hitbox_relative_point(true, true, &config(), &mut second);
        assert_eq!(a, b);
        let RelativeAimPoint::Hitbox { vertical, .. } = a else {
            panic!()
        };
        assert!(vertical > 0.5);
    }
    #[test]
    fn hitbox_offsets_remain_inside_small_entities() {
        let point = aim_position(
            RelativeAimPoint::Hitbox {
                horizontal_x: 0.4,
                vertical: 0.6,
                horizontal_z: -0.4,
            },
            [1.0, 2.0, 3.0],
            Some(Hitbox {
                width: 0.1,
                height: 0.2,
            }),
        );
        assert!((0.95..=1.05).contains(&point[0]));
        assert!((2.0..=2.2).contains(&point[1]));
    }

    #[test]
    fn deterministic_points_are_fixed_when_randomization_is_disabled() {
        let mut config = config();
        config.enabled = false;
        let mut rng = SeededRng::new(1);
        assert_eq!(
            hitbox_relative_point(true, false, &config, &mut rng),
            RelativeAimPoint::Hitbox {
                horizontal_x: 0.0,
                vertical: 0.72,
                horizontal_z: 0.0
            }
        );
        let RelativeAimPoint::Block(face) = block_relative_point(
            [0.0, 0.0, -1.0],
            BlockPosition { x: 0, y: 0, z: 0 },
            BlockAimMode::SpecificFace(BlockFace::East),
            false,
            &config,
            &mut rng,
        ) else {
            panic!()
        };
        assert_eq!(face, [0.94, 0.5, 0.5]);
        assert_eq!(
            block_relative_point(
                [0.0, 0.0, -1.0],
                BlockPosition { x: 0, y: 0, z: 0 },
                BlockAimMode::VisibleSurface,
                false,
                &config,
                &mut rng
            ),
            RelativeAimPoint::Block([0.5, 0.5, 0.06])
        );
    }

    #[test]
    fn selection_holds_a_stable_offset_and_keeps_speed_variation_bounded() {
        let mut rng = SeededRng::new(12);
        let selection = new_selection(
            RelativeAimPoint::Block([0.4, 0.5, 0.6]),
            &config(),
            0.1,
            &mut rng,
        );
        assert!(selection.hold_until > SystemTime::now());
        assert!((0.9..=1.1).contains(&selection.speed_factor));
        assert_eq!(selection.relative, RelativeAimPoint::Block([0.4, 0.5, 0.6]));
    }

    #[test]
    fn moving_hitbox_preserves_the_relative_point() {
        let relative = RelativeAimPoint::Hitbox {
            horizontal_x: 0.2,
            vertical: 0.7,
            horizontal_z: -0.2,
        };
        let first = aim_position(
            relative,
            [0.0, 64.0, 0.0],
            Some(Hitbox {
                width: 0.6,
                height: 1.8,
            }),
        );
        let moved = aim_position(
            relative,
            [3.0, 65.0, -2.0],
            Some(Hitbox {
                width: 0.6,
                height: 1.8,
            }),
        );
        assert!((moved[0] - first[0] - 3.0).abs() < 0.0001);
        assert!((moved[1] - first[1] - 1.0).abs() < 0.0001);
        assert!((moved[2] - first[2] + 2.0).abs() < 0.0001);
    }
}
