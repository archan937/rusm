// Canonical source: bridges/serve/guest.go — the serve bridge's Go guest binding (the
// per-connection WS/SSE handler controls). Synced into rusm-go (packages/rusm-go/serve.go)
// by `make sync-bridges`; edit this file, not the copy. `bridge_guest_in_sync` guards drift.

package rusm

import (
	"strings"

	"go.bytecodealliance.org/cm"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/serve"
)

// ConnectionInfo is the HTTP context of a per-connection WebSocket or SSE handler — the
// request that opened the connection, fixed for its life. Read it in your handler's Open
// (via [web.Conn.Info] / [web.Stream.Info], or [Connection] directly). The zero value's
// accessors are all empty, so a non-connection process reads blanks rather than panicking.
type ConnectionInfo struct {
	method      string
	path        string
	query       string
	remoteAddr  string
	subprotocol string
	params      [][2]string
	headers     [][2]string
}

// Method is the request method, uppercased (GET, …).
func (c ConnectionInfo) Method() string { return c.method }

// Path is the request path without the query string (/events/plan/pages/42).
func (c ConnectionInfo) Path() string { return c.path }

// Query is the raw query string without the leading '?' (empty when absent).
func (c ConnectionInfo) Query() string { return c.query }

// Params are the route parameters captured from the listener's [serve.routes] pattern.
func (c ConnectionInfo) Params() [][2]string { return c.params }

// Param returns one captured route parameter by name (":plan" → Param("plan")), or "".
func (c ConnectionInfo) Param(name string) string { return find(c.params, name, false) }

// Headers are the request headers (lowercased names, arrival order; a name may repeat).
func (c ConnectionInfo) Headers() [][2]string { return c.headers }

// Header returns the first value of header name (case-insensitive), or "".
func (c ConnectionInfo) Header(name string) string { return find(c.headers, name, true) }

// RemoteAddr is the peer address (ip:port), or "" if the transport can't report one.
func (c ConnectionInfo) RemoteAddr() string { return c.remoteAddr }

// Subprotocol is the negotiated WebSocket subprotocol, or "" (always "" for SSE).
func (c ConnectionInfo) Subprotocol() string { return c.subprotocol }

func find(pairs [][2]string, name string, fold bool) string {
	for _, p := range pairs {
		if p[0] == name || (fold && strings.EqualFold(p[0], name)) {
			return p[1]
		}
	}
	return ""
}

// Connection returns this process's connection context when it is a per-connection
// WebSocket/SSE handler; ok is false for every other process. A handler usually reads it
// through [web.Conn.Info] / [web.Stream.Info] rather than calling this directly.
func Connection() (info ConnectionInfo, ok bool) {
	o := serve.Connection()
	if o.None() {
		return ConnectionInfo{}, false
	}
	c := o.Some()
	info = ConnectionInfo{
		method:     c.Method,
		path:       c.Path,
		query:      c.Query,
		remoteAddr: c.RemoteAddr,
		params:     c.Params.Slice(),
		headers:    c.Headers.Slice(),
	}
	if !c.Subprotocol.None() {
		info.subprotocol = c.Subprotocol.Value()
	}
	return info, true
}

// WsSendText sends a text WebSocket frame on this connection (binary frames are a plain
// SendBytes to the writer pid). Returns false if this is not a WebSocket handler or the
// socket has closed. Used by web.Conn.SendText.
func WsSendText(payload []byte) bool { return serve.WsSendText(cm.ToList(payload)) }

// WsClose closes this WebSocket connection with a status code + reason (used by
// web.Conn.Close). No-op for a non-WebSocket process.
func WsClose(code uint16, reason string) { serve.WsClose(code, reason) }

// SseSend emits a rich SSE event (used by web.Stream.Emit): data plus an event name, id,
// and retry — each omitted when "" / 0. Returns false if this is not an SSE handler or the
// client disconnected.
func SseSend(data []byte, event, id string, retry uint32) bool {
	return serve.SseSend(cm.ToList(data), optString(event), optString(id), optU32(retry))
}
