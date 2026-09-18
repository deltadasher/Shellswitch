mod adapters;
mod control;
mod discovery;
mod generic;
mod lifecycle;
mod model;
mod niri;
mod ownership;
mod process;
mod ui;
mod wayland;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    version,
    about = "Discover desktop shells from evidence; preview and trial controlled switches"
)]
struct Cli {
    /// Additional scan roots (repeatable). Symlinks are canonicalized and deduplicated.
    #[arg(long, global = true)]
    root: Vec<PathBuf>,
    /// Scan only supplied roots, useful for repositories and isolated tests.
    #[arg(long, global = true)]
    isolated: bool,
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    /// Interactive three-pane browser (default)
    Tui,
    /// Read-only discovery, with evidence and launch methods
    Scan {
        #[arg(long)]
        json: bool,
    },
    /// Inspect compatibility and exact switch sequence without launching
    Plan {
        id: String,
    },
    /// Start a 20-second trial; use keep to commit or revert to restore
    Switch {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Stage/register a revision without changing configuration or starting it
    Install {
        manifest: PathBuf,
        #[arg(long)]
        payload: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    /// Enroll a user-owned Niri configuration; explicit precedence is required
    Enroll {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        user_config: Option<PathBuf>,
        #[arg(long, value_enum)]
        policy: ownership::Policy,
        /// Fixture/offline mode: validate files but do not contact a live compositor
        #[arg(long)]
        offline: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Import an edited user baseline transactionally without changing selection
    ConfigUpdate {
        #[arg(long)]
        user_config: PathBuf,
        #[arg(long, value_enum)]
        policy: ownership::Policy,
        #[arg(long)]
        yes: bool,
    },
    /// Install registered CLI, autostart and service gates without starting a shell
    Protect {
        #[arg(long)]
        yes: bool,
    },
    /// Explain configuration/lifecycle drift and immediate repair actions
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Read-only registered and statically observed startup-source inventory
    Inventory,
    /// Generate a reviewed incident-shell bridge; does not install or activate it
    Adapter {
        #[arg(value_parser=["tonantzintla", "serpantinum"])]
        kind: String,
        #[arg(long)]
        shell_root: PathBuf,
        #[arg(long)]
        fragment: Option<PathBuf>,
    },
    /// Restore saved configuration and gates, archiving external edits first
    Repair {
        #[arg(long)]
        yes: bool,
    },
    /// Recover an interrupted transaction; safe to repeat
    Recover {
        #[arg(long)]
        yes: bool,
    },
    /// Emergency stop and persistent disable for exactly one registered shell
    Disable {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Clear an emergency hold without selecting or starting the shell
    Release {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Resume only the explicitly selected shell (used by managed autostart)
    Resume {
        #[arg(long)]
        shell: Option<String>,
    },
    #[command(hide = true)]
    Gate {
        id: String,
        #[arg(long)]
        entry: Option<PathBuf>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Read-only verification of a cooperative activation/runtime lease
    Authorize {
        id: String,
        #[arg(long, default_value = "runtime")]
        purpose: String,
    },
    Keep,
    Revert,
    /// Stop only the currently managed shell
    Stop {
        #[arg(long)]
        yes: bool,
    },
    /// Adopt a currently running foreground process with a matching entrypoint
    Adopt {
        id: String,
        pid: u32,
        #[arg(long)]
        yes: bool,
    },
    /// Adopt an active user service declared by a manifest
    AdoptService {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    Status,
    /// Print a manifest scaffold; save as shellswitch.toml beside your shell
    Template,
    /// Print an editable manifest for any discovered candidate (does not execute it)
    Export {
        id: String,
    },
    #[command(hide = true)]
    Watchdog,
    #[command(hide = true)]
    Supervise {
        #[arg(long)]
        ticket: Option<PathBuf>,
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    /// Render the TUI to plain text for accessible inspection and testing
    #[command(hide = true)]
    Render {
        #[arg(long)]
        svg: bool,
    },
}
fn confirmed(yes: bool) -> Result<()> {
    if !yes {
        bail!("Review with plan first, then pass --yes to authorize this operation");
    }
    Ok(())
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.state_dir.unwrap_or_else(control::state_dir);
    let dir = if dir.is_absolute() {
        dir
    } else {
        std::env::current_dir()?.join(dir)
    };
    match &cli.command {
        Some(Action::Watchdog) => return control::watchdog(&dir),
        Some(Action::Supervise { argv, ticket }) => {
            return process::supervise(argv, ticket.as_deref());
        }
        Some(Action::Install {
            manifest,
            payload,
            yes,
        }) => {
            confirmed(*yes)?;
            let c = lifecycle::install(&dir, manifest, payload.as_deref())?;
            println!(
                "Staged {} ({}) without activation. Review inventory, then switch {} --yes",
                c.name, c.id, c.id
            );
            return Ok(());
        }
        Some(Action::Enroll {
            config,
            user_config,
            policy,
            offline,
            yes,
        }) => {
            confirmed(*yes)?;
            let baseline = user_config
                .clone()
                .unwrap_or_else(|| dir.join("user-config/auto-baseline.kdl"));
            ownership::enroll(&dir, config, &baseline, *policy, !offline)?;
            println!("Configuration enrolled; live entrypoint unchanged");
            return Ok(());
        }
        Some(Action::ConfigUpdate {
            user_config,
            policy,
            yes,
        }) => {
            confirmed(*yes)?;
            return lifecycle::update_config(&dir, user_config, *policy);
        }
        Some(Action::Protect { yes }) => {
            confirmed(*yes)?;
            return lifecycle::protect(&dir);
        }
        Some(Action::Doctor { json }) => return lifecycle::doctor(&dir, *json),
        Some(Action::Inventory) => return lifecycle::inventory(&dir),
        Some(Action::Adapter {
            kind,
            shell_root,
            fragment,
        }) => {
            print!(
                "{}",
                adapters::generate(kind, shell_root, fragment.as_deref())?
            );
            return Ok(());
        }
        Some(Action::Repair { yes }) => {
            confirmed(*yes)?;
            return lifecycle::repair(&dir);
        }
        Some(Action::Recover { yes }) => {
            confirmed(*yes)?;
            let files = lifecycle::recover_files(&dir)?;
            let pending = {
                let store = control::Store::open(&dir)?;
                store.load()?.pending.is_some()
            };
            if pending {
                return control::revert(&dir);
            }
            if !files {
                println!("No interrupted transaction");
            }
            return Ok(());
        }
        Some(Action::Disable { id, yes }) => {
            confirmed(*yes)?;
            return lifecycle::disable(&dir, id);
        }
        Some(Action::Release { id, yes }) => {
            confirmed(*yes)?;
            return lifecycle::release(&dir, id);
        }
        Some(Action::Resume { shell }) => return control::resume(&dir, shell.as_deref()),
        Some(Action::Gate { id, entry, args }) => {
            return lifecycle::gate(&dir, id, entry.as_deref(), args);
        }
        Some(Action::Authorize { id, purpose }) => return lifecycle::authorize(&dir, id, purpose),
        Some(Action::Keep) => {
            control::keep(&dir)?;
            println!("Trial kept");
            return Ok(());
        }
        Some(Action::Revert) => {
            control::revert(&dir)?;
            println!("Previous shell restored");
            return Ok(());
        }
        Some(Action::Stop { yes }) => {
            confirmed(*yes)?;
            control::stop_active(&dir)?;
            println!("Managed shell stopped");
            return Ok(());
        }
        Some(Action::Status) => {
            let store = control::Store::open(&dir)?;
            println!("{}", serde_json::to_string_pretty(&store.load()?)?);
            return Ok(());
        }
        Some(Action::Template) => {
            print!("{}", include_str!("../examples/shellswitch.toml"));
            return Ok(());
        }
        _ => {}
    }
    let mut roots = if cli.isolated {
        vec![]
    } else {
        discovery::default_roots()
    };
    roots.extend(cli.root);
    if roots.is_empty() {
        bail!("No scan roots; provide --root with --isolated");
    }
    let mut report = discovery::scan(&roots);
    // Installed revisions take precedence over mutable discovery results.
    let installed = lifecycle::registry(&dir)?;
    for c in installed {
        report.candidates.retain(|x| x.id != c.id);
        report.candidates.push(c);
    }
    report.candidates.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(message) = control::auto_adopt(&dir, &report.candidates)? {
        eprintln!("SHELLSWITCH: {message}");
    }
    let find = |id: &str| -> Result<&model::Candidate> {
        let choices: Vec<_> = report
            .candidates
            .iter()
            .filter(|c| c.id == id || c.name == id)
            .collect();
        if choices.len() > 1 {
            bail!("Ambiguous name; use the full candidate ID");
        }
        choices
            .first()
            .copied()
            .with_context(|| format!("No candidate {id:?}; run scan"))
    };
    match cli.command.unwrap_or(Action::Tui) {
        Action::Scan { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "SHELLSWITCH · {} / {} · {} files\n",
                    report.session.compositor, report.session.protocol, report.files_visited
                );
                for c in &report.candidates {
                    println!(
                        "{}  {:22} {:16} {:?} {}",
                        c.id,
                        c.name,
                        c.framework,
                        c.kind,
                        if c.running_pids.is_empty() {
                            String::new()
                        } else {
                            format!("RUNNING {:?}", c.running_pids)
                        }
                    );
                }
                for w in &report.warnings {
                    eprintln!("Scan note: {w}");
                }
                println!(
                    "\n{} candidates. Inspect with: shellswitch plan <id>",
                    report.candidates.len()
                );
            }
        }
        Action::Export { id } => {
            let c = find(&id)?;
            println!("# Review commands and compatibility before saving this manifest.");
            println!("# Source: {}", c.source.display());
            println!("name = {}", serde_json::to_string(&c.name)?);
            println!("kind = \"shell\"");
            println!("protocols = {}", serde_json::to_string(&c.protocols)?);
            println!("compositors = {}", serde_json::to_string(&c.compositors)?);
            println!(
                "required_globals = {}",
                serde_json::to_string(&c.required_globals)?
            );
            match &c.backend {
                model::Backend::Process { argv, cwd } => {
                    println!("argv = {}", serde_json::to_string(argv)?);
                    println!("cwd = {}", serde_json::to_string(cwd)?);
                }
                model::Backend::Systemd { unit } => {
                    println!("systemd_unit = {}", serde_json::to_string(unit)?)
                }
                _ if c.source.extension().is_some_and(|e| e == "service") => println!(
                    "systemd_unit = {}",
                    serde_json::to_string(&c.source.file_name().unwrap().to_string_lossy())?
                ),
                _ => {
                    println!("# Replace argv with the reviewed FOREGROUND launch command.");
                    println!("# A Bash launcher should finish with exec; do not use daemon mode.");
                    println!("argv = [\"REPLACE-WITH-FOREGROUND-EXECUTABLE\"]");
                    println!(
                        "cwd = {}",
                        serde_json::to_string(
                            &c.source.parent().unwrap_or(std::path::Path::new("/"))
                        )?
                    );
                }
            }
        }
        Action::Plan { id } => {
            let c = find(&id)?;
            let store = control::Store::open(&dir)?;
            println!(
                "{}\n\nEvidence:\n{}",
                control::plan(c, &report.session, &store.load()?)?,
                c.evidence.join("\n")
            );
        }
        Action::Switch { id, yes } => {
            confirmed(yes)?;
            control::switch(&dir, find(&id)?, &report.session, &report.candidates)?;
            println!(
                "Trial started. Run shellswitch keep within 20 seconds, or shellswitch revert. State: {}",
                dir.display()
            );
        }
        Action::Adopt { id, pid, yes } => {
            confirmed(yes)?;
            control::adopt(&dir, find(&id)?, Some(pid))?;
            println!("Process {pid} adopted");
        }
        Action::AdoptService { id, yes } => {
            confirmed(yes)?;
            let c = find(&id)?;
            if !matches!(c.backend, model::Backend::Systemd { .. }) {
                bail!("This candidate is not a user service");
            }
            control::adopt(&dir, c, None)?;
            println!("User service adopted");
        }
        Action::Tui => ui::run(report, roots, dir)?,
        Action::Render { svg } => print!(
            "{}",
            if svg {
                ui::snapshot_svg(&report)?
            } else {
                ui::snapshot(&report)?
            }
        ),
        _ => unreachable!(),
    }
    Ok(())
}
