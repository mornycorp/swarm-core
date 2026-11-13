//! The cancel token: what a call's caller sets, and what a slow call polls.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by a dropped connection, a deadline or the UI; a slow command polls it. Clones share one
/// flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
