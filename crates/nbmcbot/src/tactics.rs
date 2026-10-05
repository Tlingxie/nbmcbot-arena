use anyhow::{Result, ensure};
use azalea::{
    Client, SprintDirection, Vec3, WalkDirection,
    ecs::entity::Entity,
    entity::{LookDirection, Physics, metadata::FallFlying},
    protocol::packets::game::s_interact::InteractionHand,
    registry::builtin::ItemKind,
};
#[cfg(test)]
use azalea::{
    core::game_type::GameMode,
    entity::{LoadedBy, Position, metadata::Health},
};
use serde_json::json;

#[path = "tactics_equipment.rs"]
mod equipment;
#[path = "tactics_mace.rs"]
mod mace_flight;
#[path = "tactics_math.rs"]
mod math;
#[path = "tactics_pursuit.rs"]
mod pursuit;
#[path = "tactics_tracking.rs"]
mod tracking;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Approach,
    Pearl,
    Catch,
    Fall,
    WindJump,
    Recover,
    Takeoff,
    Climb,
    Charge,
    Exit,
    Turn,
    MaceTakeoff,
    MaceClimb,
    MaceDive,
    MaceSwap,
    MaceDrop,
}

struct Target {
    entity: Entity,
    name: String,
    position: Vec3,
    aim: Vec3,
    distance_sq: f64,
}

pub(crate) struct Duel {
    style: String,
    prefix: String,
    phase: Phase,
    since: u64,
    target: Option<Entity>,
    previous: Option<(Vec3, u64)>,
    motion_sample: Option<(Vec3, u64)>,
    motion_interval: Option<u64>,
    recent_velocity: Vec3,
    recent_turn: f64,
    velocity_ready: bool,
    peak_y: f64,
    launch_y: f64,
    launch_yaw: f32,
    last_use: u64,
    last_rocket: u64,
    last_attack: u64,
    charging: bool,
    pass_heading: Vec3,
    pass_target: Vec3,
    pass_committed: bool,
    mace_equipped_at: Option<u64>,
    mace_ground_y: Option<f64>,
    mace_climb_height: Option<f64>,
    mace_recovery_ticks: u8,
    turn_yaw: Option<f32>,
    pursuit: pursuit::Pursuit,
    tracker: tracking::Tracker,
    tracking_mode: Option<&'static str>,
}

pub(crate) struct DuelTelemetry<'a> {
    pub style: &'a str,
    pub phase: String,
    pub target: Option<&'a str>,
}

