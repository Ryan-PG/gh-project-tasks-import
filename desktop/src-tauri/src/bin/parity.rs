//! Parity oracle: print exactly what the Python CLI would print.
//!
//! `import_tasks.py` is the reference implementation. This binary is the Rust
//! side of the oracle: it reads the same files, applies the same settings
//! precedence, and writes to stdout and stderr in the same bytes, so
//! `scripts/parity_oracle.py` can diff the two with no normalisation.
//!
//! It drives the same `tasks` module the desktop app uses, so a green oracle
//! is evidence about the shipped code rather than about a copy of it.
//!
//! Usage: `parity <validate|preview> [tasks.json] [--dotenv <path>]`

use github_importer_lib::tasks::{self, Settings};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut command: Option<String> = None;
    let mut tasks_path: Option<String> = None;
    let mut dotenv_path: Option<String> = None;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dotenv" => dotenv_path = it.next(),
            other if command.is_none() => command = Some(other.to_string()),
            other => tasks_path = Some(other.to_string()),
        }
    }

    let command = match command.as_deref() {
        Some(c @ ("validate" | "preview")) => c.to_string(),
        _ => {
            eprintln!("usage: parity <validate|preview> [tasks.json] [--dotenv <path>]");
            return ExitCode::from(2);
        }
    };

    // `main()` prints the settings banner before it loads anything else, and
    // `load_settings()` writes its invalid-value warnings to stderr as it goes.
    let dotenv = dotenv_path
        .as_deref()
        .map(std::fs::read_to_string)
        .transpose()
        .map_err(|e| format!("cannot read dotenv: {e}"))
        .map(|t| t.map(|t| tasks::parse_dotenv(&t)).unwrap_or_default());

    let dotenv = match dotenv {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let settings: Settings = tasks::load_settings(&dotenv, |k| std::env::var(k).ok());
    eprint!("{}", settings.warnings_text());
    print!("{}", tasks::settings_banner(&settings));

    let text = match tasks_path.as_deref().map(std::fs::read_to_string) {
        Some(Ok(t)) => t,
        Some(Err(e)) => {
            // Python: `Cannot read {path}: {e}`, printed by `SystemExit`.
            println!("Cannot read {}: {e}", tasks_path.unwrap_or_default());
            return ExitCode::FAILURE;
        }
        None => {
            println!("Cannot read tasks.json: no path given");
            return ExitCode::FAILURE;
        }
    };

    let raw = match tasks::parse_raw(&text) {
        Ok(v) => v,
        Err(e) => {
            println!("Cannot read {}: {e}", tasks_path.unwrap_or_default());
            return ExitCode::FAILURE;
        }
    };

    let report = tasks::validate(&raw);
    print!("{}", report.to_cli_output());
    if !report.ok {
        return ExitCode::FAILURE;
    }

    if command == "preview" {
        for t in raw.iter().filter_map(tasks::to_task) {
            println!("{}", tasks::preview_line(&t));
        }
    }

    ExitCode::SUCCESS
}
