//! Pure offhand item selection. No Azalea types, no I/O.

use crate::config::OffhandPriority;

pub const TOTEM_OF_UNDYING: &str = "minecraft:totem_of_undying";
pub const SHIELD: &str = "minecraft:shield";

/// Which item id (if any) the offhand should hold, given whether a totem
/// and/or shield exist anywhere in the main inventory. `None` means "leave
/// the offhand exactly as it is" -- neither preferred item is available, so
/// whatever's already there (a torch, nothing, anything else) stays put.
pub fn desired_item(
    priority: OffhandPriority,
    has_totem: bool,
    has_shield: bool,
) -> Option<&'static str> {
    let order: [(bool, &str); 2] = match priority {
        OffhandPriority::Totem => [(has_totem, TOTEM_OF_UNDYING), (has_shield, SHIELD)],
        OffhandPriority::Shield => [(has_shield, SHIELD), (has_totem, TOTEM_OF_UNDYING)],
    };
    order
        .into_iter()
        .find_map(|(present, id)| present.then_some(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totem_priority_prefers_totem_when_both_are_available() {
        assert_eq!(
            desired_item(OffhandPriority::Totem, true, true),
            Some(TOTEM_OF_UNDYING)
        );
    }

    #[test]
    fn totem_priority_falls_back_to_shield() {
        assert_eq!(
            desired_item(OffhandPriority::Totem, false, true),
            Some(SHIELD)
        );
    }

    #[test]
    fn shield_priority_prefers_shield_when_both_are_available() {
        assert_eq!(
            desired_item(OffhandPriority::Shield, true, true),
            Some(SHIELD)
        );
    }

    #[test]
    fn shield_priority_falls_back_to_totem() {
        assert_eq!(
            desired_item(OffhandPriority::Shield, true, false),
            Some(TOTEM_OF_UNDYING)
        );
    }

    #[test]
    fn neither_available_leaves_the_offhand_unchanged() {
        assert_eq!(desired_item(OffhandPriority::Totem, false, false), None);
        assert_eq!(desired_item(OffhandPriority::Shield, false, false), None);
    }
}
