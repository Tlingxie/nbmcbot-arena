// How entity updates are processed (to avoid issues with shared worlds)
// - each bot contains a map of { entity id: updates received }
// - the shared world also contains a canonical "true" updates received for each
//   entity
// - when a client loads an entity, its "updates received" is set to the same as
//   the global "updates received"
// - when the shared world sees an entity for the first time, the "updates
//   received" is initially 0.
// - clients can force the shared "updates received" to 0 to make it so certain
//   entities (i.e. other bots in our swarm) don't get confused and updated by
//   other bots
// - when a client gets an update to an entity, we check if our "updates
//   received" is the same as the shared world's "updates received": if it is,
//   then process the update and increment the client's and shared world's
//   "updates received" if not, then we simply increment our local "updates
//   received" and do nothing else

use std::sync::Arc;

use azalea_world::{MinecraftEntityId, PartialInstance};
use bevy_ecs::prelude::*;
use derive_more::{Deref, DerefMut};
use parking_lot::RwLock;
use tracing::warn;

use crate::LocalEntity;

/// An [`EntityCommand`] that applies a "relative update" to an entity, which
/// means this update won't be run multiple times by different clients in the
/// same world.
///
/// This is used to avoid a bug where when there's multiple clients in the same
/// world and an entity sends a relative move packet to all clients, its
/// position gets desynced since the relative move is applied multiple times.
///
/// Don't use this unless you actually got an entity update packet that all
/// other clients within render distance will get too. You usually don't need
/// this when the change isn't relative either.
pub struct RelativeEntityUpdate {
    pub partial_world: Arc<RwLock<PartialInstance>>,
    // a function that takes the entity and updates it
    pub update: Box<dyn FnOnce(&mut EntityWorldMut) + Send + Sync>,
}
impl RelativeEntityUpdate {
    /// Align a newly loaded observer with the shared entity's current update count.
    pub fn track_entity(
        partial: &mut PartialInstance,
        entity_id: MinecraftEntityId,
        updates: Option<&UpdatesReceived>,
    ) {
        partial
            .entity_infos
            .updates_received
            .insert(entity_id, updates.map_or(0, |updates| updates.0));
    }
    pub fn new(
        partial_world: Arc<RwLock<PartialInstance>>,
        update: impl FnOnce(&mut EntityWorldMut) + Send + Sync + 'static,
    ) -> Self {
        Self {
            partial_world,
            update: Box::new(update),
        }
    }
}

/// A component that counts the number of times this entity has been modified.
///
/// This is used for making sure two clients don't do the same relative update
/// on an entity.
///
/// If an entity is local (i.e. it's a client/LocalEntity), this component
/// should NOT be present in the entity.
#[derive(Component, Debug, Deref, DerefMut)]
pub struct UpdatesReceived(u32);

impl EntityCommand for RelativeEntityUpdate {
    fn apply(self, mut entity: EntityWorldMut) {
        let Some(&entity_id) = entity.get::<MinecraftEntityId>() else {
            // Disconnected clients retain their ECS entity after losing game components.
            return;
        };
        let partial_entity_infos = &mut self.partial_world.write().entity_infos;

        if Some(entity.id()) == partial_entity_infos.owner_entity {
            // if the entity owns this partial world, it's always allowed to update itself
            (self.update)(&mut entity);
            return;
        };

        if entity.contains::<LocalEntity>() {
            // a client tried to update another client, which isn't allowed
            return;
        }

        let this_client_updates_received = partial_entity_infos
            .updates_received
            .get(&entity_id)
            .copied();

        let local_count = this_client_updates_received.unwrap_or(0);
        let shared_count = entity
            .get::<UpdatesReceived>()
            .map_or(0, |updates| updates.0);
        let can_update = local_count == shared_count;
        let new_updates_received = local_count.wrapping_add(1);
        // Every observer consumes the packet, even when another observer already applied it.
        partial_entity_infos
            .updates_received
            .insert(entity_id, new_updates_received);
        if can_update {
            entity.insert(UpdatesReceived(new_updates_received));

            (self.update)(&mut entity);
        }
    }
}

