use azalea::{
    Account, Client, Vec3,
    connection::RawConnection,
    core::game_type::GameMode,
    ecs::{entity::Entity, world::World},
    entity::{
        Dead, EntityUuid, LoadedBy, Physics, Position,
        indexing::{EntityIdIndex, EntityUuidIndex},
        metadata::Health,
    },
    local_player::TabList,
    player::{GameProfileComponent, PlayerInfo},
    world::InstanceName,
};

const PURSUIT_TICKS: u64 = 100;
const FORGET_TICKS: u64 = 300;
const SHARED_ENGAGEMENT_DISTANCE: f64 = 128.0;

#[derive(Default)]
pub(super) struct Tracker {
    home: Option<(InstanceName, Vec3)>,
    locked: Option<Lock>,
}

struct Lock {
    uuid: u128,
    name: String,
    position: Vec3,
    velocity: Vec3,
    motion_tick: u64,
    seen_tick: u64,
}

struct Observation {
    target: super::Target,
    local: bool,
}

enum TargetState {
    Invalid,
    Unseen,
    Observed(Observation),
}

pub(super) enum Tracking {
    Visible(super::Target),
    Pursue {
        name: Option<String>,
        position: Vec3,
        mode: &'static str,
        age: u64,
    },
    Idle,
}

impl Tracker {
    pub(super) fn target_name(&self) -> Option<&str> {
        self.locked.as_ref().map(|lock| lock.name.as_str())
    }

    pub(super) fn update(&mut self, bot: &Client, prefix: &str, tick: u64) -> Tracking {
        let position = bot.position();
        let eye = bot.eye_position();
        let ecs = bot.ecs.lock();
        let Some(dimension) = ecs.get::<InstanceName>(bot.entity) else {
            self.clear();
            return Tracking::Idle;
        };
        if !finite(position) {
            return Tracking::Idle;
        }
        if self
            .home
            .as_ref()
            .is_none_or(|(world, _)| world != dimension)
        {
            self.clear();
            self.home = Some((dimension.clone(), position));
        }
        let Some(tab) = ecs.get::<TabList>(bot.entity) else {
            return self.return_home(position);
        };
        let own_name = ecs
            .get::<GameProfileComponent>(bot.entity)
            .map(|profile| profile.name.as_str())
            .or_else(|| {
                ecs.get::<Account>(bot.entity)
                    .map(|account| account.username.as_str())
            })
            .unwrap_or("");
        let view = View {
            ecs: &ecs,
            tab,
            viewer: bot.entity,
            dimension,
            own_name,
            eye,
            position,
            prefix,
        };

        if !prefix.starts_with('=')
            && let Some(lock) = &self.locked
        {
            let state = tab
                .values()
                .find(|info| info.profile.uuid.as_u128() == lock.uuid)
                .map_or(TargetState::Invalid, |info| view.observe(info));
            let local = matches!(&state, TargetState::Observed(observation) if observation.local);
            if !local {
                let visible_enemy = tab.values().any(|info| {
                    matches!(view.observe(info), TargetState::Observed(observation) if observation.local)
                });
                if visible_enemy || !shared_in_range(prefix, position, lock.position) {
                    self.locked = None;
                }
            }
        }

        if let Some(lock) = &mut self.locked {
            let state = tab
                .values()
                .find(|info| info.profile.uuid.as_u128() == lock.uuid)
                .map_or(TargetState::Invalid, |info| view.observe(info));
            match state {
                TargetState::Invalid => self.locked = None,
                TargetState::Observed(observation) => {
                    lock.refresh(&observation.target, tick);
                    return observation.tracking();
                }
                TargetState::Unseen => {
                    let age = tick.saturating_sub(lock.seen_tick);
                    if age <= FORGET_TICKS {
                        let mut position = lock.position + lock.velocity * age.min(6) as f64;
                        let mode = if age <= PURSUIT_TICKS {
                            "last_seen"
                        } else {
                            let waypoint = ((age - PURSUIT_TICKS - 1) / 50) % 4;
                            let (x, z) = [(20.0, 0.0), (0.0, 20.0), (-20.0, 0.0), (0.0, -20.0)]
                                [waypoint as usize];
                            position.x += x;
                            position.z += z;
                            "search"
                        };
                        if !shared_in_range(prefix, view.position, position) {
                            self.locked = None;
                            return self.return_home(view.position);
                        }
                        return Tracking::Pursue {
                            name: Some(lock.name.clone()),
                            position,
                            mode,
                            age,
                        };
                    }
                    self.locked = None;
                    return self.return_home(position);
                }
            }
        }

        let best = tab
            .values()
            .filter_map(|info| match view.observe(info) {
                TargetState::Observed(observation) => {
                    Some((info.profile.uuid.as_u128(), observation))
                }
                _ => None,
            })
            .min_by(|(uuid_a, a), (uuid_b, b)| {
                b.local
                    .cmp(&a.local)
                    .then_with(|| a.target.distance_sq.total_cmp(&b.target.distance_sq))
                    .then_with(|| uuid_a.cmp(uuid_b))
            });
        if let Some((uuid, observation)) = best {
            self.locked = Some(Lock {
                uuid,
                name: observation.target.name.clone(),
                position: observation.target.position,
                velocity: Vec3::ZERO,
                motion_tick: tick,
                seen_tick: tick,
            });
            observation.tracking()
        } else {
            self.return_home(position)
        }
    }

