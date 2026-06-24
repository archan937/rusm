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

See [`../../README.md`](../../README.md) for all three bridge-host flavours.
