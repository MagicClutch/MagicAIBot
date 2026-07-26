//! Parsing for local terminal input. Execution belongs to the application layer.

use crate::{
    blocks::block_query::normalize_block_id,
    collection::{CollectRequest, ItemFilter, ItemGroup},
    error::AppError,
    movement::commands::{parse_coordinates, parse_follow_name},
};

#[derive(Debug, PartialEq)]
pub enum ConsoleCommand {
    Help,
    Status,
    Chat {
        message: String,
    },
    Players,
    Inventory,
    FurnaceStatus,
    SmeltCheck {
        output: String,
        count: u32,
    },
    FuelInfo {
        item: String,
    ContainerStatus,
    Recipe {
        id: String,
    },
    CraftCheck {
        item: String,
        count: u32,
        depth: usize,
    },
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
    Gather {
        resource: String,
        quantity: u32,
        deposit: bool,
    },
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
    StopInteraction,
    InteractionStatus,
    CollectItem(CollectRequest),
    CollectItemStatus,
    CollectItemStop,
    MineOre { target: String, count: u32, radius: Option<u32> },
    MineOreStatus,
    MineOreStop,
    Craft {
        target: String,
        count: u32,
    },
    CraftStatus,
    CraftStop,
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
    ChopTree { request: crate::tree_chopping::ChopRequest },
    ChopTreeStatus,
    ChopTreeStop,
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
        "players" => no_arguments(command, arguments, ConsoleCommand::Players)?,
        "inventory" => no_arguments(command, arguments, ConsoleCommand::Inventory)?,
        "furnace-status" => no_arguments(command, arguments, ConsoleCommand::FurnaceStatus)?,
        "smelt-check" => {
            let mut parts = arguments.split_whitespace();
            let output = minecraft_id(parts.next().ok_or_else(|| {
                AppError::MissingConsoleArgument("/smelt-check <output> <count>".into())
            })?);
            let count = parts
                .next()
                .ok_or_else(|| {
                    AppError::MissingConsoleArgument("/smelt-check <output> <count>".into())
                })?
                .parse()
                .map_err(|_| {
                    AppError::InvalidConsoleSyntax("count must be a positive integer".into())
                })?;
            if count == 0 || parts.next().is_some() {
                return Err(AppError::InvalidConsoleSyntax(
                    "/smelt-check <output> <count>".into(),
                ));
            }
            ConsoleCommand::SmeltCheck { output, count }
        }
        "fuel-info" => ConsoleCommand::FuelInfo {
            item: minecraft_id(single_argument(command, arguments, "/fuel-info <item>")?),
        },
        "containerstatus" | "container-status" => {
            no_arguments(command, arguments, ConsoleCommand::ContainerStatus)?
        }
        "recipe" => ConsoleCommand::Recipe {
            id: normalize_item_id(single_argument(command, arguments, "/recipe <recipe_id>")?)?,
        },
        "craft-check" => parse_craft_check(arguments)?,
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
        "gather" => parse_gather(arguments)?,
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
        "select-tool" => ConsoleCommand::SelectTool {
            block_id: normalize_block_id(single_argument(
                command,
                arguments,
                "/select-tool <block_id>",
            )?)?,
        },
        "place" => parse_place(arguments)?,
        "placeblock" => parse_placeblock(arguments)?,
        "stopinteraction" => no_arguments(command, arguments, ConsoleCommand::StopInteraction)?,
        "interactionstatus" => no_arguments(command, arguments, ConsoleCommand::InteractionStatus)?,
        "collect-item" | "collectdrop" => parse_collect_item(arguments)?,
        "mine-ore" | "mineore" => parse_mine_ore(arguments)?,
        "craft" => parse_craft(arguments)?,
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
        "chop-tree" | "choptree" => parse_chop_tree(arguments)?,
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

fn minecraft_id(value: &str) -> String {
    let value = value.to_ascii_lowercase();
    if value.contains(':') {
        value
    } else {
        format!("minecraft:{value}")
    }
fn parse_collect_item(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        ["status"] => Ok(ConsoleCommand::CollectItemStatus),
        ["stop"] => Ok(ConsoleCommand::CollectItemStop),
        ["nearest"] => { let mut r=CollectRequest::exact(String::new(),1); r.filter=ItemFilter::Nearest; Ok(ConsoleCommand::CollectItem(r)) },
        ["group", group, quantity] => {
            let group = match *group { "ores"=>ItemGroup::Ores,"logs"=>ItemGroup::Logs,"food"=>ItemGroup::Food,
                _=>return Err(AppError::InvalidConsoleSyntax("groups: ores, logs, food".into())) };
            let mut r=CollectRequest::exact(String::new(),parse_quantity(quantity)?); r.filter=ItemFilter::Group(group); Ok(ConsoleCommand::CollectItem(r))
        }
        [items, quantity] => { let ids=items.split(',').map(normalize_item_id).collect::<Vec<_>>();
            let mut r=CollectRequest::exact(ids[0].clone(),parse_quantity(quantity)?); if ids.len()>1 {r.filter=ItemFilter::AnyOf(ids)}; Ok(ConsoleCommand::CollectItem(r)) }
        _ => Err(AppError::InvalidConsoleSyntax("/collect-item <item[,item]> <count> | group <ores|logs|food> <count> | nearest | status | stop".into()))
    }
}
fn parse_quantity(value: &str) -> Result<u32, AppError> {
    let n = value.parse().map_err(|_| {
        AppError::InvalidConsoleSyntax("collection count must be a positive integer".into())
    })?;
    if n == 0 {
        return Err(AppError::InvalidConsoleSyntax(
            "collection count must be positive".into(),
        ));
    }
    Ok(n)
}
fn normalize_item_id(value: &str) -> String {
    if value.contains(':') {
        value.to_ascii_lowercase()
    } else {
        format!("minecraft:{}", value.to_ascii_lowercase())
    }
fn parse_chop_tree(arguments: &str) -> Result<ConsoleCommand, AppError> {
    use crate::tree_chopping::ChopRequest;
    let parts: Vec<_> = arguments.split_whitespace().collect();
    let request = match parts.as_slice() {
        ["nearest"] => return Ok(ConsoleCommand::ChopTree { request: ChopRequest::Nearest }),
        ["status"] => return Ok(ConsoleCommand::ChopTreeStatus),
        ["stop"] => return Ok(ConsoleCommand::ChopTreeStop),
        ["logs", amount] => ChopRequest::Logs(amount.parse().map_err(|_| AppError::InvalidConsoleSyntax("/chop-tree logs <positive count>".into()))?),
        ["count", amount] => ChopRequest::Count(amount.parse().map_err(|_| AppError::InvalidConsoleSyntax("/chop-tree count <positive count>".into()))?),
        [kind] => ChopRequest::TreeType(kind.to_ascii_lowercase()),
        _ => return Err(AppError::InvalidConsoleSyntax("/chop-tree nearest|<type>|logs <n>|count <n>|status|stop".into())),
    };
    if matches!(request, ChopRequest::Logs(0) | ChopRequest::Count(0)) { return Err(AppError::InvalidConsoleSyntax("chop count must be positive".into())); }
    Ok(ConsoleCommand::ChopTree { request })
fn parse_mine_ore(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        ["status"] => Ok(ConsoleCommand::MineOreStatus),
        ["stop"] => Ok(ConsoleCommand::MineOreStop),
        [target, count] | [target, count, _] => {
            let count=count.parse().map_err(|_|AppError::InvalidConsoleSyntax("/mine-ore <ore|group> <count> [radius]".into()))?;
            let radius=parts.get(2).map(|v|v.parse().map_err(|_|AppError::InvalidConsoleSyntax("radius must be a positive integer".into()))).transpose()?;
            Ok(ConsoleCommand::MineOre{target:target.to_ascii_lowercase(),count,radius})
        }
        _ => Err(AppError::InvalidConsoleSyntax("/mine-ore <ore|group> <count> [radius] | status | stop".into())),
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

fn parse_craft_check(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    if !(1..=3).contains(&parts.len()) {
        return Err(AppError::InvalidConsoleSyntax(
            "/craft-check <item> [count] [depth]".into(),
        ));
    }
    let count = parts
        .get(1)
        .map_or(Ok(1), |v| v.parse::<u32>())
        .map_err(|_| {
            AppError::InvalidConsoleSyntax("craft count must be a positive integer".into())
        })?;
    let depth = parts
        .get(2)
        .map_or(Ok(8), |v| v.parse::<usize>())
        .map_err(|_| AppError::InvalidConsoleSyntax("depth must be an integer".into()))?;
    if count == 0 || depth == 0 || depth > 64 {
        return Err(AppError::InvalidConsoleSyntax(
            "count must be positive and depth must be 1..=64".into(),
        ));
    }
    Ok(ConsoleCommand::CraftCheck {
        item: normalize_item_id(parts[0])?,
        count,
        depth,
    let value = if value.contains(':') {
        value.to_owned()
    } else {
        format!("minecraft:{value}")
    };
    let valid = value.split_once(':').is_some_and(|(namespace, path)| {
        !namespace.is_empty()
            && !path.is_empty()
            && value.bytes().all(|b| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'_' | b'-' | b'.' | b'/' | b':')
            })
    });
    if valid {
        Ok(value)
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

fn parse_craft(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    match parts.as_slice() {
        ["status"] => Ok(ConsoleCommand::CraftStatus),
        ["stop"] => Ok(ConsoleCommand::CraftStop),
        [target, count] => Ok(ConsoleCommand::Craft {
            target: normalize_item_id(target)?,
            count: count.parse().map_err(|_| {
                AppError::InvalidConsoleSyntax("/craft <item> <positive-count>".into())
            })?,
        }),
        _ => Err(AppError::InvalidConsoleSyntax(
            "/craft <item> <count>, /craft status, or /craft stop".into(),
        )),
    }
    .and_then(|command| match command {
        ConsoleCommand::Craft { count: 0, .. } => Err(AppError::InvalidConsoleSyntax(
            "craft count must be positive".into(),
        )),
        other => Ok(other),
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

fn normalize_item_id(input: &str) -> Result<String, AppError> {
    let id = if input.contains(':') {
        input.to_ascii_lowercase()
    } else {
        format!("minecraft:{}", input.to_ascii_lowercase())
    };
    if id.split_once(':').is_none_or(|(namespace, path)| {
        namespace.is_empty()
            || path.is_empty()
            || !namespace.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
            })
            || !path.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/')
            })
    }) {
        return Err(AppError::InvalidConsoleSyntax(
            "invalid item identifier".into(),
        ));
    }
    Ok(id)
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

fn parse_gather(arguments: &str) -> Result<ConsoleCommand, AppError> {
    let parts: Vec<_> = arguments.split_whitespace().collect();
    let (resource, quantity, deposit) = match parts.as_slice() {
        [resource, quantity] => (*resource, *quantity, false),
        [resource, quantity, "deposit"] => (*resource, *quantity, true),
        _ => {
            return Err(AppError::InvalidConsoleSyntax(
                "/gather <resource> <quantity> [deposit]".into(),
            ));
        }
    };
    let quantity = quantity.parse::<u32>().map_err(|_| {
        AppError::InvalidConsoleSyntax("gather quantity must be a positive integer".into())
    })?;
    if quantity == 0 || quantity > 4096 {
        return Err(AppError::InvalidConsoleSyntax(
            "gather quantity must be between 1 and 4096".into(),
        ));
    }
    let resource = crate::tasks::gather::supported_resource(resource)
        .ok_or_else(|| AppError::InvalidConsoleSyntax("unsupported gather resource; use logs, stone, coal, raw_iron, iron_ingot, diamond, apple, carrot, potato, wheat, bread, or baked_potato".into()))?;
    Ok(ConsoleCommand::Gather {
        resource: resource.item,
        quantity,
        deposit,
    })
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
    fn parses_read_only_crafting_commands() {
        assert_eq!(
            parse_input("/recipe stick").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Recipe {
                id: "minecraft:stick".into()
            })
        );
        assert_eq!(
            parse_input("/craft-check minecraft:torch 5 4").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CraftCheck {
                item: "minecraft:torch".into(),
                count: 5,
                depth: 4
            })
        );
        assert!(parse_input("/craft-check torch 0").is_err());
    }

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
            ConsoleInput::Command(ConsoleCommand::ContainerStatus)
        );
        assert_eq!(
            parse_input("/chat hello").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Chat {
                message: "hello".to_owned()
            })
        );
    }

    #[test]
    fn parses_processing_debug_commands() {
        assert_eq!(
            parse_input("/furnace-status").unwrap(),
            ConsoleInput::Command(ConsoleCommand::FurnaceStatus)
        );
        assert_eq!(
            parse_input("/smelt-check iron_ingot 3").unwrap(),
            ConsoleInput::Command(ConsoleCommand::SmeltCheck {
                output: "minecraft:iron_ingot".into(),
                count: 3
            })
        );
        assert_eq!(
            parse_input("/fuel-info coal").unwrap(),
            ConsoleInput::Command(ConsoleCommand::FuelInfo {
                item: "minecraft:coal".into()
            })
        );
        assert!(parse_input("/smelt-check iron_ingot 0").is_err());
    fn parses_crafting_debug_commands() {
        assert_eq!(
            parse_input("/craft stick 4").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Craft {
                target: "minecraft:stick".into(),
                count: 4
            })
        );
        assert_eq!(
            parse_input("/craft status").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CraftStatus)
        );
        assert_eq!(
            parse_input("/craft stop").unwrap(),
            ConsoleInput::Command(ConsoleCommand::CraftStop)
        );
        assert!(parse_input("/craft stick 0").is_err());
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
        assert_eq!(parse_input("/mine-ore diamond 3 24").unwrap(), ConsoleInput::Command(ConsoleCommand::MineOre{target:"diamond".into(),count:3,radius:Some(24)}));
        assert_eq!(parse_input("/mine-ore status").unwrap(), ConsoleInput::Command(ConsoleCommand::MineOreStatus));
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
    fn parses_tree_chopping_commands_and_rejects_zero() {
        use crate::tree_chopping::ChopRequest;
        assert_eq!(parse_input("/chop-tree nearest").unwrap(), ConsoleInput::Command(ConsoleCommand::ChopTree { request: ChopRequest::Nearest }));
        assert_eq!(parse_input("/choptree logs 12").unwrap(), ConsoleInput::Command(ConsoleCommand::ChopTree { request: ChopRequest::Logs(12) }));
        assert_eq!(parse_input("/chop-tree status").unwrap(), ConsoleInput::Command(ConsoleCommand::ChopTreeStatus));
        assert!(parse_input("/chop-tree count 0").is_err());
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
    fn parses_gather_lifecycle_commands() {
        assert_eq!(
            parse_input("/gather logs 12 deposit").unwrap(),
            ConsoleInput::Command(ConsoleCommand::Gather {
                resource: "minecraft:oak_log".into(),
                quantity: 12,
                deposit: true
            })
        );
        assert_eq!(
            parse_input("/gatherstatus").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GatherStatus)
        );
        assert_eq!(
            parse_input("/gathercancel").unwrap(),
            ConsoleInput::Command(ConsoleCommand::GatherCancel)
        );
        assert!(parse_input("/gather beef 2").is_err());
        assert!(parse_input("/gather stone 0").is_err());
    }
}
