use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use azalea::{
    BlockPos, ClientInformation, Event,
    app::{App, AppExit, Plugin, PluginGroup, Update},
    auto_reconnect::{AutoReconnectDelay, InternalReconnectAfter},
    ecs::prelude::*,
    pathfinder::{
        PathfinderOpts,
        astar::PathfinderTimeout,
        goals::{BlockPosGoal, RadiusGoal},
    },
    prelude::*,
};
use nbmcbot_core::{Command, process_memory};
use nbmcbot_plugins::{BotSnapshot, PluginAction, PluginLimits, PluginRuntime};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::mpsc;

use crate::{automation::Modules, config::Config, game};
mod fleet;
#[cfg(test)]
#[path = "runtime_respawn_tests.rs"]
mod respawn_tests;
mod telemetry;
pub use fleet::{FleetCommand, run_swarm};

#[derive(Default)]
struct Flags {
    quit: AtomicBool,
    paused: AtomicBool,
    failures: AtomicU32,
    manual_disconnects: AtomicU32,
    error: Mutex<Option<String>>,
    connecting_since: Mutex<Option<Instant>>,
}

impl Flags {
    fn fail(&self, message: impl Into<String>) {
        let mut error = self.error.lock();
        if error.is_none() {
            *error = Some(message.into());
        }
        self.quit.store(true, Ordering::Release);
    }
}

#[derive(Clone, Component, Default)]
struct BotState {
    shared: Option<Arc<Mutex<Session>>>,
    flags: Arc<Flags>,
    username: String,
}

#[derive(Clone, Resource)]
struct RuntimeFlags {
    flags: Arc<Flags>,
    max_retries: u32,
    view_distance: u8,
    reconnect_delay: Duration,
}

struct GuardPlugin(RuntimeFlags);
impl Plugin for GuardPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::aerial::AerialPlugin)
            .add_plugins(crate::damage_audit::DamageAuditPlugin)
            .insert_resource(self.0.clone())
            .add_systems(
                Update,
                (
                    guard_system
                        .after(azalea::auto_reconnect::start_rejoin_on_disconnect)
                        .before(azalea::auto_reconnect::rejoin_after_delay),
                    configure_client.before(azalea::client_information::send_client_information),
                    track_mining_updates,
                    apply_pending_reconnect,
                ),
            )
            .add_systems(
                azalea::core::tick::GameTick,
                game::use_food.after(azalea::inventory::ensure_has_sent_carried_item),
            );
    }
}

fn guard_system(
    flags: Res<RuntimeFlags>,
    bots: Query<&BotState>,
    mut commands: Commands,
    mut failures: MessageReader<azalea::join::ConnectionFailedEvent>,
    mut exits: MessageWriter<AppExit>,
) {
    for failure in failures.read() {
        let Ok(bot) = bots.get(failure.entity) else {
            continue;
        };
        if bot.flags.paused.load(Ordering::Acquire) || bot.flags.quit.load(Ordering::Acquire) {
            continue;
        }
        *bot.flags.connecting_since.lock() = Some(Instant::now() + flags.reconnect_delay);
        let count = bot.flags.failures.fetch_add(1, Ordering::AcqRel) + 1;
        println!(
            "{}",
            json!({"event":"connection_failed", "bot":bot.username,"attempt":count, "error":failure.error.to_string()})
        );
        if count > flags.max_retries {
            bot.flags.fail(format!(
                "connection failed after {count} attempts: {}",
                failure.error
            ));
            commands
                .entity(failure.entity)
                .remove::<(InternalReconnectAfter, PendingReconnect)>()
                .insert(AutoReconnectDelay::new(Duration::MAX));
        }
    }
    if flags.flags.quit.load(Ordering::Acquire) {
        exits.write(AppExit::Success);
    }
}

fn configure_client(flags: Res<RuntimeFlags>, mut clients: Query<&mut ClientInformation>) {
    for mut info in &mut clients {
        info.view_distance = flags.view_distance;
    }
}

#[derive(Component)]
struct PendingReconnect;

