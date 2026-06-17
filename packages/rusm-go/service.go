package rusm

import "encoding/json"

// A service is a set of named handlers with a dispatch loop — the Go shape of a
// rusm-ts service (exported functions) or a rusm-rs #[service]. Register typed
// handlers with the FnN adapters and run Serve as the process body:
//
//	func run() {
//		svc := rusm.NewService()
//		svc.Handle("add",   rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
//		svc.Handle("greet", rusm.Fn1(func(name string) (string, error) { return "hi " + name, nil }))
//		svc.Serve()
//	}
//
// A typed client reaches it with Call[R](pid, op, args...) — the same JSON wire, so
// Rust and TS guests interoperate.

// Handler processes one request and returns the JSON-encodable reply, or an error
// (sent back to the caller as {err}). Most handlers are built with the FnN adapters.
type Handler func(Request) (any, error)

// StreamHandler streams a sequence of items back to the caller through the Sink (one
// JSON value per chunk, back-pressured). It runs for a streaming call — the client side
// is CallStream[R]. Register it with HandleStream.
type StreamHandler func(Request, Sink) error

// Sink is the writable end of a streaming reply handed to a StreamHandler.
type Sink struct {
	stream Stream
}

// Send writes one item to the stream; false once the reader is gone (stop producing).
func (k Sink) Send(v any) bool {
	b, err := json.Marshal(v)
	if err != nil {
		return false
	}
	return k.stream.Write(b)
}

// Service maps operation names to handlers (plain calls/casts and streaming calls).
type Service struct {
	handlers       map[string]Handler
	streamHandlers map[string]StreamHandler
}

// NewService creates an empty service.
func NewService() *Service {
	return &Service{
		handlers:       make(map[string]Handler),
		streamHandlers: make(map[string]StreamHandler),
	}
}

// Handle registers a call/cast handler for op (last registration wins).
func (s *Service) Handle(op string, h Handler) { s.handlers[op] = h }

// HandleStream registers a streaming handler for op (invoked by CallStream).
func (s *Service) HandleStream(op string, h StreamHandler) { s.streamHandlers[op] = h }

// Serve runs the request → dispatch → reply loop forever (a service's body). An
// unknown op or a handler error is reported to the caller; the loop keeps running.
func (s *Service) Serve() {
	for {
		req := nextRequest()
		if req.stream {
			s.serveStream(req)
			continue
		}
		h, ok := s.handlers[req.Op]
		if !ok {
			replyErr(req, "no such function: "+req.Op)
			continue
		}
		result, err := h(req)
		if err != nil {
			replyErr(req, err.Error())
			continue
		}
		replyOk(req, result)
	}
}

// serveStream answers a streaming call by opening a stream back to the caller, running
// the handler against it, and closing it. An unknown op yields an empty stream (open +
// close), so the client's CallStream range simply ends rather than blocking forever.
func (s *Service) serveStream(req Request) {
	stream, opened := OpenStream(req.From)
	if !opened {
		return
	}
	if h, ok := s.streamHandlers[req.Op]; ok {
		_ = h(req, Sink{stream: stream})
	}
	stream.Close()
}

// FnN adapt a typed function into a Handler, decoding the positional wire args into the
// function's parameters with no reflection (TinyGo has no reflect.Value.Call). Arities
// 0–3 cover the common cases; for more, pass a single struct argument or write a
// Handler that uses Arg[T] directly.

// Fn0 adapts a zero-argument function.
func Fn0[R any](fn func() (R, error)) Handler {
	return func(Request) (any, error) { return fn() }
}

// Fn1 adapts a one-argument function.
func Fn1[A, R any](fn func(A) (R, error)) Handler {
	return func(req Request) (any, error) {
		a, err := Arg[A](req, 0)
		if err != nil {
			return nil, err
		}
		return fn(a)
	}
}

// Fn2 adapts a two-argument function.
func Fn2[A, B, R any](fn func(A, B) (R, error)) Handler {
	return func(req Request) (any, error) {
		a, err := Arg[A](req, 0)
		if err != nil {
			return nil, err
		}
		b, err := Arg[B](req, 1)
		if err != nil {
			return nil, err
		}
		return fn(a, b)
	}
}

// Fn3 adapts a three-argument function.
func Fn3[A, B, C, R any](fn func(A, B, C) (R, error)) Handler {
	return func(req Request) (any, error) {
		a, err := Arg[A](req, 0)
		if err != nil {
			return nil, err
		}
		b, err := Arg[B](req, 1)
		if err != nil {
			return nil, err
		}
		c, err := Arg[C](req, 2)
		if err != nil {
			return nil, err
		}
		return fn(a, b, c)
	}
}
