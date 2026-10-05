use super::*;
use azalea::swarm::SwarmBuilder;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone)]
pub enum FleetCommand {
    All(Command),
    Bot { username: String, command: Command },
}

type Sessions = BTreeMap<String, Arc<Mutex<Session>>>;
const MAX_SWARM_ACCOUNTS: usize = 128;
const INITIAL_JOIN_DELAY: Duration = Duration::from_millis(100);

fn validate_accounts(config: &Config, usernames: &[String]) -> Result<()> {
    ensure!(
        (1..=MAX_SWARM_ACCOUNTS).contains(&usernames.len()),
        "swarm requires 1..128 accounts"
    );
    let mut unique = HashSet::new();
    for username in usernames {
        ensure!(
            !username.eq_ignore_ascii_case("all"),
            "all is reserved for command broadcasts"
        );
        ensure!(
            unique.insert(username.to_ascii_lowercase()),
            "duplicate swarm username: {username}"
        );
        let mut individual = config.clone();
        individual.username = username.clone();
        individual.validate()?;
    }
    Ok(())
}

fn check_memory_budget(config: &Config) -> Result<nbmcbot_core::ProcessMemory> {
    let memory = process_memory()?;
    let peak = memory.peak_rss_bytes.max(memory.rss_bytes);
    ensure!(
        peak < u64::from(config.memory_limit_mb) * 1_000_000,
        "swarm memory budget exceeded: {peak} bytes"
    );
    Ok(memory)
}