#[allow(clippy::type_complexity)]
fn apply_pending_reconnect(
    mut commands: Commands,
    clients: Query<
        Entity,
        (
            With<PendingReconnect>,
            Without<azalea::connection::RawConnection>,
            Without<azalea::join::CreateConnectionTask>,
        ),
    >,
) {
    for entity in &clients {
        commands
            .entity(entity)
            .remove::<PendingReconnect>()
            .insert(InternalReconnectAfter {
                instant: Instant::now(),
            });
    }
}

#[derive(Clone, Component)]
struct MiningConfirmation {
    target: BlockPos,
    expected: azalea::block::BlockState,
    confirmed: bool,
}

fn track_mining_updates(
    mut packets: MessageReader<azalea::packet::game::ReceiveGamePacketEvent>,
    mut pending: Query<&mut MiningConfirmation>,
) {
    use azalea::protocol::packets::game::ClientboundGamePacket;
    for event in packets.read() {
        let Ok(mut pending) = pending.get_mut(event.entity) else {
            continue;
        };
        let confirms = |pos, state| pos == pending.target && state == pending.expected;
        let confirmed = match event.packet.as_ref() {
            ClientboundGamePacket::BlockUpdate(packet) => confirms(packet.pos, packet.block_state),
            ClientboundGamePacket::SectionBlocksUpdate(packet) => packet
                .states
                .iter()
                .any(|state| confirms(packet.section_pos + state.pos, state.state)),
            _ => false,
        };
        pending.confirmed |= confirmed;
    }
}

enum Task {
    Goto { target: BlockPos, started: Instant },
    Mine { target: BlockPos, started: Instant },
    Follow(String),
    Fight(crate::combat::Fight),
    Duel(Box<crate::tactics::Duel>),
}

struct Session {
    config: Config,
    account_name: String,
    account_uuid: u128,
    flags: Arc<Flags>,
    bot: Option<Client>,
    ready: bool,
    tick: u64,
    task: Option<Task>,
    modules: Modules,
    plugins: PluginRuntime,
    peak_rss: u64,
    rss: u64,
    last_plugin_error: Option<String>,
}

pub async fn run(config: Config, mut commands: mpsc::Receiver<Command>) -> Result<()> {
    config.validate()?;
    let mut plugins = PluginRuntime::new(PluginLimits::default()).map_err(anyhow::Error::msg)?;
    for path in &config.plugins {
        let id = plugins.load(path).map_err(anyhow::Error::msg)?;
        println!(
            "{}",
            json!({"event":"plugin_loaded","bot":config.username,"id":id})
        );
    }
    let authenticate = async {
        anyhow::Ok(match config.auth.as_str() {
            "offline" => Account::offline(&config.username),
            "microsoft" => {
                let mut account = Account::microsoft(&config.username)
                    .await
                    .context("Microsoft authentication failed")?;
                account
                    .request_certs()
                    .await
                    .context("chat certificate request failed")?;
                account
            }
            _ => bail!("unsupported authentication mode"),
        })
    };
    tokio::pin!(authenticate);
    let account = loop {
        tokio::select! {
            account=&mut authenticate => break account?,
            command=commands.recv() => match command {
                None|Some(Command::Quit) => {
                    let memory=process_memory()?;
                    println!("{}",json!({"event":"stopped","bot":config.username,"peak_rss_bytes":memory.peak_rss_bytes.max(memory.rss_bytes)}));
                    return Ok(());
                }
                _ => println!("{}",json!({"event":"command_error","bot":config.username,"error":"authentication is pending; only quit is available"})),
            }
        }
    };
    let flags = Arc::new(Flags::default());
    *flags.connecting_since.lock() = Some(Instant::now());
    let mut modules = Modules::default();
    for name in &config.modules {
        modules.set(name, true)?;
    }
    let session = Arc::new(Mutex::new(Session {
        config: config.clone(),
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
    let reconnect = (config.max_reconnect_attempts > 0)
        .then(|| Duration::from_secs(config.reconnect_delay_secs));
    // Native policies own respawning; upstream auto-respawn would bypass module settings.
    let builder = ClientBuilder::new_without_plugins()
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
            flags: flags.clone(),
            max_retries: config.max_reconnect_attempts,
            view_distance: config.view_distance,
            reconnect_delay: Duration::from_secs(config.reconnect_delay_secs),
        }))
        .set_handler(handle_event)
        .set_state(BotState {
            shared: Some(session.clone()),
            flags: flags.clone(),
            username: config.username.clone(),
        })
        .reconnect_after(reconnect);
    println!(
        "{}",
        json!({"event":"connecting","bot":config.username,"server":config.server,"minecraft":"1.21.11","protocol":774,"memory_limit_bytes":u64::from(config.memory_limit_mb)*1_000_000})
    );
    let runner = builder.start(account, config.server.as_str());
    let controller = control_loop(session.clone(), commands);
    tokio::pin!(runner, controller);
    tokio::select! {
        exit = &mut runner => {
            if !exit.is_success() { flags.fail("client runtime exited unsuccessfully"); }
        }
        result = &mut controller => {
            if let Err(error) = result { flags.fail(error.to_string()); }
            flags.quit.store(true, Ordering::Release);
            if tokio::time::timeout(Duration::from_secs(5), &mut runner).await.is_err() {
                flags.fail("client did not shut down within 5 seconds");
            }
        }
    }
    let mut session = session.lock();
    session.plugins.clear();
    if let Ok(memory) = process_memory() {
        session.peak_rss = session
            .peak_rss
            .max(memory.peak_rss_bytes)
            .max(memory.rss_bytes);
    }
    println!(
        "{}",
        json!({"event":"stopped","bot":session.config.username,"peak_rss_bytes":session.peak_rss})
    );
    if let Some(error) = flags.error.lock().take() {
        bail!("{error}");
    }
    Ok(())
}

