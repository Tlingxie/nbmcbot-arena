use anyhow::{Result, ensure};
use azalea::prelude::*;
use azalea::{
    Account, BlockPos, Client, Vec3,
    core::game_type::GameMode,
    ecs::entity::Entity,
    entity::{Dead, LoadedBy, Physics, Position, indexing::EntityUuidIndex, metadata::Health},
    pathfinder::{PathfinderOpts, astar::PathfinderTimeout, goals::RadiusGoal},
    physics::clip::{BlockShapeType, ClipContext, FluidPickType, clip},
};

fn eligible(is_self: bool, is_bot: bool, health: f32, mode: GameMode) -> bool {
    !is_self
        && !is_bot
        && health.is_finite()
        && health > 0.0
        && matches!(mode, GameMode::Survival | GameMode::Adventure)
}
fn can_attack(
    cooldown: usize,
    has_cooldown: bool,
    queued: bool,
    empty_synced: bool,
    visible: bool,
    distance_squared: f64,
) -> bool {
    cooldown == 0 && !has_cooldown && !queued && empty_synced && visible && distance_squared <= 9.0
}
fn empty_slot(current: u8, empty: &[bool; 9]) -> Option<u8> {
    if current < 9 && empty[usize::from(current)] {
        Some(current)
    } else {
        empty.iter().position(|empty| *empty).map(|slot| slot as u8)
    }
}

fn pursuit_radius(distance_squared: f64) -> f32 {
    if distance_squared <= 9.0 { 1.0 } else { 2.0 }
}

fn redirect_pursuit(bot: &Client, position: Vec3, distance_squared: f64) {
    // Azalea normally extends the existing route from its old endpoint.
    // Pursuit must discard that route and any pending result before retargeting.
    crate::game::stop_movement(bot, true);
    bot.start_goto_with_opts(
        RadiusGoal {
            pos: position,
            radius: pursuit_radius(distance_squared),
        },
        PathfinderOpts::new()
            .min_timeout(PathfinderTimeout::Nodes(2_000))
            .max_timeout(PathfinderTimeout::Nodes(25_000))
            .retry_on_no_path(false),
    );
}

struct Target {
    entity: Entity,
    name: String,
    position: Vec3,
    aim: Vec3,
    distance_squared: f64,
}

pub(crate) struct Fight {
    selector: String,
    target: Option<Entity>,
    last_repath: Option<u64>,
    last_goal: Option<BlockPos>,
    pursuing: bool,
    last_attack_tick: Option<u64>,
}
impl Fight {
    pub(crate) fn new(selector: String) -> Self {
        Self {
            selector,
            target: None,
            last_repath: None,
            last_goal: None,
            pursuing: false,
            last_attack_tick: None,
        }
    }
    pub(crate) fn tick(&mut self, bot: &Client, tick: u64, username: &str) -> Result<()> {
        let target = select_target(bot, &self.selector);
        let entity = target.as_ref().map(|target| target.entity);
        if entity != self.target {
            crate::game::stop_movement(bot, true);
            self.target = entity;
            self.last_repath = None;
            self.last_goal = None;
            self.pursuing = false;
        }
        let Some(target) = target else {
            return Ok(());
        };
        let (slot, selected) = {
            let ecs = bot.ecs.lock();
            let Some(inventory) = ecs.get::<azalea::entity::inventory::Inventory>(bot.entity)
            else {
                return Ok(());
            };
            ensure!(
                inventory.id == 0 && inventory.menu().try_as_player().is_some(),
                "close the container before fighting"
            );
            let hotbar = inventory.menu().slots();
            let hotbar = &hotbar[inventory.menu().hotbar_slots_range()];
            let mut empty = [false; 9];
            for (index, item) in hotbar.iter().enumerate().take(9) {
                empty[index] = item.is_empty();
            }
            let slot = empty_slot(inventory.selected_hotbar_slot, &empty).ok_or_else(|| {
                anyhow::anyhow!("fight needs an empty hotbar slot; all nine slots are occupied")
            })?;
            let selected = inventory.selected_hotbar_slot;
            if ecs
                .get::<azalea::inventory::LastSentSelectedHotbarSlot>(bot.entity)
                .is_none()
            {
                return Ok(());
            }
            (slot, selected)
        };
        if selected != slot {
            bot.set_selected_hotbar_slot(slot);
        }
        let visible = target.distance_squared <= 9.0 && line_of_sight(bot, target.aim);
        if target.distance_squared <= 9.0 && visible {
            if self.pursuing {
                crate::game::stop_movement(bot, true);
                self.pursuing = false;
            }
            bot.look_at(target.aim);
            let attack_tick = {
                let mut ecs = bot.ecs.lock();
                let actual_tick = ecs
                    .get::<azalea::tick_counter::TicksConnected>(bot.entity)
                    .map(|counter| counter.0);
                let inventory = ecs.get::<azalea::entity::inventory::Inventory>(bot.entity);
                let synced = inventory.is_some_and(|inventory| {
                    inventory.held_item().is_empty()
                        && ecs
                            .get::<azalea::inventory::LastSentSelectedHotbarSlot>(bot.entity)
                            .is_some_and(|last| last.slot == inventory.selected_hotbar_slot)
                });
                let cooldown = match (
                    ecs.get::<azalea::entity::Attributes>(bot.entity),
                    ecs.get::<azalea::attack::TicksSinceLastAttack>(bot.entity),
                ) {
                    (Some(attributes), Some(ticks)) => {
                        (azalea::attack::get_attack_strength_delay(attributes) - ticks.0 as f32)
                            .max(0.0)
                            .ceil() as usize
                    }
                    _ => usize::MAX,
                };
                let has_cooldown = ecs
                    .get::<azalea::attack::AttackStrengthScale>(bot.entity)
                    .is_none_or(|scale| scale.0 < 1.0);
                if actual_tick.is_some()
                    && self.last_attack_tick != actual_tick
                    && can_attack(
                        cooldown,
                        has_cooldown,
                        ecs.get::<azalea::attack::AttackQueued>(bot.entity)
                            .is_some(),
                        synced,
                        visible,
                        target.distance_squared,
                    )
                {
                    ecs.entity_mut(bot.entity)
                        .insert(azalea::attack::AttackQueued {
                            target: target.entity,
                        });
                    actual_tick
                } else {
                    None
                }
            };
            if let Some(attack_tick) = attack_tick {
                self.last_attack_tick = Some(attack_tick);
                println!(
                    "{}",
                    serde_json::json!({"event":"combat_attack","bot":username,"player":target.name,"tick":attack_tick,"cooldown_remaining_ticks":0,"empty_hand":true})
                );
            }
        } else if self
            .last_repath
            .is_none_or(|last| tick.saturating_sub(last) >= 10)
        {
            let goal = BlockPos::from(target.position);
            if self.last_goal != Some(goal) || !self.pursuing || bot.is_goto_target_reached() {
                redirect_pursuit(bot, target.position, target.distance_squared);
                self.last_goal = Some(goal);
                self.pursuing = true;
            }
            self.last_repath = Some(tick);
        }
        Ok(())
    }
}

