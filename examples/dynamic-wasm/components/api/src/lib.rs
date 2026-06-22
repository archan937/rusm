//! The plugin dispatcher. `GET /run/:plugin/:input` spawns the chosen **compiled WASM
//! plugin** at runtime — `spawn_from("plugin-runner", "kv:plugins/<plugin>")` — inside the
//! sandbox the operator declared, hands it the input, and returns its answer. The plugin is
//! compiled on its first use (cold) and served from the content-addressed cache on every
//! later request (hot). The dispatcher fixes *where* code runs and *what it may do*; the
//! request picks *which* code runs — it can never widen the sandbox.
use rusm_rs::http::{Params, Request, Response};
use rusm_rs::{me, receive_bytes_timeout, send_bytes, spawn_from};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    // GET /run/:plugin/:input — run a runtime-chosen plugin against the input.
    pub fn run(_req: Request, p: Params) -> Response {
        let plugin = p.get("plugin").unwrap_or("");
        let input = p.get("input").unwrap_or("");

        // Spawn the compiled plugin from the durable store, in the `plugin-runner` sandbox.
        // A missing/uncompilable bundle is a clean 404 — the dispatcher never crashes on it.
        let pid = match spawn_from("plugin-runner", &format!("kv:plugins/{plugin}")) {
            Ok(pid) => pid,
            Err(error) => {
                return Response::new(404, format!("no plugin `{plugin}`: {error}\n").into_bytes())
            }
        };

        // Request/reply over the actor wire: send "<our-pid>\n<input>"; the plugin computes
        // and answers us. A plugin that hangs or crashes simply times out — one unit, no
        // head-of-line blocking on the next request.
        send_bytes(pid, format!("{}\n{}", me().0, input).as_bytes());
        match receive_bytes_timeout(5_000) {
            Some(reply) => Response::new(200, [reply, b"\n".to_vec()].concat()),
            None => Response::new(504, b"plugin did not answer in time\n".to_vec()),
        }
    }
}
