mod ai;
mod app;
mod blocks;
mod collection;
mod config;
mod console;
mod crafting;
mod container;
mod error;
mod food;
mod interaction;
mod inventory_cleanup;
mod logging;
mod look;
pub mod minecraft;
mod movement;
mod navigation;
mod smelting;
mod processing;
mod skills;
mod tasks;
mod tree_chopping;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), error::AppError> {
    tokio::task::LocalSet::new()
        .run_until(async {
            let app = app::App::initialize().await?;
            app.run().await
        })
        .await
}