fn select_target(bot: &Client, selector: &str) -> Option<Target> {
    let eye = bot.eye_position();
    let mut ecs = bot.ecs.lock();
    let tab = ecs
        .get::<azalea::local_player::TabList>(bot.entity)?
        .clone();
    let candidates: Vec<_> = {
        let index = ecs.resource::<EntityUuidIndex>();
        tab.iter()
            .filter(|(_, info)| {
                selector.eq_ignore_ascii_case("nearest")
                    || info.profile.name.eq_ignore_ascii_case(selector)
            })
            .filter_map(|(uuid, info)| index.get(uuid).map(|entity| (entity, info.clone())))
            .collect()
    };
    let mut query = ecs.query::<(
        &Position,
        &Physics,
        &Health,
        Option<&Dead>,
        Option<&Account>,
        &LoadedBy,
    )>();
    candidates
        .into_iter()
        .filter_map(|(entity, info)| {
            let (position, physics, health, dead, account, loaded) =
                query.get(&ecs, entity).ok()?;
            if dead.is_some()
                || !loaded.contains(&bot.entity)
                || !eligible(
                    entity == bot.entity,
                    account.is_some(),
                    health.0,
                    info.gamemode,
                )
            {
                return None;
            }
            let bb = physics.bounding_box;
            let closest = Vec3::new(
                eye.x.clamp(bb.min.x, bb.max.x),
                eye.y.clamp(bb.min.y, bb.max.y),
                eye.z.clamp(bb.min.z, bb.max.z),
            );
            let aim = Vec3::new(
                (bb.min.x + bb.max.x) / 2.0,
                eye.y.clamp(bb.min.y + 0.05, bb.max.y - 0.05),
                (bb.min.z + bb.max.z) / 2.0,
            );
            Some(Target {
                entity,
                name: info.profile.name,
                position: **position,
                aim,
                distance_squared: eye.distance_squared_to(closest),
            })
        })
        .min_by(|a, b| a.distance_squared.total_cmp(&b.distance_squared))
}

pub(crate) fn line_of_sight(bot: &Client, to: Vec3) -> bool {
    let from = bot.eye_position();
    let world = bot.world();
    let world = world.read();
    let steps = (from.distance_squared_to(to).sqrt() * 5.0).ceil().max(1.0) as usize;
    for index in 0..=steps {
        let fraction = index as f64 / steps as f64;
        let point = Vec3::new(
            from.x + (to.x - from.x) * fraction,
            from.y + (to.y - from.y) * fraction,
            from.z + (to.z - from.z) * fraction,
        );
        if world
            .chunks
            .get_block_state(BlockPos::from(point))
            .is_none()
        {
            return false;
        }
    }
    clip(
        &world.chunks,
        ClipContext {
            from,
            to,
            block_shape_type: BlockShapeType::Collider,
            fluid_pick_type: FluidPickType::None,
        },
    )
    .miss
}

