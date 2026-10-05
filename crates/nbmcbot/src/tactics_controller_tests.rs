use super::*;
use azalea::ecs as bevy_ecs;
use azalea::{
    app::App,
    core::tick::GameTick,
    ecs::prelude::*,
    entity::{HasClientLoaded, Jumping, dimensions::EntityDimensions, inventory::Inventory},
    interact::BlockStatePredictionHandler,
    inventory::{ItemStack, LastSentSelectedHotbarSlot},
    movement::{StartSprintEvent, StartWalkEvent},
    packet::game::SendGamePacketEvent,
    protocol::packets::game::ServerboundGamePacket,
};
use std::sync::Arc;

#[derive(Resource, Default)]
struct Packets(Vec<ServerboundGamePacket>);

pub(super) fn client() -> Client {
    let mut app = App::new();
    app.add_plugins(crate::aerial::AerialPlugin)
        .init_resource::<Packets>()
        .add_message::<StartWalkEvent>()
        .add_message::<StartSprintEvent>()
        .add_message::<azalea::attack::AttackEvent>()
        .add_message::<azalea::pathfinder::GotoEvent>()
        .add_message::<azalea::pathfinder::PathFoundEvent>()
        .add_message::<azalea::pathfinder::StopPathfindingEvent>()
        .add_message::<azalea::mining::StartMiningBlockEvent>()
        .add_message::<azalea::mining::StopMiningBlockEvent>()
        .add_observer(azalea::inventory::handle_set_selected_hotbar_slot_event)
        .add_observer(
            |event: On<SendGamePacketEvent>, mut packets: ResMut<Packets>| {
                packets.0.push(event.packet.clone());
            },
        )
        .add_systems(GameTick, azalea::inventory::ensure_has_sent_carried_item);
    let mut inventory = Inventory::default();
    for (slot, kind) in [
        (36, ItemKind::Mace),
        (37, ItemKind::WindCharge),
        (38, ItemKind::EnderPearl),
        (39, ItemKind::NetheriteSpear),
        (6, ItemKind::Elytra),
        (45, ItemKind::FireworkRocket),
    ] {
        *inventory.inventory_menu.slot_mut(slot).unwrap() = ItemStack::new(kind, 1);
    }
    let entity = app
        .world_mut()
        .spawn((
            Position::new(Vec3::new(100.0, 70.0, 100.0)),
            EntityDimensions::new(0.6, 1.8).eye_height(1.62),
            LookDirection::new(-90.0, 0.0),
            Physics::default(),
            Jumping::default(),
            HasClientLoaded,
            Health(20.0),
            FallFlying(true),
            inventory,
            LastSentSelectedHotbarSlot { slot: 0 },
            BlockStatePredictionHandler::default(),
            azalea::tick_counter::TicksConnected(100),
            azalea::world::InstanceName(azalea::Identifier::new("minecraft:overworld")),
            azalea::entity::indexing::EntityIdIndex::default(),
            azalea::Account::offline("Test"),
        ))
        .id();
    app.world_mut()
        .entity_mut(entity)
        .insert(azalea::local_player::InstanceHolder::new(
            entity,
            Arc::new(parking_lot::RwLock::new(azalea::world::Instance::default())),
        ));
    Client {
        entity,
        ecs: Arc::new(parking_lot::Mutex::new(std::mem::take(app.world_mut()))),
    }
}

fn target(bot: &Client, x: f64, z: f64) -> Target {
    Target {
        entity: bot.ecs.lock().spawn_empty().id(),
        name: "Enemy1".into(),
        position: Vec3::new(x, 70.0, z),
        aim: Vec3::new(x, 70.9, z),
        distance_sq: 100.0,
    }
}

pub(super) fn packet_tick(bot: &Client) {
    bot.ecs.lock().run_schedule(GameTick);
}

fn use_count(bot: &Client) -> usize {
    bot.ecs
        .lock()
        .resource::<Packets>()
        .0
        .iter()
        .filter(|p| matches!(p, ServerboundGamePacket::UseItem(_)))
        .count()
}

pub(super) fn hand_use_count(bot: &Client, hand: InteractionHand) -> usize {
    bot.ecs
        .lock()
        .resource::<Packets>()
        .0
        .iter()
        .filter(|packet| {
            matches!(packet,
        ServerboundGamePacket::UseItem(packet) if packet.hand == hand)
        })
        .count()
}

fn armored_recovery_client() -> Client {
    let bot = client();
    {
        let mut ecs = bot.ecs.lock();
        ecs.entity_mut(bot.entity)
            .insert((FallFlying(false), azalea::world::MinecraftEntityId(42)));
        ecs.get_mut::<Physics>(bot.entity).unwrap().velocity.y = -2.0;
        let mut inventory = ecs.get_mut::<Inventory>(bot.entity).unwrap();
        *inventory.inventory_menu.slot_mut(6).unwrap() =
            ItemStack::new(ItemKind::NetheriteChestplate, 1);
        *inventory.inventory_menu.slot_mut(40).unwrap() = ItemStack::new(ItemKind::Elytra, 1);
        *inventory.inventory_menu.slot_mut(45).unwrap() = ItemStack::Empty;
    }
    bot
}

fn start_gliding_count(bot: &Client) -> usize {
    bot.ecs.lock().resource::<Packets>().0.iter().filter(|packet| {
        matches!(packet, ServerboundGamePacket::PlayerCommand(packet)
            if packet.action == azalea::protocol::packets::game::s_player_command::Action::StartFallFlying)
    }).count()
}

