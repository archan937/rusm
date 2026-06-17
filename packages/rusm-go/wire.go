package rusm

import (
	"encoding/json"
	"fmt"
)

// The RPC wire — the same JSON protocol rusm-ts and rusm-rs use, so a Go client and a
// Rust/TS service (or vice versa) interoperate: requests {op, args, from, ref} (with
// "stream": true for a streaming call) → replies {ref, ok} | {ref, err}. The Service
// dispatch loop and the typed Call client below speak it.

// Request is one decoded service request handed to a Handler. Args are the positional
// JSON arguments — decode one with Arg[T], or let an FnN adapter decode them for you.
type Request struct {
	Op     string
	Args   []json.RawMessage
	From   Pid
	ref    uint64
	hasRef bool
	stream bool
}

// rawRequest is the on-wire shape; From is a decimal pid string, ref/stream optional.
type rawRequest struct {
	Op     string            `json:"op"`
	Args   []json.RawMessage `json:"args"`
	From   string            `json:"from"`
	Ref    *uint64           `json:"ref"`
	Stream bool              `json:"stream"`
}

// replyOK / replyERR are the reply shapes (struct fields keep the key order rusm-rs
// emits; the receiver parses by name, so order is cosmetic but kept identical).
type replyOK struct {
	Ref uint64 `json:"ref"`
	OK  any    `json:"ok"`
}

type replyERR struct {
	Ref uint64 `json:"ref"`
	Err string `json:"err"`
}

// decodeRequest parses a message as a service request; ok is false for any message
// that is not one (a reply, a plain app message, malformed JSON) so the loop skips it.
func decodeRequest(raw []byte) (Request, bool) {
	var r rawRequest
	if err := json.Unmarshal(raw, &r); err != nil || r.Op == "" {
		return Request{}, false
	}
	req := Request{Op: r.Op, Args: r.Args, stream: r.Stream}
	if r.Ref != nil {
		req.ref, req.hasRef = *r.Ref, true
	}
	if p, ok := ParsePid(r.From); ok {
		req.From = p
	}
	return req, true
}

// nextRequest blocks for the next service request, skipping any non-request message.
func nextRequest() Request {
	for {
		if req, ok := decodeRequest(ReceiveBytes()); ok {
			return req
		}
	}
}

// Arg decodes the positional argument at index i of a request as type T.
func Arg[T any](req Request, i int) (T, error) {
	var v T
	if i >= len(req.Args) {
		return v, fmt.Errorf("missing argument %d", i)
	}
	err := json.Unmarshal(req.Args[i], &v)
	return v, err
}

// replyOk answers a call with a value (a no-op for a cast — no ref / no caller).
func replyOk(req Request, value any) {
	if !req.hasRef || req.From == 0 {
		return
	}
	if b, err := json.Marshal(replyOK{Ref: req.ref, OK: value}); err == nil {
		SendBytes(req.From, b)
	} else {
		replyErr(req, err.Error())
	}
}

// replyErr answers a call with an error message.
func replyErr(req Request, message string) {
	if !req.hasRef || req.From == 0 {
		return
	}
	if b, err := json.Marshal(replyERR{Ref: req.ref, Err: message}); err == nil {
		SendBytes(req.From, b)
	}
}
