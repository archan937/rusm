# rusm-rs — the Rust guest crate for RUSM

Write a [RUSM](https://github.com/archan937/rusm) **component** in Rust — a sandboxed,
supervised WASM process on an Erlang-style actor runtime. `rusm-rs` wraps the raw
`wit-bindgen` actor bindings into a small, idiomatic API (`Pid`, `send`/`receive`,
`spawn`, the registry, process groups (`register_tag`/`kill_tag`), `Stream`), adds
process-per-request HTTP/SSE handlers
(`#[rusm_rs::handlers]`) and per-connection WS (`ws::serve`), and hides the
component boilerplate behind `#[rusm_rs::main]`. The TypeScript twin is the
[`rusm-ts`](https://www.npmjs.com/package/rusm-ts) npm package — they share one JSON wire and
interoperate.

Built for `wasm32-wasip2`. A component crate is a `cdylib` depending on `rusm-rs` and
`wit-bindgen`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
rusm-rs = "0.4"
wit-bindgen = "0.46"
```

The fastest start is `rusm new <name> --rust` (scaffolds this for you).

## A component is just your logic

`#[rusm_rs::main]` generates the `process` world, the `Guest` impl, and `export!` — so
there's **no `wit/` dir and no `wit-bindgen` boilerplate** in your source:

```rust
use rusm_rs::{receive_bytes, send_bytes, set_label, Pid};

#[rusm_rs::main]
fn main() {
    let reply_to: u64 = String::from_utf8(receive_bytes()).unwrap().parse().unwrap();
    set_label("worker");
    send_bytes(Pid(reply_to), b"pong");
}
```

## Serving HTTP / WS / SSE

Serving is **process-per-unit-of-work**: a fresh sandboxed instance per HTTP/SSE
request, one process per WS connection — no head-of-line blocking, crash containment per
unit, full isolation. Routing is declarative in each listener's `rusm.toml`
`[serve.routes]` subtable (`"METHOD /path/:param" = "component#action"`), so handlers
carry no router code.

**HTTP** — a module of `pub fn(Request, Params) -> Response` under `#[rusm_rs::handlers]`
(no `main`, no router; the macro generates the shell + dispatch):

```rust
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    pub fn show(_req: Request, p: Params) -> Response {            // GET /users/:id
        Response::text(format!("user {}\n", p.get("id").unwrap_or("?")))
    }
}
```

**SSE** — implement `sse::Handler` (`open`/`message`/`close`) and `sse::serve` it; the host
runs one isolated process per connection (the SSE twin of WS). Subscribe to an event source
in `open` (e.g. a process-group tag), emit each pushed event with `Stream::data`; the
platform owns the wire framing, keep-alive heartbeats, and disconnect:

```rust
use rusm_rs::sse::{self, Handler, Stream};

#[derive(Default)]
struct Feed;

impl Handler for Feed {
    fn open(&mut self, _s: &Stream) { rusm_rs::register_tag("todos"); }  // subscribe
    fn message(&mut self, s: &Stream, ev: Vec<u8>) { s.data(&ev); }      // a published event → emit
    fn close(&mut self, _s: &Stream) { /* disconnect — clean or dropped */ }
}

#[rusm_rs::main]
fn main() { sse::serve(Feed::default()); }
```

**WS** — implement `ws::Handler` (`open`/`message`/`close`, reply via `Connection::send`)
and `ws::serve` it; the host runs one isolated process per connection. `open` fires on
connect, `message` per inbound frame, `close` once on disconnect (clean or dropped) —
`open` and `close` are optional:

```rust
use rusm_rs::ws::{self, Connection, Handler};

#[derive(Default)]
struct Echo;

impl Handler for Echo {
    fn open(&mut self, conn: &Connection) { conn.send(b"welcome\n"); }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) { conn.send(&data); }
    fn close(&mut self, _conn: &Connection) { /* disconnect — clean or dropped */ }
}

#[rusm_rs::main]
fn main() { ws::serve(Echo::default()); }
```

State that must outlive a request lives in a long-lived `[components.<name>]` service
(`resident = true`, reached over the actor API) or durable `kv` — never in the ephemeral
serving instance.

## Services & the typed client

`#[rusm_rs::service]` on a module of free functions (no `impl`, no `self`) generates a
`serve()` dispatch loop and a typed, blocking `Client`:

```rust
#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 { a + b }
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> { 1..=n }   // streaming
    pub fn work(progress: rusm_rs::Callback<i64>) -> String {        // callback
        for pct in [25, 50, 100] { progress.call(pct); }
        "done".into()
    }
}
```

```rust
let calc = calc::Client::spawn("calc")?;          // spawn-from-guest by name
let sum = calc.add(2, 3)?;                         // a call
for n in calc.count_to(3) { /* 1, 2, 3 */ }       // a stream
let status = calc.work(|pct| println!("{pct}"))?; // a callback (closure stays here)
```

The same JSON wire as the `rusm-ts` package, so a Rust client and a TS service
interoperate.

## What you get

`Pid`, `send`/`receive` (raw bytes and serde-typed JSON), `spawn`, the named registry
(`register`/`whereis`/`unregister`), `set_label`, `is_alive`, `kill`, `list`, a
back-pressured `Stream`, an in-guest `Supervisor`, `#[rusm_rs::main]`,
`#[rusm_rs::service]`, and the `http` (`#[rusm_rs::handlers]`, `Params`, `Sse`) / `ws`
(`ws::serve`, `Connection`) serving APIs — all over the `rusm:runtime`
actor world. See the [RUSM docs](https://archan937.github.io/rusm/) for the full guide.