/// A system that logs a warning if an entity has both [`UpdatesReceived`]
/// and [`LocalEntity`].
pub fn debug_detect_updates_received_on_local_entities(
    query: Query<Entity, (With<LocalEntity>, With<UpdatesReceived>)>,
) {
    for entity in &query {
        warn!("Entity {entity:?} has both LocalEntity and UpdatesReceived");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct UpdateApplied;

    #[derive(Component, Default)]
    struct MoveCount(u32);

    fn move_once(world: &mut World, entity: Entity, partial: Arc<RwLock<PartialInstance>>) {
        RelativeEntityUpdate::new(partial, |entity| {
            entity.get_mut::<MoveCount>().unwrap().0 += 1;
        })
        .apply(world.entity_mut(entity));
    }

    #[test]
    fn duplicate_updates_advance_both_observers_and_allow_source_handoff() {
        let mut world = World::new();
        let target = world
            .spawn((MinecraftEntityId(100), MoveCount::default()))
            .id();
        let first = Arc::new(RwLock::new(PartialInstance::new(2, None)));
        let second = Arc::new(RwLock::new(PartialInstance::new(2, None)));
        for count in 1..=3 {
            move_once(&mut world, target, first.clone());
            move_once(&mut world, target, second.clone());
            assert_eq!(world.get::<MoveCount>(target).unwrap().0, count);
            assert_eq!(
                second
                    .read()
                    .entity_infos
                    .updates_received
                    .get(&MinecraftEntityId(100)),
                Some(&count)
            );
        }
        move_once(&mut world, target, second);
        assert_eq!(world.get::<MoveCount>(target).unwrap().0, 4);
    }

    #[test]
    fn local_owner_can_update_but_other_local_entities_cannot() {
        let mut world = World::new();
        let owner = world
            .spawn((LocalEntity, MinecraftEntityId(1), MoveCount::default()))
            .id();
        let other = world
            .spawn((LocalEntity, MinecraftEntityId(2), MoveCount::default()))
            .id();
        let partial = Arc::new(RwLock::new(PartialInstance::new(2, Some(owner))));
        move_once(&mut world, owner, partial.clone());
        move_once(&mut world, other, partial);
        assert_eq!(world.get::<MoveCount>(owner).unwrap().0, 1);
        assert_eq!(world.get::<MoveCount>(other).unwrap().0, 0);
    }

    #[test]
    fn newly_loaded_observer_starts_at_shared_count_and_can_take_over() {
        let mut world = World::new();
        let target = world
            .spawn((MinecraftEntityId(100), MoveCount::default()))
            .id();
        let first = Arc::new(RwLock::new(PartialInstance::new(2, None)));
        for _ in 0..5 {
            move_once(&mut world, target, first.clone());
        }
        let second = Arc::new(RwLock::new(PartialInstance::new(2, None)));
        RelativeEntityUpdate::track_entity(
            &mut second.write(),
            MinecraftEntityId(100),
            world.get::<UpdatesReceived>(target),
        );
        move_once(&mut world, target, second);
        assert_eq!(world.get::<MoveCount>(target).unwrap().0, 6);
    }

    #[test]
    fn disconnected_local_entity_ignores_another_clients_relative_update() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        let disconnected = world.spawn((LocalEntity, MinecraftEntityId(73))).id();
        world.entity_mut(disconnected).remove::<MinecraftEntityId>();
        let partial = Arc::new(RwLock::new(PartialInstance::new(2, Some(owner))));
        RelativeEntityUpdate::new(partial.clone(), |entity| {
            entity.insert(UpdateApplied);
        })
        .apply(world.entity_mut(disconnected));
        assert!(!world.entity(disconnected).contains::<UpdateApplied>());
        assert!(partial.read().entity_infos.updates_received.is_empty());
    }
}
