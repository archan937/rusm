//! The mailer bridge's Rust host impl — authored in the platform-bridge convention
//! (`impl <iface>::Host for BridgeHost` + `pub fn add_to_linker`). Calls the Resend API
//! via `reqwest`. Set `RESEND_API_KEY` in the environment or `.env` before serving.
//!
//! `rusm build` generates `src/bindings.rs`, `src/bridges.rs`, `wit/`, and the synthesized
//! bindgen world. This file is the only one the app author writes for the bridge's behaviour.

use crate::bindings::app::mailer::smtp;
use rusm_wasm::wasmtime::component::HasSelf;
use rusm_wasm::{wasmtime, BridgeHost, BridgeLinker};

/// Register this bridge into the component linker (called by the generated `bridges::extend`).
pub fn add_to_linker(linker: &mut BridgeLinker) -> wasmtime::Result<()> {
    smtp::add_to_linker::<_, HasSelf<BridgeHost>>(linker, |host| host)
}

impl smtp::Host for BridgeHost {
    async fn send(&mut self, msg: smtp::Message) -> bool {
        let Ok(api_key) = std::env::var("RESEND_API_KEY") else {
            return false;
        };
        reqwest::Client::new()
            .post("https://api.resend.com/emails")
            .bearer_auth(&api_key)
            .json(&serde_json::json!({
                "from":    "noreply@example.com",
                "to":      msg.to,
                "subject": msg.subject,
                "html":    msg.body,
            }))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
