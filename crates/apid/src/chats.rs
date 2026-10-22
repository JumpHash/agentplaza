//! Chats (PLAN 26.2, W63): the owner's way in. The main orchestrator's chat (one, the whole plaza), a project's chats
//! (as many as the owner opens: each a conversation with the project's planner), and a task's chat (one per task: its
//! thread with its seats). Questions (cards) and notices arrive as messages in the chat they belong to.
//!
//! The records live in memory; the hub's saver keeps them in `plaza.db` (tables `chat`, `chat_messages`). A message the
//! owner sends is written at once (`crate::store::saver::save_now`), not on the saver's next pass.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// The main orchestrator's chat.
pub const MAIN: &str = "main";
/// The most messages a chat keeps in memory and in the store; the oldest go first (their content stays in the
/// transcripts of the sessions that wrote them).
pub const KEEP: usize = 5000;

/// Whose chat it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Main,
    Project,
    Task,
}

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Owner,
    Session,
    System,
}

/// What a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Talk: the owner's or a session's.
    Message,
    /// A card waiting for the owner (its buttons are the card's choices).
    Question,
    /// What the AIs decided or did, for the owner to know (W64).
    Notice,
}

/// One message of a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Message {
    pub seq: u64,
    pub author: Author,
    pub session: Option<String>,
    /// The seat the writing session filled (`builder`, `planner`, …).
    pub seat: Option<String>,
    pub kind: MessageKind,
    #[table(content)]
    pub text: String,
    /// The card of a question.
    pub card: Option<String>,
    pub at_ms: u64,
}

/// One chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "chat")]
pub struct Chat {
    #[table(key)]
    pub id: String,
    pub kind: Kind,
    #[table(index)]
    pub project: Option<String>,
    #[table(index)]
    pub task: Option<String>,
    pub title: String,
    pub archived: bool,
    pub next_seq: u64,
    pub created_ms: u64,
    pub updated_ms: u64,
    #[table(child)]
    pub messages: Vec<Message>,
}

impl Chat {
    /// The chat without its messages, for lists.
    pub fn summary(&self) -> Chat {
        Chat {
            messages: Vec::new(),
            ..self.clone()
        }
    }
}

/// What to post.
#[derive(Debug, Clone)]
pub struct Post {
    pub author: Author,
    pub session: Option<String>,
    pub seat: Option<String>,
    pub kind: MessageKind,
    pub text: String,
    pub card: Option<String>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    chats: BTreeMap<String, Chat>,
}

/// Every chat.
#[derive(Default)]
pub struct Chats {
    inner: Mutex<Inner>,
}

impl Chats {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn insert(inner: &mut Inner, id: String, kind: Kind, project: Option<String>, task: Option<String>, title: String, now_ms: u64) -> Chat {
        let chat = Chat {
            id: id.clone(),
            kind,
            project,
            task,
            title,
            archived: false,
            next_seq: 1,
            created_ms: now_ms,
            updated_ms: now_ms,
            messages: Vec::new(),
        };
        inner.chats.insert(id, chat.clone());
        chat
    }

    /// The main orchestrator's chat, made on first use.
    pub fn main(&self, now_ms: u64) -> Chat {
        let mut inner = self.lock();
        if let Some(chat) = inner.chats.get(MAIN) {
            return chat.clone();
        }
        Self::insert(&mut inner, MAIN.into(), Kind::Main, Some("plaza".into()), None, "Plaza".into(), now_ms)
    }

    /// Opens a new chat with `project`'s planner.
    pub fn open_project(&self, project: &str, title: &str, now_ms: u64) -> Chat {
        let mut inner = self.lock();
        inner.next += 1;
        let id = format!("ch{}", inner.next);
        Self::insert(&mut inner, id, Kind::Project, Some(project.into()), None, title.into(), now_ms)
    }

    /// `task`'s chat (one per task), made on first use.
    pub fn of_task(&self, task: &str, project: &str, title: &str, now_ms: u64) -> Chat {
        let id = format!("task-{task}");
        let mut inner = self.lock();
        if let Some(chat) = inner.chats.get(&id) {
            return chat.clone();
        }
        Self::insert(&mut inner, id, Kind::Task, Some(project.into()), Some(task.into()), title.into(), now_ms)
    }

