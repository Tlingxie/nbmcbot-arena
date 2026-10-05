use anyhow::{Context, Result, ensure};
use azalea::ecs as bevy_ecs;
use azalea::{
    Client,
    ecs::component::Component,
    entity::inventory::Inventory,
    inventory::operations::ClickType,
    local_player::InstanceHolder,
    packet::game::SendGamePacketEvent,
    protocol::packets::game::s_container_click::{HashedStack, ServerboundContainerClick},
    registry::builtin::ItemKind,
    tick_counter::TicksConnected,
};

#[derive(Component, Clone)]
struct PendingChestSwap {
    kind: ItemKind,
    tick: u64,
}

pub(super) fn cancel(bot: &Client) {
    if let Ok(mut entity) = bot.ecs.lock().get_entity_mut(bot.entity) {
        entity.remove::<PendingChestSwap>();
    }
}

pub(super) fn has_flight_kit(bot: &Client) -> bool {
    let ecs = bot.ecs.lock();
    let Some(inventory) = ecs.get::<Inventory>(bot.entity) else {
        return false;
    };
    let menu = &inventory.inventory_menu;
    let has = |kind| {
        [6].into_iter()
            .chain(36..45)
            .any(|slot| menu.slot(slot).is_some_and(|item| item.kind() == kind))
    };
    has(ItemKind::Elytra)
        && has(ItemKind::NetheriteChestplate)
        && menu
            .slot(45)
            .is_some_and(|item| item.kind() == ItemKind::FireworkRocket)
}

