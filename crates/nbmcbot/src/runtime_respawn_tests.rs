use super::*;
use azalea::ecs as bevy_ecs;
use azalea::{
    entity::metadata::Health,
    packet::game::SendGamePacketEvent,
    protocol::packets::game::{ServerboundGamePacket, s_client_command::Action},
    world::InstanceName,
};

#[derive(Resource, Default)]
struct Packets(Vec<ServerboundGamePacket>);

fn waiting_client(logged_in: bool) -> (Client, BotState) {
    let mut app = App::new();
    app.add_plugins(azalea::respawn::RespawnPlugin)
        .init_resource::<Packets>()
        .add_observer(
            |event: On<SendGamePacketEvent>, mut packets: ResMut<Packets>| {
                packets.0.push(event.packet.clone());
            },
        );
    let entity = app.world_mut().spawn(Health(0.0)).id();
    if logged_in {
        app.world_mut()
            .entity_mut(entity)
            .insert(InstanceName(azalea::Identifier::new("minecraft:overworld")));
    }
    let bot = Client {
        entity,
        ecs: Arc::new(Mutex::new(std::mem::take(app.world_mut()))),
    };
    let flags = Arc::new(Flags::default());
    let session = Session {
        config: Config::default(),
        account_name: "WaitingBot".into(),
        account_uuid: Account::offline("WaitingBot").uuid_or_offline().as_u128(),
        flags: flags.clone(),
        bot: Some(bot.clone()),
        ready: false,
        tick: 0,
        task: None,
        modules: Modules::default(),
        plugins: PluginRuntime::new(PluginLimits::default()).unwrap(),
        peak_rss: 0,
        rss: 0,
        last_plugin_error: None,
    };
    let state = BotState {
        shared: Some(Arc::new(Mutex::new(session))),
        flags,
        username: "WaitingBot".into(),
    };
    (bot, state)
}

fn enable(state: &BotState, enabled: bool) {
    state
        .shared
        .as_ref()
        .unwrap()
        .lock()
        .command(Command::Module {
            name: "auto-respawn".into(),
            enabled,
        })
        .unwrap();
}

async fn tick(bot: &Client, state: &BotState) {
    handle_event(bot.clone(), Event::Tick, state.clone())
        .await
        .unwrap();
    bot.ecs.lock().run_schedule(Update);
}

fn respawns(bot: &Client) -> usize {
    bot.ecs
        .lock()
        .resource::<Packets>()
        .0
        .iter()
        .filter(|packet| {
            matches!(packet, ServerboundGamePacket::ClientCommand(packet)
                if packet.action == Action::PerformRespawn)
        })
        .count()
}

#[tokio::test]
async fn dead_login_respawns_before_spawn_once_per_death() {
    let (bot, state) = waiting_client(true);
    enable(&state, true);
    tick(&bot, &state).await;
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 1);
    assert!(!state.shared.as_ref().unwrap().lock().ready);
    assert_eq!(state.shared.as_ref().unwrap().lock().tick, 0);

    bot.ecs.lock().entity_mut(bot.entity).insert(Health(20.0));
    tick(&bot, &state).await;
    bot.ecs.lock().entity_mut(bot.entity).insert(Health(0.0));
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 2);
}

#[tokio::test]
async fn enabling_after_dead_login_works_and_disabled_fights_stay_dead() {
    let (bot, state) = waiting_client(true);
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 0);
    enable(&state, true);
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 1);

    enable(&state, false);
    bot.ecs.lock().entity_mut(bot.entity).insert(Health(20.0));
    tick(&bot, &state).await;
    bot.ecs.lock().entity_mut(bot.entity).insert(Health(0.0));
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 1);
    enable(&state, true);
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 2);
}

#[tokio::test]
async fn login_and_pause_gates_prevent_premature_respawn() {
    let (bot, state) = waiting_client(false);
    enable(&state, true);
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 0);
    bot.ecs
        .lock()
        .entity_mut(bot.entity)
        .insert(InstanceName(azalea::Identifier::new("minecraft:overworld")));
    state.flags.paused.store(true, Ordering::Release);
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 0);
    state.flags.paused.store(false, Ordering::Release);
    tick(&bot, &state).await;
    assert_eq!(respawns(&bot), 1);
}
