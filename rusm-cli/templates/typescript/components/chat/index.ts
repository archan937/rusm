// A chat room over WebSocket — one isolated process per connection. Rooms are
// process-group tags: joining tags this connection `room:<name>`, and a message fans out
// to the tag's members with `whereisTag` + `send` (the platform pub/sub — no broker). A
// peer's relay arrives in this same process's mailbox (so `message` sees both the client's
// own frames and peers' relays); the wire below tells them apart. The tag releases
// automatically when the process exits, so leaving a room is just disconnecting.
//
// Wire (application protocol):
//   client → server:  {"join":"<room>"}   then   {"say":"<text>"}
//   server → client:  {"system":"..."}     and    {"from":"<pid>","text":"..."}
import { websocket, Process, type Socket } from "rusm-ts";

let room: string | null = null; // this connection's room (module state is per-connection)

const tag = (name: string) => `room:${name}`;
const system = (socket: Socket, text: string) => socket.send(JSON.stringify({ system: text }));

const decode = (data: Uint8Array): Record<string, unknown> | null => {
  try {
    return JSON.parse(new TextDecoder().decode(data));
  } catch {
    return null;
  }
};

/** Fan a relay out to the current room's members (optionally excluding this connection). */
function broadcast(relay: { from: string; text: string }, exceptSelf = false): void {
  if (!room) return;
  const self = Process.self();
  const payload = JSON.stringify(relay);
  for (const pid of Process.whereisTag(tag(room))) {
    if (!(exceptSelf && pid === self)) Process.send(pid, payload);
  }
}

export default websocket({
  open(socket) {
    system(socket, 'connected — send {"join":"<room>"} to join a room');
    console.log("chat: connected");
  },

  message(socket, data) {
    const msg = decode(data);
    if (!msg) return;

    // {"join": "<room>"} — subscribe this connection and greet.
    if (typeof msg.join === "string") {
      room = msg.join;
      Process.registerTag(tag(room));
      system(socket, `welcome to #${room}`);
      broadcast({ from: "system", text: `a new member joined #${room}` }, true);
      console.log(`chat: joined #${room}`);
      return;
    }

    // {"say": "<text>"} — fan out to the room (the sender sees their own message too).
    if (typeof msg.say === "string") {
      if (!room) return system(socket, "join a room first");
      broadcast({ from: String(Process.self()), text: msg.say });
      console.log(`chat: #${room} <${Process.self()}> ${msg.say}`);
      return;
    }

    // A peer's relay ({from, text}) landed in our mailbox — forward it to this client.
    if (typeof msg.text === "string") socket.send(data);
  },

  close() {
    console.log(`chat: left #${room ?? "—"}`);
  },
});
