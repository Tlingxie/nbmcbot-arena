use crate::automation::{AutomationAction, Observation};
use azalea::{
    BlockPos, Client, Vec3, core::direction::Direction, ecs::entity::Entity, entity::Position,
    inventory::ItemStack, prelude::*, registry::builtin::ItemKind,
};
use serde_json::json;

#[derive(azalea::ecs::component::Component)]
pub struct PendingUseFood;

pub fn use_food(
    mut commands: azalea::ecs::system::Commands,
    mut query: azalea::ecs::system::Query<
        (
            Entity,
            &azalea::entity::LookDirection,
            &mut azalea::interact::BlockStatePredictionHandler,
        ),
        azalea::ecs::query::With<PendingUseFood>,
    >,
) {
    for (entity, direction, mut prediction) in &mut query {
        commands.entity(entity).remove::<PendingUseFood>();
        commands.trigger(azalea::packet::game::SendGamePacketEvent::new(
            entity,
            azalea::protocol::packets::game::ServerboundUseItem {
                hand: azalea::protocol::packets::game::s_interact::InteractionHand::MainHand,
                seq: prediction.start_predicting(),
                y_rot: direction.y_rot(),
                x_rot: direction.x_rot(),
            },
        ));
    }
}

pub fn distance_squared(a: Vec3, b: Vec3) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)
}

pub fn player_entity(bot: &Client, name: &str) -> Option<Entity> {
    bot.player_uuid_by_username(name)
        .and_then(|uuid| bot.entity_by_uuid(uuid))
}

pub fn player_position(bot: &Client, name: &str) -> Option<Vec3> {
    bot.get_entity_component::<Position>(player_entity(bot, name)?)
        .map(|p| *p)
}

fn swap_offhand(bot: &Client, slot: u16) {
    use azalea::protocol::packets::game::s_container_click::{
        HashedStack, ServerboundContainerClick,
    };
    if !(9..45).contains(&slot) {
        return;
    }
    let world = bot.world();
    let mut ecs = bot.ecs.lock();
    let Some(mut inventory) = ecs.get_mut::<azalea::entity::inventory::Inventory>(bot.entity)
    else {
        return;
    };
    if inventory.id != 0 || inventory.menu().try_as_player().is_none() {
        return;
    }
    let source = inventory
        .menu()
        .slot(usize::from(slot))
        .cloned()
        .unwrap_or_default();
    if source.kind() != ItemKind::TotemOfUndying {
        return;
    }
    let old_offhand = inventory.menu().slot(45).cloned().unwrap_or_default();
    let packet = {
        let world = world.read();
        let mut packet = ServerboundContainerClick {
            container_id: inventory.id,
            state_id: inventory.state_id,
            slot_num: slot as i16,
            button_num: 40,
            click_type: azalea::inventory::operations::ClickType::Swap,
            changed_slots: Default::default(),
            carried_item: HashedStack::from_item_stack(&inventory.carried, &world.registries),
        };
        packet.changed_slots.insert(
            slot,
            HashedStack::from_item_stack(&old_offhand, &world.registries),
        );
        packet
            .changed_slots
            .insert(45, HashedStack::from_item_stack(&source, &world.registries));
        packet
    };
    // Protocol button 40 is the offhand; its player-menu index is 45.
    // Keep the prediction consistent with the hashes sent to the server.
    *inventory.menu_mut().slot_mut(usize::from(slot)).unwrap() = old_offhand;
    *inventory.menu_mut().slot_mut(45).unwrap() = source;
    ecs.trigger(azalea::packet::game::SendGamePacketEvent::new(
        bot.entity, packet,
    ));
}