    pub(super) fn clear(&mut self) {
        self.home = None;
        self.locked = None;
    }

    fn return_home(&self, position: Vec3) -> Tracking {
        match &self.home {
            Some((_, home)) if position.distance_squared_to(*home) > 64.0 => Tracking::Pursue {
                name: None,
                position: *home,
                mode: "home",
                age: 0,
            },
            _ => Tracking::Idle,
        }
    }
}

impl Lock {
    fn refresh(&mut self, target: &super::Target, tick: u64) {
        let dt = tick.saturating_sub(self.motion_tick);
        if target.position != self.position {
            self.velocity = if (1..=3).contains(&dt) {
                cap_velocity((target.position - self.position) / dt as f64)
            } else {
                Vec3::ZERO
            };
            self.motion_tick = tick;
            self.position = target.position;
        } else if dt > 3 {
            self.velocity = Vec3::ZERO;
        }
        self.name.clone_from(&target.name);
        self.seen_tick = tick;
    }
}

impl Observation {
    fn tracking(self) -> Tracking {
        if self.local {
            Tracking::Visible(self.target)
        } else {
            Tracking::Pursue {
                name: Some(self.target.name),
                position: self.target.position,
                mode: "shared",
                age: 0,
            }
        }
    }
}

struct View<'a> {
    ecs: &'a World,
    tab: &'a TabList,
    viewer: Entity,
    dimension: &'a InstanceName,
    own_name: &'a str,
    eye: Vec3,
    position: Vec3,
    prefix: &'a str,
}

impl View<'_> {
    fn observe(&self, info: &PlayerInfo) -> TargetState {
        let matches = if let Some(name) = self.prefix.strip_prefix('=') {
            info.profile.name.eq_ignore_ascii_case(name)
        } else {
            info.profile
                .name
                .to_ascii_lowercase()
                .starts_with(&self.prefix.to_ascii_lowercase())
        };
        if !matches || !combat_mode(info.gamemode) {
            return TargetState::Invalid;
        }
        let Some(entity) = self
            .ecs
            .get_resource::<EntityUuidIndex>()
            .and_then(|index| index.get(&info.profile.uuid))
        else {
            return TargetState::Unseen;
        };
        if entity == self.viewer
            || ((self.ecs.get::<Account>(entity).is_some()
                || self.ecs.get::<RawConnection>(entity).is_some())
                && same_team(self.own_name, &info.profile.name))
        {
            return TargetState::Invalid;
        }
        if !identity_matches(self.ecs, entity, info.profile.uuid.as_u128()) {
            return TargetState::Unseen;
        }
        if self.ecs.get::<Dead>(entity).is_some()
            || self
                .ecs
                .get::<Health>(entity)
                .is_some_and(|health| !health.0.is_finite() || health.0 <= 0.0)
            || self
                .ecs
                .get::<InstanceName>(entity)
                .is_some_and(|world| world != self.dimension)
        {
            return TargetState::Invalid;
        }
        if self.ecs.get::<InstanceName>(entity) != Some(self.dimension) {
            return TargetState::Unseen;
        }
        let Some(loaded) = self.ecs.get::<LoadedBy>(entity) else {
            return TargetState::Unseen;
        };
        let local = loaded.contains(&self.viewer) && self.in_client_index(self.viewer, entity);
        if !local
            && !loaded
                .iter()
                .any(|observer| self.shared_observer(*observer, entity))
        {
            return TargetState::Unseen;
        }
        if self.ecs.get::<Health>(entity).is_none() {
            return TargetState::Unseen;
        }
        // Position is read only after a real, eligible observer is established.
        let Some(position) = self
            .ecs
            .get::<Position>(entity)
            .map(|position| **position)
            .filter(|p| finite(*p))
        else {
            return TargetState::Unseen;
        };
        let Some(physics) = self.ecs.get::<Physics>(entity) else {
            return TargetState::Unseen;
        };
        // Reject, rather than mark unseen, so an expired shared engagement
        // cannot continue through the last-seen/search memory path.
        if !local && !shared_in_range(self.prefix, self.position, position) {
            return TargetState::Invalid;
        }
        let bb = physics.bounding_box;
        let closest = Vec3::new(
            self.eye.x.clamp(bb.min.x, bb.max.x),
            self.eye.y.clamp(bb.min.y, bb.max.y),
            self.eye.z.clamp(bb.min.z, bb.max.z),
        );
        TargetState::Observed(Observation {
            target: super::Target {
                entity,
                name: info.profile.name.clone(),
                position,
                aim: Vec3::new(
                    (bb.min.x + bb.max.x) / 2.0,
                    (bb.min.y + bb.max.y) / 2.0,
                    (bb.min.z + bb.max.z) / 2.0,
                ),
                distance_sq: self.eye.distance_squared_to(closest),
            },
            local,
        })
    }

    fn in_client_index(&self, client: Entity, entity: Entity) -> bool {
        self.ecs
            .get::<EntityIdIndex>(client)
            .is_some_and(|index| index.contains_ecs_entity(entity))
    }

    fn shared_observer(&self, observer: Entity, target: Entity) -> bool {
        if observer == self.viewer
            || !self
                .ecs
                .get::<RawConnection>(observer)
                .is_some_and(RawConnection::is_alive)
            || self.ecs.get::<Dead>(observer).is_some()
            || !self
                .ecs
                .get::<Health>(observer)
                .is_some_and(|health| health.0.is_finite() && health.0 > 0.0)
            || self.ecs.get::<InstanceName>(observer) != Some(self.dimension)
            || !self.in_client_index(observer, target)
        {
            return false;
        }
        let Some(profile) = self.ecs.get::<GameProfileComponent>(observer) else {
            return false;
        };
        same_team(self.own_name, &profile.name)
            && self
                .tab
                .get(&profile.uuid)
                .is_some_and(|info| combat_mode(info.gamemode))
    }
}

