//! PRISM — Prompt Reduction, Indexing & Semantic Memory
//! Entry point for the `prism` binary.

use anyhow::Result;
use clap::Parser;
use prism::{analytics, cli, mcp, proxy};

#[derive(Parser, Debug)]
#[command(
    name = "prism",
    about = "PRISM — Enterprise Token Optimizer",
    version,
    infer_long_args = true
)]
enum Args {
    Init {
        #[arg(long, default_value_t = false)]
        global: bool,
        #[arg(short, long, default_value_t = false)]
        guide: bool,
        /// Also install PATH shims, so `git`, `ls`, `grep` and ~100 other commands
        /// are filtered automatically.
        ///
        /// Off by default. This rewrites every shell rc file you have and puts prism
        /// in front of core system utilities — including the ones you would need to
        /// diagnose prism. `prism shim install --path` does the same thing later, and
        /// `prism shim uninstall` reverses it.
        #[arg(long, default_value_t = false)]
        shims: bool,
        /// Also add the PRISM CA to your browser and Electron trust stores.
        ///
        /// Off by default. This is a man-in-the-middle root: it is only needed once
        /// traffic is actually routed through the proxy, and installing it
        /// unattended is not something an installer should decide for you.
        #[arg(long, default_value_t = false)]
        trust_ca: bool,
    },
    /// What has prism actually enabled on this machine? Shims, CA, MCP, hub.
    Status,
    Guide {
        #[arg(value_name = "TOPIC")]
        topic: Option<String>,
    },
    Gain {
        #[arg(long, default_value_t = false)]
        history: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Discover,
    Proxy {
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    /// Audit — or with --remove, delete — every trace of PRISM on this machine.
    ///
    /// Reports by default and changes nothing, including the running processes
    /// that are still holding PRISM environment variables and that no uninstaller
    /// can repair. Exits non-zero while anything remains.
    Uninstall {
        /// Actually remove what was found. Without this the command only reports.
        #[arg(long, default_value_t = false)]
        remove: bool,
        /// With --remove, skip the confirmation prompt.
        #[arg(short, long, default_value_t = false)]
        yes: bool,
        /// List every affected process individually rather than a summary.
        #[arg(short, long, default_value_t = false)]
        verbose: bool,
    },
    Serve {
        #[arg(short, long, default_value_t = 27181)]
        port: u16,
        /// Address to bind the proxy to. Loopback by default.
        ///
        /// This is an intercepting TLS proxy holding a CA private key: bound to
        /// 0.0.0.0 it accepts connections from the entire network and will mint a
        /// certificate for any host the caller asks for. It used to bind 0.0.0.0
        /// unconditionally, with no way to narrow it.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long)]
        upstream: Option<String>,
    },
    Mcp {
        #[arg(short, long, default_value_t = 27182)]
        port: u16,
        /// Serve over stdio instead of HTTP — preferred for a local Claude Code /
        /// same-machine agent. No network surface at all.
        #[arg(long, default_value_t = false)]
        stdio: bool,
        /// Address to bind the HTTP transport to. Loopback-only by default; going
        /// wider (e.g. 0.0.0.0) requires --auth-token or a `prism hub enroll` token.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Bearer token required of HTTP callers. Defaults to the hub agent token
        /// from `prism hub enroll` when bound off-loopback and this is not given.
        #[arg(long)]
        auth_token: Option<String>,
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
    /// Enroll with, and exchange telemetry/policy with, a PRISM Hub
    Hub {
        #[command(subcommand)]
        cmd: cli::HubCmd,
    },
    /// Install PATH shims so every agent's shell commands run through prism
    Shim {
        #[command(subcommand)]
        cmd: cli::ShimCmd,
    },
    Compress {
        #[arg(short, long)]
        string: Option<String>,
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        #[arg(short, long, default_value_t = 0.5)]
        ratio: f64,
    },
    Read {
        path: std::path::PathBuf,
        #[arg(short, long, default_value = "skeleton")]
        mode: String,
        #[arg(long)]
        lines: Option<String>,
        #[arg(short = 'n', long, default_value_t = false)]
        line_numbers: bool,
    },
    Cache {
        #[command(subcommand)]
        cmd: cli::CacheCmd,
    },
    Config {
        #[command(flatten)]
        cmd: cli::ConfigCmd,
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
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
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
        Args::Init {
            global,
            guide,
            shims,
            trust_ca,
        } => cli::init(global, guide, shims, trust_ca).await,
        Args::Status => cli::status().await,
        Args::Guide { topic } => cli::guide(topic).await,
        Args::Gain { history, json } => analytics::show_gains(history, json).await,
        Args::Discover => analytics::discover().await,
        Args::Proxy { cmd } => cli::proxy(cmd).await,
        Args::Uninstall {
            remove,
            yes,
            verbose,
        } => prism::uninstall::run(remove, yes, verbose),
        Args::Serve {
            port,
            bind,
            upstream,
        } => proxy::start_server(&bind, port, upstream).await,
        Args::Mcp {
            port,
            stdio,
            bind,
            auth_token,
        } => mcp::start_mcp_server(port, stdio, bind, auth_token).await,
        Args::Memory { cmd } => cli::memory(cmd).await,
        Args::Graph { cmd } => cli::graph(cmd).await,
        Args::Toon { cmd } => cli::toon(cmd).await,
        Args::Count {
            string,
            file,
            model,
        } => cli::count(string, file, model).await,
        Args::Hook { cmd } => cli::hook(cmd).await,
        Args::Hub { cmd } => cli::hub(cmd).await,
        Args::Shim { cmd } => cli::shim(cmd).await,
        Args::Compress {
            string,
            file,
            ratio,
        } => cli::compress(string, file, ratio).await,
        Args::Read {
            path,
            mode,
            lines,
            line_numbers,
        } => cli::read(path, mode, lines, line_numbers).await,
        Args::Cache { cmd } => cli::cache(cmd).await,
        Args::Config { cmd } => cli::config(cmd).await,
        Args::Vscode { output } => cli::vscode_gen(output).await,
        Args::Cmd { args } => cli::run_command(args).await,
    }
}