    pub fn get(&self, id: &str) -> Option<Chat> {
        self.lock().chats.get(id).cloned()
    }

    /// The chats (without their messages), newest activity first; of one project when `project` is given.
    pub fn list(&self, project: Option<&str>, archived: bool) -> Vec<Chat> {
        let mut out: Vec<Chat> = self
            .lock()
            .chats
            .values()
            .filter(|c| project.is_none_or(|p| c.project.as_deref() == Some(p)))
            .filter(|c| archived || !c.archived)
            .map(Chat::summary)
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.updated_ms));
        out
    }

    /// Messages of a chat after `after` (its seq), at most `limit`.
    pub fn messages(&self, id: &str, after: u64, limit: usize) -> Option<Vec<Message>> {
        let inner = self.lock();
        let chat = inner.chats.get(id)?;
        Some(chat.messages.iter().filter(|m| m.seq > after).take(limit).cloned().collect())
    }

    /// Adds a message to a chat.
    pub fn post(&self, id: &str, post: Post, now_ms: u64) -> Option<Message> {
        let mut inner = self.lock();
        let chat = inner.chats.get_mut(id)?;
        let message = Message {
            seq: chat.next_seq,
            author: post.author,
            session: post.session,
            seat: post.seat,
            kind: post.kind,
            text: post.text,
            card: post.card,
            at_ms: now_ms,
        };
        chat.next_seq += 1;
        chat.updated_ms = now_ms;
        chat.messages.push(message.clone());
        if chat.messages.len() > KEEP {
            let over = chat.messages.len() - KEEP;
            chat.messages.drain(..over);
        }
        Some(message)
    }

    /// Archives or restores a chat (the main chat cannot be archived).
    pub fn set_archived(&self, id: &str, archived: bool) -> Option<Chat> {
        let mut inner = self.lock();
        let chat = inner.chats.get_mut(id)?;
        if chat.kind != Kind::Main {
            chat.archived = archived;
        }
        Some(chat.summary())
    }

    /// Renames a chat.
    pub fn rename(&self, id: &str, title: &str) -> Option<Chat> {
        let mut inner = self.lock();
        let chat = inner.chats.get_mut(id)?;
        chat.title = title.to_string();
        Some(chat.summary())
    }

    /// The chats' rows, for the hub's store.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        apz_store::RowSet::list(def(), inner.chats.values())
    }

    /// Replaces the chats with what the store holds (at start).
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Chat> = apz_store::table::load_list(conn, def())?;
        let mut inner = Inner::default();
        for chat in stored {
            if let Some(n) = chat.id.strip_prefix("ch").and_then(|n| n.parse::<u64>().ok()) {
                inner.next = inner.next.max(n);
            }
            inner.chats.insert(chat.id.clone(), chat);
        }
        *self.lock() = inner;
        Ok(())
    }
}

/// The chats' tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Chat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Post {
        Post {
            author: Author::Owner,
            session: None,
            seat: None,
            kind: MessageKind::Message,
            text: text.into(),
            card: None,
        }
    }

    #[test]
    fn the_main_chat_is_one_project_chats_are_many_and_a_task_has_one() {
        let chats = Chats::default();
        assert_eq!(chats.main(1).id, MAIN);
        assert_eq!(chats.main(2).id, MAIN);
        let a = chats.open_project("pr1", "Export", 3);
        let b = chats.open_project("pr1", "Docs", 4);
        assert_ne!(a.id, b.id);
        assert_eq!(chats.of_task("t1", "pr1", "T1", 5).id, chats.of_task("t1", "pr1", "T1", 6).id);
        assert_eq!(chats.list(Some("pr1"), false).len(), 3);
    }

    #[test]
    fn messages_are_numbered_and_paged_and_chats_round_trip() {
        let chats = Chats::default();
        let c = chats.open_project("pr1", "Export", 1);
        for i in 0..5 {
            chats.post(&c.id, said(&format!("m{i}")), 2 + i).unwrap();
        }
        let page = chats.messages(&c.id, 2, 2).unwrap();
        assert_eq!(page.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![3, 4]);
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &chats.rows().unwrap()).unwrap();
        let loaded = Chats::default();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.get(&c.id), chats.get(&c.id));
        assert_ne!(loaded.open_project("pr1", "Next", 9).id, c.id, "ids continue after the stored ones");
    }
}
