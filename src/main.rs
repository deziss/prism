//! PRISM — Personal Reasoning & Intelligence System for Models
//! Entry point for the `prism` binary.

use anyhow::Result;
use clap::Parser;
use prism::{analytics, cli, mcp, proxy};

#[derive(Parser, Debug)]
#[command(name = "prism", about = "PRISM — Enterprise Token Optimizer", version, infer_long_args = true)]
enum Args {
    Init {
        #[arg(long, default_value_t = false)]
        global: bool,
    },
    Gain {
        #[arg(long, default_value_t = false)]
        history: bool,
    },
    Discover,
    Proxy {
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    Serve {
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
        #[arg(long)]
        upstream: Option<String>,
    },
    Mcp {
        #[arg(short, long, default_value_t = 3003)]
        port: u16,
    },
    Memory {
        #[command(subcommand)]
        cmd: cli::MemoryCmd,
    },
    Graph {
        #[command(subcommand)]
        cmd: cli::GraphCmd,
    },
    Toon {
        #[command(subcommand)]
        cmd: cli::ToonCmd,
    },
    Count {
        #[arg(short, long)]
        string: Option<String>,
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = String::from("gpt-4"))]
        model: String,
    },
    Hook {
        #[command(subcommand)]
        cmd: cli::HookCmd,
    },
    Compress {
        #[arg(short, long)]
        string: Option<String>,
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        #[arg(short, long, default_value_t = 0.5)]
        ratio: f64,
    },
    Vscode {
        #[arg(short, long, default_value = "prism-vscode")]
        output: std::path::PathBuf,
    },
    #[allow(non_camel_case_types)]
    Cmd {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

/// Long-running servers log to stderr; one-shot CLI commands stay quiet so
/// their output remains pipeable. Override with RUST_LOG.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;
    let default = if verbose { "info" } else { "warn" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

#[tokio::main]
async fn main() -> Result<()> {
    prism::init_prism_dirs()?;

    let args = Args::parse();
    init_tracing(matches!(args, Args::Serve { .. } | Args::Mcp { .. }));
    match args {
        Args::Init { global } => cli::init(global).await,
        Args::Gain { history } => analytics::show_gains(history).await,
        Args::Discover => analytics::discover().await,
        Args::Proxy { cmd } => cli::proxy(cmd).await,
        Args::Serve { port, upstream } => proxy::start_server(port, upstream).await,
        Args::Mcp { port } => mcp::start_mcp_server(port).await,
        Args::Memory { cmd } => cli::memory(cmd).await,
        Args::Graph { cmd } => cli::graph(cmd).await,
        Args::Toon { cmd } => cli::toon(cmd).await,
        Args::Count { string, file, model } => cli::count(string, file, model).await,
        Args::Hook { cmd } => cli::hook(cmd).await,
        Args::Compress { string, file, ratio } => cli::compress(string, file, ratio).await,
        Args::Vscode { output } => cli::vscode_gen(output).await,
        Args::Cmd { args } => cli::run_command(args).await,
    }
}
