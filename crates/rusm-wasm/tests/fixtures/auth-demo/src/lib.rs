//! A per-request HTTP handler that calls a **custom application bridge** to reflect the
//! request's tenant. `run` drives `rusm_rs::http::serve_request`; the matched action calls
//! `demo:bridge/whoami.tenant()` — a typed host import whose answer comes from the host-only
//! claims context an auth hook seeded from the validated request token. The guest passes
//! nothing and cannot read or forge the tenant: the host alone decides it. The standard
//! rusm interfaces reuse `rusm-rs`'s bindings; only `whoami` is generated fresh here.

wit_bindgen::generate!({
    world: "auth-demo",
    path: "wit",
    with: {
        "rusm:runtime/types@0.1.0": rusm_rs::rusm::runtime::types,
        "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor,
        "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv,
        "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log,
        "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg,
        "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams,
        "rusm:runtime/serve@0.1.0": rusm_rs::rusm::runtime::serve,
        // The custom bridge: generate its binding fresh (it isn't part of rusm-rs).
        "demo:bridge/whoami@0.1.0": generate,
    },
});

use demo::bridge::whoami::tenant;
use rusm_rs::http::Response;

struct Component;

impl Guest for Component {
    fn run() {
        rusm_rs::http::serve_request(|action, _req, _params| match action {
            // The handler returns the tenant the bridge reports — i.e. the claims context
            // the host attached to this request. The guest never sees the raw identity.
            "me" => Some(Response::text(tenant())),
            _ => None,
        });
    }
}

export!(Component);
