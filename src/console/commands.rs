//! Parsing for local terminal input. Execution belongs to the application layer.

use crate::{
    blocks::block_query::normalize_block_id,
    error::AppError,
    movement::commands::{parse_coordinates, parse_follow_name},
};

#[derive(Debug, Eq, PartialEq)]
pub enum ConsoleCommand {
    Help,
    Status,
    Chat {
        message: String,
    },
    Players,
    Inventory,
    Entities {
        radius: Option<u32>,
    },
    Goto {
        x: i32,
        y: i32,
        z: i32,
    },
    Stop,
    Follow {
        player: String,
    },
    Movement,
    FindBlock {
        block_id: String,
        radius: Option<u32>,
        limit: Option<usize>,
    },
    NearestBlock {
        block_id: String,
        radius: Option<u32>,
    },
    Reconnect,
    Quit,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConsoleInput {
    Command(ConsoleCommand),
    ChatMessage(String),
    Empty,
}

#[must_use]
pub fn plain_chat_message(input: &ConsoleInput, enabled: bool) -> Option<&str> {
    if enabled && let ConsoleInput::ChatMessage(message) = input {
        return Some(message);
    }
    None
}

pub fn parse_input(input: &str) -> Result<ConsoleInput, AppError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(ConsoleInput::Empty);
    }
    if !input.starts_with('/') {
        return Ok(ConsoleInput::ChatMessage(input.to_owned()));
    }

    let command_line = input[1..].trim();
    let (command, arguments) = command_line
        .split_once(char::is_whitespace)
        .map_or((command_line, ""), |(command, arguments)| {
            (command, arguments.trim())
        });

    let parsed = match command.to_ascii_lowercase().as_str() {
        "help" => no_arguments(command, arguments, ConsoleCommand::Help)?,
        "status" => no_arguments(command, arguments, ConsoleCommand::Status)?,
        "players" => no_arguments(command, arguments, ConsoleCommand::Players)?,
        "inventory" => no_arguments(command, arguments, ConsoleCommand::Inventory)?,
        "entities" => {
            let radius = if arguments.is_empty() {
                None
            } else {
                Some(arguments.parse().map_err(|_| {
                    AppError::InvalidEntityQuery(
                        "radius must be a positive integer no greater than 256".into(),
                    )
                })?)
            };
            if radius.is_some_and(|value: u32| value == 0 || value > 256) {
                return Err(AppError::InvalidEntityQuery(
                    "radius must be between 1 and 256".into(),
                ));
            }
            ConsoleCommand::Entities { radius }
        }
        "goto" => {
            let destination = parse_coordinates(arguments)?;
            ConsoleCommand::Goto {
                x: destination.x as i32,
                y: destination.y as i32,
                z: destination.z as i32,
            }
        }
        "stop" => no_arguments(command, arguments, ConsoleCommand::Stop)?,
        "follow" => ConsoleCommand::Follow {
            player: parse_follow_name(arguments)?,
        },
        "movement" => no_arguments(command, arguments, ConsoleCommand::Movement)?,
        "findblock" => parse_find_block(arguments)?,
        "nearestblock" => parse_nearest_block(arguments)?,
        "reconnect" => no_arguments(command, arguments, ConsoleCommand::Reconnect)?,
        "quit" => no_arguments(command, arguments, ConsoleCommand::Quit)?,
        "chat" => {
            if arguments.is_empty() {
                return Err(AppError::MissingConsoleArgument(
                    "/chat <message>".to_owned(),
                ));
            }
            ConsoleCommand::Chat {
                message: arguments.to_owned(),
            }
        }
        unknown => return Err(AppError::UnknownConsoleCommand(unknown.to_owned())),
    };

    Ok(ConsoleInput::Command(parsed))
}

fn parse_find_block(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let mut parts = arguments.split_whitespace();
    let block_id = parts.next().ok_or_else(|| {
        AppError::MissingConsoleArgument("/findblock <block_id> [radius] [limit]".into())
    })?;
    let radius = parts
        .next()
        .map(|value| {
            value.parse().map_err(|_| {
                AppError::InvalidEntityQuery("radius must be a positive integer".into())
            })
        })
        .transpose()?;
    let limit = parts
        .next()
        .map(|value| {
            value.parse().map_err(|_| {
                AppError::InvalidEntityQuery("limit must be a positive integer".into())
            })
        })
        .transpose()?;
    if parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(
            "/findblock <block_id> [radius] [limit]".into(),
        ));
    }
    Ok(ConsoleCommand::FindBlock {
        block_id: normalize_block_id(block_id)?,
        radius,
        limit,
    })
}

fn parse_nearest_block(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let mut parts = arguments.split_whitespace();
    let block_id = parts.next().ok_or_else(|| {
        AppError::MissingConsoleArgument("/nearestblock <block_id> [radius]".into())
    })?;
    let radius = parts
        .next()
        .map(|value| {
            value.parse().map_err(|_| {
                AppError::InvalidEntityQuery("radius must be a positive integer".into())
            })
        })
        .transpose()?;
    if parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(
            "/nearestblock <block_id> [radius]".into(),
        ));
    }
    Ok(ConsoleCommand::NearestBlock {
        block_id: normalize_block_id(block_id)?,
        radius,
    })
}

fn no_arguments(
    command: &str,
    arguments: &str,
    parsed: ConsoleCommand,
) -> Result<ConsoleCommand, AppError> {
    if arguments.is_empty() {
        Ok(parsed)
    } else {
        Err(AppError::InvalidConsoleSyntax(format!(
            "/{command} does not accept arguments"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_input() {
        assert_eq!(parse_input(" \t ").unwrap(), ConsoleInput::Empty);
    }

    #[test]
    fn parses_plain_chat_with_trimmed_whitespace() {
        assert_eq!(
            parse_input("  hello server  ").unwrap(),
            ConsoleInput::ChatMessage("hello server".to_owned())
        );
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse_input("/help").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Help)
        );
        assert_eq!(
            parse_input("/findblock stone 64 5").unwrap(),
            ConsoleInput::Command(ConsoleCommand::FindBlock {
                block_id: "minecraft:stone".into(),
                radius: Some(64),
                limit: Some(5),
            })
        );
        assert_eq!(
            parse_input("/status").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Status)
        );
        assert_eq!(
            parse_input("/chat hello").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Chat {
                message: "hello".to_owned()
            })
        );
    }

    #[test]
    fn rejects_missing_chat_argument() {
        assert!(matches!(
            parse_input("/chat"),
            Err(AppError::MissingConsoleArgument(_))
        ));
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(matches!(
            parse_input("/unknown"),
            Err(AppError::UnknownConsoleCommand(_))
        ));
    }

    #[test]
    fn rejects_arguments_for_simple_commands() {
        assert!(matches!(
            parse_input("/help now"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
    }

    #[test]
    fn rejects_invalid_block_queries() {
        assert!(parse_input("/findblock").is_err());
        assert!(parse_input("/findblock minecraft:stone 0").is_ok());
        assert!(parse_input("/findblock minecraft:stone abc").is_err());
        assert!(parse_input("/nearestblock minecraft:stone 32 4").is_err());
    }

    #[test]
    fn disabled_plain_input_is_not_forwarded() {
        let input = ConsoleInput::ChatMessage("hello".to_owned());
        assert_eq!(plain_chat_message(&input, false), None);
        assert_eq!(plain_chat_message(&input, true), Some("hello"));
    }
}
