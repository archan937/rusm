# The live REPL

A running RUSM node isn't a black box you watch from the outside — it's a system you can
reach into and *drive*. `rusm attach` opens a **JavaScript shell into the live node**: type
real code against real processes while the node keeps serving. It's the BEAM's
`iex --remsh`, brought to WebAssembly — and it's how you inspect, debug, and operate a node
without redeploying it.

## Why this matters

Most runtimes hand you logs and a metrics dashboard. When something misbehaves you add more
logging and ship again. RUSM gives you a prompt *inside the process table*: ask the live
system what it's doing, poke it, and watch it respond. Nothing to wire up — the node you
started with `rusm node start` already exposes it.

```sh
rusm node start
rusm attach
```

`detail`, `help`, and `quit` are shell commands; **every other line is JavaScript**,
evaluated against the node. Bindings persist across lines, so you build up context as you go.

## The whole Process API, at a prompt

Everything a component can do, you can do live — the `Process` global is in scope:

```js
> Process.list().length            // how many processes are alive
1284
> p = Process.whereis("store")     // find a service by name (the binding persists)
43
> Process.isAlive(p)
true
> Process.whereisTag("room:general").length   // who's in a chat room right now
12
```

## Call a running service — and get the reply

`connect(name)` hands you a typed client over a resident service; call its methods and
`await` the result, straight from the shell. (`spawn(name)` does the same over a
freshly-spawned component.)

```js
> await connect("store").list()
[{"id":1,"text":"ship the docs"}]
> await connect("store").add({ text: "write the REPL page" })
{"id":2,"text":"write the REPL page"}
```

That's a real request/reply over the actor wire — the same typed client your components use,
pointed at the live node from your terminal.

## Operate the live system

Message a process, schedule a delayed send, disconnect a whole group, stop a runaway:

```js
> Process.send(p, JSON.stringify({ op: "flush" }))   // fire-and-forget
> Process.sendAfter(p, 5000, "reminder")             // deliver in 5s → a timer handle
1
> Process.killTag("room:general")                    // disconnect everyone in the room
12
> Process.kill(stuck)                                // stop a wedged process
true
```

## One prompt, every language

`whereis` / `send` / `kill` speak the actor wire, not a language — so the same shell
inspects and drives **Rust, TypeScript, and Go** processes identically. A Go WebSocket
connection, a Rust `store` service, a TypeScript worker: from here they're all just pids.

## Stateful, async, forgiving

- **Bindings persist.** `const p = Process.whereis("store")` on one line, use `p` on the
  next — `const` / `let` / `var` / `function` / `class` and bare assignments all carry over.
- **Top-level `await` just works.** `const ok = await connect("api").healthy()`.
- **`console.*` is echoed back.** Output your guest logs to your prompt with `console.log`.
- **Errors don't end your session.** A throw — or a typo — is reported; your bindings
  survive, and the next line runs.

## How it works

Each connection gets its **own** sandboxed REPL process — a `Trusted` JavaScript worker on
the shared rquickjs runner, which is RUSM's [dynamic-JS](/build-an-app/dynamic-js)
machinery pointed at *you*. Your lines run in that one process's persistent scope; it
inspects and messages the rest of the node over the ordinary actor API, and it dies with
your connection. A line that blocks forever (awaiting a message that never comes) is bounded
by a timeout that resets the session, so the shell can never wedge.

## Local-only — for now

Eval is accepted **only from a loopback client**; the node refuses it from a remote attach.
That is the deliberate interim boundary: a JavaScript shell into a node is, by design, as
powerful as `iex --remsh`, so until the attach channel is authenticated
([Phase 12](/about/roadmap)) it stays on the machine that started the node. A remote
`rusm attach` still gets full read-only [observe](/deep-dive/observe-a-running-node).

---

See [observe a running node](/deep-dive/observe-a-running-node) for the watch side, and
[live attach](/deep-dive/live-attach) for the protocol underneath.
