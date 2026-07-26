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
    GotoMine {
        x: i32,
        y: i32,
        z: i32,
    },
    PathStatus,
    Stop,
    StopAll,
    TaskStatus,
    GatherStatus,
    GatherCancel,
    TaskStatusById {
        id: u64,
    },
    TaskCancel {
        id: Option<u64>,
    },
    TaskRecent,
    Gather {
        target: String,
        quantity: u32,
    },
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
    GotoBlock {
        block_id: String,
        search_radius: Option<u32>,
        allow_mining: bool,
    },
    GotoBlockStatus,
    CancelGotoBlock,
    Look {
        x: i32,
        y: i32,
        z: i32,
    },
    LookBlock {
        block_id: String,
    },
    LookPlayer {
        player: String,
    },
    LookEntity {
        entity_type: String,
    },
    LookStop,
    LookStatus,
    BreakBlock,
    Break {
        x: i32,
        y: i32,
        z: i32,
    },
    BreakNearest {
        block_id: String,
    },
    PlaceLooked {
        block_id: String,
    },
    PlaceAt {
        x: i32,
        y: i32,
        z: i32,
        block_id: String,
    },
    StopInteraction,
    InteractionStatus,
    CollectFood {
        item: Option<String>,
        count: u32,
        food_value: bool,
    },
    CollectFoodStatus,
    CollectFoodStop,
    EnsureTool {
        block_id: String,
    },
    TestOakLog,
    OpenChest {
        x: i32,
        y: i32,
        z: i32,
    },
    TakeItem {
        item_id: String,
        count: u32,
    },
    StoreItem {
        item_id: String,
        count: u32,
    },
    ContainerStatus,
    CloseContainer,
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
        "goto-mine" => {
            let destination = parse_coordinates(arguments)?;
            ConsoleCommand::GotoMine {
                x: destination.x as i32,
                y: destination.y as i32,
                z: destination.z as i32,
            }
        }
        "path-status" => no_arguments(command, arguments, ConsoleCommand::PathStatus)?,
        "stop" | "stopmovement" => no_arguments(command, arguments, ConsoleCommand::Stop)?,
        "stopall" => no_arguments(command, arguments, ConsoleCommand::StopAll)?,
        "taskstatus" => no_arguments(command, arguments, ConsoleCommand::TaskStatus)?,
        "gather" => parse_gather(arguments)?,
        "gatherstatus" => no_arguments(command, arguments, ConsoleCommand::GatherStatus)?,
        "gathercancel" => no_arguments(command, arguments, ConsoleCommand::GatherCancel)?,
        "task" => parse_task(arguments)?,
        "tasks" => no_arguments(command, arguments, ConsoleCommand::TaskStatus)?,
        "follow" => ConsoleCommand::Follow {
            player: parse_follow_name(arguments)?,
        },
        "movement" => no_arguments(command, arguments, ConsoleCommand::Movement)?,
        "findblock" => parse_find_block(arguments)?,
        "nearestblock" => parse_nearest_block(arguments)?,
        "gotoblock" | "navigate-to-block" => parse_goto_block(arguments)?,
        "gotoblockstatus" => no_arguments(command, arguments, ConsoleCommand::GotoBlockStatus)?,
        "cancelgotoblock" => no_arguments(command, arguments, ConsoleCommand::CancelGotoBlock)?,
        "look" | "lookat" => {
            let position = parse_coordinates(arguments)?;
            ConsoleCommand::Look {
                x: position.x as i32,
                y: position.y as i32,
                z: position.z as i32,
            }
        }
        "lookblock" => ConsoleCommand::LookBlock {
            block_id: normalize_block_id(single_argument(
                command,
                arguments,
                "/lookblock <block_id>",
            )?)?,
        },
        "lookplayer" => ConsoleCommand::LookPlayer {
            player: parse_follow_name(arguments)?,
        },
        "lookentity" => ConsoleCommand::LookEntity {
            entity_type: single_argument(command, arguments, "/lookentity <entity_type>")?
                .to_ascii_lowercase(),
        },
        "lookstop" => no_arguments(command, arguments, ConsoleCommand::LookStop)?,
        "lookstatus" => no_arguments(command, arguments, ConsoleCommand::LookStatus)?,
        "breakblock" => no_arguments(command, arguments, ConsoleCommand::BreakBlock)?,
        "break" => {
            let position = parse_coordinates(arguments)?;
            ConsoleCommand::Break {
                x: position.x as i32,
                y: position.y as i32,
                z: position.z as i32,
            }
        }
        "breaknearest" => ConsoleCommand::BreakNearest {
            block_id: normalize_block_id(single_argument(
                command,
                arguments,
                "/breaknearest <block_id>",
            )?)?,
        },
        "place" => parse_place(arguments)?,
        "placeblock" => parse_placeblock(arguments)?,
        "stopinteraction" => no_arguments(command, arguments, ConsoleCommand::StopInteraction)?,
        "interactionstatus" => no_arguments(command, arguments, ConsoleCommand::InteractionStatus)?,
        "collect-food" => parse_collect_food(arguments)?,
        "collect-food-status" => {
            no_arguments(command, arguments, ConsoleCommand::CollectFoodStatus)?
        }
        "collect-food-stop" => no_arguments(command, arguments, ConsoleCommand::CollectFoodStop)?,
        "ensure-tool" | "craft-tool" => ConsoleCommand::EnsureTool {
            block_id: normalize_block_id(single_argument(
                command,
                arguments,
                "/ensure-tool <block_id>",
            )?)?,
        },
        "testoaklog" => no_arguments(command, arguments, ConsoleCommand::TestOakLog)?,
        "open-chest" => {
            let p = parse_coordinates(arguments)?;
            ConsoleCommand::OpenChest {
                x: p.x as i32,
                y: p.y as i32,
                z: p.z as i32,
            }
        }
        "take-item" => parse_container_transfer(arguments, true)?,
        "store-item" => parse_container_transfer(arguments, false)?,
        "container-status" => no_arguments(command, arguments, ConsoleCommand::ContainerStatus)?,
        "close-container" => no_arguments(command, arguments, ConsoleCommand::CloseContainer)?,
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