/// Predicts the ordinary inventory number-key swap without changing main hand.
/// A true result is local equipment readiness, not a server acknowledgement;
/// callers must also wait for server FallFlying=false before a mace smash.
pub(super) fn equip_chest(bot: &Client, kind: ItemKind) -> Result<bool> {
    ensure!(
        matches!(kind, ItemKind::Elytra | ItemKind::NetheriteChestplate),
        "unsupported flight chest item"
    );
    let mut ecs = bot.ecs.lock();
    let tick = ecs
        .get::<TicksConnected>(bot.entity)
        .context("missing equipment tick counter")?
        .0;
    let pending = ecs
        .get::<PendingChestSwap>(bot.entity)
        .filter(|swap| tick >= swap.tick)
        .cloned();
    let holder = ecs
        .get::<InstanceHolder>(bot.entity)
        .cloned()
        .context("missing equipment world")?;
    let mut inventory = ecs
        .get_mut::<Inventory>(bot.entity)
        .context("missing inventory")?;
    ensure!(
        inventory.id == 0 && inventory.container_menu.is_none(),
        "close the container before changing flight equipment"
    );
    let old_chest = inventory
        .inventory_menu
        .slot(6)
        .cloned()
        .context("missing chest slot")?;
    if old_chest.kind() == kind {
        return Ok(pending.is_none_or(|swap| swap.kind != kind || tick > swap.tick));
    }
    if let Some(swap) = pending
        && swap.kind == kind
    {
        ensure!(
            tick.saturating_sub(swap.tick) < 10,
            "flight equipment swap was not retained by the server"
        );
        return Ok(false);
    }
    let source_slot = (36..45)
        .find(|slot| {
            inventory
                .inventory_menu
                .slot(*slot)
                .is_some_and(|item| item.kind() == kind)
        })
        .context("required flight chest item is missing from hotbar")?;
    ensure!(
        source_slot != 36 + usize::from(inventory.selected_hotbar_slot),
        "flight equipment swap must preserve the selected weapon slot"
    );
    let source = inventory.inventory_menu.slot(source_slot).cloned().unwrap();
    ensure!(
        source.count() == 1 && old_chest.count() <= 1,
        "flight chest items must be unstacked"
    );
    let packet = {
        let world = holder.instance.read();
        let mut packet = ServerboundContainerClick {
            container_id: 0,
            state_id: inventory.state_id,
            slot_num: 6,
            button_num: (source_slot - 36) as u8,
            click_type: ClickType::Swap,
            changed_slots: Default::default(),
            carried_item: HashedStack::from_item_stack(&inventory.carried, &world.registries),
        };
        packet
            .changed_slots
            .insert(6, HashedStack::from_item_stack(&source, &world.registries));
        packet.changed_slots.insert(
            source_slot as u16,
            HashedStack::from_item_stack(&old_chest, &world.registries),
        );
        packet
    };
    // Packet button 4 means hotbar slot 4, while its menu slot is 40.
    *inventory.inventory_menu.slot_mut(6).unwrap() = source;
    *inventory.inventory_menu.slot_mut(source_slot).unwrap() = old_chest;
    ecs.entity_mut(bot.entity)
        .insert(PendingChestSwap { kind, tick });
    ecs.trigger(SendGamePacketEvent::new(bot.entity, packet));
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use azalea::ecs as bevy_ecs;
    use azalea::{
        app::App,
        ecs::prelude::*,
        entity::inventory::Inventory,
        inventory::{ItemStack, operations::ClickType},
        local_player::InstanceHolder,
        packet::game::SendGamePacketEvent,
        protocol::packets::game::ServerboundGamePacket,
        tick_counter::TicksConnected,
        world::Instance,
    };
    use std::sync::Arc;

    #[derive(Resource, Default)]
    struct Packets(Vec<ServerboundGamePacket>);

    fn client() -> Client {
        let mut app = App::new();
        app.init_resource::<Packets>().add_observer(
            |event: On<SendGamePacketEvent>, mut packets: ResMut<Packets>| {
                packets.0.push(event.packet.clone());
            },
        );
        let mut inventory = Inventory {
            state_id: 19,
            ..Default::default()
        };
        for (slot, kind) in [
            (6, ItemKind::Elytra),
            (36, ItemKind::Mace),
            (40, ItemKind::NetheriteChestplate),
            (45, ItemKind::FireworkRocket),
        ] {
            *inventory.inventory_menu.slot_mut(slot).unwrap() = ItemStack::new(kind, 1);
        }
        let entity = app.world_mut().spawn((inventory, TicksConnected(10))).id();
        let holder = InstanceHolder::new(
            entity,
            Arc::new(parking_lot::RwLock::new(Instance::default())),
        );
        app.world_mut().entity_mut(entity).insert(holder);
        Client {
            entity,
            ecs: Arc::new(parking_lot::Mutex::new(std::mem::take(app.world_mut()))),
        }
    }

    #[test]
    fn chest_swap_uses_hotbar_button_and_preserves_mace_without_duplicate_clicks() {
        let bot = client();
        assert!(has_flight_kit(&bot));
        assert!(!equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
        assert!(!equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
        {
            let ecs = bot.ecs.lock();
            let inventory = ecs.get::<Inventory>(bot.entity).unwrap();
            assert_eq!(inventory.selected_hotbar_slot, 0);
            assert_eq!(inventory.held_item().kind(), ItemKind::Mace);
            assert_eq!(
                inventory.inventory_menu.slot(6).unwrap().kind(),
                ItemKind::NetheriteChestplate
            );
            assert_eq!(
                inventory.inventory_menu.slot(40).unwrap().kind(),
                ItemKind::Elytra
            );
            assert!(inventory.carried.is_empty());
            let packets = &ecs.resource::<Packets>().0;
            assert_eq!(packets.len(), 1);
            let ServerboundGamePacket::ContainerClick(packet) = &packets[0] else {
                panic!("expected swap packet")
            };
            assert_eq!(
                (
                    packet.container_id,
                    packet.state_id,
                    packet.slot_num,
                    packet.button_num
                ),
                (0, 19, 6, 4)
            );
            assert_eq!(packet.click_type, ClickType::Swap);
            assert_eq!(packet.changed_slots.len(), 2);
            assert!(
                packet.changed_slots.contains_key(&6) && packet.changed_slots.contains_key(&40)
            );
        }
        bot.ecs
            .lock()
            .get_mut::<TicksConnected>(bot.entity)
            .unwrap()
            .0 += 1;
        assert!(equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
        assert!(has_flight_kit(&bot));
    }

    #[test]
    fn flight_kit_requires_both_chest_items_and_offhand_fireworks() {
        let bot = client();
        assert!(has_flight_kit(&bot));
        *bot.ecs
            .lock()
            .get_mut::<Inventory>(bot.entity)
            .unwrap()
            .inventory_menu
            .slot_mut(45)
            .unwrap() = ItemStack::Empty;
        assert!(!has_flight_kit(&bot));
    }

    #[test]
    fn swap_refuses_open_container_or_missing_equipment() {
        let bot = client();
        bot.ecs.lock().get_mut::<Inventory>(bot.entity).unwrap().id = 1;
        assert!(equip_chest(&bot, ItemKind::NetheriteChestplate).is_err());
        let mut ecs = bot.ecs.lock();
        let mut inventory = ecs.get_mut::<Inventory>(bot.entity).unwrap();
        inventory.id = 0;
        *inventory.inventory_menu.slot_mut(40).unwrap() = ItemStack::Empty;
        drop(ecs);
        assert!(equip_chest(&bot, ItemKind::NetheriteChestplate).is_err());
        assert!(bot.ecs.lock().resource::<Packets>().0.is_empty());
    }

    #[test]
    fn resetting_connection_ticks_does_not_retain_an_old_swap_barrier() {
        let bot = client();
        assert!(!equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
        bot.ecs
            .lock()
            .get_mut::<TicksConnected>(bot.entity)
            .unwrap()
            .0 = 0;
        assert!(equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
    }

    #[test]
    fn cancellation_allows_a_new_swap_after_arena_reequips_the_bot() {
        let bot = client();
        assert!(!equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
        {
            let mut ecs = bot.ecs.lock();
            ecs.get_mut::<TicksConnected>(bot.entity).unwrap().0 = 100;
            let mut inventory = ecs.get_mut::<Inventory>(bot.entity).unwrap();
            *inventory.inventory_menu.slot_mut(6).unwrap() = ItemStack::new(ItemKind::Elytra, 1);
            *inventory.inventory_menu.slot_mut(40).unwrap() =
                ItemStack::new(ItemKind::NetheriteChestplate, 1);
        }
        cancel(&bot);
        assert!(!equip_chest(&bot, ItemKind::NetheriteChestplate).unwrap());
        assert_eq!(bot.ecs.lock().resource::<Packets>().0.len(), 2);
    }
}
