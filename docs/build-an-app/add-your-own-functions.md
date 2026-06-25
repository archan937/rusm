# Add your own functions

Need your guests to call something the platform doesn't provide — a payment processor, an
email API, a signing routine, a proprietary database client? Add a **custom bridge**: a
native function *your app* defines once and calls from **any** guest — TypeScript, Rust,
or Go — as an ordinary typed import.

No lattice. No broker. No RPC dispatcher. No JSON middleware. The bridge contract is a WIT
interface; `rusm build` generates all the glue. You write one file — `host.rs`, `host.ts`,
or `host.go` — and the rest is produced for you.

## Get started in 30 seconds

The fastest way in is a ready-made template. The `mailer` template wires a Resend email
bridge end-to-end — contract, host impl, guest call, capability grant, all generated:

```sh
rusm new myapp --template mailer              # TypeScript guest (default)
rusm new myapp --template mailer --lang rust  # Rust guest
rusm new myapp --template mailer --lang go    # Go guest
cd myapp && rusm build && rusm serve
```

The `weather` template ships a **Rust host bridge** — the zero-overhead path useful as a
starting point for CPU-critical or tight-loop callers:

```sh
rusm new myapp --template weather             # Rust host, TypeScript guest
rusm new myapp --template weather --lang rust # Rust host, Rust guest
rusm new myapp --template weather --lang go   # Rust host, Go guest
```

::: tip Adding a bridge to an existing project
`rusm generate bridge <name>` scaffolds `bridges/<name>/bridge.wit` and a starter
`host.ts` (or `host.rs` / `host.go` with `--lang`) into your current project, adds the
`rusm.toml` entry, and regenerates glue on the next `rusm build`.
:::

## How a bridge works

A bridge is a directory `bridges/<name>/` with a **contract** (`bridge.wit`) and a **host
impl** — Rust, TypeScript, or Go. `rusm build` generates everything else.

| | `host.rs` | `host.ts` | `host.go` |
|---|---|---|---|
| **Runs as** | Compiled into the host binary | Resident actor (`bridge:<name>`) | Resident actor (`bridge:<name>`) |
| **Call overhead** | ~few hundred ns (native ABI) | ~1–10 µs (actor round-trip + JSON) | ~1–10 µs (actor round-trip + JSON) |
| **Best for** | CPU-critical, tight-loop callers | I/O-bound work, 3rd-party JS SDKs | I/O-bound work, existing Go libraries |

A **Rust bridge** compiles directly into the host binary — zero delegation, no extra process.
A **TypeScript or Go bridge** runs as a supervised resident actor; the generated delegation
shim dispatches calls over the actor wire. Both are capability-gated and default-deny:
guests import the bridge only if their capability profile lists it.

## 1 — Define the contract

The WIT interface is the single source of truth for all three languages:

```wit
// bridges/mailer/bridge.wit
package app:mailer@0.1.0;

interface smtp {
    record message {
        to:      string,
        subject: string,
        body:    string,
    }

    /// Send a transactional email. Returns true if the provider accepted it.
    send: func(msg: message) -> bool;
}
```

### TypeScript host

Write one file — the runner, dispatch loop, and all Rust glue are generated:

```ts
// bridges/mailer/host.ts — the ONLY file you write.
const API_KEY = process.env.RESEND_API_KEY ?? "";

export async function send(msg: { to: string; subject: string; body: string }): Promise<boolean> {
  const res = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from:    "noreply@example.com",
      to:      msg.to,
      subject: msg.subject,
      html:    msg.body,
    }),
  });
  return res.ok;
}
```

The `send` function is `async` — the resident actor awaits the Resend response on its own
Tokio task, so nothing blocks the calling guest.

### Go host

```go
// bridges/mailer/host.go — the ONLY file you write.
package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"os"
)

var (
	apiKey = os.Getenv("RESEND_API_KEY")
	client = &http.Client{}
)

// Send delivers the email via Resend. The generated dispatcher calls this with the WIT
// `message` record already serialised into a json.RawMessage.
func Send(raw json.RawMessage) bool {
	var msg struct {
		To      string `json:"to"`
		Subject string `json:"subject"`
		Body    string `json:"body"`
	}
	if err := json.Unmarshal(raw, &msg); err != nil {
		return false
	}
	payload, _ := json.Marshal(map[string]string{
		"from":    "noreply@example.com",
		"to":      msg.To,
		"subject": msg.Subject,
		"html":    msg.Body,
	})
	req, _ := http.NewRequest("POST", "https://api.resend.com/emails", bytes.NewReader(payload))
	req.Header.Set("Authorization", "Bearer "+apiKey)
	req.Header.Set("Content-Type", "application/json")
	resp, err := client.Do(req)
	if err != nil {
		return false
	}
	resp.Body.Close()
	return resp.StatusCode < 300
}
```

::: tip WIT records in Go bridges
`record`, `enum`, `variant`, and `result` params arrive as `json.RawMessage` — no
intermediate allocation. Unmarshal into your own struct for full control over field naming.
Primitive params (`string`, `uint32`, `bool`, …) are deserialised to the correct Go type
directly.
:::

### Rust host

