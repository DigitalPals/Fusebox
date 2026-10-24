//! Provider-facing sessions are scoped to a conversation's current account stint.
//! They are deliberately separate from routing assignments and prompt-cache keys.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::accounts::Account;

const MAX_SESSIONS: usize = 10_000;

struct Session {
    account: String,
    identity_epoch: u64,
    id: String,
    last_seen: i64,
}

#[derive(Default)]
pub struct Sessions {
    sessions: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    pub fn get(&self, conversation: Option<&str>, account: &Account, idle: u64) -> String {
        let identity = account.cred.read().identity(account.provider);
        let epoch = account.state.lock().identity_epoch;
        let owner = format!("{}:{identity}", account.id);
        self.at(conversation, &owner, epoch, idle, chrono::Utc::now().timestamp())
    }

    fn at(&self, conversation: Option<&str>, account: &str, identity_epoch: u64, idle: u64, now: i64) -> String {
        let Some(conversation) = conversation else { return uuid::Uuid::new_v4().to_string() };
        let mut sessions = self.sessions.lock();
        let idle = idle.min(i64::MAX as u64) as i64;
        if let Some(session) = sessions.get_mut(conversation)
            && session.account == account
            && session.identity_epoch == identity_epoch
            && now.saturating_sub(session.last_seen) < idle
        {
            session.last_seen = now;
            return session.id.clone();
        }
        if sessions.len() >= MAX_SESSIONS && !sessions.contains_key(conversation) {
            sessions.retain(|_, session| now.saturating_sub(session.last_seen) < idle);
            if sessions.len() >= MAX_SESSIONS
                && let Some(oldest) = sessions.iter().min_by_key(|(_, s)| s.last_seen).map(|(key, _)| key.clone())
            {
                sessions.remove(&oldest);
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        sessions.insert(
            conversation.to_string(),
            Session { account: account.to_string(), identity_epoch, id: id.clone(), last_seen: now },
        );
        id
    }

    pub fn end(&self, conversation: &str) {
        self.sessions.lock().remove(conversation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_back_creates_a_new_stint_but_active_turns_reuse_it() {
        let sessions = Sessions::default();
        let a = sessions.at(Some("chat-42"), "subscription-1", 0, 1800, 10);
        assert_eq!(a, sessions.at(Some("chat-42"), "subscription-1", 0, 1800, 20));
        let b = sessions.at(Some("chat-42"), "subscription-2", 0, 1800, 30);
        let c = sessions.at(Some("chat-42"), "subscription-1", 0, 1800, 40);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert!(uuid::Uuid::parse_str(&c).is_ok());
    }

    #[test]
    fn inactivity_identity_change_and_session_end_rotate_without_losing_other_chats() {
        let sessions = Sessions::default();
        let a = sessions.at(Some("chat"), "account", 0, 30, 0);
        let other = sessions.at(Some("other"), "account", 0, 30, 10);
        assert_ne!(a, other);
        let expired = sessions.at(Some("chat"), "account", 0, 30, 30);
        assert_ne!(a, expired);
        let replaced = sessions.at(Some("chat"), "account", 1, 30, 31);
        assert_ne!(expired, replaced);
        sessions.end("chat");
        assert_ne!(replaced, sessions.at(Some("chat"), "account", 1, 30, 32));
        assert_eq!(other, sessions.at(Some("other"), "account", 0, 30, 32));
    }

    #[test]
    fn missing_conversation_never_merges_independent_requests() {
        let sessions = Sessions::default();
        assert_ne!(sessions.at(None, "a", 0, 30, 1), sessions.at(None, "a", 0, 30, 1));
    }
}