pub async fn run_swarm(
    config: Config,
    usernames: Vec<String>,
    mut commands: mpsc::Receiver<FleetCommand>,
) -> Result<()> {
    validate_accounts(&config, &usernames)?;
    let global = Arc::new(Flags::default());
    let mut sessions = Sessions::new();
    let mut accounts = Vec::new();
    let mut account_ids = HashSet::new();
    for username in usernames {
        check_memory_budget(&config)?;
        let mut individual = config.clone();
        individual.username = username.clone();
        let authenticate = async {
            anyhow::Ok(if config.auth == "offline" {
                Account::offline(&username)
            } else {
                let mut account = Account::microsoft(&username)
                    .await
                    .context("Microsoft authentication failed")?;
                account
                    .request_certs()
                    .await
                    .context("chat certificate request failed")?;
                account
            })
        };
        tokio::pin!(authenticate);
        let account = loop {
            tokio::select! {
                account = &mut authenticate => break account,
                command = commands.recv() => match command {
                    None | Some(FleetCommand::All(Command::Quit)) => return Ok(()),
                    _ => emit(&username, json!({"event":"command_error","error":"authentication pending; only global quit is available"})),
                }
            }
        };
        let account = match account {
            Ok(account) => account,
            Err(error) => {
                emit(
                    &username,
                    json!({"event":"bot_failed","error":error.to_string()}),
                );
                continue;
            }
        };
        ensure!(
            account_ids.insert(account.uuid_or_offline()),
            "multiple swarm accounts authenticate to the same Minecraft UUID"
        );
        let flags = Arc::new(Flags::default());
        let mut modules = Modules::default();
        for name in &config.modules {
            modules.set(name, true)?;
        }
        let mut plugins =
            PluginRuntime::new(PluginLimits::default()).map_err(anyhow::Error::msg)?;
        for path in &config.plugins {
            let id = plugins.load(path).map_err(anyhow::Error::msg)?;
            check_memory_budget(&config)?;
            emit(&username, json!({"event":"plugin_loaded","id":id}));
        }
        let session = Arc::new(Mutex::new(Session {
            config: individual,
            account_name: account.username.clone(),
            account_uuid: account.uuid_or_offline().as_u128(),
            flags: flags.clone(),
            bot: None,
            ready: false,
            tick: 0,
            task: None,
            modules,
            plugins,
            peak_rss: 0,
            rss: 0,
            last_plugin_error: None,
        }));
        accounts.push((
            account,
            BotState {
                shared: Some(session.clone()),
                flags,
                username: username.clone(),
            },
        ));
        sessions.insert(username.clone(), session);
    }
    ensure!(!sessions.is_empty(), "no account could authenticate");
    let mut builder = SwarmBuilder::new_without_plugins()
        .add_plugins(
            azalea::DefaultPlugins
                .build()
                .set(azalea::task_pool::TaskPoolPlugin {
                    task_pool_options: azalea::task_pool::TaskPoolOptions {
                        min_total_threads: 3,
                        max_total_threads: 3,
                        ..Default::default()
                    },
                }),
        )
        .add_plugins(
            azalea::bot::DefaultBotPlugins
                .build()
                .disable::<azalea::auto_respawn::AutoRespawnPlugin>(),
        )
        .add_plugins(GuardPlugin(RuntimeFlags {
            flags: global.clone(),
            max_retries: config.max_reconnect_attempts,
            view_distance: config.view_distance,
            reconnect_delay: Duration::from_secs(config.reconnect_delay_secs),
        }))
        .set_handler(handle_event)
        .set_swarm_handler(register_clients)
        .join_delay(INITIAL_JOIN_DELAY)
        .reconnect_after(
            (config.max_reconnect_attempts > 0)
                .then(|| Duration::from_secs(config.reconnect_delay_secs)),
        );
    let started = Instant::now();
    for (index, (account, state)) in accounts.into_iter().enumerate() {
        // Scheduled accounts receive their full connection timeout after their join slot.
        *state.flags.connecting_since.lock() = Some(started + INITIAL_JOIN_DELAY * index as u32);
        builder = builder.add_account_with_state(account, state);
    }
    check_memory_budget(&config)?;
    println!(
        "{}",
        json!({"event":"swarm_connecting","server":config.server,"bots":sessions.len(),"join_delay_ms":INITIAL_JOIN_DELAY.as_millis(),"memory_limit_bytes":u64::from(config.memory_limit_mb)*1_000_000})
    );
    let runner = builder.start(config.server.as_str());
    let controller = fleet_control(&sessions, &global, &config, commands);
    tokio::pin!(runner, controller);
    tokio::select! {
        exit=&mut runner => { if !exit.is_success() { global.fail("swarm runtime exited unsuccessfully"); } },
        result=&mut controller => {
            if let Err(error)=result { global.fail(error.to_string()); }
            for session in sessions.values() { session.lock().flags.quit.store(true,Ordering::Release); }
            global.quit.store(true,Ordering::Release);
            if tokio::time::timeout(Duration::from_secs(5),&mut runner).await.is_err() { global.fail("swarm did not stop within five seconds"); }
        }
    }
    for (name, session) in &sessions {
        let mut session = session.lock();
        session.flags.quit.store(true, Ordering::Release);
        session.plugins.clear();
        emit(
            name,
            json!({"event":"stopped","peak_rss_bytes":session.peak_rss}),
        );
    }
    if let Some(error) = global.error.lock().take() {
        bail!("{error}");
    }
    Ok(())
}

pub(super) fn emit(username: &str, mut value: serde_json::Value) {
    value["bot"] = username.into();
    println!("{value}");
}

