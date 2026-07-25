//! The application's boundary around Azalea's Minecraft client.
//!
//! Azalea types are deliberately kept in this module. Callers observe only
//! our connection state and application errors.

use std::{path::PathBuf, sync::Arc, time::Duration};

use azalea::{
    Client, Event,
    account::{Account, microsoft::MicrosoftAccountOpts},
    auto_reconnect::AutoReconnectDelay,
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    config::{AccountMode, ConsoleConfig, MinecraftConfig, ReconnectConfig},
    error::AppError,
    minecraft::{
        events::handle_chat,
        world_state::{WorldState, WorldStateSnapshot},
    },
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CHAT_MESSAGE_LENGTH: usize = 256;

enum DisconnectReason {
    Kicked(String),
    ConnectionFailure(String),
}

/// The lifecycle state of the Minecraft connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Authenticating,
    JoiningWorld,
    Connected,
    Reconnecting,
}

/// Owns the Azalea client and supervises its connection lifecycle.
pub struct MinecraftClient {
    minecraft: MinecraftConfig,
    reconnect: ReconnectConfig,
    console: ConsoleConfig,
    state: watch::Receiver<ConnectionState>,
    state_tx: watch::Sender<ConnectionState>,
    current_client: Arc<Mutex<Option<Client>>>,
    world_state: Arc<Mutex<WorldState>>,
    shutdown: CancellationToken,
    supervisor: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug)]
pub struct MinecraftStatus {
    pub connection_state: ConnectionState,
    pub server: String,
    pub username: String,
    pub account_mode: String,
    pub reconnect: ReconnectConfig,
}

struct SupervisorContext {
    minecraft: MinecraftConfig,
    reconnect: ReconnectConfig,
    current_client: Arc<Mutex<Option<Client>>>,
    world_state: Arc<Mutex<WorldState>>,
    console: ConsoleConfig,
    state_tx: watch::Sender<ConnectionState>,
    shutdown: CancellationToken,
}

impl MinecraftClient {
    pub fn new(
        minecraft: MinecraftConfig,
        reconnect: ReconnectConfig,
        console: ConsoleConfig,
    ) -> Self {
        let (state_tx, state) = watch::channel(ConnectionState::Disconnected);
        Self {
            minecraft,
            reconnect,
            console,
            state,
            state_tx,
            current_client: Arc::new(Mutex::new(None)),
            world_state: Arc::new(Mutex::new(WorldState::default())),
            shutdown: CancellationToken::new(),
            supervisor: None,
        }
    }

    /// Connects and waits until Azalea reports that the bot has spawned.
    pub async fn connect(&mut self) -> Result<(), AppError> {
        if self.supervisor.is_some() {
            return Err(AppError::ConnectionFailure(
                "Minecraft client is already running".to_owned(),
            ));
        }

        self.set_state(ConnectionState::Connecting);
        info!(server = %self.minecraft.server, "connecting");

        let (client, events) = self.join_once().await?;
        self.disable_azalea_reconnect(&client);
        *self.current_client.lock().await = Some(client.clone());

        self.set_state(ConnectionState::JoiningWorld);
        let supervisor = tokio::task::spawn_local(supervise(
            events,
            SupervisorContext {
                minecraft: self.minecraft.clone(),
                reconnect: self.reconnect.clone(),
                current_client: self.current_client.clone(),
                world_state: self.world_state.clone(),
                console: self.console.clone(),
                state_tx: self.state_tx.clone(),
                shutdown: self.shutdown.clone(),
            },
        ));
        self.supervisor = Some(supervisor);

        let mut state = self.state.clone();
        let result = timeout(CONNECTION_TIMEOUT, async {
            loop {
                let current_state = *state.borrow();
                match current_state {
                    ConnectionState::Connected => return Ok(()),
                    ConnectionState::Disconnected => {
                        return Err(AppError::ConnectionFailure(
                            "connection closed before the bot joined the world".to_owned(),
                        ));
                    }
                    _ => state.changed().await.map_err(|_| {
                        AppError::ConnectionFailure("connection supervisor stopped".to_owned())
                    })?,
                }
            }
        })
        .await;

        match result {
            Ok(result) => result,
            Err(_) => Err(AppError::NetworkTimeout {
                seconds: CONNECTION_TIMEOUT.as_secs(),
            }),
        }
    }

