//! The **attach** wire protocol: what a [`Node`](crate::Node) streams to attached
//! clients (`rusm attach`, a dashboard) and the commands they send back — a live
//! view of the node's processes.

use serde::{Deserialize, Serialize};

/// One live process, observed for `rusm attach` — the serde wire form of
/// [`rusm_otp::ProcessInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u64,
    /// Optional human-readable label (`set_label`).
    pub label: Option<String>,
    /// Registry names this process holds.
    pub names: Vec<String>,
    /// Bidirectionally linked peers.
    pub links: usize,
    /// Processes monitoring this one.
    pub monitors: usize,
    /// Items waiting in the mailbox, not yet consumed.
    pub mailbox_depth: usize,
    /// Whether this process traps exits.
    pub trap_exit: bool,
}

impl From<rusm_otp::ProcessInfo> for ProcessInfo {
    fn from(p: rusm_otp::ProcessInfo) -> Self {
        Self {
            pid: p.pid.raw(),
            label: p.label,
            names: p.names,
            links: p.links,
            monitors: p.monitors,
            mailbox_depth: p.mailbox_depth,
            trap_exit: p.trap_exit,
        }
    }
}

/// A point-in-time view of a running node, broadcast to every attached client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub uptime_ms: u64,
    pub process_count: usize,
    /// The per-process detail table; empty when detail is disabled (the
    /// `process_count` above is always live). See [`ClientCommand::SetDetail`].
    pub processes: Vec<ProcessInfo>,
}

/// A command from an attached client to the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    /// Include the per-process detail table in snapshots (counts are always sent).
    SetDetail { enabled: bool },
    /// Evaluate a line of JavaScript inside the node's REPL session — the live
    /// shell behind `rusm attach`. Stateful (bindings persist across lines) and
    /// gated: the node only honours it from a loopback client and only when a
    /// REPL host is wired in (see [`crate::repl::ReplHost`]).
    Eval { code: String },
}

/// A message from the node to an attached client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Sent on connect: the node's name.
    Hello { node: String },
    /// A telemetry tick.
    Snapshot { snapshot: NodeSnapshot },
    /// A rejected command, with a human-readable reason.
    Error { message: String },
    /// The result of an [`ClientCommand::Eval`]: the rendered return `value`,
    /// any captured console `output` lines, and `error` (the thrown message) when
    /// evaluation failed. A failed eval still leaves the session alive.
    EvalResult {
        value: String,
        output: Vec<String>,
        error: Option<String>,
    },
}

impl ClientCommand {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ClientCommand always serialises")
    }
}

impl ServerMessage {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ServerMessage always serialises")
    }

    /// The snapshot, if this is a [`ServerMessage::Snapshot`].
    pub fn snapshot(&self) -> Option<&NodeSnapshot> {
        match self {
            ServerMessage::Snapshot { snapshot } => Some(snapshot),
            _ => None,
        }
    }

    /// The node name, if this is a [`ServerMessage::Hello`].
    pub fn node(&self) -> Option<&str> {
        match self {
            ServerMessage::Hello { node } => Some(node),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_command_round_trips_tagged() {
        let cmd = ClientCommand::SetDetail { enabled: false };
        let json = cmd.to_json();
        assert!(json.contains("\"type\":\"set_detail\""));
        assert_eq!(ClientCommand::from_json(&json).unwrap(), cmd);
    }

    #[test]
    fn rejects_malformed_command() {
        assert!(ClientCommand::from_json("{\"type\":\"nope\"}").is_err());
    }

    #[test]
    fn eval_command_round_trips_tagged() {
        let cmd = ClientCommand::Eval {
            code: "const p = Process.self()".into(),
        };
        let json = cmd.to_json();
        assert!(json.contains("\"type\":\"eval\""));
        assert!(json.contains("Process.self()"));
        assert_eq!(ClientCommand::from_json(&json).unwrap(), cmd);
    }

    #[test]
    fn eval_result_round_trips_with_value_output_and_error() {
        let ok = ServerMessage::EvalResult {
            value: "42".into(),
            output: vec!["hello".into(), "world".into()],
            error: None,
        };
        assert!(ok.to_json().contains("\"type\":\"eval_result\""));
        assert_eq!(ServerMessage::from_json(&ok.to_json()).unwrap(), ok);
        // An eval result is neither a snapshot nor a hello.
        assert!(ok.snapshot().is_none() && ok.node().is_none());

        let failed = ServerMessage::EvalResult {
            value: String::new(),
            output: Vec::new(),
            error: Some("ReferenceError: x is not defined".into()),
        };
        assert_eq!(ServerMessage::from_json(&failed.to_json()).unwrap(), failed);
    }

    #[test]
    fn server_messages_round_trip_and_accessors_match() {
        let snapshot = NodeSnapshot {
            uptime_ms: 1_234,
            process_count: 1,
            processes: vec![ProcessInfo {
                pid: 7,
                label: Some("worker".into()),
                names: vec!["reg".into()],
                links: 1,
                monitors: 0,
                mailbox_depth: 2,
                trap_exit: false,
            }],
        };
        let tick = ServerMessage::Snapshot {
            snapshot: snapshot.clone(),
        };
        assert_eq!(ServerMessage::from_json(&tick.to_json()).unwrap(), tick);
        assert_eq!(tick.snapshot(), Some(&snapshot));
        assert_eq!(tick.node(), None);

        let hello = ServerMessage::Hello { node: "n1".into() };
        assert_eq!(ServerMessage::from_json(&hello.to_json()).unwrap(), hello);
        assert_eq!(hello.node(), Some("n1"));
        assert!(hello.snapshot().is_none());

        let error = ServerMessage::Error {
            message: "boom".into(),
        };
        assert_eq!(ServerMessage::from_json(&error.to_json()).unwrap(), error);
        assert!(error.snapshot().is_none() && error.node().is_none());
    }

    #[test]
    fn process_info_converts_from_otp() {
        let otp = rusm_otp::ProcessInfo {
            pid: rusm_otp::Pid::from_raw(42),
            links: 2,
            monitors: 1,
            names: vec!["db".into()],
            label: Some("store".into()),
            mailbox_depth: 5,
            trap_exit: true,
        };
        let wire = ProcessInfo::from(otp);
        assert_eq!(wire.pid, 42);
        assert_eq!(wire.label.as_deref(), Some("store"));
        assert_eq!(wire.names, ["db"]);
        assert_eq!((wire.links, wire.monitors, wire.mailbox_depth), (2, 1, 5));
        assert!(wire.trap_exit);
    }
}
