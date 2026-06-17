// Package web is the HTTP / Server-Sent-Events serving surface for RUSM Go components.
// Write handler functions that take a Request + Params and return a Response (or stream
// SSE), register them by action name, and Serve. The host runs the unified per-request
// model: it resolves the rusm.toml [serve.routes] table, spawns this component fresh per
// request, dispatches the matched action here, and turns the reply into the HTTP
// response — so a handler is just normal Go, a request in and a response out:
//
//	func run() {
//		h := web.NewHandlers()
//		h.Handle("hello", func(r web.Request, p web.Params) web.Response {
//			return web.Text("hi " + p.Get("name"))
//		})
//		h.HandleSSE("ticks", func(r web.Request, p web.Params, sse web.Sse) {
//			for n := 0; n < 3; n++ { sse.Data([]byte(fmt.Sprintf("tick %d", n))) }
//		})
//		h.Serve()
//	}
package web

import (
	"encoding/json"
	"strings"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

// Request is an incoming HTTP request. Body is decoded from the wire's base64 by
// encoding/json automatically (Go encodes []byte as base64).
type Request struct {
	Method  string      `json:"method"`
	URL     string      `json:"url"`
	Headers [][2]string `json:"headers,omitempty"`
	Body    []byte      `json:"body,omitempty"`
}

// Header returns the first value for name (case-insensitive), or "" if absent.
func (r Request) Header(name string) string {
	for _, h := range r.Headers {
		if strings.EqualFold(h[0], name) {
			return h[1]
		}
	}
	return ""
}

// Response is an HTTP response. Build one with Text, JSON, or Bytes and add headers
// with Header.
type Response struct {
	Status  int         `json:"status"`
	Headers [][2]string `json:"headers,omitempty"`
	Body    []byte      `json:"body,omitempty"`
	Stream  bool        `json:"stream,omitempty"`
}

// Text is a 200 text/plain response.
func Text(body string) Response {
	return Response{
		Status:  200,
		Headers: [][2]string{{"content-type", "text/plain; charset=utf-8"}},
		Body:    []byte(body),
	}
}

// JSON is a 200 application/json response encoding v (500 if v can't be marshalled).
func JSON(v any) Response {
	b, err := json.Marshal(v)
	if err != nil {
		return Bytes(500, []byte("json: "+err.Error()))
	}
	return Response{
		Status:  200,
		Headers: [][2]string{{"content-type", "application/json"}},
		Body:    b,
	}
}

// Bytes is a response with an explicit status and raw body (no default headers).
func Bytes(status int, body []byte) Response {
	return Response{Status: status, Body: body}
}

// Header adds a header, builder-style.
func (r Response) Header(name, value string) Response {
	r.Headers = append(r.Headers, [2]string{name, value})
	return r
}

// Params are the path parameters captured from the route (/users/:id → Get("id")).
type Params struct {
	pairs [][2]string
}

// Get returns the captured value for name, or "" if the route had no such parameter.
func (p Params) Get(name string) string {
	for _, kv := range p.pairs {
		if kv[0] == name {
			return kv[1]
		}
	}
	return ""
}

// Sse is a live Server-Sent Events stream handed to a streaming handler. Each request
// runs in its own process, so a handler may block here for the whole connection — write
// events as they occur, then return (Serve closes the stream afterwards).
type Sse struct {
	stream rusm.Stream
	open   bool
}

// Write sends a raw SSE frame (e.g. []byte("data: hi\n\n")); false once the client is gone.
func (s Sse) Write(frame []byte) bool {
	return s.open && s.stream.Write(frame)
}

// Data sends a `data: <payload>\n\n` event.
func (s Sse) Data(payload []byte) bool { return s.Write(DataFrame(payload)) }

// Run live-tails until the client disconnects: each inbound message goes to mapFn
// (return emit=true with a frame to send it, emit=false to skip); an idle heartbeatMs
// writes a heartbeat comment. Returns on disconnect — let the handler then end so the
// process exits and a monitoring broker prunes this subscriber.
func (s Sse) Run(heartbeatMs uint64, mapFn func(msg []byte) (frame []byte, emit bool)) {
	for {
		msg, ok := rusm.ReceiveBytesTimeout(heartbeatMs)
		if ok {
			if frame, emit := mapFn(msg); emit && !s.Write(frame) {
				return
			}
		} else if !s.Write([]byte(": ping\n\n")) {
			return
		}
	}
}

// DataFrame builds a `data: <payload>\n\n` SSE frame.
func DataFrame(payload []byte) []byte {
	frame := make([]byte, 0, len(payload)+len("data: \n\n"))
	frame = append(frame, "data: "...)
	frame = append(frame, payload...)
	return append(frame, '\n', '\n')
}
