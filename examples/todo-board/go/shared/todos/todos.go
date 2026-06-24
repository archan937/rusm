// Package todos is the todo data layer — the single source of truth for the todo model,
// shared by the api (which mutates it), the feed (which reads it), and the store service.
// State lives in durable kv; a change is broadcast to the feed's subscribers over a
// process-group tag (the platform pub/sub primitive — no broker). The Go twin of the TS
// example's lib/todos.ts and the RS example's todos crate.
package todos

import (
	"encoding/json"
	"sort"
	"strconv"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

// FeedTag is the process-group tag feed streams subscribe to; the api and store publish
// changes to it.
const FeedTag = "todos"

// Todo is one item. The json tags pin the wire shape shared across all three guests and
// read by the web page (id/text/done).
type Todo struct {
	ID   uint64 `json:"id"`
	Text string `json:"text"`
	Done bool   `json:"done"`
}

func bucket() rusm.Bucket { return rusm.OpenBucket("todos") }

// List returns every todo, by id ascending.
func List() []Todo {
	b := bucket()
	keys, err := b.List()
	if err != nil {
		return nil
	}
	list := make([]Todo, 0, len(keys))
	for _, k := range keys {
		if v, ok, err := b.Get(k); err == nil && ok {
			var t Todo
			if json.Unmarshal(v, &t) == nil {
				list = append(list, t)
			}
		}
	}
	sort.Slice(list, func(i, j int) bool { return list[i].ID < list[j].ID })
	return list
}

// Get returns one todo by id.
func Get(id uint64) (Todo, bool) {
	v, ok, err := bucket().Get(strconv.FormatUint(id, 10))
	if err != nil || !ok {
		return Todo{}, false
	}
	var t Todo
	if json.Unmarshal(v, &t) != nil {
		return Todo{}, false
	}
	return t, true
}

func save(t Todo) {
	if b, err := json.Marshal(t); err == nil {
		_ = bucket().Set(strconv.FormatUint(t.ID, 10), b)
	}
}

func remove(id uint64) bool {
	deleted, _ := bucket().Delete(strconv.FormatUint(id, 10))
	return deleted
}

// nextID is max(id)+1 (1 for an empty list).
func nextID() uint64 {
	keys, _ := bucket().List()
	var highest uint64
	for _, k := range keys {
		if n, err := strconv.ParseUint(k, 10, 64); err == nil && n > highest {
			highest = n
		}
	}
	return highest + 1
}

// Snapshot is the current list as an SSE-ready JSON payload (what the feed emits).
func Snapshot() []byte {
	b, _ := json.Marshal(List())
	return b
}

// publish pushes the current list to every open feed stream — WhereisTag + SendBytes over
// the FeedTag process group (the platform pub/sub; subscribers auto-release on exit).
func publish() {
	payload := Snapshot()
	for _, pid := range rusm.WhereisTag(FeedTag) {
		rusm.SendBytes(pid, payload)
	}
}

// Create adds a todo and publishes; returns the new todo.
func Create(text string) Todo {
	t := Todo{ID: nextID(), Text: text, Done: false}
	save(t)
	publish()
	return t
}

// SetDone flips a todo's done and publishes; nil if it doesn't exist.
func SetDone(id uint64) *Todo {
	t, ok := Get(id)
	if !ok {
		return nil
	}
	t.Done = !t.Done
	save(t)
	publish()
	return &t
}

// Delete removes a todo and publishes; false if it didn't exist.
func Delete(id uint64) bool {
	removed := remove(id)
	if removed {
		publish()
	}
	return removed
}
