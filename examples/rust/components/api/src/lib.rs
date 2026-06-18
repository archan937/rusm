//! The todo HTTP API — a module of `#[rusm_rs::handlers]` actions (no `main`, no router;
//! routes live in rusm.toml's [serve.routes]). Each request runs in a fresh, isolated
//! instance. Reads/writes the durable todo list and publishes every change to the feed's
//! subscribers. The data layer lives in the shared `todos` crate.
use rusm_rs::http::{Params, Request, Response};
use todos::Todo;

/// Add the CORS headers every response carries (so a browser app on another origin works).
fn cors(resp: Response) -> Response {
    resp.header("access-control-allow-origin", "*")
        .header("access-control-allow-methods", "GET, POST, PATCH, DELETE, OPTIONS")
        .header("access-control-allow-headers", "content-type")
}

fn json(body: &impl serde::Serialize, status: u16) -> Response {
    cors(Response::new(status, serde_json::to_vec(body).unwrap_or_default())
        .header("content-type", "application/json"))
}

fn error(status: u16, message: &str) -> Response {
    json(&serde_json::json!({ "error": message }), status)
}

fn id_param(p: &Params) -> Option<u64> {
    p.get("id").and_then(|s| s.parse::<u64>().ok())
}

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    pub fn list(_req: Request, _p: Params) -> Response {
        json(&todos::list(), 200)
    }

    pub fn create(req: Request, _p: Params) -> Response {
        #[derive(serde::Deserialize)]
        struct New {
            text: String,
        }
        let Ok(new) = serde_json::from_slice::<New>(&req.body) else {
            return error(400, "invalid body");
        };
        let text = new.text.trim();
        if text.is_empty() {
            return error(400, "text is required");
        }
        let todo = Todo { id: todos::next_id(), text: text.to_string(), done: false };
        todos::save(&todo);
        log::info!("created #{}: {}", todo.id, todo.text);
        todos::publish();
        json(&todo, 201)
    }

    pub fn toggle(_req: Request, p: Params) -> Response {
        let Some(id) = id_param(&p) else { return error(400, "bad id") };
        let Some(mut todo) = todos::get(id) else { return error(404, "no such todo") };
        todo.done = !todo.done;
        todos::save(&todo);
        log::info!("toggled #{id} → {}", if todo.done { "done" } else { "open" });
        todos::publish();
        json(&todo, 200)
    }

    pub fn remove(_req: Request, p: Params) -> Response {
        let Some(id) = id_param(&p) else { return error(400, "bad id") };
        if !todos::remove(id) {
            return error(404, "no such todo");
        }
        log::info!("deleted #{id}");
        todos::publish();
        cors(Response::new(204, Vec::new()))
    }

    pub fn preflight(_req: Request, _p: Params) -> Response {
        cors(Response::new(204, Vec::new()))
    }
}
