package rusm

import (
	"encoding/json"
	"errors"
	"iter"
)

// The typed client side of the wire. Call is a blocking request/reply; Cast is
// fire-and-forget. Both take variadic args serialised as the JSON array the service
// (Go, Rust, or TS) decodes positionally.

// wireOut is the on-wire request this guest sends.
type wireOut struct {
	Op     string  `json:"op"`
	Args   []any   `json:"args"`
	From   string  `json:"from"`
	Ref    *uint64 `json:"ref,omitempty"`
	Stream bool    `json:"stream,omitempty"`
}

// refCounter hands out per-call correlation ids (the guest is single-threaded).
var refCounter uint64

func nextRef() uint64 {
	refCounter++
	return refCounter
}

// argsOf normalises variadic args to a non-nil slice so they serialise as a JSON
// array ([]), never null — the form every service decodes.
func argsOf(args []any) []any {
	if args == nil {
		return []any{}
	}
	return args
}

// Call sends a blocking request to a service pid and returns the typed reply. While it
// waits it sets any unrelated mail aside, so the app's own Receive still sees it (in order).
func Call[R any](to Pid, op string, args ...any) (R, error) {
	var zero R
	marked, cbIDs := prepareArgs(args) // callback args → `{"__cb": id}` markers
	defer releaseCallbacks(cbIDs)
	ref := nextRef()
	req, err := json.Marshal(wireOut{Op: op, Args: argsOf(marked), From: Self().String(), Ref: &ref})
	if err != nil {
		return zero, err
	}
	SendBytes(to, req)
	// Hold non-matching mail in a local buffer for the duration of the call, then restore it to
	// the inbox front — never re-stashing mid-loop, since ReceiveBytes drains the inbox first and
	// a re-stashed message would be re-read forever while the real reply waits behind it.
	var setAside [][]byte
	defer func() { unstashFront(setAside) }()
	for {
		raw := ReceiveBytes()
		var env map[string]json.RawMessage
		if json.Unmarshal(raw, &env) != nil {
			setAside = append(setAside, raw) // not JSON we understand — leave it for the app
			continue
		}
		if dispatchCallback(env) {
			continue // the service invoked a callback; keep awaiting the reply
		}
		if !replyMatches(env, ref) {
			setAside = append(setAside, raw) // someone else's reply, request, or plain message
			continue
		}
		if e, ok := env["err"]; ok {
			var msg string
			_ = json.Unmarshal(e, &msg)
			return zero, errors.New(msg)
		}
		var result R
		if okVal, ok := env["ok"]; ok {
			if err := json.Unmarshal(okVal, &result); err != nil {
				return zero, err
			}
		}
		return result, nil
	}
}

// CallStream sends a streaming request and returns a sequence of typed items, yielded
// as the service produces them (one per chunk; end-of-stream ends the range). Range
// over it directly:
//
//	for ev := range rusm.CallStream[Event](pid, "events", since) {
//		// handle ev
//	}
//
// Reading suspends the fiber between items; breaking out of the range stops consuming.
func CallStream[R any](to Pid, op string, args ...any) iter.Seq[R] {
	req, err := json.Marshal(wireOut{Op: op, Args: argsOf(args), From: Self().String(), Stream: true})
	if err != nil {
		return func(func(R) bool) {} // empty sequence; nothing was sent
	}
	SendBytes(to, req)
	stream := AcceptStream()
	return func(yield func(R) bool) {
		for {
			chunk, ok := stream.Read()
			if !ok {
				return // end-of-stream
			}
			var v R
			if json.Unmarshal(chunk, &v) != nil {
				return // a malformed chunk ends the stream (matches rusm-rs)
			}
			if !yield(v) {
				return // consumer broke out
			}
		}
	}
}

// Cast sends a fire-and-forget request (no reply awaited).
func Cast(to Pid, op string, args ...any) error {
	b, err := json.Marshal(wireOut{Op: op, Args: argsOf(args), From: Self().String()})
	if err != nil {
		return err
	}
	SendBytes(to, b)
	return nil
}

// replyMatches reports whether env is the reply carrying our correlation ref. A reply carries
// `ok` or `err`; matching on `ref` alone is unsound — `ref` is a per-process counter, so a
// concurrent inbound *request* (which also carries a `ref`) can collide and be mis-read as the
// reply. So require the reply shape AND the ref.
func replyMatches(env map[string]json.RawMessage, ref uint64) bool {
	_, hasOk := env["ok"]
	_, hasErr := env["err"]
	if !hasOk && !hasErr {
		return false
	}
	r, ok := env["ref"]
	if !ok {
		return false
	}
	var got uint64
	return json.Unmarshal(r, &got) == nil && got == ref
}
