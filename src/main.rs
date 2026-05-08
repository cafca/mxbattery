use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open (or focus) the preferences window. If no daemon is running, run a one-shot prefs UI.
    Prefs,
    /// Install the LaunchAgent. Pass the absolute path to the bundled `MXBattery.app`.
    Install { app: String },
    /// Remove the LaunchAgent. With --purge, also delete config + state.
    Uninstall {
        #[arg(long)]
        purge: bool,
    },
    /// Print the current battery + charging state and exit.
    Read,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        None => mxbattery::run(),
        Some(Cmd::Prefs) => mxbattery::run_prefs_subcommand(),
        Some(Cmd::Install { app }) => mxbattery::launchd::install(&app),
        Some(Cmd::Uninstall { purge }) => mxbattery::launchd::uninstall(purge),
        Some(Cmd::Read) => mxbattery::run_read_subcommand(),
    }
}
