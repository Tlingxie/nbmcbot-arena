use super::*;
use azalea::{
    Account, Identifier,
    auth::game_profile::GameProfile,
    connection::RawConnection,
    core::game_type::GameMode,
    ecs::{entity::Entity, world::World},
    entity::{
        Dead, EntityUuid, LoadedBy, Physics, Position,
        dimensions::EntityDimensions,
        indexing::{EntityIdIndex, EntityUuidIndex},
        metadata::Health,
    },
    local_player::TabList,
    player::{GameProfileComponent, PlayerInfo},
    protocol::packets::ConnectionProtocol,
    world::{InstanceName, MinecraftEntityId},
};
use std::sync::Arc;

fn dimension(name: &str) -> InstanceName {
    InstanceName(Identifier::new(name))
}

fn client() -> Client {
    let mut ecs = World::new();
    ecs.init_resource::<EntityUuidIndex>();
    let account = Account::offline("Mace001");
    let entity = ecs
        .spawn((
            GameProfileComponent(GameProfile::new(
                account.uuid_or_offline(),
                account.username.clone(),
            )),
            account,
            Position::new(Vec3::new(0.0, 64.0, 0.0)),
            EntityDimensions::new(0.6, 1.8).eye_height(1.62),
            dimension("minecraft:overworld"),
            Health(20.0),
            TabList::default(),
            EntityIdIndex::default(),
        ))
        .id();
    Client {
        entity,
        ecs: Arc::new(parking_lot::Mutex::new(ecs)),
    }
}

fn enemy(bot: &Client, name: &str, x: f64, observers: &[Entity]) -> Entity {
    let account = Account::offline(name);
    let uuid = account.uuid_or_offline();
    let position = Vec3::new(x, 64.0, 0.0);
    let mut physics = Physics::default();
    physics.bounding_box = EntityDimensions::new(0.6, 1.8).make_bounding_box(position);
    let mut ecs = bot.ecs.lock();
    let entity = ecs
        .spawn((
            EntityUuid::new(uuid),
            GameProfileComponent(GameProfile::new(uuid, name.into())),
            Position::new(position),
            physics,
            Health(20.0),
            LoadedBy(observers.iter().copied().collect()),
            dimension("minecraft:overworld"),
        ))
        .id();
    ecs.resource_mut::<EntityUuidIndex>().insert(uuid, entity);
    ecs.get_mut::<TabList>(bot.entity).unwrap().insert(
        uuid,
        PlayerInfo {
            profile: GameProfile::new(uuid, name.into()),
            uuid,
            gamemode: GameMode::Survival,
            latency: 0,
            display_name: None,
        },
    );
    ecs.get_mut::<EntityIdIndex>(bot.entity)
        .unwrap()
        .insert(MinecraftEntityId(x as i32), entity);
    for observer in observers {
        if let Some(mut index) = ecs.get_mut::<EntityIdIndex>(*observer) {
            index.insert(MinecraftEntityId(x as i32), entity);
        }
    }
    entity
}

fn ally(bot: &Client, name: &str) -> Entity {
    let account = Account::offline(name);
    let uuid = account.uuid_or_offline();
    let mut ecs = bot.ecs.lock();
    let entity = ecs
        .spawn((
            GameProfileComponent(GameProfile::new(uuid, name.into())),
            RawConnection::new_networkless(ConnectionProtocol::Game),
            EntityIdIndex::default(),
            Health(20.0),
            dimension("minecraft:overworld"),
        ))
        .id();
    ecs.get_mut::<TabList>(bot.entity).unwrap().insert(
        uuid,
        PlayerInfo {
            profile: GameProfile::new(uuid, name.into()),
            uuid,
            gamemode: GameMode::Survival,
            latency: 0,
            display_name: None,
        },
    );
    entity
}

fn visible(result: Tracking) -> Entity {
    match result {
        Tracking::Visible(target) => target.entity,
        _ => panic!("expected visible target"),
    }
}

fn pursue(result: Tracking, expected: &str) -> Vec3 {
    match result {
        Tracking::Pursue { position, mode, .. } => {
            assert_eq!(mode, expected);
            position
        }
        _ => panic!("expected pursuit mode {expected}"),
    }
}

#[test]
fn lock_survives_nearer_enemy_while_visible_but_switches_after_loss() {
    let bot = client();
    let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    assert_eq!(visible(tracker.update(&bot, "Spear", 1)), first);
    let second = enemy(&bot, "Spear002", 3.0, &[bot.entity]);
    assert_eq!(visible(tracker.update(&bot, "Spear", 2)), first);
    bot.ecs.lock().get_mut::<LoadedBy>(first).unwrap().clear();
    assert_eq!(visible(tracker.update(&bot, "Spear", 3)), second);
    assert_eq!(tracker.target_name(), Some("Spear002"));
}

