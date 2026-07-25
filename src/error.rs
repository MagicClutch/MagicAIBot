use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse configuration: {0}")]
    Config(#[from] toml::de::Error),
    #[error("invalid log level: {0}")]
    InvalidLogLevel(String),
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("authentication failed: {0}")]
    AuthenticationFailure(String),
    #[error("network timeout after {seconds} seconds")]
    NetworkTimeout { seconds: u64 },
    #[error("kicked by server: {0}")]
    KickedByServer(String),
    #[error("connection failed: {0}")]
    ConnectionFailure(String),
    #[error("cannot send chat while disconnected")]
    DisconnectedChatSend,
    #[error("chat message is empty")]
    EmptyChatMessage,
    #[error("chat message is too long: {length} characters, maximum is {maximum}")]
    ChatMessageTooLong { length: usize, maximum: usize },
    #[error("duplicate chat send suppressed")]
    DuplicateChatSend,
    #[error("unknown console command: {0}")]
    UnknownConsoleCommand(String),
    #[error("missing argument for console command: {0}")]
    MissingConsoleArgument(String),
    #[error("invalid console syntax: {0}")]
    InvalidConsoleSyntax(String),
    #[error("console input failed: {0}")]
    ConsoleInputFailure(String),
    #[error("reconnect is already in progress")]
    ReconnectAlreadyInProgress,
    #[error("failed to initialize logging: {0}")]
    Logging(#[source] Box<dyn std::error::Error + Send + Sync>),
}
