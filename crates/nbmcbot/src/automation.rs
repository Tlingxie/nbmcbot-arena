use std::fmt;

#[derive(Debug, Clone)]
pub struct Observation {
    pub tick: u64,
    pub health: f32,
    pub food: u32,
    pub connected: bool,
    pub busy: bool,
    pub selected_slot: u8,
    pub food_slot: Option<u8>,
    pub offhand_totem: bool,
    pub totem_slot: Option<u16>,
    pub dead: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AutomationAction {
    Respawn,
    Look { yaw: f32, pitch: f32 },
    SelectHotbar(u8),
    UseMainHand,
    ReleaseUse,
    SwapOffhand { slot: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleError(pub String);
impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ModuleError {}

const NAMES: [&str; 4] = ["anti-idle", "auto-respawn", "auto-eat", "auto-totem"];
#[derive(Debug, Default)]
pub struct Modules {
    enabled: [bool; 4],
    respawn_sent: bool,
    idle_at: Option<u64>,
    idle_negative: bool,
    eating: Option<(u64, u8)>,
    eat_after: u64,
    totem_attempt: Option<(u16, u64, u8)>,
    last_tick: Option<u64>,
}

impl Modules {
    pub fn set(&mut self, name: &str, enabled: bool) -> Result<(), ModuleError> {
        let index = NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .ok_or_else(|| {
                ModuleError(format!(
                    "unknown module {name}; available: {}",
                    NAMES.join(", ")
                ))
            })?;
        self.enabled[index] = enabled;
        Ok(())
    }
    pub fn list(&self) -> Vec<(&'static str, bool)> {
        NAMES.into_iter().zip(self.enabled).collect()
    }
    pub fn is_eating(&self) -> bool {
        self.eating.is_some()
    }
    /// Releases temporary item-use ownership before another task starts.
    pub fn cancel_active(&mut self) -> Vec<AutomationAction> {
        match self.eating.take() {
            Some((_, previous)) => {
                self.eat_after = self.last_tick.unwrap_or(0).saturating_add(20);
                vec![
                    AutomationAction::ReleaseUse,
                    AutomationAction::SelectHotbar(previous),
                ]
            }
            None => Vec::new(),
        }
    }
    /// Clears observations and pending ownership while preserving module settings.
    pub fn reset_world(&mut self) {
        self.respawn_sent = false;
        self.idle_at = None;
        self.idle_negative = false;
        self.eating = None;
        self.eat_after = 0;
        self.totem_attempt = None;
        self.last_tick = None;
    }
    pub(crate) fn respawn_action(&mut self, dead: bool) -> Option<AutomationAction> {
        if !dead {
            self.respawn_sent = false;
        } else if self.enabled[1] && !self.respawn_sent {
            self.respawn_sent = true;
            return Some(AutomationAction::Respawn);
        }
        None
    }
    pub fn tick(&mut self, obs: &Observation) -> Vec<AutomationAction> {
        if !obs.connected {
            self.reset_world();
            return Vec::new();
        }
        if self.last_tick.is_some_and(|last| obs.tick < last) {
            self.reset_world();
        }
        self.last_tick = Some(obs.tick);
        let mut actions = Vec::new();
        let respawn = self.respawn_action(obs.dead);
        let idle_at = self.idle_at.get_or_insert(obs.tick);
        if let Some((started, previous)) = self.eating
            && (!self.enabled[2]
                || obs.busy
                || obs.dead
                || obs.food >= 20
                || obs.tick.saturating_sub(started) >= 40)
        {
            self.eating = None;
            self.eat_after = obs.tick.saturating_add(20);
            actions.push(AutomationAction::ReleaseUse);
            actions.push(AutomationAction::SelectHotbar(previous));
        }
        if obs.dead {
            actions.extend(respawn);
            return actions;
        }
        if self.enabled[2]
            && self.eating.is_none()
            && obs.tick >= self.eat_after
            && obs.food <= 16
            && !obs.busy
            && obs.selected_slot < 9
            && let Some(slot) = obs.food_slot.filter(|slot| *slot < 9)
        {
            self.eating = Some((obs.tick, obs.selected_slot));
            if slot != obs.selected_slot {
                actions.push(AutomationAction::SelectHotbar(slot));
            }
            actions.push(AutomationAction::UseMainHand);
        }
        if self.enabled[3] && self.eating.is_none() && !obs.busy {
            if obs.offhand_totem {
                self.totem_attempt = None;
            } else if let Some(slot) = obs.totem_slot.filter(|slot| (9..=44).contains(slot)) {
                let (last, attempts) = match self.totem_attempt {
                    Some((previous, last, attempts)) if previous == slot => (Some(last), attempts),
                    _ => (None, 0),
                };
                if attempts < 3 && last.is_none_or(|last| obs.tick.saturating_sub(last) >= 100) {
                    actions.push(AutomationAction::SwapOffhand { slot });
                    self.totem_attempt = Some((slot, obs.tick, attempts + 1));
                }
            } else {
                self.totem_attempt = None;
            }
        }
        if self.enabled[0]
            && !obs.busy
            && self.eating.is_none()
            && obs.tick.saturating_sub(*idle_at) >= 1200
        {
            actions.push(AutomationAction::Look {
                yaw: if self.idle_negative { -15.0 } else { 15.0 },
                pitch: 0.0,
            });
            self.idle_negative = !self.idle_negative;
            *idle_at = obs.tick;
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_releases_ownership_once() {
        let mut modules = Modules::default();
        modules.set("auto-eat", true).unwrap();
        modules.tick(&observation(0));
        assert!(modules.is_eating());
        assert_eq!(
            modules.cancel_active(),
            vec![
                AutomationAction::ReleaseUse,
                AutomationAction::SelectHotbar(2)
            ]
        );
        assert!(!modules.is_eating());
        assert!(modules.cancel_active().is_empty());
        assert!(modules.tick(&observation(1)).is_empty());
        assert!(
            modules
                .list()
                .iter()
                .any(|(name, enabled)| *name == "auto-eat" && *enabled)
        );
    }
    #[test]
    fn busy_interrupts_eating() {
        let mut modules = Modules::default();
        modules.set("auto-eat", true).unwrap();
        modules.tick(&observation(0));
        let mut obs = observation(1);
        obs.busy = true;
        assert_eq!(
            modules.tick(&obs),
            vec![
                AutomationAction::ReleaseUse,
                AutomationAction::SelectHotbar(2)
            ]
        );
        assert!(!modules.is_eating());
    }
    #[test]
    fn anti_idle_changes_each_time() {
        let mut modules = Modules::default();
        modules.set("anti-idle", true).unwrap();
        modules.tick(&observation(0));
        assert_eq!(
            modules.tick(&observation(1200)),
            vec![AutomationAction::Look {
                yaw: 15.0,
                pitch: 0.0
            }]
        );
        assert_eq!(
            modules.tick(&observation(2400)),
            vec![AutomationAction::Look {
                yaw: -15.0,
                pitch: 0.0
            }]
        );
    }
    fn observation(tick: u64) -> Observation {
        Observation {
            tick,
            health: 20.0,
            food: 12,
            connected: true,
            busy: false,
            selected_slot: 2,
            food_slot: Some(4),
            offhand_totem: false,
            totem_slot: Some(10),
            dead: false,
        }
    }
    #[test]
    fn defaults_and_validation() {
        let mut modules = Modules::default();
        assert!(modules.list().iter().all(|(_, enabled)| !enabled));
        assert!(modules.set("unknown", true).is_err());
        assert!(modules.tick(&observation(0)).is_empty());
    }
    #[test]
    fn busy_invalid_slots_and_disabled_eating() {
        let mut modules = Modules::default();
        modules.set("auto-eat", true).unwrap();
        let mut obs = observation(0);
        obs.busy = true;
        assert!(modules.tick(&obs).is_empty());
        obs.busy = false;
        obs.food_slot = Some(9);
        assert!(modules.tick(&obs).is_empty());
        obs.food_slot = Some(4);
        assert_eq!(modules.tick(&obs).len(), 2);
        modules.set("auto-eat", false).unwrap();
        assert_eq!(
            modules.tick(&obs),
            vec![
                AutomationAction::ReleaseUse,
                AutomationAction::SelectHotbar(2)
            ]
        );
        assert!(modules.tick(&obs).is_empty());
    }
    #[test]
    fn totem_retries_stop_until_confirmation() {
        let mut modules = Modules::default();
        modules.set("auto-totem", true).unwrap();
        for tick in [0, 100, 200] {
            assert_eq!(modules.tick(&observation(tick)).len(), 1);
        }
        assert!(modules.tick(&observation(300)).is_empty());
        let mut obs = observation(301);
        obs.offhand_totem = true;
        assert!(modules.tick(&obs).is_empty());
        obs.offhand_totem = false;
        assert_eq!(modules.tick(&obs).len(), 1);
    }
    #[test]
    fn full_food_ends_use_early_and_reset_preserves_settings() {
        let mut modules = Modules::default();
        modules.set("auto-eat", true).unwrap();
        modules.tick(&observation(5));
        let mut obs = observation(6);
        obs.food = 20;
        assert_eq!(
            modules.tick(&obs),
            vec![
                AutomationAction::ReleaseUse,
                AutomationAction::SelectHotbar(2)
            ]
        );
        modules.reset_world();
        assert_eq!(modules.tick(&observation(0)).len(), 2);
    }
    #[test]
    fn eating_releases_and_restores_once() {
        let mut modules = Modules::default();
        modules.set("auto-eat", true).unwrap();
        assert_eq!(
            modules.tick(&observation(0)),
            vec![
                AutomationAction::SelectHotbar(4),
                AutomationAction::UseMainHand
            ]
        );
        assert!(modules.tick(&observation(1)).is_empty());
        assert_eq!(
            modules.tick(&observation(40)),
            vec![
                AutomationAction::ReleaseUse,
                AutomationAction::SelectHotbar(2)
            ]
        );
        assert!(modules.tick(&observation(41)).is_empty());
    }
    #[test]
    fn respawn_once_and_disconnect_clears_state() {
        let mut modules = Modules::default();
        modules.set("auto-respawn", true).unwrap();
        let mut obs = observation(0);
        obs.dead = true;
        assert_eq!(modules.tick(&obs), vec![AutomationAction::Respawn]);
        assert!(modules.tick(&obs).is_empty());
        obs.connected = false;
        assert!(modules.tick(&obs).is_empty());
        obs.connected = true;
        assert_eq!(modules.tick(&obs), vec![AutomationAction::Respawn]);
    }
    #[test]
    fn idle_and_totem_are_throttled() {
        let mut modules = Modules::default();
        modules.set("anti-idle", true).unwrap();
        modules.set("auto-totem", true).unwrap();
        assert_eq!(
            modules.tick(&observation(0)),
            vec![AutomationAction::SwapOffhand { slot: 10 }]
        );
        assert!(modules.tick(&observation(1)).is_empty());
        let mut obs = observation(1200);
        obs.offhand_totem = true;
        assert_eq!(
            modules.tick(&obs),
            vec![AutomationAction::Look {
                yaw: 15.0,
                pitch: 0.0
            }]
        );
    }
}
