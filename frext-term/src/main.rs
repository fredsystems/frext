//! frext-term binary entry point.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use frext_core::config::Config;
use frext_core::editor::Editor;
use frext_core::persistence::{Profile, Store};
use frext_term::app::TermApp;
use frext_term::clipboard::{ClipboardChoice, make_clipboard};
use frext_term::events::{EventLoop, TICK};
use frext_term::term::caps::IconChoice;
use frext_term::terminal::TerminalGuard;
use frext_term::{cli, logging, run};

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("frext-term: {err}");
            return ExitCode::FAILURE;
        }
    };

    let store = match Store::new(Profile::Term) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("frext-term: failed to initialise persistence store: {err}");
            return ExitCode::FAILURE;
        }
    };

    let log_path = store.root().join("frext-term.log");
    let level = logging::parse_level(std::env::var("RUST_LOG").ok().as_deref());
    if let Err(err) = logging::init(&log_path, level) {
        eprintln!(
            "frext-term: warning: logging disabled, cannot open {}: {err}",
            log_path.display()
        );
    }

    let settings = args.apply_to(Config::load().term);

    let choice = ClipboardChoice::from_env(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        std::env::var_os("DISPLAY").as_deref(),
    );
    log::info!("using the {choice:?} clipboard");

    let editor = Editor::open(
        store,
        make_clipboard(choice),
        &args.files,
        args.dir.as_deref(),
    );

    let base = match &args.dir {
        Some(dir) => std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone()),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);

    let (guard, mut terminal) = match TerminalGuard::enter(settings) {
        Ok(entered) => entered,
        Err(err) => {
            log::error!("failed to set up the terminal: {err}");
            editor.shutdown();
            eprintln!("frext-term: failed to set up the terminal: {err}");
            return ExitCode::FAILURE;
        }
    };
    log::info!(
        "icons: {:?}",
        IconChoice::select(settings.icons, guard.caps())
    );

    let mut app = TermApp::new(editor, guard.mode(), base, home);

    // Spawned after `enter`, so the terminal probe is not raced by the
    // input reader.
    let events = EventLoop::new();
    let spawned = events
        .spawn_input(crossterm::event::read)
        .and_then(|()| events.spawn_tick(TICK));
    if let Err(err) = spawned {
        log::error!("failed to start the event threads: {err}");
        app.shutdown();
        drop(terminal);
        drop(guard);
        eprintln!("frext-term: failed to start the event threads: {err}");
        return ExitCode::FAILURE;
    }

    let result = run::run(&mut app, &mut terminal, &events, Instant::now);
    app.shutdown();
    drop(terminal);
    drop(guard);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            log::error!("the UI loop failed: {err}");
            eprintln!("frext-term: {err}");
            ExitCode::FAILURE
        }
    }
}
