use super::*;
use azalea::{
    core::{aabb::Aabb, position::ChunkPos},
    entity::dimensions::EntityDimensions,
    local_player::InstanceHolder,
    physics::{collision::world_collisions::get_block_collisions, travel::fall_flying_velocity},
    world::Instance,
};

const LOOKAHEAD: u8 = 12;
const TURN_RATE: f32 = 90.0;

#[derive(Default)]
pub(super) struct FlightSafety {
    escape: Option<LookDirection>,
    until: u64,
    last_choice: u64,
    clear_ticks: u8,
}

struct Forecast {
    clear_steps: u8,
    reason: &'static str,
}

pub(super) struct FlightIntent {
    boost: bool,
    previous_direction: LookDirection,
    pass: Option<Pass>,
}

impl FlightIntent {
    pub(super) fn new(boost: bool, previous_direction: LookDirection) -> Self {
        Self {
            boost,
            previous_direction,
            pass: None,
        }
    }

    pub(super) fn with_pass(
        mut self,
        duel: &Duel,
        target: &Target,
        velocity: Vec3,
        age: u64,
    ) -> Self {
        self.pass = Some(Pass {
            target: target.position,
            velocity,
            age,
            committed: duel.pass_committed,
            point: duel.pass_target,
            heading: duel.pass_heading,
        });
        self
    }
}

#[derive(Clone, Copy)]
struct Pass {
    target: Vec3,
    velocity: Vec3,
    age: u64,
    committed: bool,
    point: Vec3,
    heading: Vec3,
}

struct Flight<'a> {
    world: &'a Instance,
    bounds: Aabb,
    velocity: Vec3,
    direction: LookDirection,
    pass: Option<Pass>,
}

impl Flight<'_> {
    fn with_existing_boost(&self, goal: LookDirection, boost_ticks: u8) -> Forecast {
        let unboosted = self.forecast(goal, 0);
        if boost_ticks == 0 {
            return unboosted;
        }
        let boosted = self.forecast(goal, boost_ticks);
        if boosted.clear_steps < unboosted.clear_steps {
            boosted
        } else {
            unboosted
        }
    }

    fn forecast(&self, mut goal: LookDirection, boost_ticks: u8) -> Forecast {
        let mut bounds = self.bounds;
        let mut velocity = self.velocity;
        let mut direction = self.direction;
        let mut pass = self.pass;
        let mut exit_at = None;
        for step in 0..LOOKAHEAD {
            let position = Vec3::new(
                (bounds.min.x + bounds.max.x) * 0.5,
                bounds.min.y,
                (bounds.min.z + bounds.max.z) * 0.5,
            );
            if let (Some(pass), Some(start)) = (pass, exit_at) {
                let pitch = if step - start + 1 < 4
                    || position.y < pass.target.y + pass.velocity.y * f64::from(step) + 1.2
                    || velocity.y < -0.35
                {
                    -35.0
                } else {
                    -12.0
                };
                goal = LookDirection::new(
                    (-pass.heading.x).atan2(pass.heading.z).to_degrees() as f32,
                    pitch,
                );
            }
            direction = LookDirection::new(
                math::turn_toward(
                    direction.y_rot(),
                    goal.y_rot(),
                    if exit_at.is_some() { 12.0 } else { TURN_RATE },
                ),
                goal.x_rot(),
            );
            if exit_at.is_none()
                && let Some(pass) = &mut pass
            {
                let target = pass.target + pass.velocity * f64::from(step);
                let distance = position.distance_squared_to(target);
                if distance < 36.0 && !pass.committed {
                    pass.committed = true;
                    pass.point = target;
                    pass.heading = azalea::entity::view_vector(direction);
                }
                if (pass.committed
                    && pass.age + u64::from(step) > 10
                    && math::passed_target(array(position), array(pass.point), array(pass.heading)))
                    || distance < 9.0
                    || pass.age + u64::from(step) > 100
                {
                    // Charge changes phase after choosing this tick's input.
                    // The first pullout movement is on the following tick.
                    exit_at = Some(step + 1);
                }
            }
            velocity = fall_flying_velocity(velocity, direction, 0.08);
            let swept = bounds.expand_towards(velocity).inflate_all(0.15);
            // Bound the collision scanner even after an unusual knockback packet.
            let size = swept.max - swept.min;
            if (size.x + 3.0) * (size.y + 3.0) * (size.z + 3.0) > 512.0 {
                return Forecast {
                    clear_steps: step,
                    reason: "scan_limit",
                };
            }
            if !loaded(self.world, &swept) {
                return Forecast {
                    clear_steps: step,
                    reason: "unknown",
                };
            }
            if !get_block_collisions(self.world, &swept).is_empty() {
                return Forecast {
                    clear_steps: step,
                    reason: "terrain",
                };
            }
            bounds = bounds.move_relative(velocity);
            // Match the existing legal rocket's post-physics acceleration. A
            // requested new rocket is forecast too, before allowing its use.
            if step < boost_ticks {
                let look = azalea::entity::view_vector(direction);
                velocity += look * 0.1 + (look * 1.5 - velocity) * 0.5;
            }
        }
        Forecast {
            clear_steps: LOOKAHEAD,
            reason: "clear",
        }
    }
}

