# mailer

A RUSM app with a **mailer bridge** — a TypeScript host bridge that sends
transactional email via [Resend](https://resend.com). The TypeScript guest calls
`mailer.send()` as a plain typed import; RUSM routes it to the resident host actor.

## Build & run

```sh
# 1. Set your Resend API key
cp .env.example .env
# Edit .env and fill in RESEND_API_KEY=re_...

# 2. Generate the bridge glue and compile the guest
rusm build

# 3. Serve
rusm serve

# 4. Test
curl -X POST http://127.0.0.1:8080/send \
  -H 'Content-Type: application/json' \
  -d '{"to":"you@example.com","subject":"Hello","body":"<b>It works!</b>"}'
```

Swap `noreply@example.com` in `bridges/mailer/host.ts` with a domain you own
and have verified with Resend.

## Scaffold your own

```sh
rusm new myapp --template mailer --lang ts   # TypeScript guest
rusm new myapp --template mailer --lang rust # Rust guest
rusm new myapp --template mailer --lang go   # Go guest
cd myapp && rusm build && rusm serve
```
