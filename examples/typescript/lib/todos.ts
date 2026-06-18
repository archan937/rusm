// The todo data layer — the single source of truth for the todo model, shared by the
// `api` (which mutates it) and the `feed` (which reads the current list on connect). State
// lives in durable `kv`; a change is broadcast to the feed's subscribers over a
// process-group tag (the platform pub/sub primitive — no broker).
import { kv, Process } from "rusm-ts";

export interface Todo {
  id: number;
  text: string;
  done: boolean;
}

/** The process-group tag the feed streams subscribe to; `api` publishes changes to it. */
export const FEED_TAG = "todos";

const bucket = () => kv.bucket("todos");
const decode = (bytes: Uint8Array): Todo => JSON.parse(new TextDecoder().decode(bytes));

/** Every todo, by id ascending. */
export function list(): Todo[] {
  return bucket()
    .list()
    .map((id) => decode(bucket().get(id)!))
    .sort((a, b) => a.id - b.id);
}

export function get(id: number): Todo | null {
  const v = bucket().get(String(id));
  return v ? decode(v) : null;
}

export function save(todo: Todo): void {
  bucket().set(String(todo.id), JSON.stringify(todo));
}

export function remove(id: number): boolean {
  return bucket().delete(String(id));
}

/** The next free id (max + 1; 1 for an empty list). */
export function nextId(): number {
  return Math.max(0, ...bucket().list().map(Number)) + 1;
}

/** The current list as an SSE-ready JSON payload (what the feed emits). */
export function snapshot(): string {
  return JSON.stringify(list());
}

/** Push the current list to every open feed stream — `whereisTag` + `send`, the platform
 *  pub/sub over the {@link FEED_TAG} process group. Subscribers auto-release on exit. */
export function publish(): void {
  const payload = new TextEncoder().encode(snapshot());
  for (const pid of Process.whereisTag(FEED_TAG)) Process.send(pid, payload);
}

// ── High-level operations — the single source for both the `api` (in-process) and the
// `store` service. Each persists then publishes the new list to subscribers. ──

/** Add a todo and publish; returns the new todo. */
export function create(text: string): Todo {
  const todo: Todo = { id: nextId(), text, done: false };
  save(todo);
  publish();
  return todo;
}

/** Flip a todo's `done` and publish; `null` if it doesn't exist. */
export function setDone(id: number): Todo | null {
  const todo = get(id);
  if (!todo) return null;
  todo.done = !todo.done;
  save(todo);
  publish();
  return todo;
}

/** Delete a todo and publish; `false` if it didn't exist. */
export function del(id: number): boolean {
  const ok = remove(id);
  if (ok) publish();
  return ok;
}
