//! The todo data layer — the single source of truth for the todo model, shared by the
//! `api` (which mutates it) and the `feed` (which reads it). State lives in durable `kv`;
//! a change is broadcast to the feed's subscribers over a process-group tag (the platform
//! pub/sub primitive — no broker). The Rust twin of the TS example's `lib/todos.ts`.
use rusm_rs::kv;
use serde::{Deserialize, Serialize};

/// The process-group tag the feed streams subscribe to; `api` publishes changes to it.
pub const FEED_TAG: &str = "todos";

#[derive(Serialize, Deserialize, Clone)]
pub struct Todo {
    pub id: u64,
    pub text: String,
    pub done: bool,
}

fn bucket() -> kv::Bucket {
    kv::bucket("todos")
}

/// Every todo, by id ascending.
pub fn list() -> Vec<Todo> {
    let b = bucket();
    let mut todos: Vec<Todo> = b
        .list()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| b.get(id).ok().flatten())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    todos.sort_by_key(|t| t.id);
    todos
}

pub fn get(id: u64) -> Option<Todo> {
    bucket()
        .get(&id.to_string())
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn save(todo: &Todo) {
    let _ = bucket().set(
        &todo.id.to_string(),
        &serde_json::to_vec(todo).unwrap_or_default(),
    );
}

pub fn remove(id: u64) -> bool {
    bucket().delete(&id.to_string()).unwrap_or(false)
}

/// The next free id (max + 1; 1 for an empty list).
pub fn next_id() -> u64 {
    bucket()
        .list()
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

/// The current list as an SSE-ready JSON payload (what the feed emits).
pub fn snapshot() -> Vec<u8> {
    serde_json::to_vec(&list()).unwrap_or_default()
}

/// Push the current list to every open feed stream — `whereis_tag` + `send`, the platform
/// pub/sub over the [`FEED_TAG`] process group. Subscribers auto-release on exit.
pub fn publish() {
    let payload = snapshot();
    for pid in rusm_rs::whereis_tag(FEED_TAG) {
        rusm_rs::send_bytes(pid, &payload);
    }
}

// ── High-level operations — the single source for both the `api` (in-process) and the
// `store` service. Each persists then publishes the new list to subscribers. ──

/// Add a todo and publish; returns the new todo.
pub fn create(text: &str) -> Todo {
    let todo = Todo {
        id: next_id(),
        text: text.to_string(),
        done: false,
    };
    save(&todo);
    publish();
    todo
}

/// Flip a todo's `done` and publish; `None` if it doesn't exist.
pub fn set_done(id: u64) -> Option<Todo> {
    let mut todo = get(id)?;
    todo.done = !todo.done;
    save(&todo);
    publish();
    Some(todo)
}

/// Delete a todo and publish; `false` if it didn't exist.
pub fn delete(id: u64) -> bool {
    let removed = remove(id);
    if removed {
        publish();
    }
    removed
}
