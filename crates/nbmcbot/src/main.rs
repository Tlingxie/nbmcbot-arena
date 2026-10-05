mod input;

use clap::{Parser, Subcommand};
use nbmcbot::config::Config;
use nbmcbot::runtime::FleetCommand;
use nbmcbot_core::{Command, parse_command};
use nbmcbot_plugins::{BotSnapshot, PluginAction, PluginLimits, PluginRuntime};
use serde_json::json;
use std::{io::Read, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "nbmcbot",
    version,
    about = "Minecraft Java Edition 1.21.11 bot"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Operation,
}
#[derive(Subcommand)]
enum Operation {
    Run {
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        username: Option<String>,
        #[arg(long,value_parser=["offline","microsoft"])]
        auth: Option<String>,
        #[arg(long)]
        no_reconnect: bool,
    },
    /// Run accounts on one server with shared world chunks.
    Swarm {
        #[arg(long)]
        server: Option<String>,
        #[arg(long, value_delimiter = ',', required = true)]
        usernames: Vec<String>,
        #[arg(long,value_parser=["offline","microsoft"])]
        auth: Option<String>,
        #[arg(long)]
        no_reconnect: bool,
    },
    Check,
    PluginCheck {
        file: PathBuf,
        #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u32).range(1..=10000))]
        ticks: u32,
    },
}

fn emit(value: serde_json::Value) {
    println!("{value}");
}

fn load_config(path: Option<PathBuf>) -> anyhow::Result<Config> {
    let Some(path) = path else {
        return Ok(Config::default());
    };
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() <= 65536,
        "configuration exceeds 64 KiB"
    );
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 65536, "configuration exceeds 64 KiB");
    Ok(toml::from_str(std::str::from_utf8(&bytes)?)?)
}

fn stdin_commands<T: Send + 'static>(
    sender: tokio::sync::mpsc::Sender<T>,
    parse: impl Fn(&str) -> anyhow::Result<T> + Send + 'static,
    quit_command: impl Fn() -> T + Send + 'static,
    is_quit: impl Fn(&T) -> bool + Send + 'static,
) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        loop {
            let command = match input::read_line(&mut reader) {
                Ok(input::InputLine::Text(text)) if text.trim().is_empty() => continue,
                Ok(input::InputLine::Text(text)) => match parse(&text) {
                    Ok(command) => command,
                    Err(e) => {
                        emit(json!({"event":"input_error","error":e.to_string()}));
                        continue;
                    }
                },
                Ok(input::InputLine::Eof) => quit_command(),
                Ok(input::InputLine::TooLong) => {
                    emit(
                        json!({"event":"input_error","error":"line exceeds 4096 bytes; discarded"}),
                    );
                    continue;
                }
                Ok(input::InputLine::InvalidUtf8) => {
                    emit(json!({"event":"input_error","error":"input must be UTF-8"}));
                    continue;
                }
                Err(e) => {
                    emit(json!({"event":"input_error","error":e.to_string()}));
                    quit_command()
                }
            };
            let quit = is_quit(&command);
            if sender.blocking_send(command).is_err() || quit {
                break;
            }
        }
    });
}

async fn execute(cli: Cli) -> anyhow::Result<()> {
    let mut config = load_config(cli.config)?;
    match cli.command {
        Operation::Run {
            server,
            username,
            auth,
            no_reconnect,
        } => {
            if let Some(server) = server {
                config.server = server;
            }
            if let Some(username) = username {
                config.username = username;
            }
            if let Some(auth) = auth {
                config.auth = auth;
            }
            if no_reconnect {
                config.max_reconnect_attempts = 0;
            }
            config.validate()?;
            emit(json!({"event":"starting","protocol":"1.21.11","config":config}));
            let (sender, receiver) = tokio::sync::mpsc::channel(64);
            stdin_commands(
                sender.clone(),
                |text| Ok(parse_command(text)?),
                || Command::Quit,
                |command| *command == Command::Quit,
            );
            let signal = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    let _ = sender.send(Command::Quit).await;
                }
            });
            let result = nbmcbot::runtime::run(config, receiver).await;
            signal.abort();
            result
        }
        Operation::Swarm {
            server,
            usernames,
            auth,
            no_reconnect,
        } => {
            if let Some(server) = server {
                config.server = server;
            }
            if let Some(auth) = auth {
                config.auth = auth;
            }
            if no_reconnect {
                config.max_reconnect_attempts = 0;
            }
            emit(
                json!({"event":"starting","protocol":"1.21.11","mode":"swarm","usernames":usernames,"config":config}),
            );
            let (sender, receiver) = tokio::sync::mpsc::channel(64);
            stdin_commands(
                sender.clone(),
                |text| {
                    let (target, command) = input::parse_target(text)?;
                    Ok(match target {
                        Some(username) => FleetCommand::Bot { username, command },
                        None => FleetCommand::All(command),
                    })
                },
                || FleetCommand::All(Command::Quit),
                |command| matches!(command, FleetCommand::All(Command::Quit)),
            );
            let signal = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    let _ = sender.send(FleetCommand::All(Command::Quit)).await;
                }
            });
            let result = nbmcbot::runtime::run_swarm(config, usernames, receiver).await;
            signal.abort();
            result
        }
        Operation::Check => {
            config.validate()?;
            emit(json!({"event":"config_valid","protocol":"1.21.11","config":config}));
            Ok(())
        }
        Operation::PluginCheck { file, ticks } => {
            config.validate()?;
            let mut runtime =
                PluginRuntime::new(PluginLimits::default()).map_err(anyhow::Error::msg)?;
            let id = runtime.load(file).map_err(anyhow::Error::msg)?;
            emit(json!({"event":"plugin_loaded","id":id,"protocol":"1.21.11"}));
            for tick in 0..u64::from(ticks) {
                for action in runtime.tick(BotSnapshot {
                    tick,
                    health: 20.0,
                    position: [0.0; 3],
                    connected: true,
                }) {
                    let action = match action {
                        PluginAction::Chat(message) => json!({"kind":"chat","message":message}),
                        PluginAction::Goto { x, y, z } => json!({"kind":"goto","x":x,"y":y,"z":z}),
                        PluginAction::Look { yaw, pitch } => {
                            json!({"kind":"look","yaw":yaw,"pitch":pitch})
                        }
                    };
                    emit(json!({"event":"plugin_action","tick":tick,"action":action}));
                }
                if let Some(error) = runtime.list().into_iter().find_map(|info| info.last_error) {
                    emit(json!({"event":"plugin_error","tick":tick,"error":error}));
                    anyhow::bail!("plugin callback trapped");
                }
            }
            emit(json!({"event":"plugin_check_complete","ticks":ticks}));
            Ok(())
        }
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match execute(Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            emit(json!({"event":"error","error":format!("{error:#}")}));
            std::process::ExitCode::FAILURE
        }
    }
}
