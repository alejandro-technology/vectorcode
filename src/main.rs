use clap::Parser;

use vectorcode::cli::{init_tracing, resolve_project_path, Cli, Commands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing (logs to stderr)
    init_tracing(cli.verbose, cli.quiet);

    tracing::info!("vectorcode v{}", env!("CARGO_PKG_VERSION"));

    let project_path = resolve_project_path(cli.project_path.as_ref());

    match &cli.command {
        Commands::Init(args) => {
            vectorcode::cli::init::execute(args, &project_path, cli.quiet).await?;
        }
        Commands::Index(args) => {
            vectorcode::cli::index::execute(args, &project_path, cli.quiet).await?;
        }
        Commands::Search(args) => {
            vectorcode::cli::search::execute(args, &project_path, cli.quiet).await?;
        }
        Commands::Status(args) => {
            vectorcode::cli::status::execute(args, &project_path)?;
        }
        Commands::Serve(args) => {
            vectorcode::cli::serve::execute(args, &project_path).await?;
        }
        Commands::Install(args) => {
            vectorcode::cli::install::execute(args)?;
        }
        Commands::Uninstall(args) => {
            vectorcode::cli::uninstall::execute(args)?;
        }
        Commands::Upgrade(args) => {
            vectorcode::cli::upgrade::execute(args)?;
        }
    }

    Ok(())
}