#[test]
fn automatic_shared_target_is_local_but_exact_target_can_be_remote() {
    let bot = client();
    let observer = ally(&bot, "Mace002");
    enemy(&bot, "Spear001", 1304.0, &[observer]);
    let mut automatic = Tracker::default();
    assert!(matches!(automatic.update(&bot, "Spear", 1), Tracking::Idle));
    assert_eq!(automatic.target_name(), None);
    let mut exact = Tracker::default();
    assert_eq!(
        pursue(exact.update(&bot, "=Spear001", 1), "shared").x,
        1304.0
    );
}

#[test]
fn remote_shared_lock_is_cleared_without_last_seen_fallback() {
    let bot = client();
    let observer = ally(&bot, "Mace002");
    let target = enemy(&bot, "Spear001", 128.0, &[observer]);
    let mut tracker = Tracker::default();
    assert_eq!(pursue(tracker.update(&bot, "Spear", 1), "shared").x, 128.0);
    bot.ecs.lock().get_mut::<Position>(target).unwrap().x = 129.0;
    assert!(matches!(tracker.update(&bot, "Spear", 2), Tracking::Idle));
    assert_eq!(tracker.target_name(), None);
    bot.ecs.lock().get_mut::<LoadedBy>(target).unwrap().clear();
    assert!(matches!(tracker.update(&bot, "Spear", 3), Tracking::Idle));
}

#[test]
fn remembered_automatic_lock_outside_engagement_returns_home() {
    let bot = client();
    let target = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    visible(tracker.update(&bot, "Spear", 1));
    bot.ecs.lock().get_mut::<LoadedBy>(target).unwrap().clear();
    bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap().x = 200.0;
    assert_eq!(pursue(tracker.update(&bot, "Spear", 2), "home").x, 0.0);
    assert_eq!(tracker.target_name(), None);
}

#[test]
fn shared_range_counts_vertical_separation() {
    let bot = client();
    let observer = ally(&bot, "Mace002");
    let target = enemy(&bot, "Spear001", 100.0, &[observer]);
    bot.ecs.lock().get_mut::<Position>(target).unwrap().y += 100.0;
    assert!(matches!(
        Tracker::default().update(&bot, "Spear", 1),
        Tracking::Idle
    ));
}

#[test]
fn visible_enemy_overrides_shared_lock_and_visible_range_is_unrestricted() {
    let bot = client();
    let observer = ally(&bot, "Mace002");
    enemy(&bot, "Spear001", 100.0, &[observer]);
    let mut tracker = Tracker::default();
    pursue(tracker.update(&bot, "Spear", 1), "shared");
    let near = enemy(&bot, "Spear002", 5.0, &[bot.entity]);
    assert_eq!(visible(tracker.update(&bot, "Spear", 2)), near);
    let far_bot = client();
    let far = enemy(&far_bot, "Spear001", 1304.0, &[far_bot.entity]);
    assert_eq!(
        visible(Tracker::default().update(&far_bot, "Spear", 1)),
        far
    );
}

