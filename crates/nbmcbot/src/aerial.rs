use azalea::ecs as bevy_ecs;
use azalea::{
    BlockPos, Client,
    app::{App, Plugin},
    core::tick::GameTick,
    ecs::prelude::*,
    entity::{
        HasClientLoaded, LocalEntity, LookDirection, Physics,
        inventory::Inventory,
        metadata::{AttachedToTarget, FallFlying, FireworkRocket, Health},
    },
    interact::BlockStatePredictionHandler,
    packet::game::SendGamePacketEvent,
    physics::PhysicsSystems,
    protocol::packets::game::{
        ServerboundPlayerAction, ServerboundPlayerCommand, ServerboundUseItem,
        s_interact::InteractionHand, s_player_action, s_player_command,
    },
    world::{InstanceName, MinecraftEntityId},
};

pub(crate) struct AerialPlugin;
impl Plugin for AerialPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            GameTick,
            (
                send_pending_uses
                    .after(azalea::inventory::ensure_has_sent_carried_item)
                    .before(PhysicsSystems),
                boost_from_attached_rockets
                    .after(PhysicsSystems)
                    .before(azalea::movement::send_position),
                stop_grounded_gliding.before(azalea::movement::update_pose),
            ),
        );
    }
}

#[derive(Component)]
struct PendingUse {
    hand: InteractionHand,
    direction: LookDirection,
}

fn send_pending_uses(
    mut commands: Commands,
    mut query: Query<
        (
            Entity,
            &PendingUse,
            &mut BlockStatePredictionHandler,
            &Health,
        ),
        With<HasClientLoaded>,
    >,
) {
    for (entity, pending, mut prediction, health) in &mut query {
        commands.entity(entity).remove::<PendingUse>();
        if health.0 <= 0. {
            continue;
        }
        commands.trigger(SendGamePacketEvent::new(
            entity,
            ServerboundUseItem {
                hand: pending.hand,
                seq: prediction.start_predicting(),
                y_rot: pending.direction.y_rot(),
                x_rot: pending.direction.x_rot(),
            },
        ));
    }
}

pub(crate) fn use_item(bot: &Client, hand: InteractionHand) -> bool {
    let mut ecs = bot.ecs.lock();
    if ecs.get::<HasClientLoaded>(bot.entity).is_none()
        || ecs.get::<Health>(bot.entity).is_none_or(|h| h.0 <= 0.)
    {
        return false;
    }
    let Some(direction) = ecs.get::<LookDirection>(bot.entity).copied() else {
        return false;
    };
    ecs.entity_mut(bot.entity)
        .insert(PendingUse { hand, direction });
    true
}

pub(crate) fn release_use(bot: &Client) {
    cancel(bot, true);
}

pub(crate) fn cancel(bot: &Client, send_release: bool) {
    let mut ecs = bot.ecs.lock();
    let Ok(mut entity) = ecs.get_entity_mut(bot.entity) else {
        return;
    };
    entity.remove::<PendingUse>();
    entity.remove::<azalea::interact::StartUseItemQueued>();
    if send_release && entity.contains::<HasClientLoaded>() {
        ecs.trigger(SendGamePacketEvent::new(
            bot.entity,
            ServerboundPlayerAction {
                action: s_player_action::Action::ReleaseUseItem,
                pos: BlockPos::new(0, 0, 0),
                direction: azalea::core::direction::Direction::Down,
                seq: 0,
            },
        ));
    }
}

