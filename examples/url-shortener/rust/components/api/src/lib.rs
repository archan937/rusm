//! A tiny URL shortener — routed #[rusm_rs::handlers] actions over durable kv.
//! `POST /shorten` stores a URL under a fresh code; `GET /:code` redirects to it.
use rusm_rs::http::{Params, Request, Response};
use rusm_rs::kv;

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    // POST /shorten — the body is the long URL; store it under a fresh code.
    pub fn shorten(req: Request, _p: Params) -> Response {
        let target = String::from_utf8_lossy(&req.body).trim().to_string();
        if target.is_empty() {
            return Response::new(400, b"send a URL in the body\n".to_vec());
        }
        let b = kv::bucket("links");
        let code = (b.list().unwrap_or_default().len() + 1).to_string(); // simple sequential code
        let _ = b.set(&code, target.as_bytes());
        Response::new(201, format!("/{code}\n").into_bytes())
    }

    // GET /:code — look the code up and redirect to the URL.
    pub fn expand(_req: Request, p: Params) -> Response {
        let code = p.get("code").unwrap_or("");
        match kv::bucket("links").get(code).ok().flatten() {
            Some(url) => {
                Response::new(302, Vec::new()).header("location", String::from_utf8_lossy(&url))
            }
            None => Response::new(404, b"not found\n".to_vec()),
        }
    }
}