#[test]
fn mace_airborne_recovery_restores_elytra_and_glides_without_rockets_until_landing() {
    let bot = armored_recovery_client();
    let selected = enemy(&bot, "Enemy1", 110.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::Recover;
    duel.mace_ground_y = Some(64.0);
    duel.tick(&bot, 100, "Test").unwrap();
    assert_eq!(
        duel.target,
        Some(selected),
        "recovery must still refresh target selection"
    );
    assert!(has_chest(&bot, ItemKind::Elytra));
    assert_eq!(
        start_gliding_count(&bot),
        0,
        "wait for the equipment swap tick"
    );
    bot.ecs
        .lock()
        .get_mut::<azalea::tick_counter::TicksConnected>(bot.entity)
        .unwrap()
        .0 = 101;
    duel.tick(&bot, 101, "Test").unwrap();
    duel.tick(&bot, 102, "Test").unwrap();
    assert_eq!(start_gliding_count(&bot), 1);
    assert_eq!(duel.phase, Phase::Recover);
    assert!((bot.get_component::<LookDirection>().unwrap().x_rot() - 15.0).abs() < 0.001);
    assert_eq!(
        bot.ecs
            .lock()
            .get::<Inventory>(bot.entity)
            .unwrap()
            .held_item()
            .kind(),
        ItemKind::Mace
    );
    packet_tick(&bot);
    assert_eq!(use_count(&bot), 0);
    bot.ecs
        .lock()
        .get_mut::<Physics>(bot.entity)
        .unwrap()
        .set_on_ground(true);
    duel.tick(&bot, 114, "Test").unwrap();
    assert_eq!(duel.phase, Phase::Approach);
    assert_eq!(start_gliding_count(&bot), 1);
}

#[test]
fn mace_recovery_resumes_airborne_attacks_after_the_glide_is_stable() {
    let bot = armored_recovery_client();
    let selected = enemy(&bot, "Enemy1", 110.0);
    {
        let mut ecs = bot.ecs.lock();
        *ecs.get_mut::<Position>(bot.entity).unwrap() =
            Position::new(Vec3::new(100.0, 800.0, 100.0));
        *ecs.get_mut::<Position>(selected).unwrap() = Position::new(Vec3::new(110.0, 790.0, 100.0));
        *ecs.get_mut::<Inventory>(bot.entity)
            .unwrap()
            .inventory_menu
            .slot_mut(45)
            .unwrap() = ItemStack::new(ItemKind::FireworkRocket, 64);
    }
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::Recover;
    duel.since = 100;
    duel.mace_ground_y = Some(64.0);
    duel.tick(&bot, 100, "Test").unwrap();
    bot.ecs
        .lock()
        .get_mut::<azalea::tick_counter::TicksConnected>(bot.entity)
        .unwrap()
        .0 = 101;
    duel.tick(&bot, 101, "Test").unwrap();
    assert!(has_chest(&bot, ItemKind::Elytra));
    assert_eq!(start_gliding_count(&bot), 1);
    duel.tick(&bot, 140, "Test").unwrap();
    assert_eq!(
        duel.phase,
        Phase::Recover,
        "fast descent is not a safe glide"
    );
    bot.ecs
        .lock()
        .get_mut::<Physics>(bot.entity)
        .unwrap()
        .velocity
        .y = -0.2;
    for tick in 141..=160 {
        bot.ecs
            .lock()
            .get_mut::<azalea::tick_counter::TicksConnected>(bot.entity)
            .unwrap()
            .0 = tick;
        duel.tick(&bot, tick, "Test").unwrap();
        packet_tick(&bot);
        if tick == 141 {
            assert_eq!(
                duel.phase,
                Phase::Recover,
                "one predicted glide tick is insufficient"
            );
        }
    }
    assert!(
        matches!(duel.phase, Phase::MaceClimb | Phase::MaceDive),
        "a recovered flight kit must resume attacking without descending 736 blocks"
    );
    assert!(hand_use_count(&bot, InteractionHand::OffHand) > 0);
    assert!(has_chest(&bot, ItemKind::Elytra));
    assert_eq!(duel.target, Some(selected));
}

#[test]
fn mace_recovery_without_any_target_saves_airborne_bot_but_never_glides_on_ground() {
    for grounded in [false, true] {
        let bot = armored_recovery_client();
        bot.ecs
            .lock()
            .get_mut::<Physics>(bot.entity)
            .unwrap()
            .set_on_ground(grounded);
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        duel.phase = Phase::Recover;
        duel.mace_ground_y = Some(64.0);
        duel.tick(&bot, 100, "Test").unwrap();
        bot.ecs
            .lock()
            .get_mut::<azalea::tick_counter::TicksConnected>(bot.entity)
            .unwrap()
            .0 = 101;
        duel.tick(&bot, 101, "Test").unwrap();
        assert_eq!(start_gliding_count(&bot), usize::from(!grounded));
        assert_eq!(has_chest(&bot, ItemKind::Elytra), !grounded);
        assert_eq!(duel.phase, Phase::Recover);
        assert!(duel.target.is_none());
        packet_tick(&bot);
        assert_eq!(use_count(&bot), 0);
    }
}

#[test]
fn mace_flight_kit_launches_with_rocket_while_keeping_mace_in_hand() {
    let bot = client();
    *bot.ecs
        .lock()
        .get_mut::<Inventory>(bot.entity)
        .unwrap()
        .inventory_menu
        .slot_mut(40)
        .unwrap() = ItemStack::new(ItemKind::NetheriteChestplate, 1);
    let target = target(&bot, 103.0, 100.0);
    let physics = Physics::default();
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    for tick in 100..104 {
        duel.mace(&bot, tick, "Test", &target, Vec3::ZERO, &physics)
            .unwrap();
        packet_tick(&bot);
    }
    assert_eq!(
        hand_use_count(&bot, InteractionHand::OffHand),
        1,
        "an airborne flight-kit mace should boost instead of throwing a pearl"
    );
    assert_eq!(hand_use_count(&bot, InteractionHand::MainHand), 0);
    assert_eq!(
        bot.ecs
            .lock()
            .get::<Inventory>(bot.entity)
            .unwrap()
            .selected_hotbar_slot,
        0
    );
}

#[test]
fn mace_waits_for_server_to_end_gliding_before_armored_drop() {
    let bot = client();
    let target = target(&bot, 103.0, 100.0);
    let physics = Physics::default();
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceSwap;
    duel.since = 100;
    *bot.ecs
        .lock()
        .get_mut::<Inventory>(bot.entity)
        .unwrap()
        .inventory_menu
        .slot_mut(6)
        .unwrap() = ItemStack::new(ItemKind::NetheriteChestplate, 1);
    duel.mace(&bot, 101, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(
        duel.phase,
        Phase::MaceSwap,
        "predicted equipment alone must not authorize a smash"
    );
    bot.ecs
        .lock()
        .entity_mut(bot.entity)
        .insert(FallFlying(false));
    duel.mace(&bot, 102, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::MaceDrop);
    assert_eq!(duel.peak_y, 70.0);
}

#[test]
fn mace_grounded_stale_gliding_does_not_consume_rocket() {
    let bot = client();
    let target = target(&bot, 120.0, 100.0);
    let mut physics = Physics::default();
    physics.set_on_ground(true);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceTakeoff;
    duel.mace(&bot, 101, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    packet_tick(&bot);
    assert_eq!(duel.phase, Phase::MaceTakeoff);
    assert_eq!(use_count(&bot), 0);
}

#[test]
fn mace_cuts_elytra_only_for_a_fresh_reachable_moving_intercept() {
    for (observed, speed, swaps) in [(true, 0.2, true), (false, 0.2, false), (true, 1.8, false)] {
        let bot = client();
        *bot.ecs
            .lock()
            .get_mut::<Inventory>(bot.entity)
            .unwrap()
            .inventory_menu
            .slot_mut(40)
            .unwrap() = ItemStack::new(ItemKind::NetheriteChestplate, 1);
        let mut target = target(&bot, 104.0, 100.0);
        target.position.y = 64.0;
        target.aim.y = 64.8;
        let mut physics = Physics::default();
        physics.velocity = Vec3::new(1.0, -0.5, 0.0);
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        duel.phase = Phase::MaceDive;
        duel.since = 90;
        duel.velocity_ready = observed;
        duel.mace_equipped_at = Some(50);
        duel.mace(
            &bot,
            100,
            "Test",
            &target,
            Vec3::new(speed, 0.0, 0.0),
            &physics,
        )
        .unwrap();
        let ecs = bot.ecs.lock();
        let inventory = ecs.get::<Inventory>(bot.entity).unwrap();
        assert_eq!(inventory.held_item().kind(), ItemKind::Mace);
        assert_eq!(
            inventory.inventory_menu.slot(6).unwrap().kind(),
            if swaps {
                ItemKind::NetheriteChestplate
            } else {
                ItemKind::Elytra
            }
        );
        assert_eq!(
            ecs.resource::<Packets>()
                .0
                .iter()
                .filter(|p| matches!(p, ServerboundGamePacket::ContainerClick(_)))
                .count(),
            usize::from(swaps)
        );
    }
}

#[test]
fn mace_dive_does_not_chase_a_rising_spear_into_an_unbounded_climb() {
    let bot = client();
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, 100.0, 100.0));
    let mut target = target(&bot, 115.0, 100.0);
    target.position.y = 110.0;
    target.aim.y = 110.8;
    let mut physics = Physics::default();
    physics.velocity = Vec3::new(1.0, 0.0, 0.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.launch_y = 64.0;
    duel.mace_ground_y = Some(64.0);
    duel.phase = Phase::MaceClimb;
    duel.mace_equipped_at = Some(50);
    duel.mace(&bot, 100, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::MaceClimb);
    assert!(bot.get_component::<LookDirection>().unwrap().x_rot() < 0.0);
    let height = duel.mace_climb_height.unwrap();
    assert!(height > target.position.y + 10.0);
    assert!(height <= 64.0 + 80.0);

    target.position.y = 200.0;
    target.aim.y = 200.8;
    duel.mace(
        &bot,
        101,
        "Test",
        &target,
        Vec3::new(0.0, 1.0, 0.0),
        &physics,
    )
    .unwrap();
    assert_eq!(duel.mace_climb_height, Some(height));
    assert_eq!(duel.phase, Phase::MaceClimb);

    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, height, 100.0));
    duel.mace(&bot, 102, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::MaceDive);
    duel.mace(
        &bot,
        103,
        "Test",
        &target,
        Vec3::new(0.0, 1.0, 0.0),
        &physics,
    )
    .unwrap();
    assert_eq!(
        duel.phase,
        Phase::MaceDive,
        "finish the dive instead of toggling climb every tick"
    );
    assert!(
        bot.get_component::<LookDirection>().unwrap().x_rot() > 0.0,
        "the dive should point down when the enemy climbs beyond the engagement ceiling"
    );
    for tick in [115, 130, 145] {
        *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
            Position::new(Vec3::new(100.0, height - 8.0, 100.0));
        duel.mace(&bot, tick, "Test", &target, Vec3::ZERO, &physics)
            .unwrap();
        assert_eq!(
            duel.phase,
            Phase::MaceDive,
            "an escaped target must not restart the upward chase after twelve ticks"
        );
        assert!(
            bot.get_component::<LookDirection>().unwrap().x_rot() > 0.0,
            "dropping below the old climb height must not turn the dive upward"
        );
    }
}

