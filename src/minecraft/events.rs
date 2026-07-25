//! Translation of Azalea events into application-owned state and terminal output.

use azalea::client_chat::ChatPacket;
use tracing::debug;
use uuid::Uuid;

use crate::{
    config::ConsoleConfig,
    logging,
    minecraft::world_state::{ChatMessageKind, WorldState},
};

pub fn handle_chat(
    packet: &ChatPacket,
    config: &ConsoleConfig,
    bot_uuid: Option<Uuid>,
    bot_username: &str,
    world: &mut WorldState,
) {
    let (kind, sender) = match packet {
        ChatPacket::Player(_) | ChatPacket::Disguised(_) => {
            (ChatMessageKind::Player, packet.sender())
        }
        ChatPacket::System(system) if system.overlay => (ChatMessageKind::ActionBar, None),
        ChatPacket::System(_) => (ChatMessageKind::System, None),
    };
    let text = packet.message().to_string();

    // The server echoes a bot's own chat back as a normal player chat event.
    // It was already rendered by send_chat, so do not render that echo again.
    // UUIDs are authoritative when both sides provide one; the username is
    // only a fallback for servers/packet types without a sender UUID.
    if kind == ChatMessageKind::Player
        && is_bot_message(
            packet.sender_uuid(),
            sender.as_deref(),
            bot_uuid,
            bot_username,
        )
    {
        debug!("bot chat echo suppressed");
        return;
    }

    if !world.record_received(kind, sender.clone(), text.clone()) {
        debug!("duplicate chat event suppressed");
        return;
    }

    if kind == ChatMessageKind::System && !config.show_system_messages {
        return;
    }
    if kind == ChatMessageKind::ActionBar && !config.show_action_bar_messages {
        return;
    }

    match kind {
        ChatMessageKind::Player => {
            logging::chat_incoming(sender.as_deref().unwrap_or("unknown"), &text);
        }
        ChatMessageKind::System => println!("[SYSTEM] {text}"),
        ChatMessageKind::ActionBar => println!("[ACTIONBAR] {text}"),
    }
}

fn is_bot_message(
    sender_uuid: Option<Uuid>,
    sender_name: Option<&str>,
    bot_uuid: Option<Uuid>,
    bot_username: &str,
) -> bool {
    match (sender_uuid, bot_uuid) {
        (Some(sender), Some(bot)) => sender == bot,
        _ => sender_name.is_some_and(|sender| sender.eq_ignore_ascii_case(bot_username)),
    }
}

#[cfg(test)]
mod tests {
    use super::is_bot_message;
    use uuid::Uuid;

    #[test]
    fn bot_uuid_is_preferred_for_self_chat_filtering() {
        let bot = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        assert!(is_bot_message(
            Some(bot),
            Some("OtherName"),
            Some(bot),
            "MagicBot"
        ));
        assert!(!is_bot_message(
            Some(other),
            Some("MagicBot"),
            Some(bot),
            "MagicBot"
        ));
    }

    #[test]
    fn username_is_fallback_when_uuid_is_unavailable() {
        assert!(is_bot_message(None, Some("magicbot"), None, "MagicBot"));
        assert!(!is_bot_message(None, Some("Steve"), None, "MagicBot"));
    }
}
