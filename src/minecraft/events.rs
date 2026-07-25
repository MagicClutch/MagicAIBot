//! Translation of Azalea events into application-owned state and terminal output.

use azalea::client_chat::ChatPacket;
use tracing::debug;

use crate::{
    config::ConsoleConfig,
    minecraft::world_state::{ChatMessageKind, WorldState},
};

pub fn handle_chat(packet: &ChatPacket, config: &ConsoleConfig, world: &mut WorldState) {
    let (kind, sender) = match packet {
        ChatPacket::Player(_) | ChatPacket::Disguised(_) => {
            (ChatMessageKind::Player, packet.sender())
        }
        ChatPacket::System(system) if system.overlay => (ChatMessageKind::ActionBar, None),
        ChatPacket::System(_) => (ChatMessageKind::System, None),
    };
    let text = packet.message().to_string();

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
            println!("[CHAT] {}: {text}", sender.as_deref().unwrap_or("unknown"));
        }
        ChatMessageKind::System => println!("[SYSTEM] {text}"),
        ChatMessageKind::ActionBar => println!("[ACTIONBAR] {text}"),
    }
}