#[test]
fn mace_climb_allows_for_a_rising_target_and_first_weapon_cooldown() {
    for (position_y, target_y, speed_y, equipped_at, minimum_height) in [
        (100.0, 110.0, 1.0, Some(50), 140.0),
        (72.0, 64.0, 0.0, None, 96.0),
    ] {
        let bot = client();
        *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
            Position::new(Vec3::new(100.0, position_y, 100.0));
        let mut target = target(&bot, 115.0, 100.0);
        target.position.y = target_y;
        target.aim.y = target_y + 0.8;
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        duel.phase = Phase::MaceClimb;
        duel.launch_y = 64.0;
        duel.mace_ground_y = Some(64.0);
        duel.mace_equipped_at = equipped_at;
        duel.mace(
            &bot,
            100,
            "Test",
            &target,
            Vec3::new(0.0, speed_y, 0.0),
            &Physics::default(),
        )
        .unwrap();
        assert_eq!(duel.phase, Phase::MaceClimb);
        let height = duel.mace_climb_height.unwrap();
        assert!(height >= minimum_height);
        assert!(height <= position_y + 48.0);
    }
}

#[test]
fn mace_can_get_above_an_opponent_above_the_previous_ground_ceiling() {
    let bot = client();
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, 140.0, 100.0));
    let mut target = target(&bot, 115.0, 100.0);
    target.position.y = 150.0;
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceClimb;
    duel.mace_ground_y = Some(64.0);
    duel.mace_equipped_at = Some(50);
    duel.mace(&bot, 100, "Test", &target, Vec3::ZERO, &Physics::default())
        .unwrap();
    assert!(duel.mace_climb_height.unwrap() > 150.0);
    assert!(duel.mace_climb_height.unwrap() <= 188.0);
}

