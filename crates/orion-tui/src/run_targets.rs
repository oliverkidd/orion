//! RUN TARGETS for `⌘⇧S`: what can be started in a checkout, found by
//! `orion_core::run_targets` — the same reading the DAEMON's stack command
//! check uses. Here only [`found`], which a test can stub, since the
//! tests' checkouts are paths with nothing in them.

pub use orion_core::run_targets::{detect, Target, NOTHING};
use std::path::Path;

/// What `⌘⇧S` offers in the checkout at `dir`: [`detect`] — or, under
/// test, what [`stub`] set (one `just start` unless a test says else).
pub fn found(dir: &Path, name: &str) -> Vec<Target> {
    #[cfg(test)]
    if let Some(targets) = STUB.with(|s| s.borrow().clone()) {
        return targets;
    }
    detect(dir, name)
}

#[cfg(test)]
thread_local! {
    static STUB: std::cell::RefCell<Option<Vec<Target>>> =
        std::cell::RefCell::new(Some(vec![Target::new("app", "just start")]));
}

/// Under test: what [`found`] answers on this thread — None for the
/// real [`detect`].
#[cfg(test)]
pub fn stub(targets: Option<Vec<Target>>) {
    STUB.with(|s| *s.borrow_mut() = targets);
}