async fn control_loop(
    session: Arc<Mutex<Session>>,
    mut commands: mpsc::Receiver<Command>,
) -> Result<()> {
    let mut monitor = tokio::time::interval(Duration::from_millis(250));
    let mut telemetry = telemetry::Telemetry::from_env()?;
    loop {
        tokio::select! {
            _ = telemetry::wait_for_tick(&mut telemetry) => {
                if let Some(telemetry) = &telemetry {
                    telemetry.sample(std::iter::once(&session));
                }
            }
            command = commands.recv() => {
                let mut session = session.lock();
                let command = command.unwrap_or(Command::Quit);
                if let Err(error) = session.command(command) {
                    println!("{}", json!({"event":"command_error","bot":session.config.username,"error":error.to_string()}));
                }
                if session.flags.quit.load(Ordering::Acquire) { return Ok(()); }
            }
            _ = monitor.tick() => {
                let mut session = session.lock();
                let memory = process_memory().context("could not sample process RSS")?;
                session.rss = memory.rss_bytes;
                session.peak_rss = session.peak_rss.max(memory.rss_bytes).max(memory.peak_rss_bytes);
                let limit = u64::from(session.config.memory_limit_mb) * 1_000_000;
                if session.peak_rss >= limit {
                    session.flags.fail(format!("memory budget exceeded: peak {} bytes, limit {limit}", session.peak_rss));
                }
                if !session.ready && !session.flags.paused.load(Ordering::Acquire)
                    && session.flags.connecting_since.lock().is_some_and(|since|since.elapsed() > Duration::from_secs(30)) {
                    session.flags.fail("connection did not reach game state within 30 seconds");
                }
                if session.flags.quit.load(Ordering::Acquire) { return Ok(()); }
            }
        }
    }
}

