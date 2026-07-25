use std::{path::Path, time::Instant};

use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    config::Config,
    console::{
        self,
        commands::{ConsoleCommand, ConsoleInput, plain_chat_message},
    },
    error::AppError,
    minecraft::client::MinecraftClient,
};

/// Application composition root. Future services will be owned by this type.
pub struct App {
    config: Config,
    shutdown: CancellationToken,
    minecraft: MinecraftClient,
    started_at: Instant,
}

impl App {
    /// Loads configuration and initializes logging without connecting to Minecraft.
    pub async fn initialize() -> Result<Self, AppError> {
        let _ = dotenvy::dotenv();
        println!("Starting Magic AI Bot...\n");
        println!("Loading configuration...\n");
        let config = Config::load(Path::new("config.toml"))?;
        crate::config::init_logging(&config.logging)?;
        info!(
            server = %config.minecraft.server,
            username = %config.minecraft.username,
            account_mode = ?config.minecraft.account_mode,
            "configuration loaded"
        );
        info!("application startup");

        Ok(Self {
            minecraft: MinecraftClient::new(
                config.minecraft.clone(),
                config.reconnect.clone(),
                config.console.clone(),
            ),
            config,
            shutdown: CancellationToken::new(),
            started_at: Instant::now(),
        })
    }

    /// Waits for Ctrl+C and performs the application's graceful shutdown.
    pub async fn run(mut self) -> Result<(), AppError> {
        println!("Connecting...\n");
        self.minecraft.connect().await?;
        println!("Connected!\n");
        println!("Joined world successfully.\n");

        let (input_tx, mut input_rx) = mpsc::channel(32);
        let console_task = self.config.console.enabled.then(|| {
            tokio::task::spawn_local(console::read_input(input_tx, self.shutdown.child_token()))
        });

        let loop_result = loop {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    result?;
                    break Ok(());
                }
                input = input_rx.recv() => match input {
                    Some(Ok(ConsoleInput::Empty)) => {}
                    Some(Ok(input)) => {
                        if self.execute_console_input(input).await? {
                            break Ok(());
                        }
                    }
                    Some(Err(error)) => println!("Console error: {error}"),
                    None => break Ok(()),
                }
            }
        };

        self.shutdown.cancel();
        self.minecraft.disconnect().await?;
        if let Some(task) = console_task {
            await_console_task(task).await;
        }
        info!(server = %self.config.minecraft.server, "application shutdown");
        loop_result
    }

    async fn execute_console_input(&mut self, input: ConsoleInput) -> Result<bool, AppError> {
        match input {
            ConsoleInput::ChatMessage(message) => {
                if let Some(message) = plain_chat_message(
                    &ConsoleInput::ChatMessage(message),
                    self.config.console.send_plain_input_to_chat,
                ) {
                    if let Err(error) = self.minecraft.send_chat(message).await {
                        println!("Chat error: {error}");
                    }
                } else {
                    println!("Plain console input forwarding is disabled.");
                }
            }
            ConsoleInput::Command(command) => match command {
                ConsoleCommand::Help => print_help(),
                ConsoleCommand::Status => self.print_status().await,
                ConsoleCommand::Chat { message } => {
                    if let Err(error) = self.minecraft.send_chat(&message).await {
                        println!("Chat error: {error}");
                    }
                }
                ConsoleCommand::Players => println!("Player tracking is not available yet."),
                ConsoleCommand::Reconnect => match self.minecraft.reconnect().await {
                    Ok(()) => println!("Reconnect successful."),
                    Err(error) => println!("Reconnect failed: {error}"),
                },
                ConsoleCommand::Quit => return Ok(true),
            },
            ConsoleInput::Empty => {}
        }
        Ok(false)
    }

    async fn print_status(&self) {
        let status = self.minecraft.status();
        let world = self.minecraft.world_state_snapshot().await;
        println!("Connection state: {:?}", status.connection_state);
        println!("Bot username: {}", status.username);
        println!("Server address: {}", status.server);
        println!("Account mode: {}", status.account_mode);
        println!("Joined world: {}", world.joined_world);
        println!("Current position: not available");
        println!("Reconnect enabled: {}", status.reconnect.enabled);
        println!(
            "Reconnect delay: {} seconds",
            status.reconnect.delay_seconds
        );
        println!(
            "Reconnect maximum attempts: {}",
            status.reconnect.maximum_attempts
        );
        println!(
            "Application uptime: {} seconds",
            self.started_at.elapsed().as_secs()
        );
    }
}

fn print_help() {
    println!("/help       Show available commands");
    println!("/status     Show connection and application status");
    println!("/chat TEXT  Send TEXT to Minecraft chat");
    println!("/players    Show known online players");
    println!("/reconnect  Reconnect to the configured server");
    println!("/quit       Shut down the application");
}

async fn await_console_task(task: JoinHandle<()>) {
    let _ = task.await;
}
