use rusm_node::ClientCommand;

/// A parsed line of REPL input from `rusm attach`.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplInput {
    Command(ClientCommand),
    Help,
    Quit,
    Empty,
    Unknown(String),
}

pub fn parse(line: &str) -> ReplInput {
    let trimmed = line.trim();
    let mut parts = trimmed.split_whitespace();
    let Some(verb) = parts.next() else {
        return ReplInput::Empty;
    };
    match verb {
        "help" | "?" => ReplInput::Help,
        "quit" | "exit" | "q" => ReplInput::Quit,
        "detail" => match parts.next() {
            Some("on") => ReplInput::Command(ClientCommand::SetDetail { enabled: true }),
            Some("off") => ReplInput::Command(ClientCommand::SetDetail { enabled: false }),
            _ => ReplInput::Unknown("usage: detail on|off".to_string()),
        },
        // Anything else is a line of JavaScript, evaluated against the live node
        // (loopback-only). The whole trimmed line is the code.
        _ => ReplInput::Command(ClientCommand::Eval {
            code: trimmed.to_string(),
        }),
    }
}

pub const HELP: &str = "\
commands:
  detail on|off    toggle the per-process detail table in snapshots
  help             show this help
  quit             leave the REPL
anything else is evaluated as JavaScript against the node (local-only), e.g.
  Process.list()                       the live pids
  const p = Process.whereis(\"store\")   bindings persist across lines
  Process.send(p, \"hi\")                message a process";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines_are_empty() {
        assert_eq!(parse(""), ReplInput::Empty);
        assert_eq!(parse("   "), ReplInput::Empty);
    }

    #[test]
    fn help_and_quit_aliases() {
        for s in ["help", "?"] {
            assert_eq!(parse(s), ReplInput::Help);
        }
        for s in ["quit", "exit", "q"] {
            assert_eq!(parse(s), ReplInput::Quit);
        }
    }

    #[test]
    fn detail_on_off_and_misuse() {
        assert_eq!(
            parse("detail on"),
            ReplInput::Command(ClientCommand::SetDetail { enabled: true })
        );
        assert_eq!(
            parse("detail off"),
            ReplInput::Command(ClientCommand::SetDetail { enabled: false })
        );
        let usage = ReplInput::Unknown("usage: detail on|off".to_string());
        assert_eq!(parse("detail"), usage);
        assert_eq!(parse("detail maybe"), usage);
    }

    #[test]
    fn non_meta_lines_are_evaluated_as_javascript() {
        assert_eq!(
            parse("1 + 1"),
            ReplInput::Command(ClientCommand::Eval {
                code: "1 + 1".into()
            })
        );
        assert_eq!(
            parse("const p = Process.whereis(\"store\")"),
            ReplInput::Command(ClientCommand::Eval {
                code: "const p = Process.whereis(\"store\")".into(),
            })
        );
        // A word that isn't a meta-command is just an expression (a ReferenceError
        // at eval time, not a "unknown command").
        assert_eq!(
            parse("frobnicate"),
            ReplInput::Command(ClientCommand::Eval {
                code: "frobnicate".into(),
            })
        );
    }

    #[test]
    fn extra_whitespace_is_tolerated() {
        assert_eq!(
            parse("  detail   on  "),
            ReplInput::Command(ClientCommand::SetDetail { enabled: true })
        );
        // Eval lines are trimmed too, so the code is clean.
        assert_eq!(
            parse("  1 + 1  "),
            ReplInput::Command(ClientCommand::Eval {
                code: "1 + 1".into()
            })
        );
    }
}