fn normalize_item_id(value: &str) -> Result<String, AppError> {
    let id = if value.contains(':') {
        value.to_ascii_lowercase()
    } else {
        format!("minecraft:{}", value.to_ascii_lowercase())
    };
    if id.split_once(':').is_some_and(|(namespace, path)| {
        !namespace.is_empty()
            && !path.is_empty()
            && namespace.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
            })
            && path.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/')
            })
    }) {
        Ok(id)
    } else {
        Err(AppError::InvalidConsoleSyntax(
            "invalid item identifier".into(),
        ))
    }
}

fn parse_container_transfer(arguments: &str, take: bool) -> Result<ConsoleCommand, AppError> {
    let mut parts = arguments.split_whitespace();
    let item = normalize_item_id(
        parts
            .next()
            .ok_or_else(|| AppError::MissingConsoleArgument("<item> <count>".into()))?,
    )?;
    let count: u32 = parts
        .next()
        .ok_or_else(|| AppError::MissingConsoleArgument("<item> <count>".into()))?
        .parse()
        .map_err(|_| AppError::InvalidConsoleSyntax("count must be positive".into()))?;
    if count == 0 || parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(
            "<item> <positive-count>".into(),
        ));
    }
    Ok(if take {
        ConsoleCommand::TakeItem {
            item_id: item,
            count,
        }
    } else {
        ConsoleCommand::StoreItem {
            item_id: item,
            count,
        }
    })
}

