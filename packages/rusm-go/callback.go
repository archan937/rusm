package rusm

import "encoding/json"

// Callbacks let a caller pass a function into a Call: the closure stays in the caller,
// and the service's invocations of it travel back as `{op:"__cb", cbref, args}` messages
// that Call dispatches while awaiting the reply — the same wire rusm-rs / rusm-ts use, so
// a Go client can drive a Rust or TS service's callback method (and vice versa).
//
// Pass one as a call argument with the generic CB adapter (the service's invocation is
// decoded into A):
//
//	total, _ := rusm.Call[int](pid, "process", items, rusm.CB(func(p Progress) {
//		slog.Info("progress", "done", p.Done)
//	}))
//
// Go has no macros, so a callback is the one concrete type Call detects —
// func(json.RawMessage) — which CB produces from any typed func.

// CB adapts a typed callback into the wire callback type: each service invocation is
// JSON-decoded into A and passed to f. Pass it as a callback argument to Call, whose
// reply loop dispatches the invocations (Cast is fire-and-forget and can't).
func CB[A any](f func(A)) func(json.RawMessage) {
	return func(raw json.RawMessage) {
		var a A
		if json.Unmarshal(raw, &a) == nil {
			f(a)
		}
	}
}

// callbacks holds the caller-side closures awaiting invocation, by id (the guest is
// single-threaded — one mailbox — so a plain map needs no lock, like the stash).
var callbacks = make(map[uint64]func(json.RawMessage))

// prepareArgs replaces each callback argument with a `{"__cb": id}` marker and registers
// its closure, returning the wire args and the ids to release once the call ends.
func prepareArgs(args []any) (marked []any, ids []uint64) {
	marked = make([]any, len(args))
	for i, a := range args {
		if cb, ok := a.(func(json.RawMessage)); ok {
			id := nextRef()
			callbacks[id] = cb
			marked[i] = map[string]uint64{"__cb": id}
			ids = append(ids, id)
		} else {
			marked[i] = a
		}
	}
	return marked, ids
}

// releaseCallbacks drops registered callbacks once their call has returned.
func releaseCallbacks(ids []uint64) {
	for _, id := range ids {
		delete(callbacks, id)
	}
}

// dispatchCallback handles a `{op:"__cb", cbref, args}` message by invoking the
// registered closure; it reports whether the message was a callback (so the caller's
// receive loop skips it).
func dispatchCallback(env map[string]json.RawMessage) bool {
	op, ok := env["op"]
	if !ok {
		return false
	}
	var name string
	if json.Unmarshal(op, &name) != nil || name != "__cb" {
		return false
	}
	var cbref uint64
	if r, ok := env["cbref"]; ok {
		_ = json.Unmarshal(r, &cbref)
	}
	if cb, ok := callbacks[cbref]; ok {
		arg := json.RawMessage("null")
		if a, ok := env["args"]; ok {
			var list []json.RawMessage
			if json.Unmarshal(a, &list) == nil && len(list) > 0 {
				arg = list[0]
			}
		}
		cb(arg)
	}
	return true
}

// Callback is the service side of a caller-supplied callback: invoking it sends the
// argument back to the caller, where the matching closure runs.
type Callback struct {
	to    Pid
	cbref uint64
}

// Call invokes the caller's callback with arg.
func (c Callback) Call(arg any) {
	msg, err := json.Marshal(cbMessage{Op: "__cb", Cbref: c.cbref, Args: []any{arg}})
	if err == nil {
		SendBytes(c.to, msg)
	}
}

type cbMessage struct {
	Op    string `json:"op"`
	Cbref uint64 `json:"cbref"`
	Args  []any  `json:"args"`
}

// CallbackArg reconstructs the Callback at positional argument index i of a request
// (from its `{"__cb": id}` marker), targeting the caller.
func CallbackArg(req Request, i int) Callback {
	var marker struct {
		Cb uint64 `json:"__cb"`
	}
	if i < len(req.Args) {
		_ = json.Unmarshal(req.Args[i], &marker)
	}
	return Callback{to: req.From, cbref: marker.Cb}
}
