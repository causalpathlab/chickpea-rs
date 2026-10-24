//! A step run as a child `chickpea` on a worker thread, so the screen
//! stays live: its log a line at a time, its progress bar followed, and a
//! stop that reaches it and all it started.

use legume_tui::child::{run_one, Bar, Failed, Said, Stopper};
use std::sync::{Arc, Mutex, MutexGuard};

/// Lines of a run's log kept for the screen; older ones are dropped.
const KEEP_LINES: usize = 5000;

/// What a run has said so far, and how it ended.
#[derive(Default)]
pub struct Log {
    /// Lines of its log, the newest last.
    pub lines: std::collections::VecDeque<String>,
    /// Lines dropped from the front of `lines`.
    pub dropped: usize,
    /// Its progress bar, while it has one.
    pub bar: Option<Bar>,
    /// How it ended, once it has: `Err` says why it did not finish well.
    pub end: Option<Result<(), Ended>>,
}

/// Why a run did not finish well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// It was stopped.
    Stopped,
    /// It failed, and why.
    Failed(String),
}

impl Log {
    /// Lines from `from` (counted from the run's first line) on.
    pub fn lines_from(&self, from: usize) -> impl Iterator<Item = &String> {
        self.lines
            .range(from.saturating_sub(self.dropped).min(self.lines.len())..)
    }

    /// Lines said in all.
    #[must_use]
    pub fn said(&self) -> usize {
        self.dropped + self.lines.len()
    }

    fn push(&mut self, line: String) {
        if self.lines.len() >= KEEP_LINES {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }
}

/// A run of a step: `chickpea {argv}`.
pub struct Job {
    log: Arc<Mutex<Log>>,
    stopper: Arc<Stopper>,
}

impl Job {
    /// Start `chickpea {argv}` (this binary) as `env` starts children, in a
    /// process group of its own.
    pub fn start(argv: Vec<String>, env: legume_tui::ChildEnv) -> anyhow::Result<Self> {
        let mut cmd = crate::mcp::own_command(&argv)?;
        env.apply(&mut cmd, legume_tui::Group::Own);
        let log = Arc::new(Mutex::new(Log::default()));
        let stopper = Arc::new(Stopper::new());
        let (to, held) = (Arc::clone(&log), Arc::clone(&stopper));
        std::thread::Builder::new()
            .name("chickpea-run".into())
            .spawn(move || {
                let out = run_one(cmd, &held, |said| {
                    let mut log = lock(&to);
                    Bar::track(&mut log.bar, &said);
                    if let Said::Line(line) = said {
                        log.push(line);
                    }
                });
                let end = match out {
                    Ok(()) => Ok(()),
                    Err(Failed::Stopped) => Err(Ended::Stopped),
                    Err(Failed::Start(why) | Failed::Exit(why)) => Err(Ended::Failed(why)),
                    Err(other) => Err(Ended::Failed(format!("{other:?}"))),
                };
                lock(&to).end = Some(end);
            })?;
        Ok(Self { log, stopper })
    }

    /// What it has said, held while the guard lives.
    pub fn log(&self) -> MutexGuard<'_, Log> {
        lock(&self.log)
    }

    /// Whether it is still running.
    #[must_use]
    pub fn running(&self) -> bool {
        self.log().end.is_none()
    }

    /// Stop it, with all it started.
    pub fn stop(&self) {
        self.stopper.stop();
    }
}

fn lock(log: &Mutex<Log>) -> MutexGuard<'_, Log> {
    log.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_lines_are_dropped_and_counted() {
        let mut log = Log::default();
        for i in 0..KEEP_LINES + 3 {
            log.push(i.to_string());
        }
        assert_eq!(log.dropped, 3);
        assert_eq!(log.said(), KEEP_LINES + 3);
        assert!(log.lines_from(KEEP_LINES + 1).eq(["5001", "5002"]));
        assert_eq!(log.lines_from(0).count(), KEEP_LINES);
    }
}
