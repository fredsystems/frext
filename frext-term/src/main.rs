//! frext-term binary entry point.

use std::process::ExitCode;

use frext_core::clipboard::SystemClipboard;
use frext_core::editor::Editor;
use frext_core::persistence::{Profile, Store};
use frext_term::{cli, logging};

fn main() -> ExitCode {
    let args = cli::parse(std::env::args_os().skip(1));

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

    let editor = Editor::open(
        store,
        Box::new(SystemClipboard::new()),
        &args.files,
        args.dir.as_deref(),
    );
    editor.shutdown();

    ExitCode::SUCCESS
}
