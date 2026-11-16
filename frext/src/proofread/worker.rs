//! The background proofreading thread.
//!
//! Harper can take longer than a frame on a large buffer, so checks run on a
//! dedicated thread. The UI sends [`CheckRequest`]s and polls for
//! [`CheckResult`]s; the worker coalesces requests so that while the user is
//! typing only the newest text for each tab is ever checked.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};

use super::{Diagnostic, Proofreader};
use crate::error::ProofreadError;

/// A request to proofread one tab's buffer.
#[derive(Debug, Clone)]
pub struct CheckRequest {
    /// The tab the text belongs to.
    pub tab_id: u64,
    /// An opaque revision for the (text, language) pair, echoed back in the
    /// result.
    pub revision: u64,
    /// The full buffer text.
    pub text: String,
    /// The buffer's file-extension token.
    pub language: String,
}

/// The outcome of a [`CheckRequest`].
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// The tab the diagnostics belong to.
    pub tab_id: u64,
    /// The revision of the request this answers.
    pub revision: u64,
    /// Diagnostics, sorted by position.
    pub diagnostics: Vec<Diagnostic>,
}

/// A message to the worker thread.
enum Command {
    Check(CheckRequest),
    /// Replace the personal dictionary (rebuilds the checker).
    SetUserWords(Vec<String>),
}

/// Handle to the running worker. Dropping it closes the command channel,
/// which ends the thread once any in-flight check finishes.
pub struct Worker {
    commands: Sender<Command>,
    results: Receiver<CheckResult>,
}

impl Worker {
    /// Spawn the worker with the given personal dictionary. `notify` is
    /// called on the worker thread after each result is ready (the app uses
    /// it to request a repaint so results appear without further input).
    ///
    /// # Errors
    ///
    /// Returns [`ProofreadError::Spawn`] if the thread cannot be created.
    pub fn spawn(
        user_words: Vec<String>,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self, ProofreadError> {
        let (commands, command_rx) = mpsc::channel();
        let (result_tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("frext-proofread".to_owned())
            .spawn(move || run(&command_rx, &result_tx, &user_words, &notify))
            .map_err(ProofreadError::Spawn)?;
        Ok(Self { commands, results })
    }

    /// Queue a check. Returns `false` if the worker has stopped.
    pub fn check(&self, request: CheckRequest) -> bool {
        self.commands.send(Command::Check(request)).is_ok()
    }

    /// Replace the personal dictionary. Returns `false` if the worker has
    /// stopped.
    pub fn set_user_words(&self, words: Vec<String>) -> bool {
        self.commands.send(Command::SetUserWords(words)).is_ok()
    }

    /// Every result that has arrived since the last call, without blocking.
    pub fn drain(&self) -> impl Iterator<Item = CheckResult> + '_ {
        self.results.try_iter()
    }

    /// Block until the next result arrives (tests only).
    #[cfg(test)]
    fn recv_timeout(&self, timeout: std::time::Duration) -> Option<CheckResult> {
        self.results.recv_timeout(timeout).ok()
    }
}

/// The worker loop: block for a command, coalesce everything else queued
/// behind it, then check the newest text of each tab.
fn run(
    commands: &Receiver<Command>,
    results: &Sender<CheckResult>,
    user_words: &[String],
    notify: &dyn Fn(),
) {
    let mut proofreader = Proofreader::new(user_words);
    let mut words = user_words.to_vec();

    while let Ok(first) = commands.recv() {
        let mut pending: HashMap<u64, CheckRequest> = HashMap::new();
        let mut new_words: Option<Vec<String>> = None;
        for command in std::iter::once(first).chain(commands.try_iter()) {
            match command {
                Command::Check(request) => {
                    pending.insert(request.tab_id, request);
                }
                Command::SetUserWords(list) => new_words = Some(list),
            }
        }

        if let Some(list) = new_words {
            proofreader = Proofreader::new(&list);
            words = list;
        }

        for request in pending.into_values() {
            // Harper is third-party code with internal assertions; a panic in
            // one check must not take proofreading down for the session.
            let checked = catch_unwind(AssertUnwindSafe(|| {
                proofreader.check(&request.text, &request.language)
            }));
            let diagnostics = checked.unwrap_or_else(|_| {
                log::error!("proofreading tab {} panicked; skipping", request.tab_id);
                proofreader = Proofreader::new(&words);
                Vec::new()
            });
            let result = CheckResult {
                tab_id: request.tab_id,
                revision: request.revision,
                diagnostics,
            };
            if results.send(result).is_err() {
                return;
            }
            notify();
        }
    }
}

#[cfg(test)]
mod tests {
    // `expect` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::expect_used)]

    use std::time::Duration;

    use super::*;

    /// Generous bound: the first check also loads Harper's dictionary.
    const TIMEOUT: Duration = Duration::from_secs(60);

    fn request(tab_id: u64, revision: u64, text: &str) -> CheckRequest {
        CheckRequest {
            tab_id,
            revision,
            text: text.to_owned(),
            language: String::new(),
        }
    }

    #[test]
    fn check_round_trips_through_the_thread_and_notifies() {
        let (notify_tx, notify_rx) = mpsc::channel();
        let worker = Worker::spawn(Vec::new(), move || {
            let _ = notify_tx.send(());
        })
        .expect("worker spawns");

        assert!(worker.check(request(7, 42, "A wrrod here.")));
        let result = worker.recv_timeout(TIMEOUT).expect("a result arrives");
        assert_eq!(result.tab_id, 7);
        assert_eq!(result.revision, 42);
        assert!(result.diagnostics.iter().any(|d| d.flagged == "wrrod"));
        notify_rx.recv_timeout(TIMEOUT).expect("notified");
    }

    #[test]
    fn user_words_update_takes_effect() {
        let worker = Worker::spawn(Vec::new(), || {}).expect("worker spawns");
        assert!(worker.set_user_words(vec!["wrrod".to_owned()]));
        assert!(worker.check(request(1, 1, "A wrrod here.")));
        let result = worker.recv_timeout(TIMEOUT).expect("a result arrives");
        assert!(!result.diagnostics.iter().any(|d| d.flagged == "wrrod"));
    }

    #[test]
    fn queued_requests_for_one_tab_coalesce_to_the_newest() {
        let worker = Worker::spawn(Vec::new(), || {}).expect("worker spawns");
        // The first request may be picked up alone before the rest are
        // queued; whatever is coalesced, the newest revision must be last.
        for revision in 1..=5 {
            assert!(worker.check(request(1, revision, "Some text.")));
        }
        let mut last = 0;
        while last != 5 {
            last = worker
                .recv_timeout(TIMEOUT)
                .expect("a result arrives")
                .revision;
        }
        assert!(
            worker.drain().next().is_none(),
            "no result after the newest"
        );
    }
}