impl Duel {
    pub(crate) fn telemetry(&self) -> DuelTelemetry<'_> {
        DuelTelemetry {
            style: &self.style,
            phase: format!("{:?}", self.phase),
            target: self.tracker.target_name(),
        }
    }

    pub(crate) fn new(style: String, prefix: String) -> Self {
        let phase = if style == "mace" {
            Phase::Approach
        } else {
            Phase::Takeoff
        };
        Self {
            style,
            prefix,
            phase,
            since: 0,
            target: None,
            previous: None,
            motion_sample: None,
            motion_interval: None,
            recent_velocity: Vec3::ZERO,
            recent_turn: 0.0,
            velocity_ready: false,
            peak_y: f64::NEG_INFINITY,
            launch_y: 0.0,
            launch_yaw: 0.0,
            last_use: 0,
            last_rocket: 0,
            last_attack: 0,
            charging: false,
            pass_heading: Vec3::new(1.0, 0.0, 0.0),
            pass_target: Vec3::default(),
            pass_committed: false,
            mace_equipped_at: None,
            mace_ground_y: None,
            mace_climb_height: None,
            mace_recovery_ticks: 0,
            turn_yaw: None,
            pursuit: pursuit::Pursuit::default(),
            tracker: tracking::Tracker::default(),
            tracking_mode: None,
        }
    }

    pub(crate) fn cancel(&mut self, bot: &Client, send_packets: bool) {
        self.stop_pursuit(bot, send_packets);
        self.tracker.clear();
        self.tracking_mode = None;
        crate::aerial::cancel(bot, send_packets);
        equipment::cancel(bot);
        self.charging = false;
        self.target = None;
        self.mace_ground_y = None;
        self.mace_climb_height = None;
        self.mace_recovery_ticks = 0;
        self.reset_velocity();
    }

    fn phase(&mut self, phase: Phase, tick: u64, username: &str) {
        if phase != self.phase {
            self.phase = phase;
            self.since = tick;
            self.mace_recovery_ticks = 0;
            if phase == Phase::Charge {
                self.pass_committed = false;
            }
            if phase == Phase::Fall {
                self.mace_equipped_at = None;
            }
            if phase == Phase::Turn {
                self.turn_yaw = None;
            }
            if phase == Phase::MaceClimb {
                self.mace_climb_height = None;
            }
            println!(
                "{}",
                json!({"event":"duel_phase","bot":username,"style":self.style,"phase":format!("{phase:?}"),"tick":tick})
            );
        }
    }

    pub(crate) fn tick(&mut self, bot: &Client, tick: u64, username: &str) -> Result<()> {
        if bot.health() <= 0.0 {
            return Ok(());
        }
        if self.phase == Phase::Recover
            && let Some(physics) = bot.get_component::<Physics>()
        {
            self.mace_recover(bot, &physics)?;
        }
        let tracking = self.tracker.update(bot, &self.prefix, tick);
        let target = if let tracking::Tracking::Visible(target) = tracking {
            if self.tracking_mode.take().is_some() {
                self.stop_pursuit(bot, true);
                println!(
                    "{}",
                    json!({"event":"duel_tracking","bot":username,"mode":"locked","target":target.name,"tick":tick})
                );
            }
            target
        } else {
            self.reset_velocity();
            self.mace_recovery_ticks = 0;
            if self.style == "mace" && matches!(self.phase, Phase::MaceSwap | Phase::MaceDrop) {
                self.phase(Phase::Recover, tick, username);
            }
            if self.target.take().is_some() {
                crate::aerial::release_use(bot);
                bot.walk(WalkDirection::None);
                bot.set_jumping(false);
                self.charging = false;
                let mut ecs = bot.ecs.lock();
                ecs.entity_mut(bot.entity)
                    .remove::<azalea::attack::AttackQueued>();
                crate::game::keep_other_entity_messages(
                    &mut ecs.resource_mut::<azalea::ecs::message::Messages<azalea::attack::AttackEvent>>(),
                    bot.entity, |event| &mut event.entity);
            }
            let (name, goal, mode, age) = match tracking {
                tracking::Tracking::Pursue {
                    name,
                    position,
                    mode,
                    age,
                } => (name, position, mode, age),
                _ => {
                    let mut goal = bot.position();
                    if bot
                        .get_component::<Physics>()
                        .is_some_and(|p| !p.on_ground())
                    {
                        goal.y -= 16.0;
                    }
                    (None, goal, "idle", 0)
                }
            };
            if self.tracking_mode != Some(mode) || tick.is_multiple_of(40) {
                println!(
                    "{}",
                    json!({"event":"duel_tracking","bot":username,"mode":mode,
                    "target":name,"goal":array(goal),"position":array(bot.position()),"age_ticks":age,"health":bot.health(),"tick":tick})
                );
            }
            self.tracking_mode = Some(mode);
            self.pursue(bot, tick, username, goal)?;
            return Ok(());
        };
        if self.target != Some(target.entity) {
            let reacquired = self.target.is_none();
            self.target = Some(target.entity);
            self.reset_velocity();
            self.pass_committed = false;
            if self.style == "mace" {
                if matches!(self.phase, Phase::MaceSwap | Phase::MaceDrop) {
                    self.phase(Phase::Recover, tick, username);
                } else if self.phase != Phase::Recover {
                    self.phase(Phase::Approach, tick, username);
                }
            } else if reacquired {
                self.phase(Phase::Takeoff, tick, username);
            }
            println!(
                "{}",
                json!({"event":"duel_target","bot":username,"player":target.name})
            );
        }
        let velocity = self.sample_velocity(target.position, tick);
        let Some(physics) = bot.get_component::<Physics>() else {
            return Ok(());
        };
        if self.style == "mace" {
            self.mace(bot, tick, username, &target, velocity, &physics)?;
        } else {
            self.spear(bot, tick, username, &target, velocity, &physics)?;
        }
        if tick.is_multiple_of(20) {
            let p = bot.position();
            println!(
                "{}",
                json!({"event":"duel_observation","bot":username,"style":self.style,"phase":format!("{:?}",self.phase),"position":[p.x,p.y,p.z],"velocity":[physics.velocity.x,physics.velocity.y,physics.velocity.z],"health":bot.health(),"gliding":bot.get_component::<FallFlying>().is_some_and(|f|f.0),"chest":if has_chest(bot,ItemKind::Elytra){"elytra"}else if has_chest(bot,ItemKind::NetheriteChestplate){"netherite_chestplate"}else{"other"},"target":target.name,"tick":tick})
            );
        }
        Ok(())
    }

    fn reset_velocity(&mut self) {
        self.previous = None;
        self.motion_sample = None;
        self.motion_interval = None;
        self.recent_velocity = Vec3::ZERO;
        self.recent_turn = 0.0;
        self.velocity_ready = false;
    }

    fn sample_velocity(&mut self, position: Vec3, tick: u64) -> Vec3 {
        // Unchanged ECS positions between movement packets are not new motion samples.
        let previous = self
            .previous
            .filter(|(_, t)| (1..=3).contains(&tick.saturating_sub(*t)));
        self.velocity_ready = previous.is_some();
        if previous.is_none() || self.motion_sample.is_none() {
            self.motion_sample = Some((position, tick));
            self.motion_interval = None;
            self.recent_velocity = Vec3::ZERO;
            self.recent_turn = 0.0;
        } else if let Some((p, t)) = self.motion_sample {
            let dt = tick.saturating_sub(t);
            if position != p {
                self.velocity_ready = (1..=3).contains(&dt);
                let old_velocity = self.recent_velocity;
                self.recent_velocity = if self.velocity_ready {
                    (position - p) / dt as f64
                } else {
                    Vec3::ZERO
                };
                let v = self.recent_velocity;
                self.recent_turn = if let Some(old_dt) = self.motion_interval
                    && self.velocity_ready
                    && old_velocity.horizontal_distance_squared() > 0.01
                    && v.horizontal_distance_squared() > 0.01
                {
                    ((old_velocity.x * v.z - old_velocity.z * v.x)
                        .atan2(old_velocity.x * v.x + old_velocity.z * v.z)
                        / ((old_dt + dt) as f64 * 0.5))
                        .clamp(-std::f64::consts::FRAC_PI_6, std::f64::consts::FRAC_PI_6)
                } else {
                    0.0
                };
                self.motion_sample = Some((position, tick));
                self.motion_interval = self.velocity_ready.then_some(dt);
            } else if dt > 3 {
                self.motion_interval = None;
                self.recent_velocity = Vec3::ZERO;
                self.recent_turn = 0.0;
            }
        }
        self.previous = Some((position, tick));
        self.recent_velocity
    }

    fn predicted_motion(&self, mut position: Vec3, tick: u64) -> (Vec3, Vec3) {
        let Some((_, sample_tick)) = self.motion_sample else {
            return (position, Vec3::ZERO);
        };
        let Some(interval) = self.motion_interval else {
            return (position, Vec3::ZERO);
        };
        let Some(age) = tick.checked_sub(sample_tick).filter(|age| *age <= 3) else {
            return (position, Vec3::ZERO);
        };
        if !self.velocity_ready {
            return (position, Vec3::ZERO);
        }
        let n = interval as f64;
        let half_turn = self.recent_turn * 0.5;
        let gain = if half_turn.abs() < 1e-8 {
            1.0
        } else {
            n * half_turn.sin() / (n * half_turn).sin()
        };
        // A move-then-turn chord spans headings 0..n-1; the next step uses heading n.
        let (sin, cos) = (half_turn * (n + 1.0)).sin_cos();
        let raw = self.recent_velocity;
        let mut velocity = Vec3::new(
            (raw.x * cos - raw.z * sin) * gain,
            raw.y,
            (raw.x * sin + raw.z * cos) * gain,
        );
        let (sin, cos) = self.recent_turn.sin_cos();
        for _ in 0..age {
            position += velocity;
            velocity = Vec3::new(
                velocity.x * cos - velocity.z * sin,
                velocity.y,
                velocity.x * sin + velocity.z * cos,
            );
        }
        (position, velocity)
    }

    fn mace(
        &mut self,
        bot: &Client,
        tick: u64,
        username: &str,
        target: &Target,
        velocity: Vec3,
        physics: &Physics,
    ) -> Result<()> {
        let p = bot.position();
        if matches!(
            self.phase,
            Phase::MaceTakeoff
                | Phase::MaceClimb
                | Phase::MaceDive
                | Phase::MaceSwap
                | Phase::MaceDrop
        ) || (self.phase == Phase::Approach && equipment::has_flight_kit(bot))
        {
            return self.mace_flight(bot, tick, username, target, velocity, physics);
        }
        self.peak_y = self.peak_y.max(p.y);
        let age = tick.saturating_sub(self.since);
        match self.phase {
            Phase::Approach => {
                let horizontal = horizontal_distance(p, target.position);
                if horizontal > 4.0 {
                    aim(bot, target.position.up(1.2), 35.0);
                    bot.sprint(SprintDirection::Forward);
                    bot.set_jumping(physics.horizontal_collision);
                } else {
                    bot.walk(WalkDirection::None);
                    bot.set_jumping(false);
                    self.launch_y = p.y;
                    self.peak_y = p.y;
                    self.launch_yaw = bot
                        .get_component::<LookDirection>()
                        .map(|d| d.y_rot())
                        .unwrap_or(0.0);
                    self.phase(Phase::Pearl, tick, username);
                }
            }
            Phase::Pearl => {
                bot.set_direction(self.launch_yaw, -90.0);
                if select_item(bot, ItemKind::EnderPearl)?
                    && tick.saturating_sub(self.last_use) >= 20
                    && age >= 6
                    && crate::aerial::use_item(bot, InteractionHand::MainHand)
                {
                    self.last_use = tick;
                    self.phase(Phase::Catch, tick, username);
                    action(username, "pearl_up", tick);
                }
            }
            Phase::Catch => {
                bot.set_direction(self.launch_yaw, -90.0);
                if select_item(bot, ItemKind::WindCharge)?
                    && age >= 5
                    && crate::aerial::use_item(bot, InteractionHand::MainHand)
                {
                    self.last_use = tick;
                    self.phase(Phase::Fall, tick, username);
                    action(username, "wind_catch", tick);
                }
            }
            Phase::Fall => {
                let equipped = select_item(bot, ItemKind::Mace)?;
                if equipped {
                    self.mace_equipped_at.get_or_insert(tick);
                }
                let lead = math::intercept(array(target.position), array(velocity), 3.0);
                let aimpoint = Vec3::new(lead[0], lead[1] + 0.8, lead[2]);
                aim(bot, aimpoint, 45.0);
                let horizontal = horizontal_distance(p, target.position);
                if horizontal > 0.55 {
                    bot.walk(WalkDirection::Forward);
                } else {
                    bot.walk(WalkDirection::None);
                }
                bot.set_jumping(false);
                if equipped
                    && !bot.get_component::<FallFlying>().is_some_and(|f| f.0)
                    && math::smash_ready(physics.velocity.y, self.peak_y - p.y, target.distance_sq)
                    && self
                        .mace_equipped_at
                        .is_some_and(|at| tick.saturating_sub(at) >= 34)
                    && tick.saturating_sub(self.last_attack) >= 34
                    && crate::combat::line_of_sight(bot, target.aim)
                {
                    bot.attack(target.entity);
                    self.last_attack = tick;
                    println!(
                        "{}",
                        json!({"event":"duel_attack","style":"mace","bot":username,"player":target.name,"fall_distance":self.peak_y-p.y,"tick":tick})
                    );
                    self.phase(Phase::Recover, tick, username);
                } else if physics.on_ground() && age > 35 {
                    // Retry with a normal wind jump if the two projectiles missed.
                    if p.y < self.launch_y + 2.0 {
                        self.phase(Phase::WindJump, tick, username);
                    } else {
                        self.phase(Phase::Recover, tick, username);
                    }
                }
                if age > 120 {
                    self.phase(Phase::Recover, tick, username);
                }
            }
            Phase::WindJump => {
                bot.set_direction(self.launch_yaw, 90.0);
                bot.walk(WalkDirection::None);
                if select_item(bot, ItemKind::WindCharge)? {
                    bot.set_jumping(true);
                    if crate::aerial::use_item(bot, InteractionHand::MainHand) {
                        self.peak_y = p.y;
                        self.launch_y = p.y - 3.0;
                        self.phase(Phase::Fall, tick, username);
                        action(username, "wind_jump", tick);
                    }
                }
            }
            Phase::Recover => {
                self.mace_recover(bot, physics)?;
                bot.walk(WalkDirection::None);
                bot.set_jumping(false);
                if physics.on_ground() && age >= 12 {
                    self.phase(Phase::Approach, tick, username);
                } else if !physics.on_ground()
                    && self.mace_ground_y.is_some()
                    && equipment::has_flight_kit(bot)
                    && has_chest(bot, ItemKind::Elytra)
                    && bot.get_component::<FallFlying>().is_some_and(|f| f.0)
                    && physics.velocity.y >= -0.5
                    && equipment::equip_chest(bot, ItemKind::Elytra)?
                {
                    // FallFlying is predicted locally; require a settled glide
                    // before leaving recovery, without waiting to reach ground.
                    self.mace_recovery_ticks = self.mace_recovery_ticks.saturating_add(1);
                    if age >= 12 && self.mace_recovery_ticks >= 5 {
                        self.phase(Phase::MaceClimb, tick, username);
                    }
                } else {
                    self.mace_recovery_ticks = 0;
                }
            }
            _ => self.phase(Phase::Approach, tick, username),
        }
        Ok(())
    }

    fn spear(
        &mut self,
        bot: &Client,
        tick: u64,
        username: &str,
        target: &Target,
        velocity: Vec3,
        physics: &Physics,
    ) -> Result<()> {
        let p = bot.position();
        let age = tick.saturating_sub(self.since);
        let gliding = bot.get_component::<FallFlying>().is_some_and(|f| f.0);
        ensure!(
            has_chest(bot, ItemKind::Elytra),
            "spear duel needs an equipped elytra"
        );
        let equipped = select_item(bot, ItemKind::NetheriteSpear)?;
        if !gliding && !matches!(self.phase, Phase::Takeoff) {
            self.charging = false;
            crate::aerial::release_use(bot);
            self.phase(Phase::Takeoff, tick, username);
        }
        match self.phase {
            Phase::Takeoff => {
                let (yaw, _) = angles(bot.eye_position(), target.aim);
                bot.set_direction(yaw, -25.0);
                bot.sprint(SprintDirection::Forward);
                bot.set_jumping(physics.on_ground());
                if !physics.on_ground() && physics.velocity.y < 0.05 {
                    crate::aerial::start_gliding(bot);
                }
                if gliding && !physics.on_ground() {
                    bot.set_jumping(false);
                    bot.walk(WalkDirection::None);
                    self.rocket(bot, tick, username)?;
                    self.phase(Phase::Climb, tick, username);
                }
            }
            Phase::Climb => {
                let (yaw, _) = angles(bot.eye_position(), target.aim);
                steer(bot, yaw, -22.0, 10.0);
                if physics.velocity.horizontal_distance_squared() < 0.64 {
                    self.rocket(bot, tick, username)?;
                }
                if equipped
                    && !self.charging
                    && tick.saturating_sub(self.last_rocket) >= 2
                    && crate::aerial::use_item(bot, InteractionHand::MainHand)
                {
                    self.charging = true;
                    self.last_use = tick;
                    action(username, "spear_charge", tick);
                }
                if p.y >= target.position.y + 5.0 || age > 30 {
                    self.phase(Phase::Charge, tick, username);
                }
            }
            Phase::Charge => {
                let distance = horizontal_distance(p, target.position);
                let contact_distance_sq = p.distance_squared_to(target.position);
                let lead = math::intercept(
                    array(target.position),
                    array(velocity),
                    (distance
                        / physics
                            .velocity
                            .horizontal_distance_squared()
                            .sqrt()
                            .max(0.5))
                    .min(4.0),
                );
                aim(bot, Vec3::new(lead[0], lead[1] + 1.45, lead[2]), 18.0);
                if equipped
                    && !self.charging
                    && tick.saturating_sub(self.last_rocket) >= 2
                    && crate::aerial::use_item(bot, InteractionHand::MainHand)
                {
                    self.charging = true;
                    self.last_use = tick;
                    action(username, "spear_charge", tick);
                }
                if distance > 15.0 && tick.saturating_sub(self.last_rocket) > 28 {
                    crate::aerial::release_use(bot);
                    self.charging = false;
                    self.rocket(bot, tick, username)?;
                }
                let heading = direction(bot);
                if contact_distance_sq < 36.0 && !self.pass_committed {
                    self.pass_heading = heading;
                    self.pass_target = target.position;
                    self.pass_committed = true;
                }
                if (self.pass_committed
                    && age > 10
                    && math::passed_target(
                        array(p),
                        array(self.pass_target),
                        array(self.pass_heading),
                    ))
                    || contact_distance_sq < 9.0
                    || age > 100
                {
                    self.phase(Phase::Exit, tick, username);
                    action(username, "spear_pass", tick);
                }
            }
            Phase::Exit => {
                let yaw = (-self.pass_heading.x)
                    .atan2(self.pass_heading.z)
                    .to_degrees() as f32;
                let pitch =
                    if age < 4 || p.y < target.position.y + 1.2 || physics.velocity.y < -0.35 {
                        -35.0
                    } else {
                        -12.0
                    };
                steer(bot, yaw, pitch, 12.0);
                if age >= 10 {
                    crate::aerial::release_use(bot);
                    self.charging = false;
                    self.phase(Phase::Turn, tick, username);
                }
            }
            Phase::Turn => {
                // Complete the turn before reacquiring: chasing a continuously
                // rotating bearing at speed can settle into an endless orbit.
                let yaw = *self
                    .turn_yaw
                    .get_or_insert_with(|| angles(bot.eye_position(), target.aim).0);
                let pitch = if p.y < target.position.y + 4.0 {
                    -18.0
                } else {
                    0.0
                };
                steer(bot, yaw, pitch, 18.0);
                if tick.saturating_sub(self.last_rocket) > 28 {
                    self.rocket(bot, tick, username)?;
                }
                let d = direction(bot);
                let heading = f64::from(yaw).to_radians();
                let alignment = (-heading.sin() * d.x + heading.cos() * d.z)
                    / d.horizontal_distance_squared().sqrt().max(0.01);
                if age >= 8 && alignment > 0.94 {
                    self.pass_heading = d;
                    self.pass_target = target.position;
                    self.phase(Phase::Charge, tick, username);
                }
            }
            _ => self.phase(Phase::Takeoff, tick, username),
        }
        Ok(())
    }

    fn rocket(&mut self, bot: &Client, tick: u64, username: &str) -> Result<()> {
        if tick.saturating_sub(self.last_rocket) < 20 {
            return Ok(());
        }
        ensure!(
            has_offhand(bot, ItemKind::FireworkRocket),
            "aerial duel needs rockets in offhand"
        );
        if self.charging {
            crate::aerial::release_use(bot);
            self.charging = false;
        }
        if crate::aerial::use_item(bot, InteractionHand::OffHand) {
            self.last_rocket = tick;
            action(username, "rocket", tick);
        }
        Ok(())
    }
}

