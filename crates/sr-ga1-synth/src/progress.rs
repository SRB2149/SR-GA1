//! Sign of life on stderr.
//!
//! A run at the edge of fitting can spend a minute in the placement search,
//! and a tool that sits silent through that looks hung. Every stage announces
//! itself, and the search loop reports attempts, elapsed time against the
//! budget, and the best result so far.
//!
//! A terminal gets an updating single line; anything else — a pipe, a log, a
//! CI job — gets plain periodic lines, because control characters in a log
//! file are worse than no progress at all.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

pub struct Progress {
    tty: bool,
    quiet: bool,
    last_update: Instant,
    /// Whether an updating line is currently on screen and needs clearing.
    pending: bool,
    stage: String,
}

impl Progress {
    pub fn new(quiet: bool) -> Progress {
        Progress {
            tty: std::io::stderr().is_terminal(),
            quiet,
            last_update: Instant::now() - Duration::from_secs(10),
            pending: false,
            stage: String::new(),
        }
    }

    /// Name the stage now starting.
    pub fn stage(&mut self, name: &str) {
        if self.quiet {
            return;
        }
        self.clear();
        self.stage = name.to_string();
        let _ = writeln!(std::io::stderr(), "  {}", name);
        self.last_update = Instant::now();
    }

    /// An aside that should not be overwritten.
    pub fn note(&mut self, text: &str) {
        if self.quiet {
            return;
        }
        self.clear();
        let _ = writeln!(std::io::stderr(), "    {}", text);
    }

    /// Progress within the place-and-route search. Rate-limited to once a
    /// second, but always shown at least that often.
    pub fn search(
        &mut self,
        attempts: usize,
        elapsed: Duration,
        budget: Duration,
        best_unrouted: usize,
    ) {
        if self.quiet {
            return;
        }
        if self.last_update.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_update = Instant::now();
        let best = if best_unrouted == usize::MAX {
            "none yet".to_string()
        } else if best_unrouted == 0 {
            "fitted".to_string()
        } else {
            format!("{} unrouted", best_unrouted)
        };
        let line = format!(
            "  placing and routing: {} attempts, {:.0}s of {:.0}s, best: {}",
            attempts,
            elapsed.as_secs_f64(),
            budget.as_secs_f64(),
            best
        );
        let mut err = std::io::stderr();
        if self.tty {
            let _ = write!(err, "\r\x1b[2K{}", line);
            let _ = err.flush();
            self.pending = true;
        } else {
            let _ = writeln!(err, "{}", line);
        }
    }

    /// Finish any updating line so later output starts clean.
    pub fn clear(&mut self) {
        if self.pending {
            let _ = write!(std::io::stderr(), "\r\x1b[2K");
            let _ = std::io::stderr().flush();
            self.pending = false;
        }
    }

    pub fn done(&mut self, summary: &str) {
        if self.quiet {
            return;
        }
        self.clear();
        let _ = writeln!(std::io::stderr(), "  {}", summary);
    }
}
