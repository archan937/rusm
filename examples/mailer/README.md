# mailer

Three standalone apps demonstrating the same `mailer` bridge (transactional email via
[Resend](https://resend.com)) in each bridge **host language**:

| Flavour | Bridge host | Guest | Call overhead |
|---|---|---|---|
| [`rust/`](./rust/) | `host.rs` — reqwest + tokio | Rust HTTP handler | ~few hundred ns |
| [`typescript/`](./typescript/) | `host.ts` — `fetch` | TS HTTP handler | ~1–10 µs |
| [`go/`](./go/) | `host.go` — `net/http` | Go HTTP handler | ~1–10 µs |

The WIT contract (`bridges/mailer/bridge.wit`) is identical across all three. Set
`RESEND_API_KEY` in `.env` (copy `.env.example`) before serving.

## Run any flavour

```sh
cd examples/mailer/<rust|typescript|go>
cp .env.example .env   # fill in RESEND_API_KEY
rusm build && rusm serve
curl -X POST http://127.0.0.1:8080/send \
  -H 'Content-Type: application/json' \
  -d '{"to":"you@example.com","subject":"Hello","body":"<b>It works!</b>"}'
```

## Scaffold your own

```sh
rusm new notifier --template mailer --lang ts   # TS bridge + TS guest
rusm new notifier --template mailer --lang rs   # TS bridge + Rust guest
rusm new notifier --template mailer --lang go   # TS bridge + Go guest
```
