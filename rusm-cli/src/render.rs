use rusm_node::{NodeSnapshot, ServerMessage};

/// Renders a server message as a line (or block) for the `rusm attach` display.
/// May be empty (a statement that yields no value) — the caller skips empty lines.
pub fn render_message(message: &ServerMessage) -> String {
    match message {
        ServerMessage::Hello { node } => format!("connected to node '{node}'"),
        ServerMessage::Snapshot { snapshot } => render_snapshot(snapshot),
        ServerMessage::Error { message } => format!("error: {message}"),
        ServerMessage::EvalResult {
            value,
            output,
            error,
        } => render_eval(value, output, error.as_deref()),
    }
}

/// A REPL eval result: captured console output first (as written), then either the
/// error or the return value. A bare statement yields nothing → an empty string.
fn render_eval(value: &str, output: &[String], error: Option<&str>) -> String {
    let mut lines: Vec<String> = output.to_vec();
    if let Some(error) = error {
        lines.push(format!("error: {error}"));
    } else if !value.is_empty() {
        lines.push(value.to_string());
    }
    lines.join("\n")
}

/// A one-line header (live process count + uptime) followed by the per-process
/// detail table, when present.
fn render_snapshot(snapshot: &NodeSnapshot) -> String {
    let mut out = format!(
        "{} process(es), up {:.1}s",
        snapshot.process_count,
        snapshot.uptime_ms as f64 / 1000.0
    );
    for p in &snapshot.processes {
        let label = p.label.as_deref().unwrap_or("-");
        let names = if p.names.is_empty() {
            String::new()
        } else {
            format!(" [{}]", p.names.join(","))
        };
        out.push_str(&format!(
            "\n  #{:<6} {:<16}{names} mailbox {} links {}",
            p.pid, label, p.mailbox_depth, p.links
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusm_node::ProcessInfo;

    #[test]
    fn renders_hello_with_node_name() {
        let hello = ServerMessage::Hello {
            node: "my-app".into(),
        };
        assert_eq!(render_message(&hello), "connected to node 'my-app'");
    }

    #[test]
    fn renders_snapshot_header_and_process_table() {
        let snapshot = NodeSnapshot {
            uptime_ms: 3_400,
            process_count: 1,
            processes: vec![ProcessInfo {
                pid: 7,
                label: Some("worker".into()),
                names: vec!["api".into()],
                links: 2,
                monitors: 0,
                mailbox_depth: 5,
                trap_exit: false,
            }],
        };
        let out = render_message(&ServerMessage::Snapshot { snapshot });
        assert!(out.starts_with("1 process(es), up 3.4s"));
        assert!(out.contains("#7"));
        assert!(out.contains("worker"));
        assert!(out.contains("[api]"));
        assert!(out.contains("mailbox 5"));
    }

    #[test]
    fn renders_error() {
        let message = ServerMessage::Error {
            message: "nope".to_string(),
        };
        assert_eq!(render_message(&message), "error: nope");
    }

    #[test]
    fn renders_eval_value_after_console_output() {
        let message = ServerMessage::EvalResult {
            value: "42".into(),
            output: vec!["hi".into(), "there".into()],
            error: None,
        };
        assert_eq!(render_message(&message), "hi\nthere\n42");
    }

    #[test]
    fn renders_eval_error_over_value() {
        let message = ServerMessage::EvalResult {
            value: String::new(),
            output: vec![],
            error: Some("Error: boom".into()),
        };
        assert_eq!(render_message(&message), "error: Error: boom");
    }

    #[test]
    fn a_statement_with_no_value_renders_empty() {
        let message = ServerMessage::EvalResult {
            value: String::new(),
            output: vec![],
            error: None,
        };
        assert_eq!(render_message(&message), "");
    }
}
