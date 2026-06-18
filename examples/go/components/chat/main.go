// A chat room over WebSocket in Go — one isolated process per connection. Rooms are
// process-group tags: joining tags this connection room:<name>, and a message fans out to
// the tag's members with WhereisTag + SendBytes. A peer's relay arrives in this same
// process's mailbox (so Message sees both the client's own frames and peers' relays); the
// wire below tells them apart. The tag releases when the process exits, so leaving a room
// is just disconnecting.
//
// Wire (application protocol):
//
//	client → server:  {"join":"<room>"}   then   {"say":"<text>"}
//	server → client:  {"system":"..."}      and    {"from":"<pid>","text":"..."}
package main

import (
	"encoding/json"
	"log/slog"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

// frame is the inbound wire shape; pointers distinguish an absent field from an empty one.
type frame struct {
	Join *string `json:"join,omitempty"`
	Say  *string `json:"say,omitempty"`
	Text *string `json:"text,omitempty"`
}

func roomTag(room string) string { return "room:" + room }

func system(c web.Conn, text string) {
	if b, err := json.Marshal(map[string]string{"system": text}); err == nil {
		c.Send(b)
	}
}

func run() {
	var room string // this connection's room (one handler instance per connection)

	// broadcast fans a relay out to the room's members (optionally excluding this conn).
	broadcast := func(payload []byte, exceptSelf bool) {
		if room == "" {
			return
		}
		me := rusm.Self()
		for _, pid := range rusm.WhereisTag(roomTag(room)) {
			if exceptSelf && pid == me {
				continue
			}
			rusm.SendBytes(pid, payload)
		}
	}

	web.WebSocket{
		Open: func(c web.Conn) {
			system(c, `connected — send {"join":"<room>"} to join a room`)
			slog.Info("chat: connected")
		},

		Message: func(c web.Conn, data []byte) {
			var f frame
			if json.Unmarshal(data, &f) != nil {
				return
			}

			switch {
			// {"join": "<room>"} — subscribe this connection and greet.
			case f.Join != nil:
				room = *f.Join
				rusm.RegisterTag(roomTag(room))
				system(c, "welcome to #"+room)
				announce, _ := json.Marshal(map[string]string{
					"from": "system", "text": "a new member joined #" + room,
				})
				broadcast(announce, true)
				slog.Info("chat: joined", "room", room)

			// {"say": "<text>"} — fan out to the room (the sender sees their own message too).
			case f.Say != nil:
				if room == "" {
					system(c, "join a room first")
					return
				}
				relay, _ := json.Marshal(map[string]string{
					"from": rusm.Self().String(), "text": *f.Say,
				})
				broadcast(relay, false)
				slog.Info("chat: say", "room", room, "from", rusm.Self().String())

			// A peer's relay ({from, text}) landed in our mailbox — forward it to this client.
			case f.Text != nil:
				c.Send(data)
			}
		},

		Close: func(_ web.Conn) {
			slog.Info("chat: left", "room", room)
		},
	}.Serve()
}
