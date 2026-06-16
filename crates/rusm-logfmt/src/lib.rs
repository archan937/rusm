//! Shared formatting for RUSM's coloured, columnar log lines — the **single source** for
//! the timestamp format, the ANSI palette, the column widths, and tty-gated colouring.
//!
//! The single source for every RUSM log line, of which there are two shapes:
//! [`platform_line`] — the runtime's own voice, tagged `rusm` (the [`rusm_otp`](../rusm_otp/index.html)
//! `lifecycle` spawn/exit/census log and `rusm-wasm`'s serving **access log**); and
//! [`line`] — a guest's log, tagged `component#pid` (the host's `log` op stamps it for a
//! guest's `console.*` / `log::*`). One palette, so platform, access, and guest lines
//! share the look and line up when interleaved — in particular the identifier (`who`)
//! column is the same colour for the `rusm` tag and a `component#pid`.
//!
//! Pure and dependency-free: compiles for the host and for `wasm32-wasip2` guests alike.

use std::io::IsTerminal;

/// Gray — the timestamp (leads every line).
pub const TIME: &str = "90";
/// Dim magenta — the identifier (`who`) column: the `rusm` tag or a `component#pid`. One
/// colour for both, so platform and app lines' identifiers match.
pub const WHO: &str = "2;35";
/// Cyan — an app log level, or a neutral platform verb (`spawn` / `census`).
pub const LEVEL: &str = "36";
/// Red — a crash / error.
pub const ERROR: &str = "31";
/// Yellow — a kill / warning.
pub const WARN: &str = "33";
/// Yellow — a process-group **tag**: the census's `tag=n` entries and a `kill-tag` target.
/// Shares the kill/warn hue (tags are the cancellation surface), but named for intent.
pub const TAG: &str = "33";
/// Green — a clean exit / info.
pub const OK: &str = "32";
/// Bold — a name/label (not a colour, an attribute).
pub const BOLD: &str = "1";
/// Dim — secondary detail (`#pid`, capability summaries, separators).
pub const DIM: &str = "2";

/// Width of the identifier (`who`) column. A `component#pid` (name truncated to
/// [`NAME_CAP`]) or the `rusm` tag pads to this, so the action column that follows lines
/// up across streams.
pub const WHO_WIDTH: usize = 14;
/// Width of the action/verb column (`spawn` / `census` / `info` / …).
pub const ACTION_WIDTH: usize = 6;
/// Component-name cap for the identifier column — keeps `component#pid` bounded so the
/// `who` column stays tight even for long names.
pub const NAME_CAP: usize = 10;

/// The **severity** of an application / guest log line — how it is labeled and coloured.
/// Distinct from the node's `LogLevel` *gate* (which adds `off` and treats `debug` as
/// "show spawns"): this is purely presentation, so it lives here with the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    /// The severity colour: red / yellow / cyan / dim, so an error pops.
    pub fn colour(self) -> &'static str {
        match self {
            Self::Error => ERROR,
            Self::Warn => WARN,
            Self::Info => LEVEL,
            Self::Debug => DIM,
        }
    }

    /// The bare level name; [`line`] pads it to [`ACTION_WIDTH`] to align messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// An application / guest log line — the **single source** for the look every
/// non-lifecycle log shares: `<time> <component#pid> <level> <message>`, with a gray
/// timestamp, the dark-magenta identifier (name capped to [`NAME_CAP`], padded to
/// [`WHO_WIDTH`]), the severity-coloured level (padded to [`ACTION_WIDTH`]), then the
/// message. The host's guest-log bridge formats every guest `console.*` / `log::*` line
/// through this, so they share the platform's own look and line up when interleaved.
pub fn line(level: Level, component: &str, pid: u64, message: &str) -> String {
    let name: String = component.chars().take(NAME_CAP).collect();
    let who = format!("{name}#{pid}");
    format!(
        "{} {} {} {}",
        paint(TIME, &now_hms()),
        paint(WHO, &format!("{who:<WHO_WIDTH$}")),
        paint(
            level.colour(),
            &format!("{:<w$}", level.name(), w = ACTION_WIDTH)
        ),
        message,
    )
}

/// A **platform** log line — `<time> rusm <verb> <message>`: the runtime's own voice,
/// tagged `rusm` in the identifier column, the `verb` coloured by `code` and padded to
/// [`ACTION_WIDTH`]. The single source for every platform line: the lifecycle logger
/// (`spawn`/`exit`/`census`) and the serving access log (`http`/`ws`/`sse`) both build on
/// it, so they line up. (Guest logs use [`line`] instead — their identifier column is the
/// `component#pid`, not `rusm`.)
pub fn platform_line(code: &str, verb: &str, message: &str) -> String {
    format!(
        "{} {} {} {}",
        paint(TIME, &now_hms()),
        paint(WHO, &format!("{:<w$}", "rusm", w = WHO_WIDTH)),
        paint(code, &format!("{:<w$}", verb, w = ACTION_WIDTH)),
        message,
    )
}

/// Wrap `text` in ANSI colour `code`, but only when stderr is a terminal — piped or
/// redirected logs stay plain (no escape soup in a file).
pub fn paint(code: &str, text: &str) -> String {
    if std::io::stderr().is_terminal() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// `HH:MM:SS` (UTC) for a UNIX-epoch seconds value — pure (no clock read) so the
/// formatting is unit-testable; [`now_hms`] supplies "now".
pub fn hms(unix_secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        (unix_secs / 3600) % 24,
        (unix_secs / 60) % 60,
        unix_secs % 60
    )
}

/// `HH:MM:SS` (UTC) for the current wall clock (`wasi:clocks` in a guest).
pub fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    hms(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_formats_utc_clock_with_wraparound() {
        assert_eq!(hms(0), "00:00:00");
        assert_eq!(hms(3661), "01:01:01");
        assert_eq!(hms(86_399), "23:59:59");
        assert_eq!(hms(90_061), "01:01:01"); // wraps past 24h
    }

    #[test]
    fn level_maps_to_severity_colour_and_name() {
        assert_eq!(Level::Error.colour(), ERROR); // red — errors pop
        assert_eq!(Level::Warn.colour(), WARN); // yellow
        assert_eq!(Level::Info.colour(), LEVEL); // cyan
        assert_eq!(Level::Debug.colour(), DIM); // dim
        assert_eq!(Level::Info.name(), "info");
    }

    #[test]
    fn line_lays_out_identifier_level_and_message() {
        // (Robust to the tty colour-gate: assert the text, which survives any wrapping.)
        let out = line(Level::Info, "commander", 4, "ready");
        assert!(out.contains("commander#4"), "identifier present");
        assert!(out.contains("info"), "level present");
        assert!(out.ends_with("ready"), "message is last, unpainted");
    }

    #[test]
    fn line_caps_a_long_component_name() {
        let out = line(Level::Error, "actions-agent", 9, "x");
        assert!(out.contains("actions-ag#9"), "name capped to NAME_CAP");
        assert!(!out.contains("actions-agent#9"));
    }

    #[test]
    fn platform_line_tags_rusm_and_carries_the_verb_and_message() {
        let out = platform_line(LEVEL, "http", "GET /home → 200");
        assert!(out.contains("rusm"), "the platform identifier");
        assert!(out.contains("http"), "the verb");
        assert!(out.ends_with("GET /home → 200"), "message is last");
    }
}
