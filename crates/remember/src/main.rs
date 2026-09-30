mod config;
mod server;
mod setup;
mod tools;
mod update;
mod wake;

use std::io::Write;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use remember_core::{Hub, Scope, toon};
use rmcp::ServiceExt;

use crate::server::RememberServer;

/// Shared memory and messaging for coding agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server over stdio (what agents spawn).
    Serve,
    /// Register Remember as an MCP server in an agent's config.
    Setup {
        agent: setup::AgentKind,
        /// Apply without asking for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// List memories, newest first.
    Ls {
        #[arg(long, value_parser = parse_scope)]
        scope: Option<Scope>,
        /// Only memories whose project identity contains this text.
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = Format::Toon)]
        format: Format,
    },
    /// Show one memory with its revisions.
    Show {
        id: String,
        #[arg(long, value_enum, default_value_t = Format::Toon)]
        format: Format,
    },
    /// Dump every memory and task.
    Export {
        #[arg(long, value_enum, default_value_t = Format::Toon)]
        format: Format,
    },
    /// Delete a memory by id.
    Forget { id: String },
    /// Replace this binary with the latest release (Linux and macOS).
    #[command(visible_alias = "upgrade")]
    Update {
        /// Reinstall even when already on the latest release.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Toon,
    Md,
}

fn parse_scope(value: &str) -> Result<Scope, String> {
    Scope::parse(value).map_err(|e| e.to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve => serve(),
        Command::Setup { agent, yes } => setup::run(agent, yes),
        Command::Update { force } => update::run(force),
        Command::Ls {
            scope,
            project,
            limit,
            format,
        } => {
            let memories = open()?.list_memories(scope, project.as_deref(), limit)?;
            match format {
                Format::Toon => print(&toon::encode(&serde_json::json!({ "memories": memories }))?),
                Format::Md => print(&config::markdown_memories(&memories)),
            }
        }
        Command::Show { id, format } => {
            let detail = open()?.show_memory(&id)?;
            match format {
                Format::Toon => print(&toon::encode(&detail)?),
                Format::Md => print(&config::markdown_detail(&detail)),
            }
        }
        Command::Export { format } => {
            let export = open()?.export()?;
            match format {
                Format::Toon => print(&toon::encode(&export)?),
                Format::Md => print(&config::markdown_export(&export)),
            }
        }
        Command::Forget { id } => {
            open()?.delete_memory(&id)?;
            print("ok")
        }
    }
}

fn open() -> Result<Hub> {
    let path = config::db_path()?;
    Hub::open(&path, config::limits()).with_context(|| format!("opening {}", path.display()))
}

fn print(text: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}")?;
    Ok(())
}

fn serve() -> Result<()> {
    let hub = open()?;
    hub.prune()?;
    let cwd = std::env::current_dir().context("reading cwd")?;
    // One Agent per process needs no thread pool; a single thread keeps RSS and startup low.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async move {
        // Clients stop servers by closing stdin or by signal; either way the
        // Agent must be marked offline so others stop messaging it. Handlers
        // are installed first so a signal during the handshake is caught too.
        let terminated = termination()?;
        tokio::pin!(terminated);
        let server = RememberServer::new(hub, &cwd);
        let service = tokio::select! {
            service = server.clone().serve(rmcp::transport::stdio()) => {
                service.context("MCP handshake failed")?
            }
            () = &mut terminated => {
                server.shutdown();
                return Ok(());
            }
        };
        tokio::select! {
            _ = service.waiting() => {}
            () = &mut terminated => {}
        }
        server.shutdown();
        Ok(())
    });
    // Stdin is read on a blocking thread that never returns while the client
    // keeps the pipe open (e.g. after a signal); do not wait for it.
    runtime.shutdown_background();
    result
}

/// Resolves on SIGTERM, SIGINT or SIGHUP. Handlers are registered on call.
#[cfg(unix)]
fn termination() -> Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;
    Ok(async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
    })
}

#[cfg(not(unix))]
fn termination() -> Result<impl std::future::Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}