#[cfg(test)]
mod tests {
    use super::*;
    use azalea::core::game_type::GameMode;
    #[test]
    fn redirect_discards_old_route_and_preserves_other_bot() {
        use azalea::ecs::{message::Messages, world::World};
        use azalea::pathfinder::{
            ExecutingPath, GotoEvent, PathFoundEvent, Pathfinder, StopPathfindingEvent,
        };
        use std::{
            collections::VecDeque,
            sync::{Arc, atomic::Ordering},
            time::Instant,
        };
        let mut world = World::new();
        let route = || ExecutingPath {
            path: VecDeque::new(),
            queued_path: Some(VecDeque::new()),
            last_reached_node: BlockPos::new(10, 64, 4),
            last_node_reached_at: Instant::now(),
            is_path_partial: false,
        };
        let first = world
            .spawn((
                Pathfinder::default(),
                route(),
                azalea::entity::Jumping::default(),
            ))
            .id();
        let second = world.spawn((Pathfinder::default(), route())).id();
        world.init_resource::<Messages<GotoEvent>>();
        world.init_resource::<Messages<PathFoundEvent>>();
        world.init_resource::<Messages<StopPathfindingEvent>>();
        world.init_resource::<Messages<azalea::mining::StartMiningBlockEvent>>();
        world.init_resource::<Messages<azalea::mining::StopMiningBlockEvent>>();
        world.init_resource::<Messages<azalea::attack::AttackEvent>>();
        world.init_resource::<Messages<azalea::movement::StartWalkEvent>>();
        world.write_message(GotoEvent::new(
            first,
            RadiusGoal {
                pos: Vec3::new(12.5, 64.0, 4.5),
                radius: 2.0,
            },
            PathfinderOpts::new(),
        ));
        world.write_message(GotoEvent::new(
            second,
            RadiusGoal {
                pos: Vec3::new(20.5, 64.0, 4.5),
                radius: 2.0,
            },
            PathfinderOpts::new(),
        ));
        let generation = world.get::<Pathfinder>(first).unwrap().goto_id.clone();
        let before = generation.load(Ordering::SeqCst);
        let bot = Client::new(first, Arc::new(parking_lot::Mutex::new(world)));
        redirect_pursuit(&bot, Vec3::new(4.5, 64.0, 12.5), 64.0);
        let ecs = bot.ecs.lock();
        assert!(
            ecs.get::<ExecutingPath>(first).is_none(),
            "old route would be prepended to new pursuit"
        );
        assert!(ecs.get::<ExecutingPath>(second).is_some());
        assert!(generation.load(Ordering::SeqCst) > before);
        let queue = ecs.resource::<Messages<GotoEvent>>();
        let mut cursor = queue.get_cursor();
        let goals: Vec<_> = cursor
            .read(queue)
            .filter(|event| event.entity == first)
            .collect();
        assert_eq!(goals.len(), 1);
        assert!(goals[0].goal.success(BlockPos::new(4, 64, 12)));
    }
    #[test]
    fn blocked_pursuit_has_a_reachable_node_for_off_center_feet() {
        use azalea::pathfinder::goals::Goal;
        let goal = RadiusGoal {
            pos: Vec3::new(4.2, 64.0, 4.2),
            radius: pursuit_radius(4.0),
        };
        assert!(goal.success(BlockPos::new(4, 64, 4)));
        assert!(!goal.success(BlockPos::new(2, 64, 4)));
    }
    #[test]
    fn nearest_selection_excludes_unattackable_and_local_players() {
        assert!(eligible(false, false, 20.0, GameMode::Survival));
        assert!(eligible(false, false, 20.0, GameMode::Adventure));
        assert!(!eligible(true, false, 20.0, GameMode::Survival));
        assert!(!eligible(false, true, 20.0, GameMode::Survival));
        assert!(!eligible(false, false, 0.0, GameMode::Survival));
        assert!(!eligible(false, false, f32::NAN, GameMode::Survival));
        assert!(!eligible(false, false, 20.0, GameMode::Creative));
        assert!(!eligible(false, false, 20.0, GameMode::Spectator));
    }
    #[test]
    fn attacks_require_all_live_constraints() {
        assert!(can_attack(0, false, false, true, true, 9.0));
        for values in [
            (1, false, false, true, true, 1.0),
            (0, true, false, true, true, 1.0),
            (0, false, true, true, true, 1.0),
            (0, false, false, false, true, 1.0),
            (0, false, false, true, false, 1.0),
            (0, false, false, true, true, 9.01),
        ] {
            assert!(!can_attack(
                values.0, values.1, values.2, values.3, values.4, values.5
            ));
        }
    }
    #[test]
    fn empty_hand_never_discards_items_or_selects_a_weapon() {
        assert_eq!(
            empty_slot(
                3,
                &[false, false, false, true, false, false, false, false, false]
            ),
            Some(3)
        );
        assert_eq!(
            empty_slot(
                0,
                &[false, false, true, false, false, false, false, false, false]
            ),
            Some(2)
        );
        assert_eq!(empty_slot(0, &[false; 9]), None);
    }
}
