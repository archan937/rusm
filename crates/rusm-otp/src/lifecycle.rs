//! Opt-in process **lifecycle logging** — the "see what's happening" switch a node
//! turns on explicitly (`rusm.toml [log] level`). Off by default: the spawn hot path
//! does nothing. When on, the runtime logs each **labeled** process's spawn and exit to
//! stderr — so the signal is *components* (which the host labels), not internal plumbing
//! (responders, writers — left unlabeled).
//!
//! This module owns only the platform line's *structure*; the shared look (palette,
//! column widths, timestamp, tty-gated colour) comes from [`rusm_logfmt`], so platform
//! and app logs line up when interleaved. The runtime owns the *gate* (the level) and the
//! *when* (a spawn site, and `deregister` on exit). Lines read `<time> rusm <verb>
//! <label>#<pid>  <detail>`, the spawn line carrying the process's effective capabilities.

use std::collections::BTreeMap;

use rusm_logfmt as fmt;

use crate::exit::ExitReason;
use crate::pid::Pid;

/// Platform log verbosity, declared via `rusm.toml [log] level`. Ordered, cumulative:
/// a configured level shows every event at or below it. Each lifecycle event maps to a
/// distinct level — `Error`: a **crash** (a trap / OOM); `Warn`: + a **kill** (or
/// cascade); `Info`: + a **clean exit**; `Debug`: + every **spawn**. So a restart reads
/// as a crash `exit` (Error) then a fresh `spawn` (Debug).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
pub enum LogLevel {
    /// No platform logging (the default — zero hot-path cost).
    #[default]
    Off,
    /// Crashes only (a guest trap / OOM).
    Error,
    /// + kills and link cascades.
    Warn,
    /// + clean (normal) exits — every process *ending*.
    Info,
    /// + every spawn — full lifecycle visibility.
    Debug,
}

impl LogLevel {
    /// Parse a manifest string (`off`/`error`/`warn`/`info`/`debug`); anything else is
    /// `Off`, so a typo silently quiets rather than crashes.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "info" => Self::Info,
            "debug" | "trace" => Self::Debug,
            _ => Self::Off,
        }
    }

    /// The level of a process **exit** — the single source of truth for both the gate
    /// (which level shows it) and the colour: a crash is `Error`, a kill/cascade `Warn`,
    /// a clean exit `Info`.
    pub fn for_exit(reason: ExitReason) -> Self {
        match reason {
            ExitReason::Crashed => Self::Error,
            ExitReason::Killed | ExitReason::NoProc => Self::Warn,
            ExitReason::Normal => Self::Info,
        }
    }

    /// The shared-palette colour for this level (red crash / yellow kill / green clean;
    /// cyan otherwise).
    fn colour(self) -> &'static str {
        match self {
            Self::Error => fmt::ERROR,
            Self::Warn => fmt::WARN,
            Self::Info => fmt::OK,
            _ => fmt::LEVEL,
        }
    }
}

/// `<id>` rendered as a bold name + dim `#pid` — the spawned-process **subject** of a
/// spawn/exit line (distinct from the `who` column the lead already holds).
fn ident(label: &str, pid: Pid) -> String {
    format!(
        "{}{}",
        fmt::paint(fmt::BOLD, label),
        fmt::paint(fmt::DIM, &format!("#{}", pid.0))
    )
}

/// Log a component **spawn**: `<time> rusm spawn <label>#<pid>  <detail>` (detail = its
/// effective capabilities, so a reader sees exactly what the process can do).
pub fn log_spawn(pid: Pid, label: &str, detail: &str) {
    eprintln!(
        "{}",
        fmt::platform_line(
            fmt::LEVEL, // cyan
            "spawn",
            &format!("{}  {}", ident(label, pid), fmt::paint(fmt::DIM, detail)),
        )
    );
}