pub(crate) fn start_gliding(bot: &Client) -> bool {
    let mut ecs = bot.ecs.lock();
    if ecs.get::<HasClientLoaded>(bot.entity).is_none()
        || ecs.get::<Health>(bot.entity).is_none_or(|h| h.0 <= 0.)
    {
        return false;
    }
    let Some(physics) = ecs.get::<Physics>(bot.entity) else {
        return false;
    };
    if physics.on_ground() || physics.is_in_water() || physics.is_in_lava() {
        return false;
    }
    let Some(inventory) = ecs.get::<Inventory>(bot.entity) else {
        return false;
    };
    let Some(azalea::inventory::ItemStack::Present(chest)) =
        inventory.get_equipment(azalea::inventory::components::EquipmentSlot::Chest)
    else {
        return false;
    };
    use azalea::inventory::components::{Damage, Glider, MaxDamage};
    if chest.get_component::<Glider>().is_none()
        || chest.get_component::<MaxDamage>().is_some_and(|maximum| {
            chest
                .get_component::<Damage>()
                .is_some_and(|damage| damage.amount >= maximum.amount - 1)
        })
    {
        return false;
    }
    if ecs.get::<FallFlying>(bot.entity).is_some_and(|f| f.0) {
        return true;
    }
    let Some(id) = ecs.get::<MinecraftEntityId>(bot.entity).copied() else {
        return false;
    };
    ecs.entity_mut(bot.entity).insert(FallFlying(true));
    ecs.trigger(SendGamePacketEvent::new(
        bot.entity,
        ServerboundPlayerCommand {
            id,
            action: s_player_command::Action::StartFallFlying,
            data: 0,
        },
    ));
    true
}

fn stop_grounded_gliding(mut query: Query<(&Physics, &mut FallFlying), With<LocalEntity>>) {
    for (physics, mut flying) in &mut query {
        if flying.0 && (physics.on_ground() || physics.is_in_water() || physics.is_in_lava()) {
            flying.0 = false;
        }
    }
}

