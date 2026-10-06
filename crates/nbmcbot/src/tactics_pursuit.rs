use super::*;
use azalea::pathfinder::{
    PathfinderClientExt, PathfinderOpts, astar::PathfinderTimeout, goals::RadiusGoal,
};

#[derive(Default)]
pub(super) struct Pursuit {
    goal: Option<Vec3>,
    last_repath: u64,
    navigating: bool,
}

impl Duel {
    pub(super) fn pursue(
        &mut self,
        bot: &Client,
        tick: u64,
        username: &str,
        goal: Vec3,
    ) -> Result<()> {
        let Some(physics) = bot.get_component::<Physics>() else {
            return Ok(());
        };
        let p = bot.position();
        let distance = horizontal_distance(p, goal);
        let wings = has_chest(bot, ItemKind::Elytra)
            || (36..45).any(|slot| has_slot(bot, slot, ItemKind::Elytra));
        let rockets = has_offhand(bot, ItemKind::FireworkRocket);
        bot.set_jumping(false);
        if physics.on_ground() && distance <= 4.0 && goal.y <= p.y + 4.0 {
            self.stop_pursuit(bot, true);
            bot.walk(WalkDirection::None);
            let yaw = bot
                .get_component::<LookDirection>()
                .map(|d| d.y_rot())
                .unwrap_or(0.0);
            steer(bot, yaw + 5.0, 0.0, 5.0);
            return Ok(());
        }
        if !physics.on_ground() || (wings && rockets && (distance > 24.0 || goal.y > p.y + 4.0)) {
            self.stop_pursuit(bot, true);
            self.pursuit.goal = Some(goal);
            bot.walk(WalkDirection::None);
            if !wings || !equipment::equip_chest(bot, ItemKind::Elytra)? {
                return Ok(());
            }
            let (yaw, _) = angles(p, goal);
            if physics.on_ground() {
                steer(bot, yaw, -25.0, 25.0);
                bot.sprint(SprintDirection::Forward);
                bot.set_jumping(true);
                return Ok(());
            }
            if physics.velocity.y < 0.05 {
                crate::aerial::start_gliding(bot);
            }
            let gliding = bot.get_component::<FallFlying>().is_some_and(|f| f.0);
            if !gliding {
                return Ok(());
            }
            let heading = direction(bot);
            let flight_direction = bot.get_component::<LookDirection>().unwrap_or_default();
            let bearing = f64::from(yaw).to_radians();
            let alignment = (-bearing.sin() * heading.x + bearing.cos() * heading.z)
                / heading.horizontal_distance_squared().sqrt().max(0.01);
            let pitch = if alignment < 0.5 && physics.velocity.horizontal_distance_squared() > 1.0 {
                -20.0
            } else if p.y > goal.y + 8.0 {
                15.0
            } else if goal.y > p.y + 8.0 && rockets {
                -25.0
            } else {
                -5.0
            };
            steer(bot, yaw, pitch, 25.0);
            let boost = rockets
                && alignment > 0.85
                && (distance > 24.0 || goal.y > p.y + 8.0)
                && tick.saturating_sub(self.last_rocket) >= 40
                && (physics.velocity.horizontal_distance_squared() < 1.0 || goal.y > p.y + 8.0);
            if self.avoid_flight_collision(
                bot,
                &physics,
                tick,
                username,
                flight_safety::FlightIntent::new(boost, flight_direction),
            ) {
                return Ok(());
            }
            if boost {
                self.rocket(bot, tick, username)?;
            }
            return Ok(());
        }
        if !self.pursuit.navigating
            || (tick.saturating_sub(self.pursuit.last_repath) >= 10
                && (self
                    .pursuit
                    .goal
                    .is_none_or(|old| old.distance_squared_to(goal) > 4.0)
                    || bot.is_goto_target_reached()))
        {
            crate::game::stop_movement(bot, true);
            bot.start_goto_with_opts(
                RadiusGoal {
                    pos: Vec3::new(goal.x, p.y, goal.z),
                    radius: 3.0,
                },
                PathfinderOpts::new()
                    .min_timeout(PathfinderTimeout::Nodes(500))
                    .max_timeout(PathfinderTimeout::Nodes(5_000))
                    .retry_on_no_path(false),
            );
            self.pursuit.goal = Some(goal);
            self.pursuit.last_repath = tick;
            self.pursuit.navigating = true;
        }
        Ok(())
    }

    pub(super) fn stop_pursuit(&mut self, bot: &Client, send_packets: bool) {
        if self.pursuit.navigating {
            crate::game::stop_movement(bot, send_packets);
        }
        self.pursuit = Pursuit::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tactics::controller_tests::client;
    use azalea::ecs::message::Messages;
    use azalea::pathfinder::GotoEvent;

    #[test]
    fn overhead_target_gets_a_throttled_climb_boost() {
        use crate::tactics::controller_tests::{hand_use_count, packet_tick};
        let bot = client();
        bot.set_direction(0.0, -25.0);
        let mut duel = Duel::new("spear".into(), "Enemy".into());
        for tick in 100..120 {
            duel.pursue(&bot, tick, "Test", Vec3::new(100.0, 120.0, 100.0))
                .unwrap();
            packet_tick(&bot);
        }
        assert_eq!(hand_use_count(&bot, InteractionHand::OffHand), 1);
    }

    #[test]
    fn losing_sight_steers_flight_back_instead_of_coasting_away() {
        let bot = client();
        bot.set_direction(-90.0, 0.0);
        let mut duel = Duel::new("spear".into(), "Enemy".into());
        for tick in 100..110 {
            duel.pursue(&bot, tick, "Test", Vec3::new(20.0, 70.0, 100.0))
                .unwrap();
        }
        assert!(
            direction(&bot).x < -0.8,
            "a lost enemy behind us must reverse the heading"
        );
        assert!(
            bot.ecs
                .lock()
                .get::<azalea::attack::AttackQueued>(bot.entity)
                .is_none()
        );
    }

    #[test]
    fn grounded_pursuit_discards_the_old_route_when_its_destination_changes() {
        let bot = client();
        bot.ecs
            .lock()
            .get_mut::<Physics>(bot.entity)
            .unwrap()
            .set_on_ground(true);
        let mut duel = Duel::new("mace".into(), "Enemy".into());
        let active = || {
            let ecs = bot.ecs.lock();
            let queue = ecs.resource::<Messages<GotoEvent>>();
            queue
                .get_cursor()
                .read(queue)
                .filter(|event| event.entity == bot.entity)
                .count()
        };
        duel.pursue(&bot, 100, "Test", Vec3::new(112.0, 70.0, 100.0))
            .unwrap();
        assert_eq!(active(), 1);
        duel.pursue(&bot, 120, "Test", Vec3::new(100.0, 70.0, 112.0))
            .unwrap();
        assert_eq!(active(), 1, "obsolete queued route must be canceled");
        duel.stop_pursuit(&bot, true);
        assert_eq!(active(), 0);
    }

    #[test]
    fn reaching_the_ground_search_point_stops_running_and_jumping() {
        let bot = client();
        bot.ecs
            .lock()
            .get_mut::<Physics>(bot.entity)
            .unwrap()
            .set_on_ground(true);
        bot.set_jumping(true);
        let mut duel = Duel::new("spear".into(), "Enemy".into());
        duel.pursue(&bot, 100, "Test", bot.position()).unwrap();
        assert!(
            !bot.ecs
                .lock()
                .get::<azalea::entity::Jumping>(bot.entity)
                .unwrap()
                .0
        );
        assert!(bot.ecs.lock().resource::<Messages<GotoEvent>>().is_empty());
    }
}
