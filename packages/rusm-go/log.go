package rusm

import (
	"context"
	"log"
	"log/slog"
	"strings"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/actor"
)

// Logging is a platform primitive: a Go guest logs the normal way — the standard
// `log` package and `log/slog` — and the SDK routes it to the host log op. The host
// stamps the timestamp, this process's component name + pid, and the severity colour,
// and the node's [log] level gates it; the guest supplies only severity + message, so
// a developer never wires name, pid, or format (mirroring console.* in TS and the log
// crate in Rust). initLogging is installed by Run before the process body starts.
func initLogging() {
	slog.SetDefault(slog.New(&hostHandler{}))
	log.SetOutput(logWriter{})
	log.SetFlags(0) // the host owns the timestamp/prefix
}

// hostHandler is an slog.Handler that forwards each record to the platform logger.
// Level gating lives on the host (the node's [log] level), so Enabled is always true.
type hostHandler struct {
	prefix string      // accumulated group path, e.g. "http."
	attrs  []slog.Attr // attrs bound via WithAttrs, already group-qualified
}

func (h *hostHandler) Enabled(context.Context, slog.Level) bool { return true }

func (h *hostHandler) Handle(_ context.Context, r slog.Record) error {
	var b strings.Builder
	b.WriteString(r.Message)
	for _, a := range h.attrs {
		writeAttr(&b, "", a)
	}
	r.Attrs(func(a slog.Attr) bool {
		writeAttr(&b, h.prefix, a)
		return true
	})
	actor.Log(toLevel(r.Level), b.String())
	return nil
}

func (h *hostHandler) WithAttrs(as []slog.Attr) slog.Handler {
	next := &hostHandler{prefix: h.prefix, attrs: append([]slog.Attr(nil), h.attrs...)}
	for _, a := range as {
		writeInto(&next.attrs, h.prefix, a)
	}
	return next
}

func (h *hostHandler) WithGroup(name string) slog.Handler {
	if name == "" {
		return h
	}
	return &hostHandler{prefix: h.prefix + name + ".", attrs: h.attrs}
}

// writeInto group-qualifies an attr's key and appends it to dst (for WithAttrs).
func writeInto(dst *[]slog.Attr, prefix string, a slog.Attr) {
	if prefix != "" {
		a.Key = prefix + a.Key
	}
	*dst = append(*dst, a)
}

// writeAttr appends " key=value" to b (the platform line carries structured fields
// inline, since the host owns the surrounding format).
func writeAttr(b *strings.Builder, prefix string, a slog.Attr) {
	b.WriteByte(' ')
	b.WriteString(prefix)
	b.WriteString(a.Key)
	b.WriteByte('=')
	b.WriteString(a.Value.String())
}

// toLevel maps an slog level onto the host's four severities.
func toLevel(l slog.Level) actor.LogLevel {
	switch {
	case l >= slog.LevelError:
		return actor.LogLevelError
	case l >= slog.LevelWarn:
		return actor.LogLevelWarn
	case l >= slog.LevelInfo:
		return actor.LogLevelInfo
	default:
		return actor.LogLevelDebug
	}
}

// logWriter routes the standard log package's output to the host at info severity
// (the conventional default for log.Print). A trailing newline is trimmed since the
// host writes whole lines.
type logWriter struct{}

func (logWriter) Write(p []byte) (int, error) {
	actor.Log(actor.LogLevelInfo, strings.TrimRight(string(p), "\n"))
	return len(p), nil
}

// Log emits a message at the given severity directly through the platform logger —
// the lower-level escape hatch behind slog/log, for code that wants no structured layer.
func Log(level slog.Level, message string) { actor.Log(toLevel(level), message) }