#[allow(clippy::type_complexity)]
fn boost_from_attached_rockets(
    rockets: Query<(&AttachedToTarget, &InstanceName), With<FireworkRocket>>,
    mut players: Query<
        (
            &MinecraftEntityId,
            &InstanceName,
            &FallFlying,
            &LookDirection,
            &mut Physics,
        ),
        (With<LocalEntity>, With<HasClientLoaded>),
    >,
) {
    for (id, world, flying, direction, mut physics) in &mut players {
        if !**flying {
            continue;
        }
        for (target, rocket_world) in &rockets {
            if world == rocket_world && target.0.0 == u32::try_from(id.0).ok() {
                // FireworkRocketEntity.tick: use only a server-spawned rocket
                // whose attachment names this player. Removal ends the boost.
                let look = azalea::entity::view_vector(*direction);
                let velocity = physics.velocity;
                physics.velocity += look * 0.1 + (look * 1.5 - velocity) * 0.5;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use azalea::{Vec3, entity::OptionalUnsignedInt, registry::identifier::Identifier};

    #[test]
    fn firework_only_boosts_its_gliding_owner_in_same_world() {
        let mut app = App::new();
        app.add_systems(GameTick, boost_from_attached_rockets);
        let world = InstanceName(Identifier::new("minecraft:overworld"));
        let other_world = InstanceName(Identifier::new("minecraft:the_nether"));
        let mut spawn_player = |id, name: InstanceName, flying| {
            app.world_mut()
                .spawn((
                    MinecraftEntityId(id),
                    name,
                    LocalEntity,
                    HasClientLoaded,
                    FallFlying(flying),
                    LookDirection::new(0., 0.),
                    Physics::default(),
                ))
                .id()
        };
        let owner = spawn_player(7, world.clone(), true);
        let other = spawn_player(8, world.clone(), true);
        let wrong_world = spawn_player(7, other_world, true);
        let grounded = spawn_player(9, world.clone(), false);
        let rocket = app
            .world_mut()
            .spawn((
                FireworkRocket,
                AttachedToTarget(OptionalUnsignedInt(Some(7))),
                world.clone(),
            ))
            .id();
        app.world_mut().spawn((
            FireworkRocket,
            AttachedToTarget(OptionalUnsignedInt(Some(9))),
            world,
        ));
        app.world_mut().run_schedule(GameTick);
        assert!((app.world().get::<Physics>(owner).unwrap().velocity.z - 0.85).abs() < 1e-8);
        for entity in [other, wrong_world, grounded] {
            assert_eq!(
                app.world().get::<Physics>(entity).unwrap().velocity,
                Vec3::ZERO
            );
        }
        app.world_mut().despawn(rocket);
        app.world_mut().run_schedule(GameTick);
        assert!((app.world().get::<Physics>(owner).unwrap().velocity.z - 0.85).abs() < 1e-8);
    }

    #[derive(Resource, Default)]
    struct Packets(Vec<azalea::protocol::packets::game::ServerboundGamePacket>);

    #[test]
    fn queued_item_use_sends_aim_and_hand_once_and_skips_dead_players() {
        let mut app = App::new();
        app.init_resource::<Packets>()
            .add_observer(
                |event: On<SendGamePacketEvent>, mut packets: ResMut<Packets>| {
                    packets.0.push(event.packet.clone());
                },
            )
            .add_systems(GameTick, send_pending_uses);
        for health in [20., 0.] {
            app.world_mut().spawn((
                HasClientLoaded,
                Health(health),
                BlockStatePredictionHandler::default(),
                PendingUse {
                    hand: InteractionHand::OffHand,
                    direction: LookDirection::new(90., -45.),
                },
            ));
        }
        app.world_mut().run_schedule(GameTick);
        app.world_mut().run_schedule(GameTick);
        let packets = &app.world().resource::<Packets>().0;
        assert_eq!(packets.len(), 1);
        let azalea::protocol::packets::game::ServerboundGamePacket::UseItem(packet) = &packets[0]
        else {
            panic!("wrong packet")
        };
        assert_eq!(packet.hand, InteractionHand::OffHand);
        assert_eq!((packet.y_rot, packet.x_rot), (90., -45.));
        assert_eq!(packet.seq, 1);
    }

    fn test_client() -> Client {
        let mut app = App::new();
        app.init_resource::<Packets>()
            .add_observer(
                |event: On<SendGamePacketEvent>, mut packets: ResMut<Packets>| {
                    packets.0.push(event.packet.clone());
                },
            )
            .add_systems(GameTick, send_pending_uses);
        let entity = app
            .world_mut()
            .spawn((
                HasClientLoaded,
                Health(20.),
                BlockStatePredictionHandler::default(),
                LookDirection::new(0., 0.),
                MinecraftEntityId(42),
                Physics::default(),
                Inventory::default(),
            ))
            .id();
        Client {
            entity,
            ecs: std::sync::Arc::new(parking_lot::Mutex::new(std::mem::take(app.world_mut()))),
        }
    }

    #[test]
    fn cancelled_item_use_cannot_leak_into_next_tick() {
        let bot = test_client();
        assert!(use_item(&bot, InteractionHand::MainHand));
        cancel(&bot, true);
        let mut ecs = bot.ecs.lock();
        ecs.run_schedule(GameTick);
        let packets = &ecs.resource::<Packets>().0;
        assert_eq!(packets.len(), 1);
        let azalea::protocol::packets::game::ServerboundGamePacket::PlayerAction(packet) =
            &packets[0]
        else {
            panic!("expected release")
        };
        assert_eq!(packet.action, s_player_action::Action::ReleaseUseItem);
    }

    #[test]
    fn gliding_request_requires_airborne_equipped_elytra_and_only_sends_once() {
        let bot = test_client();
        assert!(!start_gliding(&bot));
        {
            let mut ecs = bot.ecs.lock();
            ecs.get_mut::<Inventory>(bot.entity)
                .unwrap()
                .inventory_menu
                .as_player_mut()
                .armor[1] =
                azalea::inventory::ItemStack::new(azalea::registry::builtin::ItemKind::Elytra, 1);
            ecs.get_mut::<Physics>(bot.entity)
                .unwrap()
                .set_on_ground(true);
        }
        assert!(!start_gliding(&bot));
        bot.ecs
            .lock()
            .get_mut::<Physics>(bot.entity)
            .unwrap()
            .set_on_ground(false);
        assert!(start_gliding(&bot));
        assert!(start_gliding(&bot));
        let ecs = bot.ecs.lock();
        assert!(ecs.get::<FallFlying>(bot.entity).unwrap().0);
        let packets = &ecs.resource::<Packets>().0;
        assert_eq!(packets.len(), 1);
        let azalea::protocol::packets::game::ServerboundGamePacket::PlayerCommand(packet) =
            &packets[0]
        else {
            panic!("expected start glide")
        };
        assert_eq!(packet.action, s_player_command::Action::StartFallFlying);
    }
}
