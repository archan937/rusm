// A resident reporter worker: RUSM runs it once at boot. It reaches the store service
// through the typed client and exercises the whole composition surface — a plain call, a
// callback argument, a streamed result, and a fire-and-forget cast — then PARKS. Returning
// would let the supervisor restart it in a loop (and re-spawn the store each time); a
// resident worker loops or parks, it never just exits. It only seeds when the board is
// empty, so a restart is harmless. The guest-composition showcase, over the same todos the
// api serves and the feed streams — the Go twin of the TS/RS example's reporter.
package main

import (
	"log/slog"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"todoboard/store"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	client, err := store.Spawn()
	if err != nil {
		slog.Error("reporter: spawn store", "err", err)
		return
	}

	// call: a request/reply summary.
	list, err := client.List()
	if err != nil {
		slog.Error("reporter: list", "err", err)
		return
	}
	done := 0
	for _, t := range list {
		if t.Done {
			done++
		}
	}
	slog.Info("reporter: summary", "todos", len(list), "done", done)

	// callback: seed a welcome list on a fresh board; progress is reported back to us as
	// each todo lands (only when empty, so this never re-seeds).
	if len(list) == 0 {
		seeded, err := client.ImportMany(
			[]string{
				"Welcome to the RUSM todo board",
				"Watch the live feed on :8081",
				"Join the chat on :8082",
			},
			func(n int) { slog.Info("reporter: seeded", "n", n) },
		)
		if err != nil {
			slog.Error("reporter: import", "err", err)
		} else {
			slog.Info("reporter: seeded", "total", seeded)
		}
	}

	// streaming: range a streamed result (each todo arrives as one chunk).
	streamed := 0
	for range client.All() {
		streamed++
	}
	slog.Info("reporter: streamed", "todos", streamed)

	// cast: fire-and-forget — no reply awaited.
	_ = client.Ping()

	// Park: stay resident without re-running (see the note above). No message ever arrives.
	for {
		rusm.ReceiveBytes()
	}
}
