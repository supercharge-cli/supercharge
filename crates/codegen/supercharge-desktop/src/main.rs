use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "supercharge-desktop",
    about = "Supercharge desktop client over ACP stdio. Does not reimplement the agent."
)]
struct Args {
    /// Supercharge binary that implements `agent stdio` (`supercharge` or `supercharge-pager`).
    #[arg(long, env = "SUPERCHARGE_BIN")]
    bin: Option<PathBuf>,
    /// Workspace directory for `session/new`.
    #[arg(long)]
    cwd: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    supercharge_desktop::ui::run(args.bin, args.cwd)
}
