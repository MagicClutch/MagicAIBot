pub mod commands;
pub mod logger;
pub mod movement_service;
pub mod multitasking;
pub mod navigator;

pub use movement_service::MovementService;

/// The only application-level choice exposed to Azalea's pathfinder.  Route
/// generation, execution, recovery and mining execution remain Azalea's.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NavigationMode {
    #[default]
    MovementOnly,
    AllowMining,
}

impl NavigationMode {
    pub const fn allows_mining(self) -> bool {
        matches!(self, Self::AllowMining)
    }
}

#[cfg(test)]
mod tests {
    use super::NavigationMode;

    #[test]
    fn mining_is_opt_in() {
        assert!(!NavigationMode::MovementOnly.allows_mining());
        assert!(NavigationMode::AllowMining.allows_mining());
    }
}
