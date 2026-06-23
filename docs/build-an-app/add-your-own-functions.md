# Add your own functions

Need your guests to call something the platform doesn't provide — a payment processor, an
email API, a signing routine, a proprietary database client? Add a **custom bridge**: a
native function *your app* defines once and calls from **any** guest — TypeScript, Rust,
or Go — as an ordinary typed import. It's RUSM's answer to a wasmCloud **capability
provider**, but compiled-in and typed: no lattice, no broker, no RPC, no JSON dispatcher.

## 1 — Define the bridge

A bridge is a directory `bridges/<name>/` with the **contract** (`bridge.wit`) and a **host
impl** — Rust (`host.rs`), TypeScript (`host.ts`), or Go (`host.go`). All three are
first-class; pick based on what the bridge needs to do:

| | `host.rs` | `host.ts` | `host.go` |
|---|---|---|---|
| Who authors it | Rust developer | Any developer | Go developer |
| Call overhead | ~few hundred ns (native ABI) | ~1–10 µs (actor round-trip + JSON) | ~1–10 µs (actor round-trip + JSON) |
| Best for | CPU-critical, tight-loop callers | I/O-bound work, 3rd-party JS SDKs | I/O-bound work, existing Go libs |

`rusm build` generates all surrounding glue for all three — the host crate, the WIT world,
the delegation shim, and the language-specific runner. A Rust bridge compiles in; a TS or
Go bridge runs as a resident actor (`bridge:<name>`) and the generated shim dispatches
to it over the actor wire.

The example below is a **transactional email bridge** (`mailer`) using the Resend API —
a universally understood I/O-bound task that naturally belongs in a bridge: it needs
network access, an API key, and shouldn't live inside guest Wasm code.

### The contract (shared by all three host impls)

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

The runner is a long-lived resident actor — the Resend API key loads once at startup and
is available for the node's entire lifetime.

```ts
// bridges/mailer/host.ts — the ONLY file you write.
// rusm build generates the Rust delegation shim, the TS dispatch runner (_runner.ts),
// and all host crate glue. Bun bundles the runner to wasm/bridge-mailer.js.
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

The `send` function is `async` — the resident actor `await`s the Resend response on its
own Tokio task, so nothing blocks the calling guest.

### Go host

TinyGo compiles `host.go` + the generated `_runner.go` into a single Wasm component.
WIT record params arrive as `json.RawMessage` — see the callout below.

```go
// bridges/mailer/host.go — the ONLY file you write.
// rusm build generates _runner.go, a minimal go.mod (if absent), and the Rust delegation
// shim. TinyGo compiles the whole bridges/mailer/ package to wasm/bridge-mailer.wasm.
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
// `message` record already serialised into a json.RawMessage — unmarshal into your struct.
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
When a WIT function takes a `record` parameter the generated dispatcher passes a
`json.RawMessage` (raw JSON bytes) — no intermediate allocation. Your `host.go` function
receives the slice and calls `json.Unmarshal` into its own struct, giving you full control
over field naming and validation. Primitive params (`string`, `uint32`, `bool`, …) are
deserialised to the correct Go type directly.
:::

### Rust host

Rust bridges compile directly into the host binary — zero delegation, no actor, no JSON.
The host crate is a normal Rust crate; add any dependency to its `Cargo.toml`.

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

A bridge is reachable only by a component whose capability profile lists it — like every
other grant, default-deny:

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
// Bridge calls are synchronous from the guest's perspective — no await needed.
// The js-runner fiber-parks the Wasm instance while the host resolves, then resumes.
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

The bridge contract above uses a `record` — and that's just the beginning. WIT carries the
full value-type set: records, variants, enums, lists, options, results, tuples. Extend the
mailer with an optional attachment, for example:

```wit
record attachment { filename: string, content: list<u8> }

send-with-attachment: func(msg: message, att: option<attachment>) -> bool;
```

Rust and Go receive the native generated types (`smtp::Message`, `smtp::Attachment`); a
TypeScript guest gets fully typed objects (`{ filename: string; content: number[] }`),
marshaled via `serde_json`. For **Go specifically**, any `record`, `enum`, `variant`, or
`result` param arrives as `json.RawMessage` in the generated dispatcher — the user's
`host.go` function unmarshals into its own struct.

## How it builds

`rusm build` discovers `bridges/`, generates all host glue + per-guest bindings, vendors
the contract into each granted component, and compiles the components **plus a small host
binary** that wires the bridges. `rusm serve` runs that binary — same serve loop as a
pure-guest app, bridges included.

For a **Rust bridge** (`host.rs`): `rusm build` writes `src/bindings.rs`, `src/bridges.rs`,
`wit/`, and the synthesized bindgen world. You author `host.rs`; everything else is
generated.

For a **TS bridge** (`host.ts`), `rusm build` additionally writes:
- `src/bridge_<name>_delegate.rs` — the Rust delegation shim (sends JSON over the actor
  wire to the resident runner)
- `bridges/<name>/_runner.ts` — the resident TS actor (`bridge:<name>`) that dispatches
  to your `host.ts` exports
- `src/main.rs` — a generated entry point calling `serve_with_init` (only if none exists)

Bun bundles the runner to `wasm/bridge-<name>.js`. It registers as a resident actor at
startup and runs under a supervisor for the node's lifetime.

For a **Go bridge** (`host.go`), `rusm build` additionally writes:
- `src/bridge_<name>_delegate.rs` — the same Rust delegation shim (wire protocol is
  identical for TS and Go)
- `bridges/<name>/_runner.go` — the resident Go actor that dispatches to your exported
  functions; WIT record/enum/variant params passed as `json.RawMessage`
- `bridges/<name>/go.mod` — a minimal module file (only if none exists, so you can add
  extra deps freely)
- `src/main.rs` — a generated entry point (only if none exists)

TinyGo compiles the whole `bridges/<name>/` package to `wasm/bridge-<name>.wasm` — a full
Wasm component registered at startup under a supervisor.

**Get a working skeleton in seconds:**

```sh
rusm new myapp --template mailer --lang ts    # TypeScript host bridge
rusm new myapp --template mailer --lang go    # Go host bridge
rusm new myapp --template mailer --lang rust  # Rust host bridge
cd myapp && rusm build && rusm serve
```

## The platform / application split

A Rust host impl is the zero-overhead choice: the host **is** Rust, so `host.rs` compiles
directly into the binary — no delegation, no marshaling, just the WIT ABI boundary. A
TypeScript or Go host impl trades a few microseconds per call for zero-Rust authoring: you
write one `host.ts` or `host.go` and the platform generates the delegation shim, the
dispatch runner, and all wiring. The TS and Go delegation paths share the same JSON wire
protocol; only the runner language differs.

For TS, the runner is the familiar js-runner environment — `fetch`, KV, the full actor
world, and the entire Bun/Node ecosystem. For Go, the runner is a TinyGo Wasm component
with the full rusm-go actor world (`rusm.KvGet`/`KvSet`, `rusm.Send`/`Receive`/`Spawn`,
outbound `net/http`) and any Go stdlib that TinyGo supports for `wasm32-wasip2`.

All three paths are **capability-gated and default-deny**: guests call the bridge as an
ordinary typed import, and the operator decides which component profiles may reach it.
