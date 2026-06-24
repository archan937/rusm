# mailer — Go bridge host

A Go bridge (`bridges/mailer/host.go`) calling the [Resend](https://resend.com) API via
`net/http`. `rusm build` generates `_runner.go` and compiles the bridge with TinyGo.

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

`rusm build` discovers `bridges/mailer/host.go`, generates `_runner.go` and `go.mod`, then
TinyGo compiles `bridges/mailer/` → `wasm/bridge-mailer.wasm`. The Go runner is registered as a
**resident actor** at startup; each `mailer.send()` call from the guest crosses the actor wire
(~1–10µs). WIT record params arrive as `json.RawMessage` — `host.go` unmarshals and calls the
Resend API via `net/http`. `RESEND_API_KEY` is read from the environment at call time.

See [`../../README.md`](../../README.md) for all three bridge-host flavours.
