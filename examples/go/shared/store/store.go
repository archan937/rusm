// Package store is the shared `store` service contract: one source for both the service
// dispatch (Serve, run by the store component) and the typed Client (used by the
// reporter), so the two can never drift — the Go counterpart of the RS example's store-svc
// crate and the TS example's components/store + its derived Store type. It composes the
// shared todos data layer over kv and publishes each change to the feed — the same todos
// the api serves and the feed streams. This is the composition half of the example.
package store

import (
	"iter"
	"log/slog"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"todoboard/todos"
)

// Operation names — the single source shared by Serve (registration) and Client (calls),
// so a typo can't split the two halves.
const (
	opList   = "list"
	opAdd    = "add"
	opToggle = "toggle"
	opRemove = "remove"
	opAll    = "all"
	opImport = "import"
	opPing   = "ping"
)

// Serve runs the receive→dispatch→reply loop around the todo operations (the store
// component's body). Never returns.
func Serve() {
	svc := rusm.NewService()

	svc.Handle(opList, rusm.Fn0(func() ([]todos.Todo, error) { return todos.List(), nil }))
	svc.Handle(opAdd, rusm.Fn1(func(text string) (todos.Todo, error) { return todos.Create(text), nil }))
	svc.Handle(opToggle, rusm.Fn1(func(id uint64) (*todos.Todo, error) { return todos.SetDone(id), nil }))
	svc.Handle(opRemove, rusm.Fn1(func(id uint64) (bool, error) { return todos.Delete(id), nil }))

	// streaming: each todo rides one chunk to the caller — a bulk read that streams rather
	// than returning the whole slice at once.
	svc.HandleStream(opAll, func(_ rusm.Request, out rusm.Sink) error {
		for _, t := range todos.List() {
			if !out.Send(t) {
				break // reader gone
			}
		}
		return nil
	})

	// callback: bulk-add, reporting progress back to the caller as each todo lands.
	svc.Handle(opImport, func(req rusm.Request) (any, error) {
		texts, err := rusm.Arg[[]string](req, 0)
		if err != nil {
			return nil, err
		}
		onProgress := rusm.CallbackArg(req, 1)
		done := 0
		for _, text := range texts {
			todos.Create(text)
			done++
			onProgress.Call(done)
		}
		return done, nil
	})

	// cast-friendly: a fire-and-forget the caller never awaits (no return value).
	svc.Handle(opPing, rusm.Fn0(func() (any, error) {
		slog.Info("store: ping")
		return nil, nil
	}))

	svc.Serve()
}

// Client is a typed client over the store service wire.
type Client struct{ Pid rusm.Pid }

// Spawn starts a fresh store service and connects to it.
func Spawn() (Client, error) {
	pid, err := rusm.Spawn("store")
	return Client{Pid: pid}, err
}

// List is a request/reply: the current list.
func (c Client) List() ([]todos.Todo, error) { return rusm.Call[[]todos.Todo](c.Pid, opList) }

// Add appends a todo (persists + publishes), returning the new one.
func (c Client) Add(text string) (todos.Todo, error) {
	return rusm.Call[todos.Todo](c.Pid, opAdd, text)
}

// Toggle flips a todo's done; nil if it doesn't exist.
func (c Client) Toggle(id uint64) (*todos.Todo, error) {
	return rusm.Call[*todos.Todo](c.Pid, opToggle, id)
}

// Remove deletes a todo; false if it didn't exist.
func (c Client) Remove(id uint64) (bool, error) { return rusm.Call[bool](c.Pid, opRemove, id) }

// All streams the list — range over it (each todo arrives as one chunk).
func (c Client) All() iter.Seq[todos.Todo] { return rusm.CallStream[todos.Todo](c.Pid, opAll) }

// ImportMany bulk-adds, reporting progress back as each todo lands.
func (c Client) ImportMany(texts []string, onProgress func(int)) (int, error) {
	return rusm.Call[int](c.Pid, opImport, texts, rusm.CB(onProgress))
}

// Ping is a fire-and-forget cast (no reply awaited).
func (c Client) Ping() error { return rusm.Cast(c.Pid, opPing) }
