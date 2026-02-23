use clap::Parser;
use tracing_subscriber::EnvFilter;

use relais_cli::{Cli, Commands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Sites => relais_cli::commands::sites::run()?,
        Commands::Apis { site } => relais_cli::commands::apis::run(&site)?,
        Commands::Spec { path } => relais_cli::commands::spec::run(&path)?,
        Commands::Exec { path, data } => {
            relais_cli::commands::exec::run(&path, data.as_deref()).await?;
        }
        Commands::Serve { port, jwt_secret } => {
            relais_cli::commands::serve::run(port, jwt_secret).await?;
        }
        Commands::Vault { action } => relais_cli::commands::vault::run(&action)?,
    }

    Ok(())
}
