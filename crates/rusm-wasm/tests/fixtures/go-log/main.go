// A guest that logs the *normal Go* way — the standard log package and log/slog,
// including structured attrs and a group — to prove the SDK routes all of it to the
// platform logger without the guest wiring name/pid/format (the Go counterpart of the
// TS console test). On run it logs across every path, then replies; the reply proves
// none of the logging calls trapped (so the whole guest → host log bridge works).
package main

import (
	"log"
	"log/slog"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	replyTo, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}

	slog.Info("hello", "pid", rusm.Self().String(), "n", 7)
	slog.Default().WithGroup("http").With("route", "/").Warn("slow handler")
	slog.Error("boom", "err", "nope")
	log.Printf("plain log line %d", 42)

	rusm.SendBytes(replyTo, []byte("logged ok"))
}
