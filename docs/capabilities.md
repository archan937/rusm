# Capabilities & sandboxing

Every process is default-deny. Named profiles set the baseline; the `Capabilities`
builder overrides per spawn:

```rust
use rusm_wasm::{Capabilities, CapabilityProfile};

CapabilityProfile::Sandboxed.capabilities();          // CPU + bounded heap only
Capabilities::nothing()                               // start from nothing…
    .max_memory(16 << 20)                             // …a 16 MiB ceiling
    .allow_network(true)                              // …outbound sockets
    .preopen("/srv/data", "/data", /* read_only */ true) // …a mounted dir
    .env("LOG", "info");                              // …an env var
```

In the app model you declare the same grants as a `[capabilities.<name>]` profile in
`rusm.toml` (see [Configuration](./reference-configuration)); the builder above is for
[embedding](./embedding). Grants map onto standard WASI plus a `StoreLimiter` memory cap. A
breach traps *only that process*. See
[permissions & sandboxing](./concepts/permissions-and-sandboxing).
