use color_eyre::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let maybe_path = std::env::args().nth(1).map(PathBuf::from);
    writerm_app::run(maybe_path).await
}