    /// Disconnects the current Azalea client and stops supervision.
    pub async fn disconnect(&mut self) -> Result<(), AppError> {
        self.shutdown.cancel();
        if let Some(client) = self.current_client.lock().await.take() {
            client.disconnect();
        }
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.await;
        }
        self.world_state.lock().await.set_joined_world(false);
        self.set_state(ConnectionState::Disconnected);
        info!("disconnected");
        Ok(())
    }

    /// Stops the current connection and establishes a fresh one.
    pub async fn reconnect(&mut self) -> Result<(), AppError> {
        if self.connection_state() == ConnectionState::Reconnecting {
            return Err(AppError::ReconnectAlreadyInProgress);
        }
        self.disconnect().await?;
        self.shutdown = CancellationToken::new();
        self.connect().await
    }

    #[must_use]
    pub fn connection_state(&self) -> ConnectionState {
        *self.state.borrow()
    }

    /// Sends a normal chat message through Azalea's supported chat API.
    pub async fn send_chat(&self, message: &str) -> Result<(), AppError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(AppError::EmptyChatMessage);
        }
        let length = message.chars().count();
        if length > MAX_CHAT_MESSAGE_LENGTH {
            return Err(AppError::ChatMessageTooLong {
                length,
                maximum: MAX_CHAT_MESSAGE_LENGTH,
            });
        }
        if self.connection_state() != ConnectionState::Connected {
            return Err(AppError::DisconnectedChatSend);
        }

        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::DisconnectedChatSend)?;
        if !self
            .world_state
            .lock()
            .await
            .record_sent(message.to_owned())
        {
            return Err(AppError::DuplicateChatSend);
        }
        client.chat(message.to_owned());
        info!(character_count = length, "chat message sent");
        Ok(())
    }

    #[must_use]
    pub async fn world_state_snapshot(&self) -> WorldStateSnapshot {
        self.world_state.lock().await.snapshot()
    }

    #[must_use]
    pub fn status(&self) -> MinecraftStatus {
        MinecraftStatus {
            connection_state: self.connection_state(),
            server: self.minecraft.server.clone(),
            username: self.minecraft.username.clone(),
            account_mode: match self.minecraft.account_mode {
                AccountMode::Offline => "offline".to_owned(),
                AccountMode::Microsoft => "microsoft".to_owned(),
            },
            reconnect: self.reconnect.clone(),
        }
    }

    async fn join_once(
        &self,
    ) -> Result<(Client, tokio::sync::mpsc::UnboundedReceiver<Event>), AppError> {
        let account = self.account().await?;
        timeout(
            CONNECTION_TIMEOUT,
            Client::join(account, self.minecraft.server.as_str()),
        )
        .await
        .map_err(|_| AppError::NetworkTimeout {
            seconds: CONNECTION_TIMEOUT.as_secs(),
        })?
        .map_err(|error| {
            AppError::ConnectionFailure(format!("could not resolve server: {error:?}"))
        })
    }

    async fn account(&self) -> Result<Account, AppError> {
        match self.minecraft.account_mode {
            AccountMode::Offline => Ok(Account::offline(&self.minecraft.username)),
            AccountMode::Microsoft => {
                self.set_state(ConnectionState::Authenticating);
                info!(username = %self.minecraft.username, "authenticating");
                let cache_file = auth_cache_file()?;
                tokio::fs::create_dir_all(cache_file.parent().ok_or_else(|| {
                    AppError::InvalidConfiguration("invalid auth cache path".to_owned())
                })?)
                .await?;
                Account::microsoft_with_opts(
                    &self.minecraft.username,
                    MicrosoftAccountOpts {
                        cache_file: Some(cache_file),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| AppError::AuthenticationFailure(error.to_string()))
            }
        }
    }

    fn disable_azalea_reconnect(&self, client: &Client) {
        client
            .ecs
            .write()
            .insert_resource(AutoReconnectDelay::new(Duration::MAX));
    }

    fn set_state(&self, state: ConnectionState) {
        let _ = self.state_tx.send(state);
    }
}

