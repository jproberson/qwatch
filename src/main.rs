mod action;
mod config;
mod init;
mod keys;
mod name;
mod preview;
mod scan;
mod status;
mod ui;
mod watch;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use config::{Config, Profile};
use scan::Queue;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Browse and unstick file-based work queues")]
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
    #[arg(long)]
    json: bool,

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
}

fn main() -> Result<()> {
    let options = Options::parse();

    if let Some(Command::Init {
        directory,
        print,
        output,
    }) = options.command
    {
        return write_starter_config(directory, print, output);
    }

    let profile = chosen_profile(&options)?;
    let queues = scan::scan(&profile)?;

    if options.json {
        println!("{}", serde_json::to_string_pretty(&queues)?);
        return Ok(());
    }
    if options.list {
        print_listing(&queues);
        return Ok(());
    }
    ui::run(profile)
}

fn chosen_profile(options: &Options) -> Result<Profile> {
    if let Some(directory) = &options.directory {
        return Ok(Profile::for_directory(directory));
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
    Ok(Config::load(&path)?
        .select(options.profile.as_deref())?
        .clone())
}

fn print_listing(queues: &[Queue]) {
    for queue in queues {
        for state in &queue.states {
            for entry in &state.entries {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    queue.name,
                    state.state,
                    entry.status,
                    entry.label,
                    entry.path.display()
                );
            }
        }
    }
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