/// Log a process **exit**: `<time> rusm exit  <label>#<pid>  <reason>` — coloured by the
/// exit's level (red crash / yellow kill / green clean), the same mapping that gated it.
pub fn log_exit(pid: Pid, label: &str, reason: ExitReason) {
    let code = LogLevel::for_exit(reason).colour();
    eprintln!(
        "{}",
        fmt::platform_line(
            code,
            "exit",
            &format!(
                "{}  {}",
                ident(label, pid),
                fmt::paint(code, &format!("{reason:?}").to_lowercase())
            ),
        )
    );
}

/// Log a process **census**: `<time> rusm census  <comp>=<n>  …  <tag>=<n>  …` — the
/// count of live processes per component (by label), then per process-group **tag**
/// (Erlang `pg`) membership. Emitted debounced after process state settles. Bold names,
/// cyan counts; an idle node reads `(none)`.
pub fn log_census(components: &BTreeMap<String, u64>, tags: &BTreeMap<String, u64>) {
    // `<name>=<n>`: component names bold (white), tag names yellow, counts cyan.
    let entry = |name: &str, n: &u64, name_colour: &str| {
        format!(
            "{}{}{}",
            fmt::paint(name_colour, name),
            fmt::paint(fmt::DIM, "="),
            fmt::paint(fmt::LEVEL, &n.to_string())
        )
    };
    let mut entries: Vec<String> = components
        .iter()
        .map(|(name, n)| entry(name, n, fmt::BOLD))
        .collect();
    entries.extend(tags.iter().map(|(name, n)| entry(name, n, fmt::TAG)));
    let body = if entries.is_empty() {
        fmt::paint(fmt::DIM, "(none)")
    } else {
        entries.join("  ")
    };
    eprintln!("{}", fmt::platform_line(fmt::LEVEL, "census", &body));
}

/// Log a forced **kill** of one process: `<time> rusm kill  #<pid>` (yellow, like the
/// `exit` it triggers). Emitted when `kill(pid)` actually terminates a live process.
pub fn log_kill(pid: Pid) {
    eprintln!(
        "{}",
        fmt::platform_line(
            fmt::WARN,
            "kill",
            &fmt::paint(fmt::DIM, &format!("#{}", pid.0)),
        )
    );
}

/// Log a **kill-tag**: terminating a whole process group — `<time> rusm kill  <tag> → <n>`
/// (the tag in yellow, the killed count in cyan). One line per `kill_tag`, regardless of
/// group size (the members' own `exit` lines follow).
pub fn log_kill_tag(tag: &str, killed: usize) {
    let body = format!(
        "{} {} {}",
        fmt::paint(fmt::TAG, tag),
        fmt::paint(fmt::DIM, "→"),
        fmt::paint(fmt::LEVEL, &killed.to_string()),
    );
    eprintln!("{}", fmt::platform_line(fmt::WARN, "kill", &body));
}

// A supervisor **restart** intentionally has no dedicated event: it reads as the
// crashed instance's abnormal `exit` line followed by a fresh `spawn` line for the
// same component — carrying the crash reason and the new pid, which a bare "restart"
// line could not. (`LogLevel::Info` sits between `Warn` and `Debug` for that reason.)

#[cfg(test)]
mod tests {
    use super::LogLevel;

    #[test]
    fn parse_maps_known_levels_and_quiets_the_rest() {
        assert_eq!(LogLevel::parse("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("INFO"), LogLevel::Info);
        assert_eq!(LogLevel::parse("warning"), LogLevel::Warn);
        assert_eq!(LogLevel::parse("error"), LogLevel::Error);
        // Unset or unrecognised quiets to Off — a typo never accidentally goes loud.
        assert_eq!(LogLevel::parse(""), LogLevel::Off);
        assert_eq!(LogLevel::parse("loud"), LogLevel::Off);
    }

    #[test]
    fn levels_are_ordered_off_to_debug() {
        assert!(LogLevel::Off < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
    }
}
