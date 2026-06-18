//! A resident `reporter` worker: `#[rusm_rs::main]` runs it once at boot. It reaches the
//! `store` service through the generated typed client and exercises the whole composition
//! surface — a plain call, a callback argument, a streamed result, and a fire-and-forget
//! cast — then PARKS. Returning would let the supervisor restart it in a loop (and re-spawn
//! the store each time); a resident worker loops or parks, it never just exits. It only
//! seeds when the board is empty, so a restart is harmless. The guest-composition showcase,
//! over the same todos the `api` serves and the `feed` streams — the Rust twin of the TS
//! example's `components/reporter`.
use store_svc::store;

#[rusm_rs::main]
fn run() {
    let store = store::Client::spawn("store").expect("spawn store");

    // call: a request/reply summary.
    let todos = store.list().expect("list");
    let done = todos.iter().filter(|t| t.done).count();
    log::info!("reporter: {} todos, {done} done", todos.len());

    // callback: seed a welcome list on a fresh board; progress is reported back to us as
    // each todo lands (only when empty, so this never re-seeds).
    if todos.is_empty() {
        let seeded = store
            .import_many(
                vec![
                    "Welcome to the RUSM todo board".into(),
                    "Watch the live feed on :8081".into(),
                    "Join the chat on :8082".into(),
                ],
                |done| log::info!("reporter: seeded {done}"),
            )
            .expect("import_many");
        log::info!("reporter: seeded {seeded} todos");
    }

    // streaming: iterate a streamed result (each todo arrives as one byte-stream chunk).
    let streamed = store.all().count();
    log::info!("reporter: streamed {streamed} todos");

    // cast: fire-and-forget — no reply awaited (the typed client's methods are all blocking
    // calls, so a cast goes over the wire layer directly).
    rusm_rs::wire::cast(store.pid, "ping", &[(); 0]);

    // Park: stay resident without re-running (see the note above). No message ever arrives.
    loop {
        rusm_rs::receive_bytes();
    }
}
