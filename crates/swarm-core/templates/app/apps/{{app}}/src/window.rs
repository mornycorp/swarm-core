//! The window: the session, the optional control socket, the UI's state restored and saved, and the
//! egui host around the UI shell.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;

use {{crate_prefix_ident}}automation::control::{self, ControlArgs};
use {{crate_prefix_ident}}engine::{AppInfo, Session, clip};
use {{crate_prefix_ident}}ui::{Shell, Wake, views::views};

use crate::ui_state;

const USAGE: &str = "usage: {{app}} [--ui-state <file>] [--socket [--yes] [--token-file <path>] [--port <n>]]";

/// Why the window didn't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The command line is wrong: the message ends with the usage.
    Usage(String),
    /// The app couldn't start or run.
    App(String),
}

impl Failure {
    /// The process's exit code: 2 used wrongly, 1 failed.
    pub fn exit_code(&self) -> u8 {
        match self {
            Failure::Usage(_) => 2,
            Failure::App(_) => 1,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Usage(message) | Failure::App(message) => f.write_str(message),
        }
    }
}

/// The arguments the operating system gives, as text: one that isn't Unicode is a wrong use, never a
/// panic.
pub fn args(raw: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, Failure> {
    raw.into_iter()
        .enumerate()
        .map(|(at, arg)| {
            arg.into_string().map_err(|arg| Failure::Usage(format!("argument {} is not Unicode text (`{}`)\n{USAGE}", at + 1, clip(&arg.to_string_lossy()))))
        })
        .collect()
}

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    /// `--ui-state <file>`: where the UI's state is read from at the start and saved to at the end.
    ui_state: Option<PathBuf>,
    /// `--socket` and its flags: the control socket on the window's session.
    socket: Option<ControlArgs>,
}

/// What the command line asks for: the window, its UI state's file with `--ui-state`, and with
/// `--socket` the control socket too.
fn parse(args: &[String]) -> Result<Args, String> {
    let mut words: Vec<&str> = args.iter().map(String::as_str).collect();
    let ui_state = take_ui_state(&mut words)?;
    if words.is_empty() {
        return Ok(Args { ui_state, socket: None });
    }
    if !words.contains(&"--socket") {
        let first = words.first().copied().unwrap_or_default();
        return Err(format!("no option `{}`\n{USAGE}", clip(first)));
    }
    let flags: Vec<&str> = words.into_iter().filter(|w| *w != "--socket").collect();
    let socket = control::parse(&flags).map_err(|e| format!("{e}\n{USAGE}"))?;
    Ok(Args { ui_state, socket: Some(socket) })
}

/// `--ui-state <file>`, taken out of `words`.
fn take_ui_state(words: &mut Vec<&str>) -> Result<Option<PathBuf>, String> {
    let Some(at) = words.iter().position(|w| *w == "--ui-state") else { return Ok(None) };
    let file = words.get(at + 1).copied().filter(|f| !f.starts_with("--")).ok_or_else(|| format!("`--ui-state` needs a file\n{USAGE}"))?;
    words.drain(at..=at + 1);
    Ok(Some(PathBuf::from(file)))
}

/// Runs the window until it closes; the control socket, when asked for, is served meanwhile and
/// stopped after. With `--ui-state`, the UI's state is restored first and saved when the window
/// closes, unless the file couldn't be read (it is then the person's to fix, never overwritten).
pub fn run(args: &[String]) -> Result<(), Failure> {
    let Args { ui_state, socket } = parse(args).map_err(Failure::Usage)?;
    let app = AppInfo { name: env!("CARGO_PKG_NAME").to_owned(), version: env!("CARGO_PKG_VERSION").to_owned() };
    let session = Arc::new(Session::new(app).map_err(|e| Failure::App(e.to_string()))?);
    let save_to = ui_state.filter(|path| match ui_state::restore(&session, path) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("{}: the UI state wasn't read, so the window starts from the default and won't save over it: {why}", env!("CARGO_PKG_NAME"));
            false
        }
    });
    let _server = match socket {
        Some(control_args) => {
            let (server, line) =
                control::start(Arc::clone(&session), &control_args).map_err(|e| Failure::App(format!("{}\nfix: {}", e.message, e.remediation)))?;
            eprintln!("{line}");
            Some(server)
        }
        None => None,
    };
    let ui_session = Arc::clone(&session);
    let create: eframe::AppCreator<'_> = Box::new(move |cc| {
        let ctx = cc.egui_ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        // A `ui.set` from the socket repaints the window, so what an agent sets shows at once.
        ui_session.watch_ui(Arc::clone(&wake)).map_err(|e| e.message)?;
        Ok(Box::new(Window { shell: Shell::new(ui_session, views(), wake) }))
    });
    let ran = eframe::run_native(env!("CARGO_PKG_NAME"), eframe::NativeOptions::default(), create).map_err(|e| Failure::App(e.to_string()));
    if let Some(path) = save_to {
        ui_state::save(&session, &path).map_err(Failure::App)?;
    }
    ran
}

/// The eframe app: the shell draws itself into the root `Ui`.
struct Window {
    shell: Shell,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Args, String> {
        parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn the_window_takes_no_arguments_or_the_socket_flags() {
        assert_eq!(parsed(&[]), Ok(Args::default()));
        let socket = parsed(&["--socket", "--yes", "--port", "5000"]).unwrap().socket.unwrap();
        assert_eq!((socket.yes, socket.port, socket.token_file), (true, Some(5000), None));
        let both = parsed(&["--socket", "--ui-state", "ui.json", "--yes"]).unwrap();
        assert_eq!((both.ui_state, both.socket.map(|s| s.yes)), (Some(PathBuf::from("ui.json")), Some(true)));
        assert_eq!(parsed(&["--ui-state", "ui.json"]).unwrap(), Args { ui_state: Some(PathBuf::from("ui.json")), socket: None });
        for bad in [&["--yes"][..], &["stray"], &["--socket", "--nope"], &["--socket", "--port", "x"], &["--ui-state"], &["--ui-state", "--socket"]] {
            assert!(parsed(bad).unwrap_err().contains("usage:"), "{bad:?}");
            let wrong = run(&bad.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>()).unwrap_err();
            assert_eq!(wrong.exit_code(), 2, "{bad:?}: a wrong use is exit code 2, before any window");
        }
    }

    #[test]
    fn an_argument_that_is_not_unicode_is_wrong_use_not_a_panic() {
        #[cfg(unix)]
        let bad = std::os::unix::ffi::OsStringExt::from_vec(vec![b'a', 0xff]);
        #[cfg(windows)]
        let bad = std::os::windows::ffi::OsStringExt::from_wide(&[u16::from(b'a'), 0xd800]);
        let failure = args([OsString::from("--socket"), bad]).unwrap_err();
        assert_eq!(failure.exit_code(), 2);
        assert!(failure.to_string().contains("argument 2 is not Unicode") && failure.to_string().contains("usage:"), "{failure}");
        assert_eq!(args([OsString::from("--socket")]), Ok(vec!["--socket".to_owned()]));
    }
}
