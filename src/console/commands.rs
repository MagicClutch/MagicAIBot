//! Parsing for local terminal input. Execution belongs to the application layer.

use crate::error::AppError;

#[derive(Debug, Eq, PartialEq)]
pub enum ConsoleCommand {
    Help,
    Status,
    Chat { message: String },
    Players,
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
    fn disabled_plain_input_is_not_forwarded() {
        let input = ConsoleInput::ChatMessage("hello".to_owned());
        assert_eq!(plain_chat_message(&input, false), None);
        assert_eq!(plain_chat_message(&input, true), Some("hello"));
    }
}