#[test]
fn exact_player_target_never_matches_a_suffix_or_falls_back_to_another_player() {
    let bot = client();
    let target = enemy(&bot, "sdkl", 12.0, &[bot.entity]);
    enemy(&bot, "sdklSuffix", 3.0, &[bot.entity]);
    enemy(&bot, "Other", 2.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    assert_eq!(visible(tracker.update(&bot, "=sdkl", 1)), target);
    assert_eq!(tracker.target_name(), Some("sdkl"));
    bot.ecs.lock().get_mut::<LoadedBy>(target).unwrap().clear();
    assert_eq!(
        pursue(tracker.update(&bot, "=sdkl", 2), "last_seen").x,
        12.0
    );
    assert_eq!(tracker.target_name(), Some("sdkl"));
    bot.ecs.lock().entity_mut(target).insert(Dead);
    assert!(matches!(tracker.update(&bot, "=sdkl", 3), Tracking::Idle));
    assert_eq!(tracker.target_name(), None);
}

#[test]
fn exact_names_are_case_insensitive_and_legacy_prefixes_still_match_suffixes() {
    let bot = client();
    let target = enemy(&bot, "sdkl", 12.0, &[bot.entity]);
    let suffix = enemy(&bot, "sdklSuffix", 3.0, &[bot.entity]);
    assert_eq!(visible(Tracker::default().update(&bot, "=SDKL", 1)), target);
    assert_eq!(visible(Tracker::default().update(&bot, "sdkl", 1)), suffix);
}

#[test]
fn exact_target_excludes_self_and_same_team_local_bots() {
    let bot = client();
    let teammate = enemy(&bot, "Mace002", 12.0, &[bot.entity]);
    bot.ecs
        .lock()
        .entity_mut(teammate)
        .insert(Account::offline("Mace002"));
    let opponent = enemy(&bot, "Spear002", 20.0, &[bot.entity]);
    bot.ecs
        .lock()
        .entity_mut(opponent)
        .insert(Account::offline("Spear002"));
    let mut tracker = Tracker::default();
    assert!(matches!(
        tracker.update(&bot, "=Mace002", 1),
        Tracking::Idle
    ));
    assert!(matches!(tracker.update(&bot, "Mace", 2), Tracking::Idle));
    assert_eq!(visible(tracker.update(&bot, "=Spear002", 3)), opponent);
    let own = Account::offline("Mace001");
    let uuid = own.uuid_or_offline();
    {
        let mut ecs = bot.ecs.lock();
        ecs.resource_mut::<EntityUuidIndex>()
            .insert(uuid, bot.entity);
        ecs.get_mut::<TabList>(bot.entity).unwrap().insert(
            uuid,
            PlayerInfo {
                profile: GameProfile::new(uuid, own.username),
                uuid,
                gamemode: GameMode::Survival,
                latency: 0,
                display_name: None,
            },
        );
    }
    assert!(matches!(
        tracker.update(&bot, "=Mace001", 4),
        Tracking::Idle
    ));
}

#[test]
fn teammate_observation_is_navigation_only_and_disconnected_ally_is_ignored() {
    let bot = client();
    let observer = ally(&bot, "Mace002");
    let target = enemy(&bot, "Spear001", 120.0, &[observer]);
    let mut tracker = Tracker::default();
    assert_eq!(pursue(tracker.update(&bot, "Spear", 1), "shared").x, 120.0);
    bot.ecs
        .lock()
        .entity_mut(observer)
        .remove::<RawConnection>();
    bot.ecs.lock().get_mut::<Position>(target).unwrap().x = 900.0;
    assert_eq!(
        pursue(tracker.update(&bot, "Spear", 2), "last_seen").x,
        120.0
    );
}

#[test]
fn dead_target_changes_lock_to_remaining_enemy() {
    let bot = client();
    let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    assert_eq!(visible(tracker.update(&bot, "Spear", 1)), first);
    let next = enemy(&bot, "Spear002", 20.0, &[bot.entity]);
    bot.ecs.lock().entity_mut(first).insert(Dead);
    assert_eq!(visible(tracker.update(&bot, "Spear", 2)), next);
}

#[test]
fn rebuilt_entity_is_reacquired_by_uuid_without_changing_lock() {
    let bot = client();
    let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    assert_eq!(visible(tracker.update(&bot, "Spear", 1)), first);
    bot.ecs.lock().despawn(first);
    pursue(tracker.update(&bot, "Spear", 2), "last_seen");
    enemy(&bot, "Spear002", 3.0, &[bot.entity]);
    let rebuilt = enemy(&bot, "Spear001", 15.0, &[bot.entity]);
    assert_eq!(visible(tracker.update(&bot, "Spear", 3)), rebuilt);
}

#[test]
fn lost_target_search_is_bounded_then_returns_to_home() {
    let bot = client();
    let target = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    tracker.update(&bot, "Spear", 1);
    bot.ecs.lock().get_mut::<Position>(target).unwrap().x = 14.0;
    tracker.update(&bot, "Spear", 2);
    bot.ecs.lock().get_mut::<LoadedBy>(target).unwrap().clear();
    assert_eq!(
        pursue(tracker.update(&bot, "Spear", 100), "last_seen").x,
        26.0
    );
    let search = pursue(tracker.update(&bot, "Spear", 103), "search");
    assert!(
        (16.0..=24.0).contains(
            &(search - Vec3::new(26.0, 64.0, 0.0))
                .horizontal_distance_squared()
                .sqrt()
        )
    );
    bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap().x = 500.0;
    assert_eq!(
        pursue(tracker.update(&bot, "Spear", 303), "home"),
        Vec3::new(0.0, 64.0, 0.0)
    );
}

#[test]
fn clear_and_dimension_change_forget_old_memory_and_home() {
    let bot = client();
    let target = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    tracker.update(&bot, "Spear", 1);
    tracker.clear();
    bot.ecs.lock().get_mut::<LoadedBy>(target).unwrap().clear();
    bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap().x = 300.0;
    assert!(matches!(tracker.update(&bot, "Spear", 2), Tracking::Idle));
    bot.ecs
        .lock()
        .entity_mut(bot.entity)
        .insert(dimension("minecraft:the_nether"));
    bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap().x = 600.0;
    assert!(matches!(tracker.update(&bot, "Spear", 3), Tracking::Idle));
}

#[test]
fn local_visibility_requires_client_entity_index_and_matching_dimension() {
    let bot = client();
    let target = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    bot.ecs
        .lock()
        .get_mut::<EntityIdIndex>(bot.entity)
        .unwrap()
        .remove_by_ecs_entity(target);
    let mut tracker = Tracker::default();
    assert!(matches!(tracker.update(&bot, "Spear", 1), Tracking::Idle));
    bot.ecs
        .lock()
        .get_mut::<EntityIdIndex>(bot.entity)
        .unwrap()
        .insert(MinecraftEntityId(55), target);
    bot.ecs
        .lock()
        .entity_mut(target)
        .insert(dimension("minecraft:the_nether"));
    assert!(matches!(tracker.update(&bot, "Spear", 2), Tracking::Idle));
}

#[test]
fn leaving_tab_or_becoming_spectator_releases_the_lock_immediately() {
    for leave_tab in [true, false] {
        let bot = client();
        let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
        let mut tracker = Tracker::default();
        assert_eq!(visible(tracker.update(&bot, "Spear", 1)), first);
        let next = enemy(&bot, "Spear002", 20.0, &[bot.entity]);
        let uuid = Account::offline("Spear001").uuid_or_offline();
        {
            let mut ecs = bot.ecs.lock();
            let mut tab = ecs.get_mut::<TabList>(bot.entity).unwrap();
            if leave_tab {
                tab.remove(&uuid);
            } else {
                tab.get_mut(&uuid).unwrap().gamemode = GameMode::Spectator;
            }
        }
        assert_eq!(visible(tracker.update(&bot, "Spear", 2)), next);
    }
}

#[test]
fn enemy_or_dead_or_different_dimension_observers_cannot_supply_positions() {
    for (name, health, world) in [
        ("Spear002", 20.0, "minecraft:overworld"),
        ("Mace002", 0.0, "minecraft:overworld"),
        ("Mace002", 20.0, "minecraft:the_nether"),
    ] {
        let bot = client();
        let observer = ally(&bot, name);
        bot.ecs
            .lock()
            .entity_mut(observer)
            .insert((Health(health), dimension(world)));
        enemy(&bot, "Spear001", 120.0, &[observer]);
        let mut tracker = Tracker::default();
        assert!(matches!(tracker.update(&bot, "Spear", 1), Tracking::Idle));
    }
}

#[test]
fn stale_uuid_index_never_turns_a_different_entity_into_the_locked_target() {
    let bot = client();
    let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    assert_eq!(visible(tracker.update(&bot, "Spear", 1)), first);
    let other_uuid = Account::offline("Unrelated").uuid_or_offline();
    bot.ecs
        .lock()
        .entity_mut(first)
        .insert(EntityUuid::new(other_uuid));
    bot.ecs.lock().get_mut::<Position>(first).unwrap().x = 500.0;
    assert_eq!(
        pursue(tracker.update(&bot, "Spear", 2), "last_seen").x,
        12.0
    );
}

#[test]
fn memory_retains_only_one_lock_and_extrapolates_at_most_twelve_blocks() {
    let bot = client();
    let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    tracker.update(&bot, "=Spear001", 1);
    bot.ecs.lock().get_mut::<Position>(first).unwrap().x = 1012.0;
    tracker.update(&bot, "=Spear001", 2);
    bot.ecs.lock().get_mut::<LoadedBy>(first).unwrap().clear();
    for tick in [10, 50, 100] {
        assert_eq!(
            pursue(tracker.update(&bot, "=Spear001", tick), "last_seen").x,
            1024.0
        );
    }
    assert!(std::mem::size_of::<Tracker>() <= 256);
}

#[test]
fn search_visits_four_fixed_waypoints_and_stops_after_300_ticks() {
    let bot = client();
    let first = enemy(&bot, "Spear001", 12.0, &[bot.entity]);
    let mut tracker = Tracker::default();
    tracker.update(&bot, "Spear", 1);
    bot.ecs.lock().get_mut::<LoadedBy>(first).unwrap().clear();
    let points =
        [102, 152, 202, 252].map(|tick| pursue(tracker.update(&bot, "Spear", tick), "search"));
    for (i, point) in points.iter().enumerate() {
        assert_eq!(
            (*point - Vec3::new(12.0, 64.0, 0.0)).horizontal_distance_squared(),
            400.0
        );
        assert!(points[..i].iter().all(|other| other != point));
    }
    assert!(matches!(tracker.update(&bot, "Spear", 302), Tracking::Idle));
    assert!(tracker.locked.is_none());
}
