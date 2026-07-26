mod ai;
mod app;
mod blocks;
mod config;
mod console;
mod error;
mod food;
mod interaction;
mod logging;
mod look;
pub mod minecraft;
mod movement;
mod navigation;
mod skills;
mod tasks;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), error::AppError> {
    tokio::task::LocalSet::new()
        .run_until(async {
            let app = app::App::initialize().await?;
            app.run().await
        })
        .await
}
