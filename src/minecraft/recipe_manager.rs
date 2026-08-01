//! Manages server-provided recipe data.
use std::collections::HashMap;

use azalea_protocol::packets::game::c_update_recipes::ClientboundUpdateRecipes;
use azalea_registry::identifier::Identifier;

/// Stores and provides access to server-provided recipe data.
#[derive(Debug, Default)]
pub struct RecipeManager {
    /// The raw data from the last ClientboundUpdateRecipes packet.
    raw_recipes: Option<ClientboundUpdateRecipes>,
    // TODO: Add processed recipe data structures here
}

impl RecipeManager {
    /// Updates the stored raw recipe data.
    pub fn update_raw_recipes(&mut self, packet: ClientboundUpdateRecipes) {
        self.raw_recipes = Some(packet);
        // TODO: Trigger processing of raw recipes into usable formats
    }

    /// Returns a reference to the raw recipe data.
    pub fn raw_recipes(&self) -> Option<&ClientboundUpdateRecipes> {
        self.raw_recipes.as_ref()
    }

    // TODO: Add methods to query processed recipes
}