fn loaded(world: &Instance, bounds: &Aabb) -> bool {
    if bounds.min.y >= f64::from(world.chunks.min_y) + f64::from(world.chunks.height) + 1.0 {
        return true;
    }
    // Include the collision-shape iterator's one-block border. Missing chunks
    // must not inherit its default-air behavior.
    let min_x = ((bounds.min.x.floor() as i32) - 1).div_euclid(16);
    let max_x = ((bounds.max.x.floor() as i32) + 1).div_euclid(16);
    let min_z = ((bounds.min.z.floor() as i32) - 1).div_euclid(16);
    let max_z = ((bounds.max.z.floor() as i32) + 1).div_euclid(16);
    (min_x..=max_x)
        .all(|x| (min_z..=max_z).all(|z| world.chunks.get(&ChunkPos::new(x, z)).is_some()))
}

impl Duel {
    pub(super) fn avoid_flight_collision(
        &mut self,
        bot: &Client,
        physics: &Physics,
        tick: u64,
        username: &str,
        intent: FlightIntent,
    ) -> bool {
        if physics.on_ground() {
            self.flight_safety = FlightSafety::default();
            return false;
        }
        let Some(holder) = bot.get_component::<InstanceHolder>() else {
            crate::aerial::release_use(bot);
            self.charging = false;
            return true;
        };
        let direction = bot.get_component::<LookDirection>().unwrap_or_default();
        let position = bot.position();
        let dimensions = bot
            .get_component::<EntityDimensions>()
            .unwrap_or_else(|| EntityDimensions::new(0.6, 1.8));
        let world = holder.instance.read();
        let flight = Flight {
            world: &world,
            bounds: dimensions.make_bounding_box(position),
            velocity: physics.velocity,
            direction: intent.previous_direction,
            pass: None,
        };
        let active_boost = if self.last_rocket != 0 {
            30_u64
                .saturating_sub(tick.saturating_sub(self.last_rocket))
                .min(u64::from(LOOKAHEAD)) as u8
        } else {
            0
        };
        let planned_flight = Flight {
            direction,
            pass: intent.pass,
            ..flight
        };
        let planned = if intent.boost && tick.saturating_sub(self.last_rocket) >= 20 {
            planned_flight.forecast(direction, LOOKAHEAD)
        } else {
            // A rocket may expire during prediction. Neither its lift nor its
            // acceleration is safe to assume for the entire remaining window.
            planned_flight.with_existing_boost(direction, active_boost)
        };
        let was_avoiding = self.flight_safety.escape.is_some();
        if planned.clear_steps == LOOKAHEAD {
            self.flight_safety.clear_ticks = self.flight_safety.clear_ticks.saturating_add(1);
            if !was_avoiding
                || (tick >= self.flight_safety.until && self.flight_safety.clear_ticks >= 3)
            {
                self.flight_safety = FlightSafety::default();
                return false;
            }
        } else {
            self.flight_safety.clear_ticks = 0;
            self.flight_safety.until = tick + 8;
        }
        let old_forecast = self
            .flight_safety
            .escape
            .map(|escape| flight.with_existing_boost(escape, active_boost));
        if !was_avoiding
            || tick.saturating_sub(self.flight_safety.last_choice) >= 4
            || old_forecast.is_some_and(|forecast| forecast.clear_steps <= 3)
        {
            let heading = if physics.velocity.horizontal_distance_squared() > 0.04 {
                (-physics.velocity.x).atan2(physics.velocity.z).to_degrees() as f32
            } else {
                direction.y_rot()
            };
            let mut best = LookDirection::new(heading, -35.0);
            let mut best_clear = 0;
            for offset in [0.0, 60.0, -60.0, 120.0, -120.0, 180.0] {
                let candidate = LookDirection::new(heading + offset, -35.0);
                let forecast = flight.with_existing_boost(candidate, active_boost);
                if forecast.clear_steps > best_clear {
                    best = candidate;
                    best_clear = forecast.clear_steps;
                }
                if best_clear == LOOKAHEAD {
                    break;
                }
            }
            self.flight_safety.escape = Some(best);
            self.flight_safety.last_choice = tick;
        }
        let escape = self.flight_safety.escape.unwrap();
        drop(world);
        // Override from the actual previous tick, not the proposed target aim:
        // that aim must not pull against the escape turn every tick.
        bot.set_direction(
            math::turn_toward(intent.previous_direction.y_rot(), escape.y_rot(), TURN_RATE),
            escape.x_rot(),
        );
        crate::aerial::release_use(bot);
        self.charging = false;
        self.pass_committed = false;
        if !was_avoiding || tick.is_multiple_of(40) {
            println!(
                "{}",
                json!({
                    "event":"duel_flight_avoidance", "bot":username, "tick":tick,
                    "reason":planned.reason, "clear_ticks":planned.clear_steps,
                    "position":array(bot.position()), "velocity":array(physics.velocity),
                    "yaw":escape.y_rot(), "pitch":escape.x_rot()
                })
            );
        }
        true
    }
}
