mod app;
mod blocks;
mod bridging;
mod collection;
mod config;
mod console;
mod container;
mod crafting;
mod error;
mod food;
mod interaction;
mod inventory;
mod inventory_cleanup;
mod logging;
mod look;
pub mod minecraft;
mod movement;
mod navigation;
mod processing;
mod skills;
mod smelting;
mod tasks;
mod tree_chopping;

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
