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
    #[error("invalid world-state configuration: {0}")]
    InvalidWorldStateConfiguration(String),
    #[error("world state is unavailable")]
    WorldStateUnavailable,
    #[error("inventory is unavailable")]
    InventoryUnavailable,
    #[error("invalid entity query: {0}")]
    InvalidEntityQuery(String),
    #[error("world-state update failed: {0}")]
    WorldStateUpdateFailure(String),
    #[error("invalid movement configuration: {0}")]
    InvalidMovementConfiguration(String),
    #[error("invalid block-search configuration: {0}")]
    InvalidBlockSearchConfiguration(String),
    #[error("invalid block identifier: {0}")]
    InvalidBlockIdentifier(String),
    #[error("unknown block identifier: {0}")]
    UnknownBlockIdentifier(String),
    #[error("invalid block-search radius {radius}; maximum is {maximum}")]
    InvalidBlockSearchRadius { radius: u32, maximum: u32 },
    #[error("invalid block-search result limit {limit}; maximum is {maximum}")]
    InvalidBlockSearchResultLimit { limit: usize, maximum: usize },
    #[error("invalid block-search vertical range: {0}")]
    InvalidBlockSearchVerticalRange(u32),
    #[error("invalid block-navigation configuration: {0}")]
    InvalidBlockNavigationConfiguration(String),
    #[error("bot position is unavailable")]
    BlockSearchPositionUnavailable,
    #[error("bot dimension is unavailable")]
    BlockSearchDimensionUnavailable,
    #[error("no loaded chunks are available")]
    NoLoadedChunks,
    #[error("block search was cancelled")]
    BlockSearchCancelled,
    #[error("chunk became unavailable during block search")]
    ChunkUnavailableDuringSearch,
    #[error("block-state conversion failed: {0}")]
    BlockStateConversionFailure(String),
    #[error("block search is unavailable while disconnected")]
    BlockSearchUnavailable,
    #[error("no matching block found")]
    NoMatchingBlock,
    #[error("no valid approach position")]
    NoValidApproachPosition,
    #[error("target block disappeared")]
    TargetBlockDisappeared,
    #[error("target chunk unloaded")]
    TargetChunkUnloaded,
    #[error("movement stalled")]
    MovementStalled,
    #[error("block navigation timed out")]
    BlockNavigationTimeout,
    #[error("invalid coordinates: {0}")]
    InvalidCoordinates(String),
    #[error("unknown player: {0}")]
    UnknownPlayer(String),
    #[error("movement is unavailable while disconnected")]
    MovementUnavailable,
    #[error("pathfinding failed: {0}")]
    PathfindingFailure(String),
    #[error("movement was cancelled")]
    MovementCancelled,
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
