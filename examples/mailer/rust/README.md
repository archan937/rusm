# mailer — Rust bridge host

A Rust bridge (`bridges/mailer/host.rs`) calling the [Resend](https://resend.com) API via
`reqwest`. The bridge impl compiles directly into the host binary — zero delegation, no actor,
no JSON marshaling.

## Run

```sh
cp .env.example .env   # add RESEND_API_KEY=re_...
rusm build
rusm serve
curl -X POST http://127.0.0.1:8080/send \
  -H 'Content-Type: application/json' \
  -d '{"to":"you@example.com","subject":"Hello","body":"<b>It works!</b>"}'
```

## How it works

`rusm build` discovers `bridges/mailer/`, regenerates `src/{bindings,bridges}.rs` and `wit/`,
vendors the contract into `components/api/`, and compiles the Rust guest. `rusm serve` runs the
host binary, where `bridges::extend` wires `bridges/mailer/host.rs` directly into the component
linker — no actor round-trip, no JSON marshaling. `RESEND_API_KEY` is read from the environment
at the time of each `send` call via `std::env::var`.

See [`../../README.md`](../../README.md) for all three bridge-host flavours.
