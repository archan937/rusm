// A `reporter` **worker** (resident): RUSM runs `default` once at boot. It reaches the
// `store` service through the concealed typed client and exercises the whole composition
// surface — a plain call, a callback argument, a streamed result, and a fire-and-forget
// cast — then returns. This is the guest-composition showcase (services + typed client),
// folded into the todo board: it reports on (and seeds) the same todos the api serves and
// the feed streams. Idempotent, so a supervised restart is harmless.
import { spawn, Process } from "rusm-ts";
import type { Store } from "../store";

export default async function reporter(): Promise<void> {
  const store = spawn<Store>("store");

  // call: a request/reply summary.
  const todos = await store.list();
  console.log(`reporter: ${todos.length} todos, ${todos.filter((t) => t.done).length} done`);

  // callback: seed a welcome list on a fresh board; progress is reported back to us as
  // each todo lands. (Only when empty, so this never re-seeds.)
  if (todos.length === 0) {
    const seeded = await store.importMany(
      ["Welcome to the RUSM todo board", "Watch the live feed on :8081", "Join the chat on :8082"],
      (done) => console.log(`reporter: seeded ${done}`),
    );
    console.log(`reporter: seeded ${seeded} todos`);
  }

  // streaming: `for await` a generator handler's chunks.
  let streamed = 0;
  for await (const _todo of store.all()) streamed++;
  console.log(`reporter: streamed ${streamed} todos`);

  // cast: fire-and-forget — no reply awaited.
  store.cast.ping();

  // Stay resident without re-running: park forever (a long-lived worker awaiting work it
  // never gets). Returning would let the supervisor restart us in a loop — and re-spawn
  // the store each time. A resident worker either loops or parks; it never just exits.
  await Process.receive();
}