Rust bridges compile directly into the host binary — zero delegation, no actor, no JSON:

```rust
// bridges/mailer/host.rs — the ONLY Rust an app must add for a Rust bridge.
// Add `reqwest = { version = "0.12", features = ["json"] }` to Cargo.toml.
use crate::bindings::app::mailer::smtp;
use rusm_wasm::wasmtime::component::HasSelf;
use rusm_wasm::{wasmtime, BridgeHost, BridgeLinker};

pub fn add_to_linker(linker: &mut BridgeLinker) -> wasmtime::Result<()> {
    smtp::add_to_linker::<_, HasSelf<BridgeHost>>(linker, |host| host)
}

impl smtp::Host for BridgeHost {
    async fn send(&mut self, msg: smtp::Message) -> bool {
        let Ok(api_key) = std::env::var("RESEND_API_KEY") else { return false };
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
```

## 2 — Grant it (default-deny)

A bridge is reachable only by a component whose capability profile lists it:

```toml
[capabilities.notifier]
inherits = "sandboxed"
bridges = ["mailer"]        # this profile may import the `mailer` bridge

[components.api]
capability = "notifier"
```

## 3 — Call it from a guest

A guest calls the bridge as a plain typed import — no dispatcher, no marshaling:

::: code-group

```ts [TypeScript]
/// <reference path="../../bridges.d.ts" />
import { http } from "rusm-ts";

// `mailer` is a typed global the per-app js-runner exposes (rusm build compiles it in).
// Bridge calls are synchronous from the guest's perspective — the fiber parks while
// the host resolves, then resumes. No await needed.
export default http({
  async post(req) {
    const { to, subject, body } = await req.json();
    const sent = mailer.send({ to, subject, body });
    return sent
      ? new Response("queued", { status: 202 })
      : new Response("delivery failed", { status: 502 });
  },
});
```

```rust [Rust]
use rusm_rs::http::{Params, Request, Response};
use serde::Deserialize;

#[derive(Deserialize)]
struct Payload { to: String, subject: String, body: String }

// The `bridge = …` attribute imports the bridge; call it at `crate::<iface>`.
#[rusm_rs::handlers(bridge = "app:mailer/smtp@0.1.0")]
pub mod api {
    use super::*;
    pub fn notify(req: Request, _p: Params) -> Response {
        let Ok(p) = req.json::<Payload>() else { return Response::bad_request() };
        if crate::smtp::send(&smtp::Message { to: p.to, subject: p.subject, body: p.body }) {
            Response::status(202)
        } else {
            Response::status(502)
        }
    }
}
```

```go [Go]
import (
    "github.com/archan937/rusm/packages/rusm-go/web"
    smtp "go-api/internal/wit/app/mailer/smtp" // generated by rusm build
)

func run() {
    h := web.NewHandlers()
    h.Handle("notify", func(req web.Request, _ web.Params) web.Response {
        var p struct{ To, Subject, Body string }
        if err := req.JSON(&p); err != nil { return web.Status(400) }
        if smtp.Send(smtp.Message{To: p.To, Subject: p.Subject, Body: p.Body}) {
            return web.Status(202)
        }
        return web.Status(502)
    })
    h.Serve()
}
```

:::

## Rich types, not just strings

The bridge contract uses a `record` — and that's just the beginning. WIT carries the full
value-type set: records, variants, enums, lists, options, results, tuples. Extend the
mailer with an optional attachment:

```wit
record attachment { filename: string, content: list<u8> }

send-with-attachment: func(msg: message, att: option<attachment>) -> bool;
```

Rust and Go receive native generated types (`smtp::Message`, `smtp::Attachment`); a
TypeScript guest gets fully typed objects (`{ filename: string; content: number[] }`).

## What `rusm build` generates

`rusm build` discovers `bridges/`, generates all host glue and per-guest bindings, vendors
the contract into each granted component, and compiles a host binary that wires the bridges
into the serve loop — same `rusm serve` command, bridges included.

- **Rust bridge** — writes `src/bindings.rs`, `src/bridges.rs`, `wit/`, and the synthesized
  bindgen world. You author `host.rs`; everything else is generated.
- **TypeScript bridge** — additionally writes `src/bridge_<name>_delegate.rs` (the Rust
  delegation shim), `bridges/<name>/_runner.ts` (the resident actor), and `src/main.rs` (if
  absent). Bun bundles the runner to `wasm/bridge-<name>.js`.
- **Go bridge** — additionally writes the delegation shim, `bridges/<name>/_runner.go`, a
  minimal `go.mod` (if absent), and `src/main.rs` (if absent). TinyGo compiles the whole
  package to `wasm/bridge-<name>.wasm`.

All three runners register as supervised resident actors at startup — they stay alive for the
node's lifetime and are restarted by the supervisor on failure.

## Go deeper

- [Grant capabilities](/build-an-app/grant-capabilities) — the full capability model and custom profiles
- [Runnable mailer example](https://github.com/archan937/rusm/tree/main/examples/mailer) — TS, Rust, and Go guests against a real Resend bridge
- [Runnable weather-api example](https://github.com/archan937/rusm/tree/main/examples/weather-api) — Rust host bridge, all three guest languages
