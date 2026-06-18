// The live todo feed — Server-Sent Events, one isolated process per connection. On
// connect it subscribes to the todo change tag and emits the current list; thereafter the
// `api` pushes each change straight to this stream's mailbox (true push, never a poll).
// `close` fires on disconnect, and the subscription auto-releases when the process exits.
import { sse, Process } from "rusm-ts";
import { FEED_TAG, snapshot } from "../../lib/todos";

export default sse({
  open(stream) {
    Process.registerTag(FEED_TAG); // subscribe to changes the api publishes
    stream.data(snapshot()); // the current list, so a new client sees state immediately
    console.log("feed: client connected");
  },
  message(stream, event) {
    stream.data(event); // a published change (the new list) → emit it verbatim
  },
  close() {
    console.log("feed: client left");
  },
});
