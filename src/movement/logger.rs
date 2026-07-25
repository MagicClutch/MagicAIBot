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
pub fn reached() {
    logging::success("Position reached");
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
