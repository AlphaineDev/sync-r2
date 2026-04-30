use anyhow::Result;
use syncr2::cli::{run_cli, Cli};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse_args();
    run_cli(cli).await
}
