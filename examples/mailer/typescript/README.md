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

See [`../../README.md`](../../README.md) for all three bridge-host flavours.
