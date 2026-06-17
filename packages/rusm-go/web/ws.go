package web

import rusm "github.com/archan937/rusm/packages/rusm-go"

// Conn is one WebSocket connection — the per-connection process's view of its socket.
// The host runs one process per connection, so a handler's state is just that one
// connection's. Reply with Send; the host's writer process owns the actual socket sink.
type Conn struct {
	writer rusm.Pid
}

// Writer returns the connection's writer pid (the reply target).
func (c Conn) Writer() rusm.Pid { return c.writer }

// Send writes one frame back to the client (dropped if the socket has closed).
func (c Conn) Send(frame []byte) { rusm.SendBytes(c.writer, frame) }

// WebSocket is a per-connection WebSocket handler: Open fires once when the connection
// opens (optional), Message once per inbound frame. The host runs one process per
// connection, so keep per-connection state in the closures — shared state belongs in a
// resident [components.<name>] service or kv. Run it as the component body:
//
//	func run() {
//		web.WebSocket{
//			Open:    func(c web.Conn) { c.Send([]byte("welcome")) },
//			Message: func(c web.Conn, data []byte) { c.Send(data) }, // echo
//		}.Serve()
//	}
type WebSocket struct {
	Open    func(conn Conn)
	Message func(conn Conn, data []byte)
}

// Serve runs this connection: learn the writer pid (the host's message 1), fire Open,
// then dispatch each inbound frame to Message. It never returns — the host kills the
// process when the socket closes (exit cascades clean up; no close callback needed).
func (ws WebSocket) Serve() {
	writer, _ := rusm.ParsePid(rusm.ReceiveString()) // message 1: the writer pid
	conn := Conn{writer: writer}
	if ws.Open != nil {
		ws.Open(conn)
	}
	for {
		frame := rusm.ReceiveBytes()
		if ws.Message != nil {
			ws.Message(conn, frame)
		}
	}
}
