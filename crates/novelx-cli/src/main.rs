use anyhow::Result;
use clap::{Parser, Subcommand};
use novelx_llm::{load_llm_config, LlmClient};
use novelx_pipeline::{
    init_project, list_projects, load_project_state, migrate_project_reader_formats,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "novel", about = "NovelX — multi-agent novel creation (Rust)")]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,

    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a novel project
    Init {
        name: String,
        #[arg(long, default_value = "未定")]
        genre: String,
        #[arg(long, default_value_t = 100)]
        chapters: u32,
    },
    /// Run chapter pipeline via Codex Session + SubAgent spawn chain
    Run {
        name: String,
        chapter: u32,
        /// Revise existing draft (local patch preferred)
        #[arg(long)]
        revise: bool,
        #[arg(long)]
        instructions: Option<String>,
    },
    /// Show project status
    Status { name: String },
    /// List projects
    Projects,
    /// List registered pipeline agents
    Agents,
    /// Migrate a project toward locked reader/content-formats schemas
    Migrate {
        /// Project directory name under projects/
        name: String,
    },
    /// Start NovelX web server
    Web {
        #[arg(long, default_value = "127.0.0.1:8765")]
        bind: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let cli = Cli::parse();
    let root = std::fs::canonicalize(&cli.root).unwrap_or(cli.root);
    let _ = load_dotenv_from(&root.join(".env"));
    let _ = load_dotenv_from(&PathBuf::from(".env"));
    let projects = root.join("projects");
    let config = root.join("config");

    match cli.cmd {
        Commands::Init {
            name,
            genre,
            chapters,
        } => {
            let dir = init_project(&projects, &name, &genre, chapters)?;
            println!("created {}", dir.display());
        }
        Commands::Run {
            name,
            chapter,
            revise,
            instructions,
        } => {
            let llm_cfg = load_llm_config(&config.join("llm.yaml"))?;
            let llm = Arc::new(LlmClient::new(llm_cfg));
            let core = novelx_core::NovelxCore::new(projects, config, llm);
            let run = core
                .run_chapter_headless(&name, chapter, revise, instructions)
                .await?;
            println!("{}", serde_json::to_string_pretty(&run)?);
        }
        Commands::Status { name } => {
            let state = load_project_state(&projects.join(&name))?;
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Commands::Projects => {
            for n in list_projects(&projects)? {
                println!("{n}");
            }
        }
        Commands::Agents => {
            for a in novelx_harness::PipelineConfig::load(&config).order() {
                println!("{a}");
            }
        }
        Commands::Migrate { name } => {
            let dir = projects.join(&name);
            if !dir.is_dir() {
                anyhow::bail!("项目不存在：{}", dir.display());
            }
            let report = migrate_project_reader_formats(&dir);
            println!("migrated project: {name}");
            if !report.outlines_migrated.is_empty() {
                println!("  outlines→json: {:?}", report.outlines_migrated);
            }
            if !report.outlines_padded.is_empty() {
                println!("  outlines padded items/locations: {:?}", report.outlines_padded);
            }
            if !report.entities_stub_folded.is_empty() {
                println!(
                    "  entity sync stubs folded: {}",
                    report.entities_stub_folded.len()
                );
            }
            if !report.entities_padded.is_empty() {
                println!("  entities normalized: {}", report.entities_padded.len());
            }
            if !report.plots_normalized.is_empty() {
                println!("  plots normalized: {:?}", report.plots_normalized);
            }
            if report.bible_rewritten {
                println!("  bible.md restructured");
            }
            for n in &report.notes {
                println!("  note: {n}");
            }
            for (ch, err) in &report.outlines_failed {
                println!("  outline fail ch{ch}: {err}");
            }
            for (p, err) in &report.plot_failed {
                println!("  plot fail {p}: {err}");
            }
        }
        Commands::Web { bind } => {
            let addr: SocketAddr = bind.parse()?;
            novelx_app_server::serve(root, addr).await?;
        }
    }
    Ok(())
}

fn load_dotenv_from(path: &std::path::Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    for line in std::fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if k.is_empty() {
                continue;
            }
            if std::env::var(k).is_err() {
                // SAFETY: process startup, before worker threads
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
    Ok(())
}