#[test]
fn mace_boosts_a_slow_dive_to_catch_a_flying_target() {
    let bot = client();
    let mut target = target(&bot, 120.0, 100.0);
    target.position.y = 64.0;
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceDive;
    duel.since = 90;
    duel.last_rocket = 50;
    duel.mace_ground_y = Some(64.0);
    duel.mace_climb_height = Some(80.0);
    let mut physics = Physics::default();
    physics.velocity = Vec3::new(0.1, -0.2, 0.0);
    duel.mace(
        &bot,
        100,
        "Test",
        &target,
        Vec3::new(1.5, 0.0, 0.0),
        &physics,
    )
    .unwrap();
    packet_tick(&bot);
    assert_eq!(hand_use_count(&bot, InteractionHand::OffHand), 1);
    assert_eq!(hand_use_count(&bot, InteractionHand::MainHand), 0);
}

#[test]
fn mace_rebuilds_height_after_a_spear_escapes_above_an_established_dive() {
    let bot = client();
    let mut target = target(&bot, 120.0, 100.0);
    target.position.y = 80.0;
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceDive;
    duel.since = 80;
    duel.mace_ground_y = Some(64.0);
    duel.mace(&bot, 100, "Test", &target, Vec3::ZERO, &Physics::default())
        .unwrap();
    assert_eq!(duel.phase, Phase::MaceClimb);
}

#[test]
fn target_turn_rate_is_bounded_and_expires_with_stale_motion() {
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.sample_velocity(Vec3::new(0.0, 64.0, 0.0), 100);
    duel.sample_velocity(Vec3::new(1.0, 64.0, 0.0), 101);
    duel.sample_velocity(Vec3::new(1.0, 64.0, 1.0), 102);
    assert!((duel.recent_turn - std::f64::consts::FRAC_PI_6).abs() < 1e-10);
    duel.sample_velocity(Vec3::new(1.0, 64.0, 1.0), 103);
    assert!(duel.recent_turn > 0.0);
    for tick in 104..107 {
        duel.sample_velocity(Vec3::new(1.0, 64.0, 1.0), tick);
    }
    assert_eq!(duel.recent_turn, 0.0);
}

fn circular_motion(tick: u64, turn: f64) -> (Vec3, Vec3) {
    let mut position = Vec3::new(0.0, 64.0, 0.0);
    for step in 0..tick {
        position += Vec3::new((step as f64 * turn).cos(), 0.2, (step as f64 * turn).sin());
    }
    let velocity = Vec3::new((tick as f64 * turn).cos(), 0.2, (tick as f64 * turn).sin());
    (position, velocity)
}

#[test]
fn turn_rate_uses_interval_midpoints_when_packet_spacing_changes() {
    for turn in [-std::f64::consts::PI / 18.0, std::f64::consts::PI / 18.0] {
        for samples in [[0, 3, 4], [0, 1, 4], [0, 2, 5], [0, 3, 6]] {
            let mut duel = Duel::new("mace".into(), "Enemy".into());
            for tick in samples {
                duel.sample_velocity(circular_motion(tick, turn).0, 100 + tick);
            }
            assert!(
                (duel.recent_turn - turn).abs() < 1e-10,
                "samples {samples:?}"
            );
        }
    }
}

#[test]
fn predicted_velocity_is_the_next_step_tangent_without_mutating_raw_chord() {
    for interval in 1..=3 {
        let turn = std::f64::consts::PI / 12.0;
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        for tick in [0, interval, 2 * interval] {
            duel.sample_velocity(circular_motion(tick, turn).0, 100 + tick);
        }
        let tick = 2 * interval;
        let (actual_position, actual_velocity) = circular_motion(tick, turn);
        let raw_chord = duel.recent_velocity;
        let (position, velocity) = duel.predicted_motion(actual_position, 100 + tick);
        assert!(position.distance_squared_to(actual_position) < 1e-18);
        assert!(velocity.distance_squared_to(actual_velocity) < 1e-18);
        assert_eq!(duel.recent_velocity, raw_chord);
    }
}

#[test]
fn predicted_motion_advances_stale_packet_origin_then_expires() {
    for turn in [
        0.0,
        std::f64::consts::PI / 18.0,
        -std::f64::consts::PI / 18.0,
    ] {
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        for tick in [0, 3, 6] {
            duel.sample_velocity(circular_motion(tick, turn).0, 100 + tick);
        }
        let packet_position = circular_motion(6, turn).0;
        for age in 1..=3 {
            duel.sample_velocity(packet_position, 106 + age);
            let (position, velocity) = duel.predicted_motion(packet_position, 106 + age);
            let (actual_position, actual_velocity) = circular_motion(6 + age, turn);
            assert!(
                position.distance_squared_to(actual_position) < 1e-18,
                "age {age}"
            );
            assert!(
                velocity.distance_squared_to(actual_velocity) < 1e-18,
                "age {age}"
            );
        }
        duel.sample_velocity(packet_position, 110);
        assert_eq!(
            duel.predicted_motion(packet_position, 110),
            (packet_position, Vec3::ZERO)
        );
    }
}

#[test]
fn prediction_does_not_survive_a_gap_or_target_reset() {
    for reset in [false, true] {
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        let turn = std::f64::consts::PI / 18.0;
        for tick in [0, 3, 6] {
            duel.sample_velocity(circular_motion(tick, turn).0, 100 + tick);
        }
        let position = Vec3::new(100.0, 80.0, 100.0);
        if reset {
            duel.reset_velocity();
            duel.sample_velocity(position, 107);
        } else {
            duel.sample_velocity(position, 110);
        }
        assert!(!duel.velocity_ready);
        assert_eq!(duel.predicted_motion(position, 110), (position, Vec3::ZERO));
    }
}

#[test]
fn mace_retargeting_in_air_preserves_ground_base_until_landing() {
    let bot = client();
    let mut target = target(&bot, 115.0, 100.0);
    target.position.y = 200.0;
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceTakeoff;
    let mut physics = Physics::default();
    physics.set_on_ground(true);
    duel.mace(&bot, 100, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.mace_ground_y, Some(70.0));

    physics.set_on_ground(false);
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, 130.0, 100.0));
    duel.phase(Phase::MaceClimb, 101, "Test");
    duel.mace(&bot, 101, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.mace_climb_height, Some(178.0));

    duel.phase(Phase::Approach, 102, "Test");
    duel.mace_flight(&bot, 102, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    duel.mace_flight(&bot, 103, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.mace_ground_y, Some(70.0));
    assert_eq!(duel.mace_climb_height, Some(178.0));

    physics.set_on_ground(true);
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, 80.0, 100.0));
    duel.mace(&bot, 104, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.mace_ground_y, Some(80.0));
    physics.set_on_ground(false);
    duel.mace(&bot, 105, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    duel.mace(&bot, 106, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.mace_climb_height, Some(128.0));
}