fn parse_collect_food(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    let parsed = match parts.as_slice() {
        [] => ConsoleCommand::CollectFood {
            item: None,
            count: 1,
            food_value: false,
        },
        ["status"] => ConsoleCommand::CollectFoodStatus,
        ["stop"] => ConsoleCommand::CollectFoodStop,
        ["value", value] => ConsoleCommand::CollectFood {
            item: None,
            count: value.parse().map_err(|_| {
                AppError::InvalidConsoleSyntax("/collect-food value <positive points>".into())
            })?,
            food_value: true,
        },
        [item, count] => ConsoleCommand::CollectFood {
            item: Some(normalize_item_id(item)?),
            count: count.parse().map_err(|_| {
                AppError::InvalidConsoleSyntax("/collect-food <item> <positive count>".into())
            })?,
            food_value: false,
        },
        _ => {
            return Err(AppError::InvalidConsoleSyntax(
                "/collect-food [<item> <count>|value <points>]".into(),
            ));
        }
    };
    if matches!(parsed, ConsoleCommand::CollectFood { count: 0, .. }) {
        return Err(AppError::InvalidConsoleSyntax(
            "collection target must be positive".into(),
        ));
    }
    Ok(parsed)
}

fn parse_task(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        ["status", id] => Ok(ConsoleCommand::TaskStatusById {
            id: id
                .parse()
                .map_err(|_| AppError::InvalidConsoleSyntax("/task status <id>".into()))?,
        }),
        ["cancel", "all"] => Ok(ConsoleCommand::TaskCancel { id: None }),
        ["cancel", id] => Ok(ConsoleCommand::TaskCancel {
            id: Some(
                id.parse()
                    .map_err(|_| AppError::InvalidConsoleSyntax("/task cancel <id|all>".into()))?,
            ),
        }),
        ["recent"] | ["history"] => Ok(ConsoleCommand::TaskRecent),
        _ => Err(AppError::InvalidConsoleSyntax(
            "/task status <id> | /task cancel <id|all> | /task recent".into(),
        )),
    }
}

fn parse_gather(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    let [target, quantity] = parts.as_slice() else {
        return Err(AppError::InvalidConsoleSyntax(
            "/gather <item|logs|stone|ores|food> <quantity>".into(),
        ));
    };
    let quantity = quantity.parse().map_err(|_| {
        AppError::InvalidConsoleSyntax("gather quantity must be a positive integer".into())
    })?;
    if quantity == 0 {
        return Err(AppError::InvalidConsoleSyntax(
            "gather quantity must be positive".into(),
        ));
    }
    Ok(ConsoleCommand::Gather {
        target: target.to_ascii_lowercase(),
        quantity,
    })
}

fn parse_place(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        [block_id] => Ok(ConsoleCommand::PlaceLooked {
            block_id: normalize_block_id(block_id)?,
        }),
        [x, y, z, block_id] => Ok(ConsoleCommand::PlaceAt {
            x: x.parse()
                .map_err(|_| AppError::InvalidCoordinates("x must be an integer".into()))?,
            y: y.parse()
                .map_err(|_| AppError::InvalidCoordinates("y must be an integer".into()))?,
            z: z.parse()
                .map_err(|_| AppError::InvalidCoordinates("z must be an integer".into()))?,
            block_id: normalize_block_id(block_id)?,
        }),
        _ => Err(AppError::InvalidConsoleSyntax(
            "/place <block_id> or /place <x> <y> <z> <block_id>".into(),
        )),
    }
}

/// `/placeblock` keeps the task-oriented block-first syntax while mapping to
/// the same placement command variants and workflow as `/place`.
fn parse_placeblock(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        [block_id] => Ok(ConsoleCommand::PlaceLooked {
            block_id: normalize_block_id(block_id)?,
        }),
        [block_id, x, y, z] => Ok(ConsoleCommand::PlaceAt {
            x: x.parse()
                .map_err(|_| AppError::InvalidCoordinates("x must be an integer".into()))?,
            y: y.parse()
                .map_err(|_| AppError::InvalidCoordinates("y must be an integer".into()))?,
            z: z.parse()
                .map_err(|_| AppError::InvalidCoordinates("z must be an integer".into()))?,
            block_id: normalize_block_id(block_id)?,
        }),
        _ => Err(AppError::InvalidConsoleSyntax(
            "/placeblock <block> or /placeblock <block> <x> <y> <z>".into(),
        )),
    }
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

