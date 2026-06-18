//! Keyed publish/subscribe **fan-out** — the broker *mechanics* as a reusable
//! primitive, so an app never hand-rolls subscriber tracking, fan-out, or dead-peer
//! cleanup. Embed [`Topics`] in a process and you get:
//!   - `subscribe(topic, pid)` — register a subscriber and **monitor** it,
//!   - `publish(topic, msg)` — fan a message out to a topic's subscribers,
//!   - automatic **pruning** of a dead subscriber (clean exit *or* crash) the moment
//!     its monitor `__down` arrives — the crash-safe OTP cleanup, no protocol needed.
//!
//! Separation of concerns: this owns the *mechanism*; the app owns the *content* —
//! which topics exist, what is published, and any late-join snapshot (which reads the
//! app's own state). The app's loop just routes inbound messages:
//!
//! ```ignore
//! let mut topics = Topics::new();
//! loop {
//!     let msg = rusm_rs::receive_bytes();
//!     if topics.handle_down(&msg) { continue; }   // a subscriber died → pruned
//!     match parse(&msg) {
//!         Sub { topic, pid } => topics.subscribe(&topic, pid),  // + snapshot, app's call
//!         Pub { topic, data } => topics.publish(&topic, &data),
//!     }
//! }
//! ```
//!
//! Auto-pruning needs the **monitor** capability (`process_control` or `spawn`); a
//! subscriber whose monitor was denied still receives fan-out but isn't pruned on death.

use std::collections::{HashMap, HashSet};

use crate::Pid;

/// A set of named topics with their subscribers — embed one per broker process.
#[derive(Default)]
pub struct Topics {
    /// topic → its subscribers (fan-out targets). The hot `publish` path reads this.
    by_topic: HashMap<String, Vec<Pid>>,
    /// Every monitored pid, so each is monitored exactly once across all its topics.
    monitored: HashSet<Pid>,
}

impl Topics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe `pid` to `topic` (idempotent) and [`monitor`](crate::monitor) it the
    /// first time it's seen, so a later `__down` prunes it via [`handle_down`].
    pub fn subscribe(&mut self, topic: &str, pid: Pid) {
        let subs = self.by_topic.entry(topic.to_string()).or_default();
        if !subs.contains(&pid) {
            subs.push(pid);
        }
        // Monitor each pid once, however many topics it joins (a duplicate monitor
        // would just deliver a redundant — harmless — `__down`, but once is cleaner).
        if self.monitored.insert(pid) {
            crate::monitor(pid);
        }
    }

    /// Remove `pid` from `topic` (an explicit leave; death is handled by
    /// [`handle_down`]). The pid stays monitored if it remains in other topics.
    pub fn unsubscribe(&mut self, topic: &str, pid: Pid) {
        if let Some(subs) = self.by_topic.get_mut(topic) {
            subs.retain(|&p| p != pid);
        }
    }

    /// Fan `msg` out to every current subscriber of `topic` (the hot path). Sending to
    /// an already-gone pid is a harmless no-op, so this never fails.
    pub fn publish(&self, topic: &str, msg: &[u8]) {
        if let Some(subs) = self.by_topic.get(topic) {
            for &pid in subs {
                crate::send_bytes(pid, msg);
            }
        }
    }

    /// If `msg` is a monitor `__down`, prune that subscriber from **every** topic and
    /// return `true` (the message is consumed). Returns `false` for anything else — so
    /// a broker calls this first and treats a `false` as an ordinary command. A fast
    /// prefix check keeps non-`__down` messages from ever being parsed.
    pub fn handle_down(&mut self, msg: &[u8]) -> bool {
        match crate::down_pid(msg) {
            Some(pid) => {
                self.prune(pid);
                true
            }
            None => false,
        }
    }

    /// The number of subscribers on `topic` (introspection / capacity).
    pub fn subscriber_count(&self, topic: &str) -> usize {
        self.by_topic.get(topic).map_or(0, Vec::len)
    }

    /// Drop a subscriber from every topic it was in (cold path — on death).
    fn prune(&mut self, pid: Pid) {
        for subs in self.by_topic.values_mut() {
            subs.retain(|&p| p != pid);
        }
        self.monitored.remove(&pid);
    }
}
