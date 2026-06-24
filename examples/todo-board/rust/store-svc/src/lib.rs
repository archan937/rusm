//! The `store` service contract — its exported functions ARE the API. RUSM runs the
//! receive→dispatch→reply loop (`store::serve()`) around them, and callers reach it with
//! the generated typed `store::Client` (`Client::spawn("store")` → blocking `.list()`,
//! streaming `.all()`, callback `.import_many(..)`). Client and service are generated from
//! this one module, so they can never drift — the Rust counterpart of the TS example's
//! `components/store` and its derived `Store` type.
//!
//! It composes the shared `todos` data layer over `kv` and publishes each change to the
//! feed — the same todos the `api` serves and the `feed` streams. This is the *composition*
//! half of the example, exercised by the `reporter` worker. (The HTTP `api` mutates the
//! todos directly: it has no mailbox to host a client; this service is the actor-side door
//! onto the same data.)
#[rusm_rs::service]
pub mod store {
    use todos::Todo;

    /// call: the current list.
    pub fn list() -> Vec<Todo> {
        todos::list()
    }

    /// call: add a todo (persists + publishes to the feed), returning the new one.
    pub fn add(text: String) -> Todo {
        todos::create(&text)
    }

    /// call: flip a todo's `done`; `None` if it doesn't exist.
    pub fn toggle(id: u64) -> Option<Todo> {
        todos::set_done(id)
    }

    /// call: delete a todo; `false` if it didn't exist.
    pub fn remove(id: u64) -> bool {
        todos::delete(id)
    }

    /// streaming: the list rides a byte stream the caller iterates — a bulk read that
    /// streams item-by-item rather than returning the whole vec at once.
    pub fn all() -> impl Iterator<Item = Todo> {
        todos::list().into_iter()
    }

    /// callback: bulk-add, reporting progress back to the caller as each todo lands.
    pub fn import_many(texts: Vec<String>, on_progress: rusm_rs::Callback<i64>) -> i64 {
        let mut done = 0;
        for text in &texts {
            todos::create(text);
            done += 1;
            on_progress.call(done);
        }
        done
    }

    /// cast-friendly: a fire-and-forget the caller never awaits (no return value).
    pub fn ping() {
        log::info!("store: ping");
    }
}