fn parse_goto_block(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let mut parts = arguments.split_whitespace();
    let block_id = parts.next().ok_or_else(|| {
        AppError::MissingConsoleArgument("/gotoblock <block_id> [search_radius]".into())
    })?;
    let mut allow_mining = false;
    let search_radius = match parts.next() {
        Some("mine") => {
            allow_mining = true;
            None
        }
        Some(value) => Some(value.parse().map_err(|_| {
            AppError::InvalidEntityQuery("search_radius must be a positive integer".into())
        })?),
        None => None,
    };
    if matches!(parts.next(), Some("mine")) {
        allow_mining = true;
    } else if parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(
            "/gotoblock <block_id> [search_radius] [mine]".into(),
        ));
    }
    Ok(ConsoleCommand::GotoBlock {
        block_id: normalize_block_id(block_id)?,
        search_radius,
        allow_mining,
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

fn single_argument<'a>(
    command: &str,
    arguments: &'a str,
    usage: &str,
) -> Result<&'a str, AppError> {
    let mut parts = arguments.split_whitespace();
    let value = parts
        .next()
        .ok_or_else(|| AppError::MissingConsoleArgument(usage.to_owned()))?;
    if parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(format!(
            "/{command} accepts one argument"
        )));
    }
    Ok(value)
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

    #[test]
    fn parses_block_navigation_commands() {
        assert_eq!(
            parse_input("/gotoblock stone 64").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GotoBlock {
                block_id: "minecraft:stone".into(),
                search_radius: Some(64),
                allow_mining: false,
            })
        );
        assert_eq!(
            parse_input("/navigate-to-block stone mine").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GotoBlock {
                block_id: "minecraft:stone".into(),
                search_radius: None,
                allow_mining: true,
            })
        );
        assert_eq!(
            parse_input("/goto-mine 1 64 -2").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GotoMine { x: 1, y: 64, z: -2 })
        );
        assert_eq!(
            parse_input("/gotoblockstatus").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GotoBlockStatus)
        );
        assert_eq!(
            parse_input("/cancelgotoblock").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CancelGotoBlock)
        );
    }

    #[test]
    fn parses_task_runtime_and_gather_commands() {
        assert_eq!(
            parse_input("/task status 42").unwrap(),
            ConsoleInput::Command(ConsoleCommand::TaskStatusById { id: 42 })
        );
        assert_eq!(
            parse_input("/task cancel all").unwrap(),
            ConsoleInput::Command(ConsoleCommand::TaskCancel { id: None })
        );
        assert_eq!(
            parse_input("/task recent").unwrap(),
            ConsoleInput::Command(ConsoleCommand::TaskRecent)
        );
        assert_eq!(
            parse_input("/gather logs 16").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Gather {
                target: "logs".into(),
                quantity: 16
            })
        );
        assert!(parse_input("/gather stone 0").is_err());
    }

    #[test]
    fn parses_look_commands() {
        assert_eq!(
            parse_input("/look 1 64 -2").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Look { x: 1, y: 64, z: -2 })
        );
        assert_eq!(
            parse_input("/lookblock stone").unwrap(),
            ConsoleInput::Command(ConsoleCommand::LookBlock {
                block_id: "minecraft:stone".into()
            })
        );
        assert_eq!(
            parse_input("/lookplayer Steve").unwrap(),
            ConsoleInput::Command(ConsoleCommand::LookPlayer {
                player: "Steve".into()
            })
        );
        assert_eq!(
            parse_input("/lookentity zombie").unwrap(),
            ConsoleInput::Command(ConsoleCommand::LookEntity {
                entity_type: "zombie".into()
            })
        );
    }

    #[test]
    fn parses_independent_task_control_commands() {
        assert_eq!(
            parse_input("/stopmovement").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Stop)
        );
        assert_eq!(
            parse_input("/stopall").unwrap(),
            ConsoleInput::Command(ConsoleCommand::StopAll)
        );
        assert_eq!(
            parse_input("/taskstatus").unwrap(),
            ConsoleInput::Command(ConsoleCommand::TaskStatus)
        );
        assert_eq!(
            parse_input("/lookat 1 64 -2").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Look { x: 1, y: 64, z: -2 })
        );
    }

    #[test]
    fn parses_interaction_commands() {
        assert_eq!(
            parse_input("/breakblock").unwrap(),
            ConsoleInput::Command(ConsoleCommand::BreakBlock)
        );
        assert_eq!(
            parse_input("/break 1 64 -2").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Break { x: 1, y: 64, z: -2 })
        );
        assert_eq!(
            parse_input("/breaknearest oak_log").unwrap(),
            ConsoleInput::Command(ConsoleCommand::BreakNearest {
                block_id: "minecraft:oak_log".into()
            })
        );
        assert_eq!(
            parse_input("/place cobblestone").unwrap(),
            ConsoleInput::Command(ConsoleCommand::PlaceLooked {
                block_id: "minecraft:cobblestone".into()
            })
        );
        assert_eq!(
            parse_input("/place 1 64 -2 cobblestone").unwrap(),
            ConsoleInput::Command(ConsoleCommand::PlaceAt {
                x: 1,
                y: 64,
                z: -2,
                block_id: "minecraft:cobblestone".into()
            })
        );
        assert_eq!(
            parse_input("/placeblock oak_log 901 91 984").unwrap(),
            ConsoleInput::Command(ConsoleCommand::PlaceAt {
                x: 901,
                y: 91,
                z: 984,
                block_id: "minecraft:oak_log".into()
            })
        );
        assert_eq!(
            parse_input("/placeblock minecraft:oak_log").unwrap(),
            ConsoleInput::Command(ConsoleCommand::PlaceLooked {
                block_id: "minecraft:oak_log".into()
            })
        );
        assert!(parse_input("/place 1 2").is_err());
        assert_eq!(
            parse_input("/testoaklog").unwrap(),
            ConsoleInput::Command(ConsoleCommand::TestOakLog)
        );
    }
    #[test]
    fn parses_container_debug_commands() {
        assert_eq!(
            parse_input("/open-chest 1 64 -2").unwrap(),
            ConsoleInput::Command(ConsoleCommand::OpenChest { x: 1, y: 64, z: -2 })
        );
        assert_eq!(
            parse_input("/take-item diamond 3").unwrap(),
            ConsoleInput::Command(ConsoleCommand::TakeItem {
                item_id: "minecraft:diamond".into(),
                count: 3
            })
        );
        assert_eq!(
            parse_input("/store-item cobblestone 64").unwrap(),
            ConsoleInput::Command(ConsoleCommand::StoreItem {
                item_id: "minecraft:cobblestone".into(),
                count: 64
            })
        );
        assert!(parse_input("/take-item diamond 0").is_err());
    }

    #[test]
    fn parses_food_collection_and_controls() {
        assert_eq!(
            parse_input("/collect-food carrot 4").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CollectFood {
                item: Some("minecraft:carrot".into()),
                count: 4,
                food_value: false
            })
        );
        assert_eq!(
            parse_input("/collect-food value 12").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CollectFood {
                item: None,
                count: 12,
                food_value: true
            })
        );
        assert_eq!(
            parse_input("/collect-food status").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CollectFoodStatus)
        );
        assert_eq!(
            parse_input("/collect-food stop").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CollectFoodStop)
        );
        assert!(parse_input("/collect-food carrot 0").is_err());
    }
}
