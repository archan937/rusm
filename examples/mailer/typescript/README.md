# mailer — TypeScript bridge host

A TypeScript bridge (`bridges/mailer/host.ts`) that calls the [Resend](https://resend.com)
API as a resident actor. The TS guest calls `mailer.send()` as a plain typed import.

## Run

```sh
cp .env.example .env   # add your RESEND_API_KEY
bun install
rusm build
rusm serve
curl -X POST http://127.0.0.1:8080/send \
  -H 'Content-Type: application/json' \
  -d '{"to":"you@example.com","subject":"Hello","body":"<b>It works!</b>"}'
```

## How it works

`rusm build` discovers `bridges/mailer/host.ts`, generates the Rust delegation shim, the TS
runner (`bridges/mailer/_runner.ts`), and the host binary entry point. Bun bundles the runner
to `wasm/bridge-mailer.js`. At runtime the runner is a **resident actor**; each `mailer.send()`
call from the guest crosses the actor wire (~1–10µs), and the runner calls the Resend API via
`fetch`. `RESEND_API_KEY` is read from the environment at call time, not at startup.

See [`../../README.md`](../../README.md) for all three bridge-host flavours.