async fn supervise(
    mut events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    context: SupervisorContext,
) {
    let SupervisorContext {
        minecraft,
        reconnect,
        current_client,
        world_state,
        console,
        state_tx,
        shutdown,
    } = context;
    loop {
        let outcome =
            wait_for_disconnect(&mut events, &state_tx, &shutdown, &world_state, &console).await;
        if shutdown.is_cancelled() {
            return;
        }

        let Some(reason) = outcome else {
            let _ = state_tx.send(ConnectionState::Disconnected);
            return;
        };
        let _ = state_tx.send(ConnectionState::Disconnected);
        world_state.lock().await.set_joined_world(false);
        match &reason {
            DisconnectReason::Kicked(reason) => {
                info!(reason, "disconnected");
                let kick_error = AppError::KickedByServer(reason.clone());
                warn!(error = %kick_error, "kick reason");
            }
            DisconnectReason::ConnectionFailure(reason) => {
                let connection_error = AppError::ConnectionFailure(reason.clone());
                error!(error = %connection_error, "connection error");
            }
        }

        if !reconnect.enabled {
            return;
        }

        let mut reconnected = false;
        for attempt in 1..=reconnect.maximum_attempts {
            let _ = state_tx.send(ConnectionState::Reconnecting);
            info!(
                attempt,
                maximum_attempts = reconnect.maximum_attempts,
                "reconnect attempt"
            );
            if wait_for_reconnect_delay(reconnect.delay_seconds, &shutdown).await {
                return;
            }

            match join_for_reconnect(&minecraft, &state_tx, &shutdown).await {
                Ok((new_client, new_events)) => {
                    new_client
                        .ecs
                        .write()
                        .insert_resource(AutoReconnectDelay::new(Duration::MAX));
                    *current_client.lock().await = Some(new_client.clone());
                    events = new_events;
                    if wait_for_spawn(&mut events, &shutdown, &world_state, &console).await {
                        let _ = state_tx.send(ConnectionState::Connected);
                        info!("reconnect success");
                        reconnected = true;
                        break;
                    }
                }
                Err(error) => {
                    error!(attempt, error = %error, "reconnect failure");
                }
            }
        }

        if !reconnected {
            warn!("reconnect attempts exhausted");
            let _ = state_tx.send(ConnectionState::Disconnected);
            return;
        }
    }
}

async fn join_for_reconnect(
    minecraft: &MinecraftConfig,
    state_tx: &watch::Sender<ConnectionState>,
    shutdown: &CancellationToken,
) -> Result<(Client, tokio::sync::mpsc::UnboundedReceiver<Event>), AppError> {
    let account = match minecraft.account_mode {
        AccountMode::Offline => Account::offline(&minecraft.username),
        AccountMode::Microsoft => {
            let cache_file = auth_cache_file()?;
            tokio::fs::create_dir_all(cache_file.parent().ok_or_else(|| {
                AppError::InvalidConfiguration("invalid auth cache path".to_owned())
            })?)
            .await?;
            let _ = state_tx.send(ConnectionState::Authenticating);
            Account::microsoft_with_opts(
                &minecraft.username,
                MicrosoftAccountOpts {
                    cache_file: Some(cache_file),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| AppError::AuthenticationFailure(error.to_string()))?
        }
    };

    let join = Client::join(account, minecraft.server.as_str());
    tokio::select! {
        _ = shutdown.cancelled() => Err(AppError::ConnectionFailure("shutdown requested".to_owned())),
        result = timeout(CONNECTION_TIMEOUT, join) => result
            .map_err(|_| AppError::NetworkTimeout { seconds: CONNECTION_TIMEOUT.as_secs() })?
            .map_err(|error| AppError::ConnectionFailure(format!("could not resolve server: {error:?}"))),
    }
}

async fn wait_for_disconnect(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    state_tx: &watch::Sender<ConnectionState>,
    shutdown: &CancellationToken,
    world_state: &Arc<Mutex<WorldState>>,
    console: &ConsoleConfig,
) -> Option<DisconnectReason> {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return None,
            event = events.recv() => match event? {
                Event::Spawn => {
                    let _ = state_tx.send(ConnectionState::Connected);
                    world_state.lock().await.set_joined_world(true);
                    info!("joined world");
                }
                Event::Chat(packet) => {
                    let mut world = world_state.lock().await;
                    handle_chat(&packet, console, &mut world);
                }
                Event::Disconnect(reason) => return Some(DisconnectReason::Kicked(reason.map_or_else(|| "server closed the connection".to_owned(), |reason| reason.to_string()))),
                Event::ConnectionFailed(error) => return Some(DisconnectReason::ConnectionFailure(error.to_string())),
                _ => {}
            }
        }
    }
}