pub fn stop_movement(bot: &Client, send_packets: bool) {
    use azalea::mining::{
        MineBlockPos, MineProgress, Mining, MiningQueued, StartMiningBlockEvent,
        StopMiningBlockEvent,
    };
    use azalea::{
        ecs::message::Messages,
        pathfinder::{
            ComputePath, ExecutingPath, GotoEvent, PathFoundEvent, Pathfinder, StopPathfindingEvent,
        },
    };
    let mut ecs = bot.ecs.lock();
    keep_other_entity_messages(
        &mut ecs.resource_mut::<Messages<azalea::attack::AttackEvent>>(),
        bot.entity,
        |event| &mut event.entity,
    );
    keep_other_entity_messages(
        &mut ecs.resource_mut::<Messages<GotoEvent>>(),
        bot.entity,
        |event| &mut event.entity,
    );
    keep_other_entity_messages(
        &mut ecs.resource_mut::<Messages<PathFoundEvent>>(),
        bot.entity,
        |event| &mut event.entity,
    );
    keep_other_entity_messages(
        &mut ecs.resource_mut::<Messages<StopPathfindingEvent>>(),
        bot.entity,
        |event| &mut event.entity,
    );
    keep_other_entity_messages(
        &mut ecs.resource_mut::<Messages<StartMiningBlockEvent>>(),
        bot.entity,
        |event| &mut event.entity,
    );
    keep_other_entity_messages(
        &mut ecs.resource_mut::<Messages<StopMiningBlockEvent>>(),
        bot.entity,
        |event| &mut event.entity,
    );
    if ecs.get_entity(bot.entity).is_err() {
        return;
    }
    let abort = send_packets
        .then(|| {
            ecs.get::<Mining>(bot.entity)
                .and_then(|_| ecs.get::<MineBlockPos>(bot.entity))
                .and_then(|pos| pos.0)
        })
        .flatten();
    ecs.entity_mut(bot.entity).remove::<(
        ComputePath,
        ExecutingPath,
        Mining,
        MiningQueued,
        PendingUseFood,
        azalea::attack::AttackQueued,
    )>();
    if let Some(mut pathfinder) = ecs.get_mut::<Pathfinder>(bot.entity) {
        pathfinder.goal = None;
        pathfinder.opts = None;
        pathfinder.is_calculating = false;
        pathfinder
            .goto_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    if let Some(mut progress) = ecs.get_mut::<MineProgress>(bot.entity) {
        progress.0 = 0.0;
    }
    if let Some(pos) = abort {
        ecs.trigger(azalea::packet::game::SendGamePacketEvent::new(
            bot.entity,
            azalea::protocol::packets::game::s_player_action::ServerboundPlayerAction {
                action: azalea::protocol::packets::game::s_player_action::Action::AbortDestroyBlock,
                pos,
                direction: Direction::Down,
                seq: 0,
            },
        ));
    }
    drop(ecs);
    if send_packets {
        bot.walk(azalea::WalkDirection::None);
        bot.set_jumping(false);
    }
}

fn safe_food(item: &ItemStack) -> bool {
    matches!(
        item.kind(),
        ItemKind::Bread
            | ItemKind::CookedBeef
            | ItemKind::CookedPorkchop
            | ItemKind::CookedChicken
            | ItemKind::CookedMutton
            | ItemKind::CookedRabbit
            | ItemKind::CookedCod
            | ItemKind::CookedSalmon
            | ItemKind::BakedPotato
            | ItemKind::Carrot
            | ItemKind::GoldenCarrot
            | ItemKind::Apple
            | ItemKind::MelonSlice
            | ItemKind::PumpkinPie
    )
}

pub fn observation(bot: &Client, tick: u64, busy: bool) -> Observation {
    let menu = bot.menu();
    let is_player = menu.try_as_player().is_some();
    let slots = menu.slots();
    let hotbar = menu.hotbar_slots_range();
    let food_slot = slots[hotbar]
        .iter()
        .position(safe_food)
        .map(|index| index as u8);
    let offhand_totem = is_player
        && slots
            .get(45)
            .is_some_and(|item| item.kind() == ItemKind::TotemOfUndying);
    let totem_slot = if is_player {
        (9..45)
            .find(|&index| slots[index].kind() == ItemKind::TotemOfUndying)
            .map(|i| i as u16)
    } else {
        None
    };
    Observation {
        tick,
        health: bot.health(),
        food: bot.hunger().food,
        connected: true,
        busy: busy || !is_player,
        selected_slot: bot.selected_hotbar_slot(),
        food_slot,
        offhand_totem,
        totem_slot,
        dead: bot.health() <= 0.0,
    }
}

pub fn apply_automation(bot: &Client, action: AutomationAction) {
    match action {
        AutomationAction::Respawn => {
            bot.ecs
                .lock()
                .write_message(azalea::respawn::PerformRespawnEvent { entity: bot.entity });
        }
        AutomationAction::Look { yaw, pitch } => bot.set_direction(yaw, pitch),
        AutomationAction::SelectHotbar(slot) => bot.set_selected_hotbar_slot(slot),
        AutomationAction::UseMainHand => {
            // Send after the hotbar update, without interacting with a ceiling or container.
            bot.ecs.lock().entity_mut(bot.entity).insert(PendingUseFood);
        }
        AutomationAction::ReleaseUse => {
            {
                let mut ecs = bot.ecs.lock();
                keep_other_entity_messages(&mut ecs.resource_mut::<azalea::ecs::message::Messages<azalea::interact::StartUseItemEvent>>(), bot.entity, |event| &mut event.entity);
                ecs.entity_mut(bot.entity)
                    .remove::<(azalea::interact::StartUseItemQueued, PendingUseFood)>();
            }
            bot.write_packet(
                azalea::protocol::packets::game::s_player_action::ServerboundPlayerAction {
                    action:
                        azalea::protocol::packets::game::s_player_action::Action::ReleaseUseItem,
                    pos: BlockPos::new(0, 0, 0),
                    direction: Direction::Down,
                    seq: 0,
                },
            );
        }
        AutomationAction::SwapOffhand { slot } => swap_offhand(bot, slot),
    }
}

pub fn print_inventory(bot: &Client) {
    let menu = bot.menu();
    let items:Vec<_>=menu.slots().iter().enumerate().filter(|(_,item)|item.is_present())
        .map(|(slot,item)|json!({"slot":slot,"item":format!("{:?}",item.kind()),"count":item.count()})).collect();
    println!(
        "{}",
        json!({"event":"inventory","bot":bot.username(),"selected_hotbar_slot":bot.selected_hotbar_slot(),"items":items})
    );
}

pub fn loaded_chunks(bot: &Client) -> usize {
    let partial = {
        let ecs = bot.ecs.lock();
        ecs.get::<azalea::local_player::InstanceHolder>(bot.entity)
            .map(|holder| holder.partial_instance.clone())
    };
    partial.map_or(0, |partial| {
        partial.read().chunks.chunks().flatten().count()
    })
}

pub(crate) fn keep_other_entity_messages<M: azalea::ecs::message::Message>(
    queue: &mut azalea::ecs::message::Messages<M>,
    entity: Entity,
    owner: impl Fn(&mut M) -> &mut Entity,
) {
    // Preserve message IDs so another bot's already-read intentions are not replayed.
    let mut cursor = queue.get_cursor();
    for message in cursor.read_mut(queue) {
        let target = owner(message);
        if *target == entity {
            *target = Entity::PLACEHOLDER;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopping_one_bot_cancels_only_its_queued_attack() {
        use azalea::ecs::{message::Messages, world::World};
        use azalea::mining::{StartMiningBlockEvent, StopMiningBlockEvent};
        use azalea::pathfinder::{GotoEvent, PathFoundEvent, StopPathfindingEvent};
        let mut world = World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        world
            .entity_mut(first)
            .insert(azalea::attack::AttackQueued { target: second });
        world
            .entity_mut(second)
            .insert(azalea::attack::AttackQueued { target: first });
        world.init_resource::<Messages<GotoEvent>>();
        world.init_resource::<Messages<PathFoundEvent>>();
        world.init_resource::<Messages<StopPathfindingEvent>>();
        world.init_resource::<Messages<StartMiningBlockEvent>>();
        world.init_resource::<Messages<StopMiningBlockEvent>>();
        world.init_resource::<Messages<azalea::attack::AttackEvent>>();
        world.write_message(azalea::attack::AttackEvent {
            entity: first,
            target: second,
        });
        world.write_message(azalea::attack::AttackEvent {
            entity: second,
            target: first,
        });
        let mut cursor = world
            .resource::<Messages<azalea::attack::AttackEvent>>()
            .get_cursor();
        assert_eq!(
            cursor
                .read(world.resource::<Messages<azalea::attack::AttackEvent>>())
                .count(),
            2
        );
        let client = Client::new(first, std::sync::Arc::new(parking_lot::Mutex::new(world)));
        stop_movement(&client, false);
        let ecs = client.ecs.lock();
        assert!(ecs.get::<azalea::attack::AttackQueued>(first).is_none());
        assert!(ecs.get::<azalea::attack::AttackQueued>(second).is_some());
        let queue = ecs.resource::<Messages<azalea::attack::AttackEvent>>();
        assert_eq!(cursor.read(queue).count(), 0);
        let mut observer = queue.get_cursor();
        let active: Vec<_> = observer
            .read(queue)
            .filter(|event| event.entity != Entity::PLACEHOLDER)
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].entity, second);
    }
    #[test]
    fn queued_stop_preserves_other_bot_intentions() {
        use azalea::ecs::{message::Messages, world::World};
        use azalea::pathfinder::{GotoEvent, PathfinderOpts, goals::BlockPosGoal};
        let mut world = World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        let mut queue = Messages::<GotoEvent>::default();
        queue.write(GotoEvent::new(
            first,
            BlockPosGoal(BlockPos::new(1, 64, 1)),
            PathfinderOpts::new(),
        ));
        queue.write(GotoEvent::new(
            second,
            BlockPosGoal(BlockPos::new(2, 64, 2)),
            PathfinderOpts::new(),
        ));
        let mut consumer = queue.get_cursor();
        assert_eq!(consumer.read(&queue).count(), 2);
        keep_other_entity_messages(&mut queue, first, |event| &mut event.entity);
        assert_eq!(
            consumer.read(&queue).count(),
            0,
            "cancellation must not replay any consumed message"
        );
        let remaining: Vec<_> = queue
            .drain()
            .filter(|event| event.entity != Entity::PLACEHOLDER)
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].entity, second);
    }
    #[test]
    fn excludes_harmful_foods_and_does_not_confuse_distance_with_squared_distance() {
        assert!(!safe_food(&ItemStack::new(ItemKind::RottenFlesh, 1)));
        assert!(!safe_food(&ItemStack::new(ItemKind::Pufferfish, 1)));
        assert!(safe_food(&ItemStack::new(ItemKind::Bread, 1)));
        assert_eq!(
            distance_squared(Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 6.0, 3.0)),
            25.0
        );
    }
}
