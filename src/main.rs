mod action;
mod config;
mod init;
mod keys;
mod name;
mod preview;
mod remember;
mod scan;
mod status;
#[cfg(test)]
mod testing;
mod ui;
mod watch;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use config::{Config, Profile};
use scan::Queue;
use std::collections::BTreeMap;
use std::io::{ErrorKind, IsTerminal, Write};
use std::path::PathBuf;

const EXAMPLES: &str = "\
Examples:
  qwatch                          browse the default profile
  qwatch ~/queues                 browse a directory with no config at all
  qwatch --profile ingest         browse a named profile
  qwatch --list | grep failed     one line per file, for grepping
  qwatch --json | jq '.[].name'   the same thing, structured
  qwatch init ~/queues            work out a config and write it
";

#[derive(Parser)]
#[command(
    version,
    about = "Browse and unstick file-based work queues",
    after_help = EXAMPLES
)]
struct Options {
    /// Directory to browse, instead of a profile from the config file
    directory: Option<PathBuf>,

    /// Profile to browse, from the config file
    #[arg(short, long, value_name = "NAME")]
    profile: Option<String>,

    /// Config file to read, defaults to $XDG_CONFIG_HOME/qwatch/config.toml
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// List every file and exit
    #[arg(short, long)]
    list: bool,

    /// List every file as JSON and exit
    #[arg(long, conflicts_with = "list")]
    json: bool,

    /// Never colour the output, the same as setting NO_COLOR
    #[arg(long)]
    no_color: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Look at a directory and write a starter config for it
    Init {
        /// Directory to inspect, defaults to the current one
        directory: Option<PathBuf>,

        /// Print the config instead of writing it to a file
        #[arg(long)]
        print: bool,

        /// Write the config here instead of the default location
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },

    /// Print a completion script for your shell
    Completions {
        /// bash, zsh, fish, elvish or powershell
        shell: clap_complete::Shell,
    },
}

fn main() -> Result<()> {
    let options = Options::parse();

    match options.command {
        Some(Command::Init {
            directory,
            print,
            output,
        }) => return write_starter_config(directory, print, output),
        Some(Command::Completions { shell }) => return print_completions(shell),
        None => {}
    }

    let chosen = chosen_profile(&options)?;
    let queues = scan::scan(&chosen.profile)?;

    if options.json {
        let rendered = serde_json::to_string_pretty(&queues)?;
        return to_stdout(|out| writeln!(out, "{rendered}"));
    }
    if options.list {
        return to_stdout(|out| print_listing(out, &queues));
    }
    if !std::io::stdout().is_terminal() {
        bail!("the browser needs a terminal. Use --list or --json to send output elsewhere");
    }
    ui::run(chosen)
}

pub struct Chosen {
    pub profile: Profile,
    pub catalogue: BTreeMap<String, Profile>,
    pub name: String,
    pub book: Option<PathBuf>,
    pub colored: bool,
}

fn chosen_profile(options: &Options) -> Result<Chosen> {
    if let Some(directory) = &options.directory {
        return Ok(Chosen {
            profile: Profile::for_directory(directory),
            catalogue: BTreeMap::new(),
            name: directory.display().to_string(),
            book: config::default_config_path()
                .as_deref()
                .map(remember::beside),
            colored: wants_colour(options),
        });
    }

    let path = match &options.config {
        Some(path) => path.clone(),
        None => config::default_config_path().context("cannot find a home directory")?,
    };
    if !path.exists() {
        bail!(
            "no directory given and no config at {}\n\ntry: qwatch <directory>",
            path.display()
        );
    }
    let config = Config::load(&path)?;
    let profile = config.select(options.profile.as_deref())?.clone();
    let name = config
        .profile
        .iter()
        .find(|(_, other)| other.root == profile.root)
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| "default".to_string());

    Ok(Chosen {
        profile,
        catalogue: config.profile,
        name,
        book: Some(remember::beside(&path)),
        colored: wants_colour(options),
    })
}

fn wants_colour(options: &Options) -> bool {
    !options.no_color && std::env::var_os("NO_COLOR").is_none()
}

fn print_listing(out: &mut dyn Write, queues: &[Queue]) -> std::io::Result<()> {
    for queue in queues {
        for state in &queue.states {
            for entry in &state.entries {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}",
                    queue.name,
                    state.state,
                    entry.status,
                    entry.label,
                    entry.path.display()
                )?;
            }
        }
    }
    Ok(())
}

fn to_stdout(render: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match render(&mut out).and_then(|()| out.flush()) {
        Err(closed) if closed.kind() == ErrorKind::BrokenPipe => std::process::exit(0),
        other => Ok(other?),
    }
}

fn print_completions(shell: clap_complete::Shell) -> Result<()> {
    let mut command = Options::command();
    let name = command.get_name().to_string();
    to_stdout(|out| {
        clap_complete::generate(shell, &mut command, name, out);
        Ok(())
    })
}

fn write_starter_config(
    directory: Option<PathBuf>,
    print: bool,
    output: Option<PathBuf>,
) -> Result<()> {
    let root = init::resolve(directory)?;
    let layout = init::detect(&root)?;
    let name = init::suggested_name(&root);
    let text = init::config_text(&layout, &name);

    describe(&layout);

    if print {
        print!("{text}");
        return Ok(());
    }

    let elsewhere = output.is_some();
    let path = match output {
        Some(given) => config::expand_home(&given.to_string_lossy()),
        None => config::default_config_path().context("cannot find a home directory")?,
    };
    if path.exists() {
        eprintln!(
            "\n{} already exists, so here is the config instead:\n",
            path.display()
        );
        print!("{text}");
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &text)?;
    eprintln!("\nwrote {}", path.display());
    match elsewhere {
        true => eprintln!("run `qwatch --config {}` to browse it", path.display()),
        false => eprintln!("run `qwatch` to browse it, or `qwatch --profile {name}`"),
    }
    Ok(())
}

fn describe(layout: &init::Layout) {
    eprintln!("looking at {}", layout.root.display());
    match &layout.suffix {
        Some(suffix) => eprintln!(
            "found {} queue{} paired with a \"{suffix}\" directory: {}",
            layout.queues.len(),
            if layout.queues.len() == 1 { "" } else { "s" },
            layout.queues.join(", ")
        ),
        None if layout.states.is_empty() => {
            eprintln!("no subdirectories here, so every file is one state")
        }
        None => eprintln!(
            "no paired directories, so each subdirectory becomes a state: {}",
            layout.states.join(", ")
        ),
    }
}
