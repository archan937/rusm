package web

import (
	"encoding/json"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

// Handlers maps action names to buffered handler functions. The action is the `#action`
// half of a `component#action` route. (Streaming is per-connection now: see [Sse].)
type Handlers struct {
	buffered map[string]func(Request, Params) Response
}

// NewHandlers creates an empty handler set.
func NewHandlers() *Handlers {
	return &Handlers{
		buffered: make(map[string]func(Request, Params) Response),
	}
}

// Handle registers a buffered handler for action.
func (h *Handlers) Handle(action string, fn func(Request, Params) Response) {
	h.buffered[action] = fn
}

// incoming is the host's fetch envelope ({op,ref,from,action,params,request}); op is
// ignored (unknown fields are dropped).
type incoming struct {
	Action  string      `json:"action"`
	Params  [][2]string `json:"params"`
	From    string      `json:"from"`
	Ref     uint64      `json:"ref"`
	Request Request     `json:"request"`
}

// headReply is the reply envelope {ref, ok: response} the host's responder reads.
type headReply struct {
	Ref uint64   `json:"ref"`
	OK  Response `json:"ok"`
}

// Serve dispatches the single request the host sent to the matching handler and replies,
// then returns — process-per-request, so the instance exits after one request. Call it
// as the component body, after registering handlers.
func (h *Handlers) Serve() {
	var inc incoming
	if json.Unmarshal(rusm.ReceiveBytes(), &inc) != nil {
		return
	}
	to, ok := rusm.ParsePid(inc.From)
	if !ok {
		return // no reply target — nothing to answer
	}
	params := Params{pairs: inc.Params}

	if fn, ok := h.buffered[inc.Action]; ok {
		reply(to, inc.Ref, fn(inc.Request, params))
		return
	}
	reply(to, inc.Ref, Bytes(404, []byte("no such action")))
}

func reply(to rusm.Pid, ref uint64, resp Response) {
	if b, err := json.Marshal(headReply{Ref: ref, OK: resp}); err == nil {
		rusm.SendBytes(to, b)
	}
}