async fn handle_event(bot: Client, event: Event, state: BotState) -> Result<()> {
    let Some(shared) = state.shared else {
        return Ok(());
    };
    let mut session = shared.lock();
    if session.flags.quit.load(Ordering::Acquire) {
        return Ok(());
    }
    if bot.ecs.lock().get_entity(bot.entity).is_err() {
        return Ok(());
    }
    match event {
        Event::Init => {
            bot.set_client_information(ClientInformation {
                view_distance: session.config.view_distance,
                ..Default::default()
            });
            session.bot = Some(bot.clone());
            if session.flags.paused.load(Ordering::Acquire) {
                session.command(Command::Disconnect)?;
            }
        }
        Event::Login => {
            session.cancel_task(&bot, false);
            game::stop_movement(&bot, false);
            *session.flags.connecting_since.lock() = Some(Instant::now());
            session.ready = false;
            session.modules.reset_world();
        }
        Event::Spawn => {
            if session.flags.paused.load(Ordering::Acquire) {
                return Ok(());
            }
            let Some(pos) = bot.get_component::<azalea::entity::Position>() else {
                return Ok(());
            };
            session.bot = Some(bot.clone());
            session.ready = true;
            session.cancel_task(&bot, false);
            session.flags.failures.store(0, Ordering::Release);
            *session.flags.connecting_since.lock() = None;
            println!(
                "{}",
                json!({"event":"spawn","bot":session.config.username,"position":[pos.x,pos.y,pos.z],"username":session.config.username})
            );
        }
        Event::Tick if session.ready => {
            if let Err(error) = session.tick(&bot) {
                session.stop();
                println!(
                    "{}",
                    json!({"event":"task_error","bot":session.config.username,"error":error.to_string()})
                );
            }
        }
        Event::Tick if !session.flags.paused.load(Ordering::Acquire) && bot.logged_in() => {
            // A dead login may never reach Spawn until we request respawning.
            if let Some(health) = bot.get_component::<azalea::entity::metadata::Health>()
                && let Some(action) = session.modules.respawn_action(health.0 <= 0.0)
            {
                game::apply_automation(&bot, action);
            }
        }
        Event::Death(_) => session.stop(),
        Event::KeepAlive(id) if std::env::var_os("NBMCBOT_TRACE_KEEPALIVE").is_some() => {
            let observed_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            println!(
                "{}",
                json!({"event":"keepalive_received","bot":session.config.username,"id":id,"observed_at_ms":observed_at_ms,"tick":session.tick})
            );
        }
        Event::Chat(chat) => println!(
            "{}",
            json!({"event":"chat","bot":session.config.username,"text":chat.message().to_string()})
        ),
        Event::Disconnect(reason) => {
            session.cancel_task(&bot, false);
            game::stop_movement(&bot, false);
            *session.flags.connecting_since.lock() =
                Some(Instant::now() + Duration::from_secs(session.config.reconnect_delay_secs));
            session.ready = false;
            session.modules.reset_world();
            println!(
                "{}",
                json!({"event":"disconnected","bot":session.config.username,"reason":reason.map(|text|text.to_string())})
            );
            let intentional = session
                .flags
                .manual_disconnects
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    count.checked_sub(1)
                })
                .is_ok();
            if !intentional
                && !session.flags.paused.load(Ordering::Acquire)
                && !session.flags.quit.load(Ordering::Acquire)
            {
                let count = session.flags.failures.fetch_add(1, Ordering::AcqRel) + 1;
                if count > session.config.max_reconnect_attempts {
                    session
                        .flags
                        .fail("disconnected; reconnect budget exhausted");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl Session {
    fn connected_bot(&self) -> Result<Client> {
        ensure!(self.ready, "bot has not spawned or is disconnected");
        let bot = self.bot.clone().context("no active client")?;
        ensure!(bot.logged_in(), "connection is no longer in game state");
        Ok(bot)
    }

    fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Help => println!(
                "{}",
                json!({"event":"help","bot":self.config.username,"commands":["status","say <text>","goto <x> <y> <z>","follow <player>","mine <x> <y> <z>","look <yaw> <pitch>","attack <player>","fight <player|nearest>","duel mace|spear <enemy-prefix|=player>","inventory","stop","modules","module <name> on|off","plugins","plugin load <path>","plugin unload <id>","disconnect","reconnect","quit"]})
            ),
            Command::Status => {
                let bot = self
                    .bot
                    .as_ref()
                    .filter(|bot| self.ready && bot.logged_in());
                let pos = bot
                    .and_then(|bot| bot.get_component::<azalea::entity::Position>())
                    .map(|p| [p.x, p.y, p.z]);
                let health = bot.map(|bot| bot.health());
                let food = bot.map(|bot| bot.hunger().food);
                let loaded_chunks = bot.map_or(0, game::loaded_chunks);
                println!(
                    "{}",
                    json!({"event":"status","bot":self.config.username,"connected":bot.is_some(),"position":pos,"health":health,"food":food,"loaded_chunks":loaded_chunks,"rss_bytes":self.rss,"peak_rss_bytes":self.peak_rss,"tick":self.tick,"task":self.task_name()})
                );
            }
            Command::Quit => {
                self.stop();
                self.flags.quit.store(true, Ordering::Release);
            }
            Command::Disconnect => {
                self.flags.paused.store(true, Ordering::Release);
                self.stop();
                self.ready = false;
                if let Some(bot) = &self.bot {
                    let mut ecs = bot.ecs.lock();
                    ensure!(
                        ecs.get_entity(bot.entity).is_ok(),
                        "bot entity no longer exists"
                    );
                    ecs.entity_mut(bot.entity).remove::<(
                        AutoReconnectDelay,
                        InternalReconnectAfter,
                        PendingReconnect,
                    )>();
                    ecs.entity_mut(bot.entity)
                        .insert(AutoReconnectDelay::new(Duration::MAX));
                    ecs.entity_mut(bot.entity)
                        .remove::<azalea::join::CreateConnectionTask>();
                    ecs.entity_mut(bot.entity)
                        .insert(azalea::join::SuspendedJoin);
                    drop(ecs);
                    self.flags.manual_disconnects.fetch_add(1, Ordering::AcqRel);
                    bot.disconnect();
                }
            }
            Command::Reconnect => {
                let bot = self
                    .bot
                    .clone()
                    .context("initial connection is still pending")?;
                ensure!(!self.ready, "disconnect before reconnecting manually");
                self.flags.paused.store(false, Ordering::Release);
                self.flags.failures.store(0, Ordering::Release);
                *self.flags.connecting_since.lock() = Some(Instant::now());
                let mut ecs = bot.ecs.lock();
                ensure!(
                    ecs.get_entity(bot.entity).is_ok(),
                    "bot entity no longer exists"
                );
                ecs.entity_mut(bot.entity)
                    .remove::<azalea::join::SuspendedJoin>();
                if self.config.max_reconnect_attempts > 0 {
                    ecs.entity_mut(bot.entity).insert(AutoReconnectDelay::new(
                        Duration::from_secs(self.config.reconnect_delay_secs),
                    ));
                }
                ecs.entity_mut(bot.entity).insert(PendingReconnect);
            }
            Command::Stop => self.stop(),
            Command::Chat(message) => {
                ensure!(
                    message.chars().count() <= 256 && !message.contains(['\n', '\r', '\0']),
                    "chat must be at most 256 characters with no control lines"
                );
                self.connected_bot()?.chat(&message);
                println!(
                    "{}",
                    json!({"event":"chat_sent","bot":self.config.username})
                );
            }
            Command::Goto { x, y, z } => self.goto(BlockPos::new(x, y, z))?,
            Command::Follow(name) => {
                let bot = self.connected_bot()?;
                ensure!(
                    game::player_position(&bot, &name).is_some(),
                    "player is not visible: {name}"
                );
                self.stop();
                self.task = Some(Task::Follow(name.clone()));
                println!(
                    "{}",
                    json!({"event":"task_started","bot":self.config.username,"task":"follow","player":name})
                );
            }
            Command::Mine { x, y, z } => {
                let bot = self.connected_bot()?;
                let target = BlockPos::new(x, y, z);
                ensure!(
                    game::distance_squared(bot.eye_position(), target.center()) <= 20.25,
                    "block is outside mining reach; navigate closer first"
                );
                let state = bot
                    .world()
                    .read()
                    .get_block_state(target)
                    .context("target chunk is unknown")?;
                ensure!(
                    state != azalea::block::BlockState::AIR,
                    "target block is already air"
                );
                self.stop();
                let expected = azalea::block::BlockState::from(
                    azalea::block::fluid_state::FluidState::from(state),
                );
                bot.ecs
                    .lock()
                    .entity_mut(bot.entity)
                    .insert(MiningConfirmation {
                        target,
                        expected,
                        confirmed: false,
                    });
                bot.look_at(target.center());
                bot.start_mining(target);
                self.task = Some(Task::Mine {
                    target,
                    started: Instant::now(),
                });
                println!(
                    "{}",
                    json!({"event":"task_started","bot":self.config.username,"task":"mine","target":[x,y,z]})
                );
            }
            Command::Look { yaw, pitch } => {
                ensure!(
                    self.task.is_none() && !self.modules.is_eating(),
                    "stop active task or eating before changing its rotation"
                );
                self.connected_bot()?.set_direction(yaw, pitch);
            }
            Command::Attack(name) => {
                let bot = self.connected_bot()?;
                let entity = game::player_entity(&bot, &name).context("player is not visible")?;
                let pos = bot
                    .get_entity_component::<azalea::entity::Position>(entity)
                    .context("player position is unavailable")?;
                ensure!(
                    game::distance_squared(bot.position(), *pos) <= 9.0,
                    "player is outside attack reach"
                );
                self.stop();
                bot.look_at(pos.up(1.5));
                bot.attack(entity);
            }
            Command::Fight(player) => {
                self.connected_bot()?;
                self.stop();
                self.task = Some(Task::Fight(crate::combat::Fight::new(player.clone())));
                println!(
                    "{}",
                    json!({"event":"task_started","bot":self.config.username,"task":"fight","player":player})
                );
            }
            Command::Duel {
                style,
                enemy_prefix,
            } => {
                self.connected_bot()?;
                self.stop();
                self.task = Some(Task::Duel(Box::new(crate::tactics::Duel::new(
                    style.clone(),
                    enemy_prefix.clone(),
                ))));
                println!(
                    "{}",
                    json!({"event":"task_started","bot":self.config.username,"task":"duel","style":style,"enemy_prefix":enemy_prefix})
                );
            }
            Command::Inventory => game::print_inventory(&self.connected_bot()?),
            Command::Modules => println!(
                "{}",
                json!({"event":"modules","bot":self.config.username,"modules":self.modules.list()})
            ),
            Command::Module { name, enabled } => {
                self.modules.set(&name, enabled)?;
                println!(
                    "{}",
                    json!({"event":"module_changed","bot":self.config.username,"name":name,"enabled":enabled})
                );
            }
            Command::Plugins => {
                let plugins: Vec<_> = self.plugins.list().into_iter().map(|p|json!({"id":p.id,"module_bytes":p.module_bytes,"last_error":p.last_error})).collect();
                println!(
                    "{}",
                    json!({"event":"plugins","bot":self.config.username,"plugins":plugins})
                );
            }
            Command::PluginLoad(path) => {
                let id = self.plugins.load(path).map_err(anyhow::Error::msg)?;
                println!(
                    "{}",
                    json!({"event":"plugin_loaded","bot":self.config.username,"id":id})
                );
            }
            Command::PluginUnload(id) => {
                ensure!(self.plugins.unload(&id), "plugin is not loaded: {id}");
                println!(
                    "{}",
                    json!({"event":"plugin_unloaded","bot":self.config.username,"id":id})
                );
            }
        }
        Ok(())
    }

    fn task_name(&self) -> Option<&'static str> {
        self.task.as_ref().map(|task| match task {
            Task::Goto { .. } => "goto",
            Task::Mine { .. } => "mine",
            Task::Follow(_) => "follow",
            Task::Fight(_) => "fight",
            Task::Duel(_) => "duel",
        })
    }

    fn cancel_task(&mut self, bot: &Client, send_packets: bool) {
        if let Some(Task::Duel(duel)) = &mut self.task {
            duel.cancel(bot, send_packets);
        }
        self.task = None;
    }

    fn stop(&mut self) {
        if let Some(bot) = self.bot.clone() {
            self.cancel_task(&bot, self.ready && bot.logged_in());
        } else {
            self.task = None;
        }
        if let Some(bot) = self
            .bot
            .as_ref()
            .filter(|bot| self.ready && bot.logged_in())
        {
            for action in self.modules.cancel_active() {
                game::apply_automation(bot, action);
            }
            game::stop_movement(bot, true);
            bot.ecs
                .lock()
                .entity_mut(bot.entity)
                .remove::<MiningConfirmation>();
        }
    }

    fn goto(&mut self, target: BlockPos) -> Result<()> {
        let bot = self.connected_bot()?;
        self.stop();
        bot.start_goto_with_opts(BlockPosGoal(target), path_opts());
        self.task = Some(Task::Goto {
            target,
            started: Instant::now(),
        });
        println!(
            "{}",
            json!({"event":"task_started","bot":self.config.username,"task":"goto","target":[target.x,target.y,target.z]})
        );
        Ok(())
    }

    fn tick(&mut self, bot: &Client) -> Result<()> {
        if !bot.logged_in() {
            return Ok(());
        }
        self.tick = self.tick.wrapping_add(1);
        if bot.health() <= 0.0 {
            self.stop();
        }
        if let Some(Task::Fight(fight)) = &mut self.task {
            fight.tick(bot, self.tick, &self.config.username)?;
        }
        if let Some(Task::Duel(duel)) = &mut self.task {
            duel.tick(bot, self.tick, &self.config.username)?;
        }
        let mut finished = None;
        match &self.task {
            Some(Task::Goto { target, started }) => {
                if azalea::pathfinder::player_pos_to_block_pos(bot.position()) == *target {
                    finished = Some("goto");
                } else if started.elapsed() > Duration::from_secs(120) {
                    self.stop();
                    bail!("navigation timed out");
                } else if started.elapsed() > Duration::from_millis(500)
                    && bot.is_goto_target_reached()
                {
                    self.stop();
                    bail!("navigation ended before reaching target");
                }
            }
            Some(Task::Mine { target, started }) => {
                if bot
                    .get_component::<MiningConfirmation>()
                    .is_some_and(|state| state.confirmed)
                {
                    println!(
                        "{}",
                        json!({"event":"block_changed","bot":self.config.username,"source":"server","target":[target.x,target.y,target.z]})
                    );
                    finished = Some("mine");
                } else if started.elapsed() > Duration::from_secs(30) {
                    self.stop();
                    bail!("mining timed out");
                }
            }
            Some(Task::Follow(name)) if self.tick.is_multiple_of(20) => {
                if let Some(pos) = game::player_position(bot, name) {
                    if game::distance_squared(bot.position(), pos) > 9.0 {
                        bot.start_goto_with_opts(RadiusGoal { pos, radius: 2.0 }, path_opts());
                    } else if !bot.is_goto_target_reached() {
                        game::stop_movement(bot, true);
                    }
                } else {
                    self.stop();
                    bail!("follow target is no longer visible");
                }
            }
            _ => {}
        }
        if let Some(task) = finished {
            self.stop();
            println!(
                "{}",
                json!({"event":"task_finished","bot":self.config.username,"task":task})
            );
        }
        let observation = game::observation(bot, self.tick, self.task.is_some());
        for action in self.modules.tick(&observation) {
            game::apply_automation(bot, action);
        }
        let p = bot.position();
        let actions = self.plugins.tick(BotSnapshot {
            tick: self.tick,
            health: bot.health(),
            position: [p.x, p.y, p.z],
            connected: true,
        });
        for action in actions {
            match action {
                PluginAction::Chat(text) => bot.chat(&text),
                PluginAction::Look { yaw, pitch }
                    if self.task.is_none() && !self.modules.is_eating() =>
                {
                    bot.set_direction(yaw, pitch)
                }
                PluginAction::Goto { x, y, z }
                    if self.task.is_none() && !self.modules.is_eating() =>
                {
                    self.goto(BlockPos::new(x, y, z))?
                }
                _ => println!(
                    "{}",
                    json!({"event":"plugin_action_rejected","bot":self.config.username,"reason":"movement or rotation is owned by an active task or module"})
                ),
            }
        }
        let error = self.plugins.list().iter().find_map(|plugin| {
            plugin
                .last_error
                .as_ref()
                .map(|e| format!("{}: {e}", plugin.id))
        });
        if error != self.last_plugin_error {
            if let Some(error) = &error {
                println!(
                    "{}",
                    json!({"event":"plugin_error","bot":self.config.username,"error":error})
                );
            }
            self.last_plugin_error = error;
        }
        Ok(())
    }
}

fn path_opts() -> PathfinderOpts {
    PathfinderOpts::new()
        .min_timeout(PathfinderTimeout::Nodes(2_000))
        .max_timeout(PathfinderTimeout::Nodes(25_000))
        .retry_on_no_path(false)
}
