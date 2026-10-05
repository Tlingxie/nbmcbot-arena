use super::*;

impl Duel {
    pub(super) fn mace_recover(&self, bot: &Client, physics: &Physics) -> Result<()> {
        if self.style != "mace"
            || self.phase != Phase::Recover
            || self.mace_ground_y.is_none()
            || physics.on_ground()
            || (!has_chest(bot, ItemKind::Elytra)
                && !(36..45).any(|slot| has_slot(bot, slot, ItemKind::Elytra)))
        {
            return Ok(());
        }
        bot.walk(WalkDirection::None);
        bot.set_jumping(false);
        let yaw = bot
            .get_component::<LookDirection>()
            .map(|look| look.y_rot())
            .unwrap_or(0.0);
        steer(bot, yaw, 15.0, 25.0);
        if equipment::equip_chest(bot, ItemKind::Elytra)? {
            crate::aerial::start_gliding(bot);
        }
        Ok(())
    }

    pub(super) fn mace_flight(
        &mut self,
        bot: &Client,
        tick: u64,
        username: &str,
        target: &Target,
        velocity: Vec3,
        physics: &Physics,
    ) -> Result<()> {
        let p = bot.position();
        let (target_position, velocity) = if self.motion_sample.is_some() {
            self.predicted_motion(target.position, tick)
        } else {
            (target.position, velocity)
        };
        if self.mace_ground_y.is_none() || physics.on_ground() {
            self.mace_ground_y = Some(p.y);
        }
        let ground_y = self.mace_ground_y.unwrap_or(p.y);
        let gliding = bot.get_component::<FallFlying>().is_some_and(|f| f.0);
        let equipped = select_item(bot, ItemKind::Mace)?;
        if equipped {
            self.mace_equipped_at.get_or_insert(tick);
        } else {
            self.mace_equipped_at = None;
        }
        let age = tick.saturating_sub(self.since);
        bot.set_jumping(false);
        if self.phase == Phase::Approach {
            self.launch_y = p.y;
            self.phase(Phase::MaceTakeoff, tick, username);
        }
        match self.phase {
            Phase::MaceTakeoff => {
                if !equipment::equip_chest(bot, ItemKind::Elytra)? {
                    return Ok(());
                }
                let (yaw, _) = angles(bot.eye_position(), target.aim);
                steer(bot, yaw, -55.0, 20.0);
                bot.sprint(SprintDirection::Forward);
                bot.set_jumping(physics.on_ground());
                if !physics.on_ground() && physics.velocity.y < 0.05 {
                    crate::aerial::start_gliding(bot);
                }
                if gliding && !physics.on_ground() {
                    bot.walk(WalkDirection::None);
                    bot.set_jumping(false);
                    self.rocket(bot, tick, username)?;
                    self.phase(Phase::MaceClimb, tick, username);
                }
            }
            Phase::MaceClimb => {
                if physics.on_ground() || !gliding {
                    self.phase(Phase::MaceTakeoff, tick, username);
                    return Ok(());
                }
                bot.walk(WalkDirection::None);
                let lead = math::intercept_turning(
                    array(target_position),
                    array(velocity),
                    6.0,
                    self.recent_turn,
                );
                let (yaw, _) = angles(p, Vec3::new(lead[0], lead[1], lead[2]));
                let cooled_down = self
                    .mace_equipped_at
                    .is_some_and(|at| tick.saturating_sub(at) >= 34);
                let height = *self.mace_climb_height.get_or_insert_with(|| {
                    let rise = (velocity.y.clamp(0.0, 1.7) * 20.0).min(24.0);
                    let height = (p.y + 8.0).max(target.position.y + 18.0 + rise);
                    let height = if cooled_down {
                        height
                    } else {
                        height.max(ground_y + 32.0)
                    };
                    height.min(p.y + 48.0)
                });
                steer(bot, yaw, if p.y < height { -65.0 } else { 0.0 }, 15.0);
                if p.y < height && tick.saturating_sub(self.last_rocket) >= 30 {
                    self.rocket(bot, tick, username)?;
                }
                if p.y >= height && cooled_down {
                    self.phase(Phase::MaceDive, tick, username);
                }
            }
            Phase::MaceDive => {
                if physics.on_ground() || !gliding {
                    self.phase(Phase::MaceTakeoff, tick, username);
                    return Ok(());
                }
                if age >= 12 && target.position.y > p.y + 6.0 && p.y < ground_y + 16.0 {
                    self.phase(Phase::MaceClimb, tick, username);
                    return Ok(());
                }
                let time = (horizontal_distance(p, target.position)
                    / physics
                        .velocity
                        .horizontal_distance_squared()
                        .sqrt()
                        .max(0.6))
                .clamp(2.0, 6.0);
                let lead = math::intercept_turning(
                    array(target_position),
                    array(velocity),
                    time,
                    self.recent_turn,
                );
                aim(
                    bot,
                    Vec3::new(
                        lead[0],
                        (lead[1] + 0.8)
                            .min(self.mace_climb_height.unwrap_or(p.y) - 4.0)
                            .min(p.y - 4.0),
                        lead[2],
                    ),
                    25.0,
                );
                let candidate = math::drop_intercept_turning(
                    array(p),
                    array(physics.velocity),
                    array(target_position),
                    array(velocity),
                    self.recent_turn,
                );
                if tick.is_multiple_of(5) && std::env::var_os("NBMCBOT_TRACE_DUEL").is_some() {
                    println!(
                        "{}",
                        json!({"event":"duel_drop_candidate","bot":username,"candidate":candidate,
                        "position":array(p),"velocity":array(physics.velocity),"target":array(target.position),
                        "estimated_target":array(target_position),"target_velocity":array(velocity),
                        "target_turn_rate":self.recent_turn,"velocity_ready":self.velocity_ready,"tick":tick})
                    );
                }
                if let Some((point, ticks, miss)) = candidate
                    && (5.0..=14.0).contains(&ticks)
                    && self.velocity_ready
                    && miss <= 2.4
                    && p.y - point[1] >= 2.5
                {
                    crate::aerial::release_use(bot);
                    self.phase(Phase::MaceSwap, tick, username);
                    equipment::equip_chest(bot, ItemKind::NetheriteChestplate)?;
                    println!(
                        "{}",
                        json!({"event":"duel_intercept","bot":username,"player":target.name,
                            "position":array(p),"target":array(target.position),"estimated_target":array(target_position),"target_velocity":array(velocity),
                            "target_turn_rate":self.recent_turn,"predicted_target":point,"time_ticks":ticks,"horizontal_miss":miss,"tick":tick})
                    );
                    action(username, "chestplate_swap", tick);
                    return Ok(());
                }
                if age >= 2
                    && horizontal_distance(p, target.position) > 6.0
                    && physics.velocity.horizontal_distance_squared() < 0.64
                {
                    self.rocket(bot, tick, username)?;
                }
                if p.y < ground_y + 2.0 || age > 80 {
                    self.phase(Phase::MaceClimb, tick, username);
                }
            }
            Phase::MaceSwap => {
                bot.walk(WalkDirection::None);
                // Inventory clicks predict the slots locally. The server's
                // FallFlying=false metadata confirms that glide has ended.
                if has_chest(bot, ItemKind::NetheriteChestplate) && !gliding {
                    self.peak_y = p.y;
                    self.phase(Phase::MaceDrop, tick, username);
                    action(username, "armored_drop", tick);
                } else if age > 10 {
                    self.phase(Phase::MaceTakeoff, tick, username);
                }
            }
            Phase::MaceDrop => {
                self.peak_y = self.peak_y.max(p.y);
                let lead = math::intercept_turning(
                    array(target_position),
                    array(velocity),
                    2.0,
                    self.recent_turn,
                );
                aim(bot, Vec3::new(lead[0], lead[1] + 0.8, lead[2]), 45.0);
                bot.walk(if horizontal_distance(p, target.position) > 0.8 {
                    WalkDirection::Forward
                } else {
                    WalkDirection::None
                });
                if equipped
                    && has_chest(bot, ItemKind::NetheriteChestplate)
                    && !gliding
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
                        json!({"event":"duel_attack","style":"mace","technique":"elytra_chestplate",
                        "bot":username,"player":target.name,"fall_distance":self.peak_y-p.y,"tick":tick})
                    );
                    self.phase(Phase::Recover, tick, username);
                } else if physics.on_ground() || age > 45 {
                    self.phase(Phase::Recover, tick, username);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
