mod app;
mod clipboard;
mod config;
mod editor;
mod evaluator;
mod keymap;
mod output;
mod protocol;
mod ui;

use std::io::Write;

fn main() {
    match run() {
        Ok(exit) => {
            if let Some(command) = exit.command {
                print!("{command}");
                let _ = std::io::stdout().flush();
            }
            std::process::exit(exit.code);
        }
        Err(error) => {
            eprintln!("silk: {error:#}");
            std::process::exit(1);
        }
    }
}

fn run() -> anyhow::Result<app::AppExit> {
    let mut app = app::App::new()?;
    app.run()
}
