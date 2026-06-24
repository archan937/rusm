// The live todo feed in Go — Server-Sent Events, one isolated process per connection. On
// connect it subscribes to the todo change tag and emits the current list; thereafter the
// api and store push each change straight to this stream's mailbox (true push, not
// polling). Close fires on disconnect, and the subscription releases when the process exits.
package main

import (
	"log/slog"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
	"todoboard/todos"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.Sse{
		Open: func(s web.Stream) {
			rusm.RegisterTag(todos.FeedTag) // subscribe to changes the api publishes
			s.Data(todos.Snapshot())        // the current list, so a new client sees state at once
			slog.Info("feed: client connected")
		},
		Message: func(s web.Stream, event []byte) {
			s.Data(event) // a published change (the new list) → emit it verbatim
		},
		Close: func(_ web.Stream) {
			slog.Info("feed: client left")
		},
	}.Serve()
}