fn action(username: &str, action: &str, tick: u64) {
    println!(
        "{}",
        json!({"event":"duel_action","bot":username,"action":action,"tick":tick})
    );
}
fn array(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}
fn horizontal_distance(a: Vec3, b: Vec3) -> f64 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}
fn angles(from: Vec3, to: Vec3) -> (f32, f32) {
    let d = Vec3::new(to.x - from.x, to.y - from.y, to.z - from.z);
    (
        (-d.x).atan2(d.z).to_degrees() as f32,
        (-d.y)
            .atan2(d.horizontal_distance_squared().sqrt())
            .to_degrees() as f32,
    )
}
fn steer(bot: &Client, yaw: f32, pitch: f32, limit: f32) {
    let current = bot
        .get_component::<LookDirection>()
        .unwrap_or(LookDirection::new(yaw, pitch));
    bot.set_direction(
        math::turn_toward(current.y_rot(), yaw, limit),
        pitch.clamp(-80.0, 80.0),
    );
}
fn aim(bot: &Client, to: Vec3, limit: f32) {
    let (yaw, pitch) = angles(bot.eye_position(), to);
    steer(bot, yaw, pitch, limit);
}
fn direction(bot: &Client) -> Vec3 {
    let d = bot
        .get_component::<LookDirection>()
        .unwrap_or(LookDirection::new(0.0, 0.0));
    let yaw = f64::from(d.y_rot()).to_radians();
    let pitch = f64::from(d.x_rot()).to_radians();
    Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    )
}
fn select_item(bot: &Client, kind: ItemKind) -> Result<bool> {
    let selected = {
        let ecs = bot.ecs.lock();
        let Some(inv) = ecs.get::<azalea::entity::inventory::Inventory>(bot.entity) else {
            return Ok(false);
        };
        ensure!(
            inv.id == 0 && inv.menu().try_as_player().is_some(),
            "close the container before duel"
        );
        let range = inv.menu().hotbar_slots_range();
        let slot = inv.menu().slots()[range]
            .iter()
            .position(|item| item.kind() == kind && !item.is_empty());
        let Some(slot) = slot else {
            anyhow::bail!("duel needs {kind:?} in hotbar");
        };
        let slot = slot as u8;
        if inv.selected_hotbar_slot == slot
            && ecs
                .get::<azalea::inventory::LastSentSelectedHotbarSlot>(bot.entity)
                .is_some_and(|sent| sent.slot == slot)
        {
            return Ok(true);
        }
        slot
    };
    bot.set_selected_hotbar_slot(selected);
    Ok(false)
}
fn has_chest(bot: &Client, kind: ItemKind) -> bool {
    has_slot(bot, 6, kind)
}
fn has_offhand(bot: &Client, kind: ItemKind) -> bool {
    has_slot(bot, 45, kind)
}
fn has_slot(bot: &Client, slot: usize, kind: ItemKind) -> bool {
    let ecs = bot.ecs.lock();
    ecs.get::<azalea::entity::inventory::Inventory>(bot.entity)
        .and_then(|i| i.menu().slot(slot))
        .is_some_and(|item| item.kind() == kind && !item.is_empty())
}

#[cfg(test)]
#[path = "tactics_controller_tests.rs"]
mod controller_tests;
