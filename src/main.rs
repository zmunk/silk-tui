mod app;
mod clipboard;
mod config;
mod editor;
mod evaluator;
mod keymap;
mod output;
mod protocol;
mod terminal;
mod ui;

fn main() {
    protocol::install_panic_hook();
    match run() {
        Ok(exit) => {
            if let Err(error) = protocol::write_command(std::io::stdout(), exit.command.as_deref())
            {
                eprintln!("silk: failed to write command: {error}");
                std::process::exit(1);
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
    let query = protocol::parse_args()?;
    let mut app = app::App::new(&query)?;
    app.run()
}
