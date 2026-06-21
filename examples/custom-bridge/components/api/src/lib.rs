//! A per-request HTTP handler that calls the app's own **custom `weather` bridge** — a
//! native host function, invoked as an ordinary typed WIT import. The `bridge = "…"`
//! attribute tells `#[handlers]` to bind the generated component world (which `rusm build`
//! synthesizes because this component's `forecaster` profile grants `weather`) and to
//! re-export the bridge at `crate::forecast`.

use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers(bridge = "weather:bridge/forecast@0.1.0")]
pub mod api {
    use super::*;

    /// `GET /forecast/:city` → the host bridge's forecast for `city`.
    pub fn forecast(_req: Request, p: Params) -> Response {
        let city = p.get("city").unwrap_or("nowhere");
        Response::text(crate::forecast::lookup(city))
    }
}