#[test]
fn wind_jump_waits_for_slot_sync_and_uses_wind_once() {
    let bot = client();
    let target = target(&bot, 104.0, 100.0);
    let physics = Physics::default();
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::WindJump;
    duel.since = 100;
    duel.mace(&bot, 100, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::WindJump);
    assert_eq!(use_count(&bot), 0);
    assert_eq!(
        bot.ecs
            .lock()
            .get::<Inventory>(bot.entity)
            .unwrap()
            .selected_hotbar_slot,
        1
    );
    packet_tick(&bot);
    duel.mace(&bot, 101, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::Fall);
    packet_tick(&bot);
    assert_eq!(use_count(&bot), 1);
    let ecs = bot.ecs.lock();
    let packets = &ecs.resource::<Packets>().0;
    let select = packets
        .iter()
        .position(|p| matches!(p,ServerboundGamePacket::SetCarriedItem(p) if p.slot==1))
        .unwrap();
    let used = packets
        .iter()
        .position(|p| matches!(p, ServerboundGamePacket::UseItem(_)))
        .unwrap();
    assert!(select < used);
}

#[test]
fn spear_does_not_commit_a_pass_while_vertically_separated() {
    for target_y in [42.55, 97.45] {
        let bot = client();
        let mut duel = Duel::new("spear".into(), "Enemy".into());
        duel.phase = Phase::Charge;
        duel.since = 100;
        duel.last_rocket = 110;
        let mut separated = target(&bot, 103.15, 100.0);
        separated.position.y = target_y;
        separated.aim.y = target_y + 0.9;
        duel.spear(
            &bot,
            112,
            "Test",
            &separated,
            Vec3::ZERO,
            &Physics::default(),
        )
        .unwrap();
        assert_eq!(duel.phase, Phase::Charge);
        assert!(
            !duel.pass_committed,
            "horizontal proximity must not arm a pass tens of blocks above or below the target"
        );
    }
}

#[test]
fn spear_does_not_exit_a_charge_while_vertically_separated() {
    for target_y in [42.55, 97.45] {
        let bot = client();
        let mut duel = Duel::new("spear".into(), "Enemy".into());
        duel.phase = Phase::Charge;
        duel.since = 100;
        duel.last_rocket = 110;
        let mut separated = target(&bot, 102.5, 100.0);
        separated.position.y = target_y;
        separated.aim.y = target_y + 0.9;
        duel.spear(
            &bot,
            112,
            "Test",
            &separated,
            Vec3::ZERO,
            &Physics::default(),
        )
        .unwrap();
        assert_eq!(
            duel.phase,
            Phase::Charge,
            "keep approaching until actually close instead of pulling up above the target"
        );
        duel.spear(
            &bot,
            201,
            "Test",
            &separated,
            Vec3::ZERO,
            &Physics::default(),
        )
        .unwrap();
        assert_eq!(
            duel.phase,
            Phase::Exit,
            "a missed charge must still time out"
        );
    }
}

#[test]
fn spear_pass_latches_heading_and_does_not_cross_an_unvisited_origin() {
    let bot = client();
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.phase = Phase::Charge;
    duel.since = 100;
    duel.last_rocket = 110;
    let mut physics = Physics::default();
    physics.velocity = Vec3::new(1.0, 0.0, 0.0);
    let far = target(&bot, 130.0, 100.0);
    duel.spear(&bot, 111, "Test", &far, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::Charge);
    assert!(!duel.pass_committed);
    packet_tick(&bot);
    let near = target(&bot, 105.0, 100.0);
    duel.spear(&bot, 112, "Test", &near, Vec3::ZERO, &physics)
        .unwrap();
    let heading = duel.pass_heading;
    assert!(duel.pass_committed);
    packet_tick(&bot);
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(112.0, 70.0, 100.0));
    duel.spear(&bot, 121, "Test", &near, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::Exit);
    assert_eq!(duel.pass_heading, heading);
}

#[test]
fn cancel_removes_queued_charge_use_before_next_packet_tick() {
    let bot = client();
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.charging = true;
    assert!(crate::aerial::use_item(&bot, InteractionHand::MainHand));
    duel.cancel(&bot, true);
    packet_tick(&bot);
    assert_eq!(use_count(&bot), 0);
    assert!(!duel.charging);
    assert!(bot.ecs.lock().resource::<Packets>().0.iter().any(|p|matches!(p,ServerboundGamePacket::PlayerAction(p) if p.action==azalea::protocol::packets::game::s_player_action::Action::ReleaseUseItem)));
}

#[test]
fn spear_keeps_one_use_held_through_the_eight_tick_charge_delay() {
    let bot = client();
    let target = target(&bot, 110.0, 100.0);
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.phase = Phase::Charge;
    duel.since = 100;
    duel.last_rocket = 100;
    let physics = Physics::default();
    for tick in 100..=110 {
        duel.spear(&bot, tick, "Test", &target, Vec3::ZERO, &physics)
            .unwrap();
        packet_tick(&bot);
        assert_eq!(duel.phase, Phase::Charge);
        if tick == 101 {
            assert_eq!(hand_use_count(&bot, InteractionHand::MainHand), 0);
        }
        if tick == 102 {
            assert_eq!(hand_use_count(&bot, InteractionHand::MainHand), 1);
        }
    }
    assert!(duel.charging);
    assert_eq!(duel.last_use, 102);
    assert!(110 - duel.last_use >= 8);
    assert_eq!(use_count(&bot), 1);
}

