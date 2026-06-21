// Package rusm is the ergonomic Go guest API for RUSM — write a WebAssembly
// component (or a service) in plain Go over the rusm:runtime actor world, the Go
// twin of rusm-ts and rusm-rs. It wraps the wit-bindgen-go actor bindings in a
// small, idiomatic surface: Pid, Send/Receive (raw or JSON), Spawn, the registry,
// process-group tags, Stream, KV, and a Supervisor — all callable as normal Go.
//
// Blocking "just works": Receive (and Stream.Read) suspend the instance on the host
// fiber until data arrives — the guest writes straight-line code and the host makes
// it async, exactly like an Erlang receive.
//
// A component is three lines of shell — register the entry in init, an empty main:
//
//	func init() { rusm.Run(run) }
//	func main()  {}
//	func run()   { /* the process body — normal Go */ }
//
// The message wire (JSON) is shared with rusm-ts and rusm-rs, so guests of any
// language interoperate. The capabilities are split by concern across files: the
// Process core is actor.go (the actor bridge), with kv.go / pg.go / serve.go /
// stream.go / log.go for the other bridges; this file holds the package entry (Run)
// and the small helpers those bridges share.
package rusm

import (
	"go.bytecodealliance.org/cm"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/actor"
	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/process"
)

// Run registers entry as this component's start function: the host calls it once, on
// its own fiber, to run the process. Call it from init (see the package doc). It also
// routes the standard log and log/slog packages to the platform logger.
func Run(entry func()) {
	process.Exports.Run = func() {
		initLogging()
		entry()
	}
}

// optString / optU32 turn a Go zero value into a wit `none` — shared by the serve bridge
// (serve.go) for the optional SSE event fields.
func optString(s string) cm.Option[string] {
	if s == "" {
		return cm.None[string]()
	}
	return cm.Some(s)
}

func optU32(n uint32) cm.Option[uint32] {
	if n == 0 {
		return cm.None[uint32]()
	}
	return cm.Some(n)
}

// pids converts a wit list of actor pids into a Go slice of Pid — shared by actor.go's
// List and pg.go's WhereisTag.
func pids(l cm.List[actor.Pid]) []Pid {
	src := l.Slice()
	out := make([]Pid, len(src))
	for i, p := range src {
		out[i] = Pid(p)
	}
	return out
}
