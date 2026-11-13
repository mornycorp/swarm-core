//! What a caller brings to a call: who it is, how to cancel it and whether it has agreed to the
//! command's effects (docs/app-architecture.md › One dispatch).

pub use {{crate_prefix_ident}}backends::CancelToken;

use crate::error::CommandError;

/// Which surface made the call: logged, never branched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Cli,
    Serve,
    Socket,
    Mcp,
    Ui,
    Test,
}

/// What a caller brings to [`Session::call`](crate::Session::call).
#[derive(Debug, Clone)]
pub struct CallOpts {
    pub caller: Caller,
    pub cancel: CancelToken,
    /// The caller has agreed to the command's effects.
    pub consent: bool,
}

impl CallOpts {
    /// A call by `caller`, with no consent given and nothing cancelled.
    pub fn new(caller: Caller) -> CallOpts {
        CallOpts { caller, cancel: CancelToken::default(), consent: false }
    }

    /// The same, with the caller's consent given.
    #[must_use]
    pub fn consenting(mut self) -> CallOpts {
        self.consent = true;
        self
    }
}

/// What a running command is told about its call.
#[derive(Debug, Clone)]
pub struct CallContext {
    pub caller: Caller,
    pub cancel: CancelToken,
}

impl CallContext {
    /// `cancelled`, once the call's token is set: a slow command polls this between steps.
    pub fn check_cancelled(&self) -> Result<(), CommandError> {
        if self.cancel.is_cancelled() { Err(CommandError::cancelled()) } else { Ok(()) }
    }
}