fn same_team(own_name: &str, other_name: &str) -> bool {
    let team = own_name.trim_end_matches(|c: char| c.is_ascii_digit());
    !team.is_empty()
        && other_name
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .eq_ignore_ascii_case(team)
}

fn identity_matches(ecs: &World, entity: Entity, uuid: u128) -> bool {
    ecs.get::<EntityUuid>(entity)
        .map(|id| id.as_u128())
        .or_else(|| {
            ecs.get::<GameProfileComponent>(entity)
                .map(|profile| profile.uuid.as_u128())
        })
        .or_else(|| {
            ecs.get::<Account>(entity)
                .map(|account| account.uuid_or_offline().as_u128())
        })
        == Some(uuid)
}

fn combat_mode(mode: GameMode) -> bool {
    matches!(mode, GameMode::Survival | GameMode::Adventure)
}

fn finite(position: Vec3) -> bool {
    position.x.is_finite() && position.y.is_finite() && position.z.is_finite()
}

fn cap_velocity(velocity: Vec3) -> Vec3 {
    let length = velocity.length();
    if !length.is_finite() {
        Vec3::ZERO
    } else if length > 2.0 {
        velocity * (2.0 / length)
    } else {
        velocity
    }
}

fn shared_in_range(prefix: &str, viewer: Vec3, target: Vec3) -> bool {
    prefix.starts_with('=')
        || viewer.distance_squared_to(target) <= SHARED_ENGAGEMENT_DISTANCE.powi(2)
}

#[cfg(test)]
#[path = "tactics_tracking_tests.rs"]
mod tests;

#[cfg(test)]
mod engagement_tests {
    use super::*;

    #[test]
    fn automatic_shared_engagement_has_a_three_dimensional_boundary() {
        let origin = Vec3::ZERO;
        assert!(shared_in_range("Spear", origin, Vec3::new(128.0, 0.0, 0.0)));
        assert!(!shared_in_range(
            "Spear",
            origin,
            Vec3::new(128.01, 0.0, 0.0)
        ));
        assert!(!shared_in_range(
            "Spear",
            origin,
            Vec3::new(100.0, 100.0, 0.0)
        ));
        assert!(!shared_in_range(
            "Spear",
            origin,
            Vec3::new(1304.0, 0.0, 0.0)
        ));
    }

    #[test]
    fn explicit_target_retains_remote_pursuit() {
        assert!(shared_in_range(
            "=Player",
            Vec3::ZERO,
            Vec3::new(1304.0, 500.0, 0.0)
        ));
    }
}