#[test]
fn team_selector_can_target_another_local_account_and_filters_ineligible_players() {
    use azalea::{
        Account,
        auth::game_profile::GameProfile,
        entity::{LocalEntity, indexing::EntityUuidIndex},
        local_player::TabList,
        player::PlayerInfo,
    };
    let bot = client();
    let mut ecs = bot.ecs.lock();
    ecs.init_resource::<EntityUuidIndex>();
    let mut tab = TabList::default();
    let mut expected = None;
    for (name, mode, health, loaded, is_self) in [
        ("EnemyAlive", GameMode::Survival, 20.0, true, false),
        ("EnemyDead", GameMode::Survival, 0.0, true, false),
        ("EnemyCreative", GameMode::Creative, 20.0, true, false),
        ("EnemyUnloaded", GameMode::Survival, 20.0, false, false),
        ("AllyAlive", GameMode::Survival, 20.0, true, false),
        ("EnemySelf", GameMode::Survival, 20.0, true, true),
    ] {
        let account = Account::offline(name);
        let uuid = account.uuid_or_offline();
        let position = Vec3::new(104.0, 70.0, 100.0);
        let mut physics = Physics::default();
        physics.bounding_box = EntityDimensions::new(0.6, 1.8).make_bounding_box(position);
        let entity = if is_self {
            bot.entity
        } else {
            ecs.spawn_empty().id()
        };
        ecs.entity_mut(entity).insert((
            account,
            LocalEntity,
            azalea::entity::EntityUuid::new(uuid),
            azalea::world::InstanceName(azalea::Identifier::new("minecraft:overworld")),
            Position::new(position),
            physics,
            Health(health),
            LoadedBy(if loaded {
                [bot.entity].into_iter().collect()
            } else {
                Default::default()
            }),
        ));
        ecs.resource_mut::<EntityUuidIndex>().insert(uuid, entity);
        let id = azalea::world::MinecraftEntityId(tab.len() as i32 + 1);
        ecs.get_mut::<azalea::entity::indexing::EntityIdIndex>(bot.entity)
            .unwrap()
            .insert(id, entity);
        tab.insert(
            uuid,
            PlayerInfo {
                profile: GameProfile::new(uuid, name.into()),
                uuid,
                gamemode: mode,
                latency: 0,
                display_name: None,
            },
        );
        if name == "EnemyAlive" {
            expected = Some(entity);
        }
    }
    ecs.entity_mut(bot.entity).insert(tab);
    drop(ecs);
    let tracking::Tracking::Visible(target) =
        tracking::Tracker::default().update(&bot, "enemy", 100)
    else {
        panic!("eligible local opponent must be selected");
    };
    assert_eq!(Some(target.entity), expected);
}

fn enemy(bot: &Client, name: &str, x: f64) -> Entity {
    use azalea::{
        Account,
        auth::game_profile::GameProfile,
        entity::{LocalEntity, indexing::EntityUuidIndex},
        local_player::TabList,
        player::PlayerInfo,
    };
    let mut ecs = bot.ecs.lock();
    ecs.init_resource::<EntityUuidIndex>();
    let account = Account::offline(name);
    let uuid = account.uuid_or_offline();
    let position = Vec3::new(x, 70.0, 100.0);
    let mut physics = Physics::default();
    physics.bounding_box = EntityDimensions::new(0.6, 1.8).make_bounding_box(position);
    let world = ecs
        .get::<azalea::world::InstanceName>(bot.entity)
        .unwrap()
        .clone();
    let id = azalea::world::MinecraftEntityId(ecs.entities().len() as i32 + 1);
    let entity = ecs
        .spawn((
            account,
            LocalEntity,
            Position::new(position),
            physics,
            Health(20.0),
            LoadedBy([bot.entity].into_iter().collect()),
            azalea::entity::EntityUuid::new(uuid),
            world,
            id,
        ))
        .id();
    ecs.resource_mut::<EntityUuidIndex>().insert(uuid, entity);
    ecs.get_mut::<azalea::entity::indexing::EntityIdIndex>(bot.entity)
        .unwrap()
        .insert(id, entity);
    let mut tab = ecs.get::<TabList>(bot.entity).cloned().unwrap_or_default();
    tab.insert(
        uuid,
        PlayerInfo {
            profile: GameProfile::new(uuid, name.into()),
            uuid,
            gamemode: GameMode::Survival,
            latency: 0,
            display_name: None,
        },
    );
    ecs.entity_mut(bot.entity).insert(tab);
    entity
}