async fn fleet_control(
    sessions: &Sessions,
    global: &Arc<Flags>,
    config: &Config,
    mut commands: mpsc::Receiver<FleetCommand>,
) -> Result<()> {
    let mut monitor = tokio::time::interval(Duration::from_millis(250));
    let mut telemetry = telemetry::Telemetry::from_env()?;
    let mut reported: HashSet<String> = HashSet::new();
    loop {
        tokio::select! {
            _ = telemetry::wait_for_tick(&mut telemetry) => {
                if let Some(telemetry) = &telemetry {
                    telemetry.sample(sessions.values());
                }
            }
            command=commands.recv() => {
                match command.unwrap_or(FleetCommand::All(Command::Quit)) {
                    FleetCommand::All(Command::Quit) => {
                        for session in sessions.values() { let mut session = session.lock(); session.stop(); session.flags.quit.store(true,Ordering::Release); }
                        return Ok(());
                    },
                    FleetCommand::All(command) => {
                        for (name,session) in sessions {
                            if command == Command::Reconnect { reported.remove(name); }
                            if let Err(error)=execute_command(&mut session.lock(),command.clone()) { emit(name,json!({"event":"command_error","error":error.to_string()})); }
                        }
                        if command == Command::Status { print_stats(sessions)?; }
                    },
                    FleetCommand::Bot{username,command} => {
                        if command == Command::Reconnect {
                            reported.retain(|name| !name.eq_ignore_ascii_case(&username));
                        }
                        ensure_bot_command(sessions,&username,command);
                    },
                }
            }
            _=monitor.tick() => {
                let memory=check_memory_budget(config)?;
                let peak=memory.peak_rss_bytes.max(memory.rss_bytes);
                for (name,session) in sessions {
                    let mut session=session.lock();
                    session.rss=memory.rss_bytes;
                    session.peak_rss=session.peak_rss.max(peak);
                    if !session.ready && !session.flags.paused.load(Ordering::Acquire)
                        && session.flags.connecting_since.lock().is_some_and(|time| time.elapsed()>Duration::from_secs(30)) {
                        session.flags.fail("connection did not reach game state within30seconds");
                    }
                    if session.flags.quit.load(Ordering::Acquire) && reported.insert(name.clone()) {
                        session.ready=false;
                        session.task=None;
                        session.modules.reset_world();
                        session.plugins.clear();
                        if let Some(bot)=&session.bot {
                            game::stop_movement(bot,false);
                            let mut ecs=bot.ecs.lock();
                            if let Ok(mut entity)=ecs.get_entity_mut(bot.entity) {
                                entity.remove::<(InternalReconnectAfter,PendingReconnect,azalea::join::CreateConnectionTask)>();
                                entity.insert((AutoReconnectDelay::new(Duration::MAX),azalea::join::SuspendedJoin));
                            }
                            let connected = ecs.get::<azalea::connection::RawConnection>(bot.entity).is_some();
                            drop(ecs);
                            if connected { bot.disconnect(); }
                        }
                        emit(name,json!({"event":"bot_failed","error":session.flags.error.lock().clone()}));
                    }
                }
                if global.quit.load(Ordering::Acquire) { return Ok(()); }
            }
        }
    }
}

fn ensure_bot_command(sessions: &Sessions, username: &str, command: Command) {
    let Some((name, session)) = sessions
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(username))
    else {
        emit(
            username,
            json!({"event":"command_error","error":"unknown swarm bot"}),
        );
        return;
    };
    if command == Command::Quit {
        emit(
            name,
            json!({"event":"command_error","error":"use disconnect for one bot; quit exits the whole swarm"}),
        );
        return;
    }
    let mut session = session.lock();
    if let Err(error) = execute_command(&mut session, command) {
        emit(
            name,
            json!({"event":"command_error","error":error.to_string()}),
        );
    }
}

fn execute_command(session: &mut Session, command: Command) -> Result<()> {
    let reconnect = command == Command::Reconnect;
    session.command(command)?;
    if reconnect {
        session.flags.quit.store(false, Ordering::Release);
        session.flags.error.lock().take();
    }
    Ok(())
}

async fn register_clients(
    swarm: azalea::swarm::Swarm,
    event: azalea::swarm::SwarmEvent,
    _: azalea::swarm::NoSwarmState,
) -> Result<()> {
    if matches!(event, azalea::swarm::SwarmEvent::Login) {
        let clients: Vec<_> = {
            let mut ecs = swarm.ecs_lock.lock();
            let mut query = ecs.query::<(Entity, &BotState)>();
            query
                .iter(&ecs)
                .filter_map(|(entity, state)| state.shared.clone().map(|session| (entity, session)))
                .collect()
        };
        for (entity, session) in clients {
            let mut session = session.lock();
            if session.bot.is_none() {
                session.bot = Some(Client::new(entity, swarm.ecs_lock.clone()));
            }
            if session.flags.paused.load(Ordering::Acquire)
                || session.flags.quit.load(Ordering::Acquire)
            {
                session.command(Command::Disconnect)?;
            }
        }
    }
    Ok(())
}

