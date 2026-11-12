//! Workers: a call made off the UI thread, and the message that brings its result back. The UI
//! never blocks on a call (docs/app-architecture.md › The UI is a client): [`Jobs::start`] returns at
//! once, the worker runs the view's typed call and sends the result, and the next frame
//! [`Jobs::poll`]s it. The worker wakes the UI when it is done, so the result is drawn without
//! waiting for input.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use {{crate_prefix_ident}}engine::{CallOpts, CancelToken, CommandError, Session};

use crate::catalog;
use crate::views::{Shown, View};

/// What the worker calls when it has sent a result: the app gives one that repaints the window
/// (`egui::Context::request_repaint`), so this crate's state needs no toolkit.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A call that has finished: which one, and what it gave.
#[derive(Debug)]
pub struct Finished {
    /// The number [`Jobs::start`] gave the call.
    pub call: u64,
    pub id: &'static str,
    pub result: Result<Shown, CommandError>,
}

/// A call started: its number and the token that cancels it.
#[derive(Debug, Clone)]
pub struct Started {
    pub call: u64,
    pub cancel: CancelToken,
}

/// Starts calls on worker threads and collects their results.
pub struct Jobs {
    session: Arc<Session>,
    wake: Wake,
    sender: Sender<Finished>,
    receiver: Receiver<Finished>,
    started: u64,
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jobs").field("started", &self.started).finish_non_exhaustive()
    }
}

impl Jobs {
    pub fn new(session: Arc<Session>, wake: Wake) -> Jobs {
        let (sender, receiver) = mpsc::channel();
        Jobs { session, wake, sender, receiver, started: 0 }
    }

    /// Runs `view` with `opts` on a worker thread and returns at once. The result comes back through
    /// [`Jobs::poll`]. `internal` when the thread can't be made, and as the result of a view that
    /// panics, so the shell never waits for a result that won't come.
    pub fn start(&mut self, view: View, opts: CallOpts) -> Result<Started, CommandError> {
        self.started += 1;
        let call = self.started;
        let cancel = opts.cancel.clone();
        let (session, sender, wake) = (Arc::clone(&self.session), self.sender.clone(), Arc::clone(&self.wake));
        thread::Builder::new()
            .name("ui-call".to_owned())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| (view.run)(&session, &opts)))
                    .unwrap_or_else(|_| Err(CommandError::internal(catalog::view_panicked(view.id))));
                // The window may be gone, and then nobody wants the result.
                let _ = sender.send(Finished { call, id: view.id, result });
                wake();
            })
            .map_err(|e| CommandError::internal(catalog::no_worker(view.id, &e.to_string())))?;
        Ok(Started { call, cancel })
    }

    /// Every result that has arrived since the last look, without waiting for one.
    pub fn poll(&self) -> Vec<Finished> {
        self.receiver.try_iter().collect()
    }
}
