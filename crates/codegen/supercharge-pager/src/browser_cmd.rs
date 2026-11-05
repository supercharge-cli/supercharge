//! `supercharge browser` — install, status, and uninstall the optional local
//! headless browser binary used by browser tools.

use clap::Subcommand;

#[derive(Debug, clap::Args, Clone)]
pub struct BrowserArgs {
    #[command(subcommand)]
    pub command: BrowserCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum BrowserCommand {
    /// Download the platform browser binary into the Supercharge home
    /// (release builds from Obscura; Apache-2.0).
    Install,
    /// Show whether the browser binary is installed
    Status,
    /// Remove the installed browser binary and its skill copy
    Uninstall,
}

pub async fn run(args: BrowserArgs) -> anyhow::Result<()> {
    match args.command {
        BrowserCommand::Install => {
            let path = supercharge_tools::browser::install_latest().await?;
            println!("installed {}", path.display());
            println!("{}", supercharge_tools::browser::status_report());
        }
        BrowserCommand::Status => {
            print!("{}", supercharge_tools::browser::status_report());
            if !supercharge_tools::browser::status_report().ends_with('\n') {
                println!();
            }
        }
        BrowserCommand::Uninstall => {
            supercharge_tools::browser::uninstall()?;
            let skill = supercharge_dirs::supercharge_home()
                .join("skills")
                .join("browser");
            let _ = std::fs::remove_dir_all(skill);
            println!("browser uninstalled");
        }
    }
    Ok(())
}
