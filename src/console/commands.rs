//! Parsing for local terminal input. Execution belongs to the application layer.

use crate::{
    blocks::block_query::normalize_block_id,
    error::AppError,
    movement::commands::{parse_coordinates, parse_follow_name},
};

#[derive(Debug, PartialEq)]
pub enum ConsoleCommand {
    Help,
    Status,
    Where,
    Health,
    Chat {
        message: String,
    },
    Players,
    Inventory,
    ObservedContainerStatus,
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
    GetResource {
        resource_id: String,
        amount: u32,
    },
    Mine {
        block_ids: Vec<String>,
        amount: u32,
    },
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
    /// Debug-only: score the hotbar and select the policy winner for a block.
    SelectTool {
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
    InteractNearest {
        block_id: String,
        items: Vec<String>,
        radius: u32,
    },
    StopInteraction,
    InteractionStatus,
    Equip {
        item: String,
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

#[derive(Debug, PartialEq)]
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
        "where" => no_arguments(command, arguments, ConsoleCommand::Where)?,
        "health" => no_arguments(command, arguments, ConsoleCommand::Health)?,
        "players" => no_arguments(command, arguments, ConsoleCommand::Players)?,
        "inventory" => no_arguments(command, arguments, ConsoleCommand::Inventory)?,
        "containerstatus" => {
            no_arguments(command, arguments, ConsoleCommand::ObservedContainerStatus)?
        }
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
        "follow" => ConsoleCommand::Follow {
            player: parse_follow_name(arguments)?,
        },
        "movement" => no_arguments(command, arguments, ConsoleCommand::Movement)?,
        "find" | "findblock" => parse_find_block(arguments)?,
        "nearestblock" => parse_nearest_block(arguments)?,
        "gotoblock" | "navigate-to-block" => parse_goto_block(arguments)?,
        "gotoblockstatus" => no_arguments(command, arguments, ConsoleCommand::GotoBlockStatus)?,
        "cancelgotoblock" => no_arguments(command, arguments, ConsoleCommand::CancelGotoBlock)?,
        "get" => parse_get(arguments)?,
        "mine" => parse_mine(arguments)?,
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
        "select-tool" => ConsoleCommand::SelectTool {
            block_id: normalize_block_id(single_argument(
                command,
                arguments,
                "/select-tool <block_id>",
            )?)?,
        },
        "place" => parse_place(arguments)?,
        "placeblock" => parse_placeblock(arguments)?,
        "interact" => parse_interact(arguments)?,
        "stopinteraction" => no_arguments(command, arguments, ConsoleCommand::StopInteraction)?,
        "interactionstatus" => no_arguments(command, arguments, ConsoleCommand::InteractionStatus)?,
        "equip" => ConsoleCommand::Equip {
            item: normalize_item_id(single_argument(command, arguments, "/equip <item>")?)?,
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
    if value.is_empty() || value.chars().any(char::is_whitespace) || value.matches(':').count() > 1
    {
        return Err(AppError::InvalidConsoleSyntax(
            "invalid namespaced identifier".into(),
        ));
    }
    Ok(if value.contains(':') {
        value.to_ascii_lowercase()
    } else {
        format!("minecraft:{}", value.to_ascii_lowercase())
    })
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

fn parse_place(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        [block_id] => Ok(ConsoleCommand::PlaceLooked {
            block_id: normalize_block_id(block_id)?,
        }),
        // Block-first order (`/place <block> <x> <y> <z>`) is the documented
        // console syntax; coordinate-first is kept for backward compatibility
        // with existing callers/tests. The two are unambiguous because block
        // identifiers never parse as integers, so whichever end holds the
        // integer triple decides the order.
        [first, second, third, fourth] if first.parse::<i32>().is_ok() => {
            Ok(ConsoleCommand::PlaceAt {
                x: first
                    .parse()
                    .map_err(|_| AppError::InvalidCoordinates("x must be an integer".into()))?,
                y: second
                    .parse()
                    .map_err(|_| AppError::InvalidCoordinates("y must be an integer".into()))?,
                z: third
                    .parse()
                    .map_err(|_| AppError::InvalidCoordinates("z must be an integer".into()))?,
                block_id: normalize_block_id(fourth)?,
            })
        }
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
            "/place <block_id> or /place <block_id> <x> <y> <z>".into(),
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

fn parse_interact(arguments: &str) -> Result<ConsoleCommand, AppError> {
    const USAGE: &str = "/interact <block_id> <item_id[,item_id...]> [radius]";
    let mut parts = arguments.split_whitespace();
    let block_id = parts
        .next()
        .ok_or_else(|| AppError::MissingConsoleArgument(USAGE.into()))?;
    let items = parts
        .next()
        .ok_or_else(|| AppError::MissingConsoleArgument(USAGE.into()))?;
    let radius = parts
        .next()
        .map(|value| {
            value.parse().map_err(|_| {
                AppError::InvalidEntityQuery("radius must be a positive integer".into())
            })
        })
        .transpose()?
        .unwrap_or(32);
    if parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(USAGE.into()));
    }
    let items = items
        .split(',')
        .map(normalize_item_id)
        .collect::<Result<Vec<_>, _>>()?;
    if items.is_empty() {
        return Err(AppError::InvalidConsoleSyntax(USAGE.into()));
    }
    Ok(ConsoleCommand::InteractNearest {
        block_id: normalize_block_id(block_id)?,
        items,
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

/// Item-based resource gathering: `/get <item> <amount>` (also reachable
/// from Minecraft chat as `#get <item> <amount>`). `item` is resolved --
/// never used directly as a mining target -- via `crate::mobs::resolve_resource`,
/// which tries, in order: the ore/conversion table (`blocks::drop_blocks_for_item`,
/// e.g. `diamond` -> mine `diamond_ore` or `deepslate_diamond_ore`, whichever
/// is nearer), the mob-drop table (`leather` -> hunt `cow`), and finally
/// "just mine a block with this exact id" for anything that drops itself
/// (`oak_log`, `cobblestone`, ...). `#get` never counts inventory against
/// the block it mined, only against the resolved item -- see
/// `App::run_get_item`. Contrast with `/mine`, which targets the block
/// itself and never resolves anything.
fn parse_get(arguments: &str) -> Result<ConsoleCommand, AppError> {
    const USAGE: &str = "/get <item> <amount>";
    let mut parts = arguments.split_whitespace();
    let resource = parts
        .next()
        .ok_or_else(|| AppError::MissingConsoleArgument(USAGE.into()))?;
    let amount_raw = parts
        .next()
        .ok_or_else(|| AppError::MissingConsoleArgument(USAGE.into()))?;
    let amount: u32 = amount_raw
        .parse()
        .map_err(|_| AppError::InvalidConsoleSyntax("amount must be a positive integer".into()))?;
    if amount == 0 {
        return Err(AppError::InvalidConsoleSyntax(
            "amount must be greater than zero".into(),
        ));
    }
    if parts.next().is_some() {
        return Err(AppError::InvalidConsoleSyntax(USAGE.into()));
    }
    let resource_id = match crate::mobs::resolve_resource(resource)? {
        crate::mobs::ResourceKind::Ore { resource_id, .. } => resource_id,
        crate::mobs::ResourceKind::Mob { resource_id, .. } => resource_id,
    };
    Ok(ConsoleCommand::GetResource {
        resource_id,
        amount,
    })
}

/// Direct block mining: `/mine <block> [block...] <amount>` (also reachable
/// as `#mine ...`). Unlike `/get`, `block` is never resolved to anything --
/// it names the exact block(s) to mine, and `#mine` counts blocks destroyed,
/// not items received. More than one block id may be given so a single run
/// can target "whichever of these is closer" (e.g.
/// `/mine diamond_ore deepslate_diamond_ore 10`); the last argument is
/// always the amount.
fn parse_mine(arguments: &str) -> Result<ConsoleCommand, AppError> {
    const USAGE: &str = "/mine <block> [block...] <amount>";
    let tokens: Vec<&str> = arguments.split_whitespace().collect();
    let Some((amount_raw, block_tokens)) = tokens.split_last() else {
        return Err(AppError::MissingConsoleArgument(USAGE.into()));
    };
    if block_tokens.is_empty() {
        return Err(AppError::MissingConsoleArgument(USAGE.into()));
    }
    let amount: u32 = amount_raw
        .parse()
        .map_err(|_| AppError::InvalidConsoleSyntax("amount must be a positive integer".into()))?;
    if amount == 0 {
        return Err(AppError::InvalidConsoleSyntax(
            "amount must be greater than zero".into(),
        ));
    }
    let block_ids = block_tokens
        .iter()
        .map(|token| normalize_block_id(token))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ConsoleCommand::Mine { block_ids, amount })
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
            parse_input("/containerstatus").unwrap(),
            ConsoleInput::Command(ConsoleCommand::ObservedContainerStatus)
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
            parse_input("/select-tool diamond_ore").unwrap(),
            ConsoleInput::Command(ConsoleCommand::SelectTool {
                block_id: "minecraft:diamond_ore".into()
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
    fn parses_interact_command() {
        assert_eq!(
            parse_input("/interact dirt wooden_hoe,stone_hoe").unwrap(),
            ConsoleInput::Command(ConsoleCommand::InteractNearest {
                block_id: "minecraft:dirt".into(),
                items: vec!["minecraft:wooden_hoe".into(), "minecraft:stone_hoe".into()],
                radius: 32,
            })
        );
        assert_eq!(
            parse_input("/interact minecraft:grass_block wooden_shovel 16").unwrap(),
            ConsoleInput::Command(ConsoleCommand::InteractNearest {
                block_id: "minecraft:grass_block".into(),
                items: vec!["minecraft:wooden_shovel".into()],
                radius: 16,
            })
        );
        assert!(parse_input("/interact dirt").is_err());
        assert!(parse_input("/interact dirt wooden_hoe extra garbage").is_err());
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
    fn parses_status_query_commands() {
        assert_eq!(
            parse_input("/where").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Where)
        );
        assert_eq!(
            parse_input("/health").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Health)
        );
        assert!(matches!(
            parse_input("/where now"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
    }

    #[test]
    fn parses_equip_command() {
        assert_eq!(
            parse_input("/equip diamond_pickaxe").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Equip {
                item: "minecraft:diamond_pickaxe".into()
            })
        );
        assert!(matches!(
            parse_input("/equip"),
            Err(AppError::MissingConsoleArgument(_))
        ));
        assert!(matches!(
            parse_input("/equip diamond_pickaxe extra"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
    }

    #[test]
    fn parses_find_as_findblock_alias() {
        assert_eq!(
            parse_input("/find stone 64").unwrap(),
            ConsoleInput::Command(ConsoleCommand::FindBlock {
                block_id: "minecraft:stone".into(),
                radius: Some(64),
                limit: None,
            })
        );
        assert!(parse_input("/find").is_err());
    }

    #[test]
    fn parses_place_in_block_first_and_coordinate_first_order() {
        assert_eq!(
            parse_input("/place minecraft:cobblestone 915 91 1005").unwrap(),
            ConsoleInput::Command(ConsoleCommand::PlaceAt {
                x: 915,
                y: 91,
                z: 1005,
                block_id: "minecraft:cobblestone".into()
            })
        );
        assert_eq!(
            parse_input("/place 915 91 1005 minecraft:cobblestone").unwrap(),
            ConsoleInput::Command(ConsoleCommand::PlaceAt {
                x: 915,
                y: 91,
                z: 1005,
                block_id: "minecraft:cobblestone".into()
            })
        );
    }

    #[test]
    fn parses_get_resource_command() {
        assert_eq!(
            parse_input("/get diamond_ore 15").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:diamond_ore".into(),
                amount: 15,
            })
        );
        assert_eq!(
            parse_input("/get minecraft:oak_log 10").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:oak_log".into(),
                amount: 10,
            })
        );
    }

    #[test]
    fn parses_get_resource_command_for_ore_items() {
        // `#get diamond 10`: the argument is the *item*, not a block --
        // resolution to `diamond_ore`/`deepslate_diamond_ore` happens at
        // dispatch time (see `mobs::resolve_resource`'s own tests); parsing
        // only needs to accept the item id and carry it through unchanged.
        assert_eq!(
            parse_input("/get diamond 10").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:diamond".into(),
                amount: 10,
            })
        );
        assert_eq!(
            parse_input("/get raw_iron 20").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:raw_iron".into(),
                amount: 20,
            })
        );
        assert_eq!(
            parse_input("/get coal 50").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:coal".into(),
                amount: 50,
            })
        );
    }

    #[test]
    fn parses_get_resource_command_for_mob_drops() {
        assert_eq!(
            parse_input("/get leather 10").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:leather".into(),
                amount: 10,
            })
        );
        assert_eq!(
            parse_input("/get porkchop 16").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:porkchop".into(),
                amount: 16,
            })
        );
        // A wool color is also a real block id, but resolves through the
        // mob-drop table (sheep) -- see `mobs::resolve_resource`.
        assert_eq!(
            parse_input("/get white_wool 20").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GetResource {
                resource_id: "minecraft:white_wool".into(),
                amount: 20,
            })
        );
    }

    #[test]
    fn rejects_invalid_get_resource_arguments() {
        assert!(matches!(
            parse_input("/get"),
            Err(AppError::MissingConsoleArgument(_))
        ));
        assert!(matches!(
            parse_input("/get diamond_ore"),
            Err(AppError::MissingConsoleArgument(_))
        ));
        assert!(matches!(
            parse_input("/get diamond_ore 0"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
        assert!(matches!(
            parse_input("/get diamond_ore -5"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
        assert!(matches!(
            parse_input("/get diamond_ore abc"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
        assert!(matches!(
            parse_input("/get diamond_ore 15 extra"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
        assert!(matches!(
            parse_input("/get not_a_real_thing 5"),
            Err(AppError::UnknownResourceIdentifier(_))
        ));
    }

    #[test]
    fn parses_mine_command_with_a_single_block() {
        assert_eq!(
            parse_input("/mine stone 100").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Mine {
                block_ids: vec!["minecraft:stone".into()],
                amount: 100,
            })
        );
    }

    #[test]
    fn parses_mine_command_with_multiple_blocks() {
        // `#mine diamond_ore deepslate_diamond_ore 10`: unlike `/get`, both
        // arguments before the amount are taken literally as block ids --
        // no item resolution happens here at all.
        assert_eq!(
            parse_input("/mine diamond_ore deepslate_diamond_ore 10").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Mine {
                block_ids: vec![
                    "minecraft:diamond_ore".into(),
                    "minecraft:deepslate_diamond_ore".into(),
                ],
                amount: 10,
            })
        );
    }

    #[test]
    fn mine_never_resolves_item_names_to_blocks() {
        // `#get`'s ore resolution must not leak into `/mine`: a plain item
        // name that is not itself a valid block id is rejected outright
        // rather than silently expanded to source blocks.
        assert!(matches!(
            parse_input("/mine diamond 10"),
            Err(AppError::UnknownBlockIdentifier(_))
        ));
        assert!(matches!(
            parse_input("/mine raw_iron 10"),
            Err(AppError::UnknownBlockIdentifier(_))
        ));
    }

    #[test]
    fn rejects_invalid_mine_arguments() {
        assert!(matches!(
            parse_input("/mine"),
            Err(AppError::MissingConsoleArgument(_))
        ));
        assert!(matches!(
            parse_input("/mine 10"),
            Err(AppError::MissingConsoleArgument(_))
        ));
        assert!(matches!(
            parse_input("/mine stone 0"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
        assert!(matches!(
            parse_input("/mine stone -5"),
            Err(AppError::InvalidConsoleSyntax(_))
        ));
        assert!(matches!(
            parse_input("/mine not_a_real_block 10"),
            Err(AppError::UnknownBlockIdentifier(_))
        ));
    }

    #[test]
    fn help_command_parses() {
        assert_eq!(
            parse_input("/help").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Help)
        );
    }
}
