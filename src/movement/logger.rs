use crate::{logging, minecraft::world_state::PositionSnapshot};

pub fn going_to(destination: PositionSnapshot) {
    logging::info(format!(
        "Going to position ({:.0}, {:.0}, {:.0})",
        destination.x, destination.y, destination.z
    ));
}
pub fn following(name: &str) {
    logging::info(format!("Following player {name}"));
}
/// Deliberately `info`, not `success`: this fires once per movement
/// sub-goal reached (every waypoint of a path, not just a whole command's
/// final destination -- see `going_to`'s pairing), so tagging it a
/// milestone would flood chat with dozens of identical lines for a single
/// `/goto`/`#get`/`#mine` run and risk a spam kick.
pub fn reached() {
    logging::info("Position reached");
}
pub fn cannot_reach(reason: &str) {
    logging::warning(format!("Cannot reach destination ({reason})"));
}
pub fn cancelled() {
    logging::info("Movement cancelled");
}
pub fn lost_player(name: &str) {
    logging::warning(format!("Lost player {name}"));
}
pub fn stopped_following() {
    logging::info("Stopped following");
}
