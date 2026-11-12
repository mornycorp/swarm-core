//! The app's own lock (docs/app-architecture.md › Never hang on a lock): a caller that can't take
//! it within its deadline gets an `internal` error that names it, never a wait for ever.
//! `std::sync::Mutex` waits for ever on a deadlock and says nothing, which is why `clippy.toml`
//! refuses it and points here.
//!
//! - **Held only inside [`Lock::with`]**: there is no guard to keep, so a lock can't be held
//!   across a call, a wait or another statement by accident.
//! - **One lock a statement.** `with` takes one lock; a closure that takes a second while it
//!   holds the first is the mistake this rule is about, and the deadline is what turns it from a
//!   frozen app into an error that names the lock.
//! - **A poisoned lock is taken all the same**: the dispatch turns a panic into an error
//!   ([`Session::call`](crate::Session::call)), so what a lock guards must be left whole by each
//!   change, and is.

use std::fmt;
use std::sync::{RwLock, TryLockError};
use std::time::{Duration, Instant};

use crate::CommandError;

/// How long a caller waits for a lock before it calls it a deadlock.
pub const DEADLINE: Duration = Duration::from_secs(2);

/// How often a waiting caller looks again.
const STEP: Duration = Duration::from_millis(1);

/// A value behind a lock with a name and a deadline.
pub struct Lock<T> {
    /// What it guards, as its error says it: `the UI state`.
    name: &'static str,
    deadline: Duration,
    held: RwLock<T>,
}

impl<T> fmt::Debug for Lock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lock").field("name", &self.name).finish_non_exhaustive()
    }
}

impl<T> Lock<T> {
    /// `value` behind a lock called `name`, waited for at most [`DEADLINE`].
    pub fn new(name: &'static str, value: T) -> Lock<T> {
        Lock::with_deadline(name, value, DEADLINE)
    }

    /// [`Lock::new`], waited for at most `deadline`.
    pub fn with_deadline(name: &'static str, value: T, deadline: Duration) -> Lock<T> {
        Lock { name, deadline, held: RwLock::new(value) }
    }

    /// `f` with the lock held, and what it returns. An `internal` error naming the lock when it
    /// can't be taken within its deadline.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, CommandError> {
        let start = Instant::now();
        loop {
            match self.held.try_write() {
                Ok(mut held) => return Ok(f(&mut held)),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(f(&mut poisoned.into_inner())),
                Err(TryLockError::WouldBlock) if start.elapsed() < self.deadline => std::thread::sleep(STEP),
                Err(TryLockError::WouldBlock) => {
                    return Err(CommandError::internal(format!("{}'s lock was held for over {} s: a deadlock", self.name, self.deadline.as_secs_f32())));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;
    use std::sync::Arc;
    use std::sync::mpsc;

    #[test]
    fn the_value_is_read_and_changed_under_the_lock() {
        let lock = Lock::new("the counter", 1);
        assert_eq!(lock.with(|n| *n += 1), Ok(()));
        assert_eq!(lock.with(|n| *n), Ok(2));
        assert!(format!("{lock:?}").contains("the counter"));
    }

    /// A second lock taken in the statement that holds the first, by two threads in the two
    /// orders, is the deadlock this lock is for: each gives up at its deadline with an error that
    /// names the lock it wanted, and neither waits for ever.
    #[test]
    fn a_deadlock_is_an_error_that_names_the_lock_not_a_hang() {
        let deadline = Duration::from_millis(150);
        let a = Arc::new(Lock::with_deadline("the first", 0, deadline));
        let b = Arc::new(Lock::with_deadline("the second", 0, deadline));
        let (ready, go) = mpsc::channel();
        let other = {
            let (a, b) = (Arc::clone(&a), Arc::clone(&b));
            std::thread::spawn(move || {
                b.with(|_| {
                    let _ = ready.send(());
                    std::thread::sleep(Duration::from_millis(50));
                    a.with(|_| ()).map_err(|e| e.message)
                })
            })
        };
        let started = Instant::now();
        let mine = a.with(|_| {
            let _ = go.recv();
            b.with(|_| ()).map_err(|e| e.message)
        });
        let theirs = other.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        let errors: Vec<String> = [mine, theirs].into_iter().flat_map(|taken| taken.ok()).filter_map(Result::err).collect();
        assert!(!errors.is_empty() && errors.iter().all(|e| e.contains("lock was held for over") && e.contains("a deadlock")), "{errors:?}");
        assert!(errors.iter().any(|e| e.starts_with("the second's") || e.starts_with("the first's")), "{errors:?}");
    }

    #[test]
    fn a_lock_held_too_long_fails_internal() {
        let lock = Arc::new(Lock::with_deadline("the slow thing", (), Duration::from_millis(50)));
        let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let lock = Arc::clone(&lock);
            std::thread::spawn(move || {
                let _ = lock.with(|()| {
                    let _ = held.0.send(());
                    let _ = release.1.recv();
                });
            })
        };
        let _ = held.1.recv();
        let e = lock.with(|()| ()).unwrap_err();
        assert_eq!(e.code, ErrorCode::Internal);
        assert!(e.message.starts_with("the slow thing's lock was held for over 0.05 s"), "{e}");
        let _ = release.0.send(());
        let _ = holder.join();
        assert_eq!(lock.with(|()| ()), Ok(()), "free again once it is let go");
    }
}
