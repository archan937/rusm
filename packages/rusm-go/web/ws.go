package web

import rusm "github.com/archan937/rusm/packages/rusm-go"

// Conn is one WebSocket connection — the per-connection process's view of its socket.
// The host runs one process per connection, so a handler's state is just that one
// connection's. Reply with Send; the host's writer process owns the actual socket sink.
type Conn struct {
	writer rusm.Pid
	info   rusm.ConnectionInfo
}

// Writer returns the connection's writer pid (the reply target).
func (c Conn) Writer() rusm.Pid { return c.writer }

// Info returns this connection's request context — method, path, query, route params,
// headers, peer address, and negotiated subprotocol (e.g. c.Info().Param("room")).
func (c Conn) Info() rusm.ConnectionInfo { return c.info }

// Send writes one frame back to the client (dropped if the socket has closed).
func (c Conn) Send(frame []byte) { rusm.SendBytes(c.writer, frame) }

// WebSocket is a per-connection WebSocket handler: Open fires once when the connection
// opens (optional), Message once per inbound frame, and Close once when it closes
// (optional). The host runs one process per connection, so keep per-connection state in
// the closures — shared state belongs in a resident [components.<name>] service or kv.
// Run it as the component body:
//
//	func run() {
//		web.WebSocket{
//			Open:    func(c web.Conn) { c.Send([]byte("welcome")) },
//			Message: func(c web.Conn, data []byte) { c.Send(data) }, // echo
//			Close:   func(c web.Conn) { /* disconnect — clean or dropped */ },
//		}.Serve()
//	}
type WebSocket struct {
	Open    func(conn Conn)
	Message func(conn Conn, data []byte)
	// Close fires once when the connection closes — the client disconnected, cleanly or
	// by a dropped socket — before the process exits. Optional.
	Close func(conn Conn)
}

// Serve runs this connection: learn the writer pid (the host's message 1), fire Open,
// dispatch each inbound frame to Message, and fire Close when the socket closes — then
// return. The writer process owns the socket, so its death IS the disconnect (clean or
// dropped); monitoring it turns that into the Close callback.
func (ws WebSocket) Serve() {
	writer, _ := rusm.ParsePid(rusm.ReceiveString()) // message 1: the writer pid
	info, _ := rusm.Connection()
	conn := Conn{writer: writer, info: info}
	rusm.Monitor(writer)
	if ws.Open != nil {
		ws.Open(conn)
	}
	for {
		frame := rusm.ReceiveBytes()
		if dead, ok := rusm.DownPid(frame); ok && dead == writer {
			if ws.Close != nil {
				ws.Close(conn)
			}
			return
		}
		if ws.Message != nil {
			ws.Message(conn, frame)
		}
	}
}
