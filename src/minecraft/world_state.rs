//! Small application-owned state used by chat and console status reporting.

use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatMessageKind {
    Player,
    System,
    ActionBar,
}

#[derive(Clone, Debug)]
pub struct ChatRecord {
    pub kind: ChatMessageKind,
    pub sender: Option<String>,
    pub text: String,
    pub timestamp: SystemTime,
}

#[derive(Clone, Debug, Default)]
pub struct WorldStateSnapshot {
    pub joined_world: bool,
    pub last_received_chat: Option<ChatRecord>,
    pub last_sent_chat: Option<ChatRecord>,
}

#[derive(Debug, Default)]
pub struct WorldState {
    joined_world: bool,
    last_received_chat: Option<ChatRecord>,
    last_sent_chat: Option<ChatRecord>,
    last_displayed_signature: Option<(String, Instant)>,
}

impl WorldState {
    pub fn set_joined_world(&mut self, joined: bool) {
        self.joined_world = joined;
    }

    pub fn record_received(
        &mut self,
        kind: ChatMessageKind,
        sender: Option<String>,
        text: String,
    ) -> bool {
        let now = SystemTime::now();
        self.last_received_chat = Some(ChatRecord {
            kind,
            sender: sender.clone(),
            text: text.clone(),
            timestamp: now,
        });

        let signature = format!("{kind:?}|{sender:?}|{text}");
        let duplicate = self
            .last_displayed_signature
            .as_ref()
            .is_some_and(|(previous, at)| {
                previous == &signature && at.elapsed() < Duration::from_millis(100)
            });
        self.last_displayed_signature = Some((signature, Instant::now()));
        !duplicate
    }

    pub fn record_sent(&mut self, text: String) -> bool {
        let now = SystemTime::now();
        let duplicate = self.last_sent_chat.as_ref().is_some_and(|previous| {
            previous.text == text
                && now
                    .duration_since(previous.timestamp)
                    .unwrap_or(Duration::MAX)
                    < Duration::from_millis(100)
        });
        if !duplicate {
            self.last_sent_chat = Some(ChatRecord {
                kind: ChatMessageKind::Player,
                sender: None,
                text,
                timestamp: now,
            });
        }
        !duplicate
    }

    #[must_use]
    pub fn snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            joined_world: self.joined_world,
            last_received_chat: self.last_received_chat.clone(),
            last_sent_chat: self.last_sent_chat.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_sent_message_is_suppressed_briefly() {
        let mut state = WorldState::default();
        assert!(state.record_sent("hello".to_owned()));
        assert!(!state.record_sent("hello".to_owned()));
        assert!(state.record_sent("different".to_owned()));
    }

    #[test]
    fn received_chat_is_recorded() {
        let mut state = WorldState::default();
        assert!(state.record_received(
            ChatMessageKind::Player,
            Some("Alex".to_owned()),
            "hello".to_owned()
        ));
        let snapshot = state.snapshot();
        assert_eq!(snapshot.last_received_chat.unwrap().text, "hello");
    }
}
