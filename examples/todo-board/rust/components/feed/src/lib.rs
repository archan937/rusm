//! The live todo feed — Server-Sent Events, one isolated process per connection. On
//! connect it subscribes to the todo change tag and emits the current list; thereafter the
//! `api` pushes each change straight to this stream's mailbox (true push, never a poll).
//! `close` fires on disconnect, and the subscription releases when the process exits.
use rusm_rs::sse::{self, Handler, Stream};
use todos::{snapshot, FEED_TAG};

struct Feed;

impl Handler for Feed {
    fn open(&mut self, stream: &Stream) {
        rusm_rs::register_tag(FEED_TAG); // subscribe to changes the api publishes
        stream.data(&snapshot()); // the current list, so a new client sees state at once
        log::info!("feed: client connected");
    }
    fn message(&mut self, stream: &Stream, event: Vec<u8>) {
        stream.data(&event); // a published change (the new list) → emit it verbatim
    }
    fn close(&mut self, _stream: &Stream) {
        log::info!("feed: client left");
    }
}

#[rusm_rs::main]
fn run() {
    sse::serve(Feed);
}
