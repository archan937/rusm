package web

import rusm "github.com/archan937/rusm/packages/rusm-go"

// Stream is one SSE connection — the per-connection process's view of its stream, the
// SSE twin of Conn. Emit events with Data; the host's writer process owns the response
// body (it frames each payload as a `data:` event and pings on idle). SSE is one-way
// (server → client), so there are no inbound frames — events reach a handler through its
// mailbox (typically a process-group tag it subscribes to in Sse.Open).
type Stream struct {
	writer rusm.Pid
	done   *bool
	info   rusm.ConnectionInfo
}

// Writer returns the connection's writer pid (the emit target).
func (s Stream) Writer() rusm.Pid { return s.writer }

// Info returns this stream's request context — method, path, query, route params,
// headers, and peer address (e.g. s.Info().Param("plan") or the "last-event-id" header).
func (s Stream) Info() rusm.ConnectionInfo { return s.info }

// Data emits one event to the client. The platform frames it as a `data:` SSE event;
// dropped if the client has disconnected.
func (s Stream) Data(payload []byte) { rusm.SendBytes(s.writer, payload) }

// Emit sends a rich SSE event — Data plus an optional event Name, ID (echoed by the client
// as Last-Event-ID on reconnect), and Retry backoff (each omitted when "" / 0). Returns
// false if the client has disconnected. (Data is the data:-only shortcut.)
func (s Stream) Emit(e Event) bool { return rusm.SseSend(e.Data, e.Name, e.ID, e.Retry) }

// Event is a rich SSE event for Stream.Emit. ID is echoed by the client as Last-Event-ID;
// Name is the SSE event type; Retry is the reconnect backoff in ms. Empty ID/Name and a
// zero Retry are omitted.
type Event struct {
	Data  []byte
	ID    string
	Name  string
	Retry uint32
}

// Close ends the stream and this process (a server-initiated close). Sse.Close then
// fires once — the same teardown as a client disconnect.
func (s Stream) Close() { *s.done = true }

// Sse is a per-connection SSE handler — the SSE twin of WebSocket. Open fires once when
// the stream opens (subscribe to your event source here, e.g. rusm.RegisterTag), Message
// once per pushed event (emit it with stream.Data), and Close once on disconnect or a
// Stream.Close. The host runs one process per connection, so keep per-connection state in
// the closures. Run it as the component body:
//
//	func run() {
//		web.Sse{
//			Open:    func(s web.Stream) { rusm.RegisterTag("todos") },     // subscribe
//			Message: func(s web.Stream, ev []byte) { s.Data(ev) },         // a published event → emit
//			Close:   func(s web.Stream) {},                                // disconnect — clean or dropped
//		}.Serve()
//	}
type Sse struct {
	Open    func(stream Stream)
	Message func(stream Stream, event []byte)
	Close   func(stream Stream)
}

// Serve runs this connection: learn the writer pid (the host's message 1), fire Open,
// dispatch each pushed event to Message, and fire Close when the stream ends (a client
// disconnect or a Stream.Close) — then return. The writer owns the body, so its death IS
// the disconnect; monitoring it turns that into the Close callback.
func (sse Sse) Serve() {
	writer, _ := rusm.ParsePid(rusm.ReceiveString()) // message 1: the writer pid
	done := false
	info, _ := rusm.Connection()
	stream := Stream{writer: writer, done: &done, info: info}
	rusm.Monitor(writer)
	if sse.Open != nil {
		sse.Open(stream)
	}
	for !done {
		event := rusm.ReceiveBytes()
		if dead, ok := rusm.DownPid(event); ok {
			if dead != writer {
				continue // a __down for another monitored pid, not an event
			}
			break // client disconnected
		}
		if sse.Message != nil {
			sse.Message(stream, event)
		}
	}
	if sse.Close != nil {
		sse.Close(stream)
	}
}
