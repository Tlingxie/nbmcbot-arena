use azalea::ecs as bevy_ecs;
use azalea::{
    Account,
    app::{App, Plugin, Update},
    ecs::prelude::*,
    entity::indexing::EntityIdIndex,
    packet::game::ReceiveGamePacketEvent,
    player::GameProfileComponent,
    protocol::packets::game::ClientboundGamePacket,
    tick_counter::TicksConnected,
    world::MinecraftEntityId,
};

pub(crate) struct DamageAuditPlugin;
impl Plugin for DamageAuditPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DuelDamage>()
            .add_systems(Update, (observe_damage, print_damage).chain());
    }
}

#[derive(Clone, Debug, Message)]
pub(crate) struct DuelDamage {
    pub victim: String,
    pub attacker: Option<String>,
    pub cause_entity_id: Option<u32>,
    pub damage_type_id: u32,
    pub tick: u64,
}

fn observe_damage(
    mut packets: MessageReader<ReceiveGamePacketEvent>,
    clients: Query<(
        &MinecraftEntityId,
        &Account,
        &EntityIdIndex,
        &TicksConnected,
    )>,
    profiles: Query<&GameProfileComponent>,
    mut observed: MessageWriter<DuelDamage>,
) {
    for event in packets.read() {
        let ClientboundGamePacket::DamageEvent(packet) = event.packet.as_ref() else {
            continue;
        };
        let Ok((own_id, account, index, tick)) = clients.get(event.entity) else {
            continue;
        };
        // The server broadcasts damage to observers too; count only its victim's copy.
        if packet.entity_id != *own_id {
            continue;
        }
        let attacker = packet
            .source_cause_id
            .0
            .and_then(|id| i32::try_from(id).ok())
            .and_then(|id| index.get_by_minecraft_entity(MinecraftEntityId(id)))
            .and_then(|entity| profiles.get(entity).ok())
            .map(|profile| profile.name.clone());
        observed.write(DuelDamage {
            victim: account.username.clone(),
            attacker,
            cause_entity_id: packet.source_cause_id.0,
            damage_type_id: packet.source_type_id,
            tick: tick.0,
        });
    }
}

fn print_damage(mut observed: MessageReader<DuelDamage>) {
    for hit in observed.read() {
        println!(
            "{}",
            serde_json::json!({
                "event":"duel_damage", "source":"server", "bot":hit.victim,
                "victim":hit.victim, "attacker":hit.attacker,
                "cause_entity_id":hit.cause_entity_id, "damage_type_id":hit.damage_type_id,
                "tick":hit.tick
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use azalea::{
        auth::game_profile::GameProfile,
        entity::metadata::Health,
        protocol::packets::game::{ClientboundDamageEvent, c_damage_event::OptionalEntityId},
    };
    use std::sync::Arc;

    #[test]
    fn only_victim_connection_records_server_damage_and_uses_its_own_attacker_index() {
        let mut app = App::new();
        app.add_message::<ReceiveGamePacketEvent>()
            .add_plugins(DamageAuditPlugin);
        let profile = |name: &str| {
            let account = Account::offline(name);
            GameProfileComponent(GameProfile::new(account.uuid_or_offline(), name.into()))
        };
        let attacker = app.world_mut().spawn(profile("Spear001")).id();
        let unrelated = app.world_mut().spawn(profile("WrongWorld")).id();
        let mut victim_index = EntityIdIndex::default();
        victim_index.insert(MinecraftEntityId(42), attacker);
        let mut watcher_index = EntityIdIndex::default();
        watcher_index.insert(MinecraftEntityId(42), unrelated);
        let victim = app
            .world_mut()
            .spawn((
                MinecraftEntityId(10),
                Account::offline("Mace001"),
                victim_index,
                TicksConnected(201),
                Health(20.0),
            ))
            .id();
        let watcher = app
            .world_mut()
            .spawn((
                MinecraftEntityId(11),
                Account::offline("Mace002"),
                watcher_index,
                TicksConnected(202),
            ))
            .id();
        for cause in [Some(42), Some(999), None] {
            let packet = Arc::new(ClientboundGamePacket::DamageEvent(ClientboundDamageEvent {
                entity_id: MinecraftEntityId(10),
                source_type_id: 13,
                source_cause_id: OptionalEntityId(cause),
                source_direct_id: OptionalEntityId(cause),
                source_position: None,
            }));
            for entity in [victim, watcher] {
                app.world_mut().write_message(ReceiveGamePacketEvent {
                    entity,
                    packet: packet.clone(),
                });
            }
        }
        app.update();
        let hits: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<DuelDamage>>()
            .drain()
            .collect();
        assert_eq!(
            hits.len(),
            3,
            "observer clients must not duplicate the victim's hit"
        );
        assert_eq!(hits[0].victim, "Mace001");
        assert_eq!(hits[0].attacker.as_deref(), Some("Spear001"));
        assert_eq!(hits[0].cause_entity_id, Some(42));
        assert_eq!(hits[0].damage_type_id, 13);
        assert_eq!(hits[0].tick, 201);
        assert_eq!(hits[1].attacker, None);
        assert_eq!(hits[1].cause_entity_id, Some(999));
        assert_eq!(hits[2].attacker, None);
        assert_eq!(hits[2].cause_entity_id, None);
        assert_eq!(app.world().get::<Health>(victim).unwrap().0, 20.0);
    }
}