async fn wait_for_spawn(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    shutdown: &CancellationToken,
    world_state: &Arc<Mutex<WorldState>>,
    console: &ConsoleConfig,
) -> bool {
    matches!(
        timeout(CONNECTION_TIMEOUT, async {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => return false,
                    event = events.recv() => match event {
                        Some(Event::Spawn) => {
                            world_state.lock().await.set_joined_world(true);
                            return true;
                        }
                        Some(Event::Chat(packet)) => {
                            let mut world = world_state.lock().await;
                            handle_chat(&packet, console, &mut world);
                        }
                        Some(Event::Disconnect(reason)) => {
                            warn!(reason = ?reason, "reconnect connection dropped before spawn");
                            return false;
                        }
                        Some(Event::ConnectionFailed(error)) => {
                            warn!(error = %error, "reconnect connection failed before spawn");
                            return false;
                        }
                        Some(_) => {}
                        None => return false,
                    }
                }
            }
        })
        .await,
        Ok(true)
    )
}

async fn wait_for_reconnect_delay(seconds: u64, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => true,
        _ = sleep(Duration::from_secs(seconds)) => false,
    }
}

fn auth_cache_file() -> Result<PathBuf, AppError> {
    Ok(PathBuf::from("auth-cache").join("azalea-auth.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> MinecraftClient {
        MinecraftClient::new(
            MinecraftConfig {
                server: "localhost:25565".to_owned(),
                username: "MagicBot".to_owned(),
                account_mode: AccountMode::Offline,
            },
            ReconnectConfig {
                enabled: false,
                delay_seconds: 10,
                maximum_attempts: 5,
            },
            ConsoleConfig::default(),
        )
    }

    #[tokio::test]
    async fn rejects_empty_chat() {
        assert!(matches!(
            client().send_chat("   ").await,
            Err(AppError::EmptyChatMessage)
        ));
    }

    #[tokio::test]
    async fn rejects_chat_longer_than_protocol_limit() {
        let message = "x".repeat(MAX_CHAT_MESSAGE_LENGTH + 1);
        assert!(matches!(
            client().send_chat(&message).await,
            Err(AppError::ChatMessageTooLong { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_chat_when_disconnected() {
        assert!(matches!(
            client().send_chat("hello").await,
            Err(AppError::DisconnectedChatSend)
        ));
    }

    #[test]
    fn connection_state_transitions_are_runtime_independent() {
        let client = client();
        assert_eq!(client.connection_state(), ConnectionState::Disconnected);
        client.set_state(ConnectionState::Connecting);
        assert_eq!(client.connection_state(), ConnectionState::Connecting);
        client.set_state(ConnectionState::Connected);
        assert_eq!(client.connection_state(), ConnectionState::Connected);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_signals_shutdown_and_rejects_duplicate_connection() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut client = client();
                let shutdown = client.shutdown.clone();
                client.supervisor = Some(tokio::task::spawn_local(async {}));

                assert!(matches!(
                    client.connect().await,
                    Err(AppError::ConnectionFailure(message))
                        if message.contains("already running")
                ));
                client
                    .disconnect()
                    .await
                    .expect("disconnect should succeed");
                assert!(shutdown.is_cancelled());
                assert_eq!(client.connection_state(), ConnectionState::Disconnected);
            })
            .await;
    }
}
