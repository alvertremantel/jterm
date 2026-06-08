pub mod app;
pub mod draw;

pub use app::WritermApp;

use color_eyre::Result;
use std::path::PathBuf;
use writerm_config::Config;

pub async fn run(maybe_path: Option<PathBuf>) -> Result<()> {
    jones_terminal::install_panic_hook();
    let config = Config::load()?;
    let mut terminal = jones_terminal::setup_terminal_with_mouse(config.ui.mouse)?;
    let mut app = WritermApp::with_config(maybe_path, config)?;
    let result = app.run(&mut terminal).await;
    jones_terminal::restore_terminal(&mut terminal)?;
    result
}
