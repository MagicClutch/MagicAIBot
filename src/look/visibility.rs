#![allow(dead_code)]

use crate::minecraft::world_state::PositionSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Visible,
    NotVisible,
    Unknown,
}

/// Visibility is deliberately an application-owned seam. The pinned Azalea
/// release exposes the low-level pick/raycast query, but it requires ECS query
/// state that should remain inside the client adapter. Until that adapter is
/// needed by interaction code, loaded targets are reported as Unknown rather
/// than pretending that a clear line of sight was proven.
pub fn can_see(_eye: PositionSnapshot, _target: PositionSnapshot) -> Visibility {
    Visibility::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_is_an_upgradeable_abstraction() {
        assert_eq!(
            can_see(PositionSnapshot::default(), PositionSnapshot::default()),
            Visibility::Unknown
        );
    }
}
