mod app;
mod blocks;
mod bridging;
mod config;
mod console;
mod container;
mod control;
mod equipment;
mod error;
mod interaction;
mod items;
mod logging;
mod look;
pub mod minecraft;
mod mobs;
mod movement;
mod navigation;
mod survival;
mod tasks;

fn main() -> Result<(), anyhow::Error> {
    // Azalea's connection/bootstrap path can temporarily use more stack than
    // the platform's default main-thread stack. Keep the async runtime on a
    // dedicated, larger-stack thread so a deep protocol setup cannot abort the
    // process with STATUS_STACK_OVERFLOW.
    std::thread::Builder::new()
        .name("magic-ai-runtime".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(tokio::task::LocalSet::new().run_until(async {
                let app = app::App::initialize().await?;
                app.run().await
            }))
        })?
        .join()
        .map_err(|_| anyhow::anyhow!("runtime thread panicked"))??;
    Ok(())
}