fn print_stats(sessions: &Sessions) -> Result<()> {
    let memory = process_memory()?;
    let mut worlds = HashSet::new();
    let mut chunks = HashSet::new();
    let mut references = 0;
    let mut connected = 0;
    for session in sessions.values() {
        let session = session.lock();
        let Some(bot) = session
            .bot
            .as_ref()
            .filter(|bot| session.ready && bot.logged_in())
        else {
            continue;
        };
        connected += 1;
        let (world, partial) = {
            let ecs = bot.ecs.lock();
            let Some(holder) = ecs.get::<azalea::local_player::InstanceHolder>(bot.entity) else {
                continue;
            };
            (holder.instance.clone(), holder.partial_instance.clone())
        };
        worlds.insert(Arc::as_ptr(&world) as usize);
        let partial = partial.read();
        for chunk in partial.chunks.chunks().flatten() {
            references += 1;
            chunks.insert(Arc::as_ptr(chunk) as usize);
        }
    }
    println!(
        "{}",
        json!({"event":"swarm_status","bots":sessions.len(),"connected":connected,"rss_bytes":memory.rss_bytes,"peak_rss_bytes":memory.peak_rss_bytes.max(memory.rss_bytes),"chunk_references":references,"unique_chunks":chunks.len(),"unique_worlds":worlds.len()})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> Arc<Mutex<Session>> {
        Arc::new(Mutex::new(Session {
            account_name: name.into(),
            account_uuid: Account::offline(name).uuid_or_offline().as_u128(),
            config: Config {
                username: name.into(),
                ..Default::default()
            },
            flags: Arc::new(Flags::default()),
            bot: None,
            ready: false,
            tick: 0,
            task: None,
            modules: Modules::default(),
            plugins: PluginRuntime::new(PluginLimits::default()).unwrap(),
            peak_rss: 0,
            rss: 0,
            last_plugin_error: None,
        }))
    }

    #[test]
    fn reconnect_failure_flags_are_account_local() {
        let first = session("First");
        let second = session("Second");
        first.lock().flags.fail("first connection failed");
        assert!(!second.lock().flags.quit.load(Ordering::Acquire));
        assert!(execute_command(&mut first.lock(), Command::Reconnect).is_err());
        assert!(first.lock().flags.quit.load(Ordering::Acquire));
        assert!(first.lock().flags.error.lock().is_some());
        assert!(!second.lock().flags.quit.load(Ordering::Acquire));
    }

    #[test]
    fn targeted_quit_does_not_stop_any_account() {
        let first = session("First");
        let second = session("Second");
        let sessions = BTreeMap::from([
            ("First".into(), first.clone()),
            ("Second".into(), second.clone()),
        ]);
        ensure_bot_command(&sessions, "first", Command::Quit);
        assert!(!first.lock().flags.quit.load(Ordering::Acquire));
        assert!(!second.lock().flags.quit.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn validates_fleet_account_boundaries_before_connecting() {
        for usernames in [
            vec![],
            vec!["First".into(), "FIRST".into()],
            vec!["all".into()],
            vec!["Bad Name".into()],
            (0..129).map(|i| format!("Bot{i}")).collect(),
        ] {
            let (_sender, receiver) = mpsc::channel(1);
            assert!(
                run_swarm(Config::default(), usernames, receiver)
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn accepts_one_hundred_accounts_and_preserves_a_finite_ceiling() {
        for count in [1, 32, 100, 128] {
            let usernames = (0..count).map(|i| format!("Bot{i}")).collect::<Vec<_>>();
            assert!(validate_accounts(&Config::default(), &usernames).is_ok());
        }
        let usernames = (0..129).map(|i| format!("Bot{i}")).collect::<Vec<_>>();
        assert!(validate_accounts(&Config::default(), &usernames).is_err());
    }
}
