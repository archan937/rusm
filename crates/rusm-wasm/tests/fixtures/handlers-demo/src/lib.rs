//! A per-request HTTP handler component: just `pub fn`s — no `main`, no router, no wire.
//! The host resolves the route and dispatches the matched action here. (Server-Sent
//! Events are a per-connection `rusm_rs::sse::serve` handler now, not a 3-arg action.)
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod demo {
    use super::*;

    pub fn hello(_req: Request, p: Params) -> Response {
        Response::text(format!("hi {}\n", p.get("name").unwrap_or("world")))
    }

    pub fn echo(req: Request, _p: Params) -> Response {
        Response::new(200, req.body)
    }
}
