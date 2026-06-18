//! A chat room over WebSocket — one isolated process per connection. Rooms are
//! process-group tags: joining tags this connection `room:<name>`, and a message fans out
//! to the tag's members with `whereis_tag` + `send`. A peer's relay arrives in this same
//! process's mailbox (so `message` sees both the client's own frames and peers' relays);
//! the wire below tells them apart. The tag releases when the process exits, so leaving a
//! room is just disconnecting.
//!
//! Wire (application protocol):
//!   client → server:  {"join":"<room>"}   then   {"say":"<text>"}
//!   server → client:  {"system":"..."}     and    {"from":"<pid>","text":"..."}
use rusm_rs::ws::{self, Connection, Handler};
use serde::Deserialize;
use serde_json::json;

#[derive(Default)]
struct Chat {
    room: Option<String>, // this connection's room (one handler instance per connection)
}

#[derive(Deserialize)]
struct Frame {
    join: Option<String>,
    say: Option<String>,
    text: Option<String>,
}

impl Chat {
    fn tag(room: &str) -> String {
        format!("room:{room}")
    }
    fn system(conn: &Connection, text: &str) {
        conn.send(json!({ "system": text }).to_string().as_bytes());
    }
    /// Fan a relay out to the room's members (optionally excluding this connection).
    fn broadcast(&self, relay: &serde_json::Value, except_self: bool) {
        let Some(room) = &self.room else { return };
        let me = rusm_rs::me();
        let payload = relay.to_string();
        for pid in rusm_rs::whereis_tag(&Self::tag(room)) {
            if !(except_self && pid == me) {
                rusm_rs::send_bytes(pid, payload.as_bytes());
            }
        }
    }
}

impl Handler for Chat {
    fn open(&mut self, conn: &Connection) {
        Self::system(
            conn,
            "connected — send {\"join\":\"<room>\"} to join a room",
        );
        log::info!("chat: connected");
    }

    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        let Ok(frame) = serde_json::from_slice::<Frame>(&data) else {
            return;
        };

        // {"join": "<room>"} — subscribe this connection and greet.
        if let Some(room) = frame.join {
            rusm_rs::register_tag(&Self::tag(&room));
            Self::system(conn, &format!("welcome to #{room}"));
            let announce =
                json!({ "from": "system", "text": format!("a new member joined #{room}") });
            self.room = Some(room.clone());
            self.broadcast(&announce, true);
            log::info!("chat: joined #{room}");
            return;
        }

        // {"say": "<text>"} — fan out to the room (the sender sees their own message too).
        if let Some(say) = frame.say {
            if self.room.is_none() {
                return Self::system(conn, "join a room first");
            }
            self.broadcast(
                &json!({ "from": rusm_rs::me().to_string(), "text": say }),
                false,
            );
            log::info!(
                "chat: #{} <{}> {}",
                self.room.as_deref().unwrap_or("-"),
                rusm_rs::me(),
                say
            );
            return;
        }

        // A peer's relay ({from, text}) landed in our mailbox — forward it to this client.
        if frame.text.is_some() {
            conn.send(&data);
        }
    }

    fn close(&mut self, _conn: &Connection) {
        log::info!("chat: left #{}", self.room.as_deref().unwrap_or("—"));
    }
}

#[rusm_rs::main]
fn run() {
    ws::serve(Chat::default());
}
