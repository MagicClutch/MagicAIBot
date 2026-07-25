mod ai;
mod app;
mod config;
mod console;
mod error;
mod logging;
pub mod minecraft;
mod movement;
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