#[test]
fn mace_keeps_its_live_opponent_when_another_becomes_nearer() {
    let bot = client();
    let selected = enemy(&bot, "EnemyFar", 120.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::Recover;
    duel.tick(&bot, 100, "Test").unwrap();
    enemy(&bot, "EnemyNear", 110.0);
    duel.tick(&bot, 101, "Test").unwrap();
    assert_eq!(duel.target, Some(selected));
}

#[test]
fn spear_keeps_its_live_opponent_when_another_becomes_nearer() {
    let bot = client();
    let selected = enemy(&bot, "EnemyFar", 120.0);
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.tick(&bot, 100, "Test").unwrap();
    enemy(&bot, "EnemyNear", 110.0);
    duel.tick(&bot, 101, "Test").unwrap();
    assert_eq!(duel.target, Some(selected));
}

#[test]
fn hidden_locked_target_is_pursued_without_attacking_a_nearer_replacement() {
    let bot = client();
    let selected = enemy(&bot, "EnemyFar", 120.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::Recover;
    duel.tick(&bot, 100, "Test").unwrap();
    bot.ecs
        .lock()
        .get_mut::<LoadedBy>(selected)
        .unwrap()
        .clear();
    enemy(&bot, "EnemyNear", 110.0);
    duel.tick(&bot, 101, "Test").unwrap();
    assert_eq!(
        duel.target, None,
        "a remembered enemy is a navigation goal, not an attackable entity"
    );
    assert!(
        bot.ecs
            .lock()
            .get::<azalea::attack::AttackQueued>(bot.entity)
            .is_none()
    );
}

#[test]
fn mace_velocity_ready_requires_two_recent_samples_of_the_same_target() {
    let bot = client();
    let selected = enemy(&bot, "Enemy1", 120.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::Recover;
    duel.target = Some(selected);
    duel.tick(&bot, 100, "Test").unwrap();
    assert!(!duel.velocity_ready);
    *bot.ecs.lock().get_mut::<Position>(selected).unwrap() =
        Position::new(Vec3::new(121.0, 70.0, 100.0));
    duel.tick(&bot, 101, "Test").unwrap();
    assert!(duel.velocity_ready);
    duel.tick(&bot, 105, "Test").unwrap();
    assert!(
        !duel.velocity_ready,
        "stale samples must not authorize a chestplate swap"
    );
    duel.tick(&bot, 105, "Test").unwrap();
    assert!(
        !duel.velocity_ready,
        "duplicate tick is not a velocity sample"
    );
    duel.tick(&bot, 106, "Test").unwrap();
    assert!(duel.velocity_ready);
}

#[test]
fn sparse_position_updates_keep_real_velocity_between_packets_and_expire_when_stopped() {
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    for tick in 100..=111 {
        let x = 120.0 + ((tick.min(106) - 100) / 3 * 3) as f64;
        let velocity = duel.sample_velocity(Vec3::new(x, 70.0, 100.0), tick);
        let expected = if (103..=109).contains(&tick) {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::ZERO
        };
        assert_eq!(velocity, expected, "incorrect velocity at tick {tick}");
        assert_eq!(duel.velocity_ready, tick != 100);
    }
}

#[test]
fn cached_motion_is_discarded_after_an_observation_gap_or_new_target() {
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    let position = Vec3::new(120.0, 70.0, 100.0);
    let moving = Vec3::new(123.0, 70.0, 100.0);
    assert_eq!(duel.sample_velocity(position, 100), Vec3::ZERO);
    assert_eq!(duel.sample_velocity(moving, 103), Vec3::new(1.0, 0.0, 0.0));
    assert_eq!(duel.sample_velocity(moving, 107), Vec3::ZERO);
    assert!(!duel.velocity_ready);
    assert_eq!(duel.sample_velocity(moving, 108), Vec3::ZERO);
    assert!(duel.velocity_ready);

    let resumed = Vec3::new(126.0, 70.0, 100.0);
    assert_eq!(duel.sample_velocity(resumed, 110), Vec3::new(1.0, 0.0, 0.0));
    duel.previous = None;
    assert_eq!(
        duel.sample_velocity(Vec3::new(200.0, 80.0, 100.0), 111),
        Vec3::ZERO
    );
    assert!(!duel.velocity_ready);
}

#[test]
fn losing_the_drop_target_waits_for_landing_before_starting_another_attack() {
    let bot = client();
    let selected = enemy(&bot, "Enemy1", 110.0);
    let replacement = enemy(&bot, "Enemy2", 120.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceDrop;
    duel.since = 100;
    duel.target = Some(selected);
    duel.previous = Some((Vec3::new(110.0, 70.0, 100.0), 100));
    duel.velocity_ready = true;
    bot.ecs.lock().get_mut::<Health>(selected).unwrap().0 = 0.0;
    duel.tick(&bot, 101, "Test").unwrap();
    assert_eq!(duel.target, Some(replacement));
    assert_eq!(duel.phase, Phase::Recover);
    assert!(
        !duel.velocity_ready,
        "do not reuse the defeated target's velocity"
    );
    duel.tick(&bot, 114, "Test").unwrap();
    assert_eq!(
        duel.phase,
        Phase::Recover,
        "an airborne bot must finish landing"
    );
    bot.ecs
        .lock()
        .get_mut::<Physics>(bot.entity)
        .unwrap()
        .set_on_ground(true);
    duel.tick(&bot, 115, "Test").unwrap();
    assert_eq!(duel.phase, Phase::Approach);
}

#[test]
fn reacquiring_after_an_empty_drop_target_set_keeps_airborne_recovery() {
    let bot = client();
    let selected = enemy(&bot, "Enemy1", 110.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceSwap;
    duel.target = Some(selected);
    duel.previous = Some((Vec3::new(110.0, 70.0, 100.0), 100));
    duel.velocity_ready = true;
    bot.ecs.lock().get_mut::<Health>(selected).unwrap().0 = 0.0;
    duel.tick(&bot, 101, "Test").unwrap();
    assert_eq!(duel.target, None);
    assert!(duel.previous.is_none());
    assert!(!duel.velocity_ready);
    assert_eq!(duel.phase, Phase::Recover);

    let replacement = enemy(&bot, "Enemy2", 120.0);
    duel.tick(&bot, 114, "Test").unwrap();
    assert_eq!(duel.target, Some(replacement));
    assert!(!duel.velocity_ready);
    assert_eq!(duel.phase, Phase::Recover);
}

#[test]
fn replacing_a_dive_target_restarts_approach_without_reusing_velocity() {
    let bot = client();
    let selected = enemy(&bot, "Enemy1", 110.0);
    let replacement = enemy(&bot, "Enemy2", 120.0);
    let mut duel = Duel::new("mace".into(), "Enemy".into());
    duel.phase = Phase::MaceDive;
    duel.target = Some(selected);
    duel.previous = Some((Vec3::new(110.0, 70.0, 100.0), 100));
    duel.velocity_ready = true;
    bot.ecs.lock().get_mut::<Health>(selected).unwrap().0 = 0.0;
    duel.tick(&bot, 101, "Test").unwrap();
    assert_eq!(duel.target, Some(replacement));
    assert_eq!(duel.phase, Phase::Approach);
    assert!(!duel.velocity_ready);
    duel.tick(&bot, 102, "Test").unwrap();
    assert!(duel.velocity_ready);
}

#[test]
fn takeoff_ignores_stale_gliding_flag_while_still_grounded() {
    let bot = client();
    let target = target(&bot, 110.0, 100.0);
    let mut physics = Physics::default();
    physics.set_on_ground(true);
    *bot.ecs.lock().get_mut::<Physics>(bot.entity).unwrap() = physics.clone();
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.spear(&bot, 100, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    packet_tick(&bot);
    assert_eq!(
        duel.phase,
        Phase::Takeoff,
        "grounded bot must finish jumping before climb"
    );
    assert_eq!(
        use_count(&bot),
        0,
        "grounded rocket cannot accelerate elytra flight"
    );
    assert!(bot.ecs.lock().get::<Jumping>(bot.entity).unwrap().0);
}

#[test]
fn climb_retries_missed_rocket_as_soon_as_cooldown_expires() {
    let bot = client();
    let target = target(&bot, 110.0, 100.0);
    let mut physics = Physics::default();
    physics.set_on_ground(false);
    physics.velocity = Vec3::new(0.1, -0.1, 0.0);
    *bot.ecs.lock().get_mut::<Physics>(bot.entity).unwrap() = physics.clone();
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.phase = Phase::Climb;
    duel.since = 100;
    duel.last_rocket = 111;
    duel.spear(&bot, 130, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    packet_tick(&bot);
    assert_eq!(
        hand_use_count(&bot, InteractionHand::OffHand),
        0,
        "rocket cooldown must be respected"
    );
    duel.spear(&bot, 131, "Test", &target, Vec3::ZERO, &physics)
        .unwrap();
    packet_tick(&bot);
    assert_eq!(
        hand_use_count(&bot, InteractionHand::OffHand),
        1,
        "slow climb needs the previously deferred boost"
    );
    assert_eq!(duel.last_rocket, 131);
    assert!(
        bot.ecs
            .lock()
            .resource::<Packets>()
            .0
            .iter()
            .any(|packet| matches!(packet,
        ServerboundGamePacket::UseItem(packet) if packet.hand == InteractionHand::OffHand))
    );
}

#[test]
fn spear_arms_during_climb_and_preserves_use_until_a_warm_close_pass() {
    let bot = client();
    {
        let mut ecs = bot.ecs.lock();
        ecs.get_mut::<Inventory>(bot.entity)
            .unwrap()
            .selected_hotbar_slot = 3;
        ecs.get_mut::<LastSentSelectedHotbarSlot>(bot.entity)
            .unwrap()
            .slot = 3;
    }
    let far = target(&bot, 114.0, 100.0);
    let mut physics = Physics::default();
    physics.velocity = Vec3::new(1.0, 0.2, 0.0);
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.phase = Phase::Climb;
    duel.since = 100;
    duel.last_rocket = 101;
    duel.spear(&bot, 104, "Test", &far, Vec3::ZERO, &physics)
        .unwrap();
    packet_tick(&bot);
    assert!(
        duel.charging,
        "start spear use while the launch rocket is lifting the bot"
    );
    assert_eq!(hand_use_count(&bot, InteractionHand::MainHand), 1);
    let started = duel.last_use;
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, 76.0, 100.0));
    duel.spear(&bot, 110, "Test", &far, Vec3::ZERO, &physics)
        .unwrap();
    assert_eq!(duel.phase, Phase::Charge);
    assert!(
        duel.charging,
        "entering the approach must preserve an already held spear"
    );
    duel.spear(&bot, 111, "Test", &far, Vec3::ZERO, &physics)
        .unwrap();
    packet_tick(&bot);
    assert_eq!(duel.last_use, started);
    assert_eq!(hand_use_count(&bot, InteractionHand::MainHand), 1);
    *bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap() =
        Position::new(Vec3::new(100.0, 70.0, 100.0));
    let near = target(&bot, 102.5, 100.0);
    duel.spear(&bot, 112, "Test", &near, Vec3::ZERO, &physics)
        .unwrap();
    assert!(
        112 - started >= 8,
        "spear must be active before crossing the target"
    );
    assert_eq!(
        duel.phase,
        Phase::Exit,
        "pull up while still within valid spear range"
    );
}

#[test]
fn cold_spear_close_pass_still_exits_without_queuing_a_melee_hit() {
    let bot = client();
    let near = target(&bot, 102.5, 100.0);
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.phase = Phase::Charge;
    duel.since = 108;
    duel.charging = true;
    duel.last_use = 108;
    duel.last_rocket = 101;
    duel.spear(&bot, 112, "Test", &near, Vec3::ZERO, &Physics::default())
        .unwrap();
    packet_tick(&bot);
    assert_eq!(
        duel.phase,
        Phase::Exit,
        "a missed cold pass must turn away from the ground"
    );
    assert!(
        bot.ecs
            .lock()
            .get::<azalea::attack::AttackQueued>(bot.entity)
            .is_none()
    );
    assert!(
        !bot.ecs
            .lock()
            .resource::<Packets>()
            .0
            .iter()
            .any(|packet| matches!(packet, ServerboundGamePacket::Interact(_)))
    );
}

#[test]
fn turn_finishes_against_a_moving_bearing_then_charge_reacquires_the_live_target() {
    let bot = client();
    let physics = Physics::default();
    let mut duel = Duel::new("spear".into(), "Enemy".into());
    duel.last_rocket = 100;
    duel.turn_yaw = Some(-135.0);
    duel.pass_committed = true;
    duel.phase(Phase::Turn, 100, "Test");
    assert_eq!(duel.turn_yaw, None);
    let mut latched = None;
    let mut finished_at = None;
    for offset in 0..=20 {
        // Keep the live target well ahead of the current turn, reproducing an orbit.
        let yaw = bot.get_component::<LookDirection>().unwrap().y_rot() + 120.0;
        let radians = f64::from(yaw).to_radians();
        let moving = target(
            &bot,
            100.0 - radians.sin() * 12.0,
            100.0 + radians.cos() * 12.0,
        );
        duel.spear(&bot, 100 + offset, "Test", &moving, Vec3::ZERO, &physics)
            .unwrap();
        packet_tick(&bot);
        let initial = *latched.get_or_insert(duel.turn_yaw.unwrap());
        assert_eq!(
            duel.turn_yaw,
            Some(initial),
            "turn destination must not chase the moving bearing"
        );
        if duel.phase == Phase::Charge {
            finished_at = Some(100 + offset);
            break;
        }
    }
    let finished_at =
        finished_at.expect("turn should complete within twenty ticks despite target motion");
    assert!(
        !duel.pass_committed,
        "a new charge must discard the previous pass plane"
    );
    let before = bot.get_component::<LookDirection>().unwrap().y_rot();
    let desired = before + 90.0;
    let radians = f64::from(desired).to_radians();
    let new_target = target(
        &bot,
        100.0 - radians.sin() * 10.0,
        100.0 + radians.cos() * 10.0,
    );
    duel.spear(
        &bot,
        finished_at + 1,
        "Test",
        &new_target,
        Vec3::ZERO,
        &physics,
    )
    .unwrap();
    let after = bot.get_component::<LookDirection>().unwrap().y_rot();
    let error = |yaw: f32| ((desired - yaw + 180.0).rem_euclid(360.0) - 180.0).abs();
    assert!(
        error(after) < error(before),
        "charge must steer toward the target's new position"
    );
    assert_eq!(duel.phase, Phase::Charge);
    assert!(!duel.pass_committed);
}
