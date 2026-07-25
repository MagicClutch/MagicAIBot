use std::time::SystemTime;

use crate::minecraft::world_state::{MovementSnapshot, MovementStatus, PositionSnapshot};

pub fn moving_snapshot(
    destination: PositionSnapshot,
    start: Option<PositionSnapshot>,
) -> MovementSnapshot {
    MovementSnapshot {
        status: MovementStatus::MovingToPosition,
        destination: Some(destination),
        target_player: None,
        start_position: start,
        started_at: Some(SystemTime::now()),
        estimated_distance: start.map(|position| distance(position, destination)),
        last_movement_update: Some(SystemTime::now()),
        failure_reason: None,
    }
}

pub fn following_snapshot(
    name: String,
    destination: PositionSnapshot,
    start: Option<PositionSnapshot>,
) -> MovementSnapshot {
    MovementSnapshot {
        status: MovementStatus::FollowingPlayer,
        destination: Some(destination),
        target_player: Some(name),
        start_position: start,
        started_at: Some(SystemTime::now()),
        estimated_distance: start.map(|position| distance(position, destination)),
        last_movement_update: Some(SystemTime::now()),
        failure_reason: None,
    }
}

pub fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}
pub fn arrived(
    current: Option<PositionSnapshot>,
    destination: Option<PositionSnapshot>,
    threshold: f64,
) -> bool {
    current
        .zip(destination)
        .is_some_and(|(current, destination)| distance(current, destination) <= threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_state_starts_as_moving() {
        let snapshot = moving_snapshot(
            PositionSnapshot {
                x: 10.,
                y: 64.,
                z: 10.,
            },
            None,
        );
        assert_eq!(snapshot.status, MovementStatus::MovingToPosition);
        assert_eq!(snapshot.destination.unwrap().x, 10.);
    }

    #[test]
    fn follow_state_keeps_target_name() {
        let snapshot = following_snapshot("Steve".into(), PositionSnapshot::default(), None);
        assert_eq!(snapshot.status, MovementStatus::FollowingPlayer);
        assert_eq!(snapshot.target_player.as_deref(), Some("Steve"));
    }

    #[test]
    fn arrival_detection_uses_threshold() {
        assert!(arrived(
            Some(PositionSnapshot::default()),
            Some(PositionSnapshot {
                x: 1.,
                y: 0.,
                z: 0.
            }),
            1.5
        ));
        assert!(!arrived(
            Some(PositionSnapshot::default()),
            Some(PositionSnapshot {
                x: 2.,
                y: 0.,
                z: 0.
            }),
            1.5
        ));
    }
}
