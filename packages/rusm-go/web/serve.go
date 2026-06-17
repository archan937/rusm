package web

import (
	"encoding/json"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

// Handlers maps action names to handler functions — buffered (Handle) or streaming
// SSE (HandleSSE). The action is the `#action` half of a `component#action` route.
type Handlers struct {
	buffered map[string]func(Request, Params) Response
	streamed map[string]func(Request, Params, Sse)
}

// NewHandlers creates an empty handler set.
func NewHandlers() *Handlers {
	return &Handlers{
		buffered: make(map[string]func(Request, Params) Response),
		streamed: make(map[string]func(Request, Params, Sse)),
	}
}

// Handle registers a buffered handler for action.
func (h *Handlers) Handle(action string, fn func(Request, Params) Response) {
	h.buffered[action] = fn
}

// HandleSSE registers a streaming (Server-Sent Events) handler for action.
func (h *Handlers) HandleSSE(action string, fn func(Request, Params, Sse)) {
	h.streamed[action] = fn
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
// as the component body, after registering handlers. A streaming handler replies a
// text/event-stream head, then pumps events over a byte stream the host drains into the
// chunked HTTP body.
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
	if fn, ok := h.streamed[inc.Action]; ok {
		head := Response{
			Status:  200,
			Headers: [][2]string{{"content-type", "text/event-stream"}},
			Stream:  true,
		}
		reply(to, inc.Ref, head)
		stream, opened := rusm.OpenStream(to)
		fn(inc.Request, params, Sse{stream: stream, open: opened})
		if opened {
			stream.Close()
		}
		return
	}
	reply(to, inc.Ref, Bytes(404, []byte("no such action")))
}

func reply(to rusm.Pid, ref uint64, resp Response) {
	if b, err := json.Marshal(headReply{Ref: ref, OK: resp}); err == nil {
		rusm.SendBytes(to, b)
	}
}
