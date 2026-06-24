// A Rust HTTP handler that calls the mailer bridge.
// The `bridge = …` attribute imports the bridge and exposes the generated types at `crate::smtp`.
use rusm_rs::http::{Params, Request, Response};
use serde::Deserialize;

#[derive(Deserialize)]
struct Payload {
    to: String,
    subject: String,
    body: String,
}

#[rusm_rs::handlers(bridge = "app:mailer/smtp@0.1.0")]
pub mod api {
    use super::*;
    pub fn post(req: Request, _p: Params) -> Response {
        let Ok(p) = req.json::<Payload>() else {
            return Response::bad_request();
        };
        let msg = crate::smtp::Message { to: p.to, subject: p.subject, body: p.body };
        if crate::smtp::send(&msg) {
            Response::status(202)
        } else {
            Response::status(502)
        }
    }
}
