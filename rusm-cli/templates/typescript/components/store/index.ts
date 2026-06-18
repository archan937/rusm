// A `store` **service** — its exported functions ARE the API; RUSM runs the
// receive→dispatch→reply loop around them, so there's no Process plumbing. Callers reach
// it with the concealed typed client `spawn<Store>("store")`: `await store.list()` (call),
// `for await (… of store.all())` (streaming), `store.cast.ping()` (fire-and-forget). It
// composes the shared todo data layer over `kv` and publishes each change to the feed.
//
// The HTTP `api` serves CRUD directly (it has no mailbox to host a typed client); this
// service is the *composition* half of the example, exercised by the `reporter` worker —
// over the same todos the api serves and the feed streams.
import * as todos from "../../lib/todos";

export function list(): todos.Todo[] {
  return todos.list();
}

export function add(input: { text: string }): todos.Todo {
  return todos.create(input.text);
}

export function toggle(id: number): todos.Todo | null {
  return todos.setDone(id);
}

export function remove(id: number): boolean {
  return todos.del(id);
}

/** A streaming method: the list rides a byte stream to the caller, who `for await`s it —
 *  a bulk read that streams rather than returning the whole array at once. */
export async function* all(): AsyncGenerator<todos.Todo> {
  for (const todo of todos.list()) yield todo;
}

/** A callback argument: bulk-add, reporting progress back to the caller as each lands. */
export async function importMany(
  texts: string[],
  onProgress: (done: number) => void,
): Promise<number> {
  let done = 0;
  for (const text of texts) {
    todos.create(text);
    onProgress(++done);
  }
  return done;
}

/** A cast-friendly method (no return) — a fire-and-forget the caller never awaits. */
export function ping(): void {
  console.log("store: ping");
}

/** The published contract — derived from the functions above, so a typed client can never
 *  drift from the implementation. Callers import this *type* only (erased at build). */
export type Store = typeof import(".");
