mod app;
mod clipboard;
mod config;
mod editor;
mod evaluator;
mod keymap;
mod output;
mod protocol;
mod ui;

fn main() -> anyhow::Result<()> {
    let mut app = app::App::new();
    app.run()
}
