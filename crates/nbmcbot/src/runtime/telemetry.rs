use std::{
    future::pending,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::{
        Arc,
        mpsc::{SyncSender, sync_channel},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use azalea::{
    Vec3,
    connection::RawConnection,
    entity::{
        Dead, LookDirection, Physics, Position,
        inventory::Inventory,
        metadata::{FallFlying, Health},
    },
    world::InstanceName,
};
use parking_lot::Mutex;
use serde::Serialize;

use super::{Session, Task};

const MAX_PACKET_BYTES: usize = 60_000;
const FRAME_OVERHEAD: usize = 256;

#[derive(Clone, Debug, Default, Serialize)]
struct EquipmentSnapshot {
    mainhand: Option<String>,
    chest: Option<String>,
    offhand: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PlayerSnapshot {
    name: String,
    uuid: String,
    source: &'static str,
    position: Option<[f64; 3]>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    velocity: Option<[f64; 3]>,
    health: Option<f32>,
    alive: Option<bool>,
    connected: bool,
    dimension: Option<String>,
    task: Option<String>,
    style: Option<String>,
    phase: Option<String>,
    target: Option<String>,
    gliding: Option<bool>,
    equipment: EquipmentSnapshot,
}

struct Sample {
    sampled_at_ms: u64,
    players: Vec<PlayerSnapshot>,
}

pub(super) struct Telemetry {
    sender: SyncSender<Sample>,
    interval: tokio::time::Interval,
}

impl Telemetry {
    pub(super) fn from_env() -> Result<Option<Self>> {
        let value = std::env::var("NBMCBOT_TELEMETRY_ADDR").ok();
        parse_target(value.as_deref())?.map(Self::new).transpose()
    }

    fn new(target: SocketAddr) -> Result<Self> {
        ensure!(
            target.ip().is_loopback() && target.port() != 0,
            "telemetry target must be loopback with a nonzero port"
        );
        let bind = match target.ip() {
            IpAddr::V4(_) => SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            IpAddr::V6(_) => SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
        };
        let socket = UdpSocket::bind(bind).context("could not bind telemetry socket")?;
        socket.set_nonblocking(true)?;
        socket
            .connect(target)
            .context("could not set telemetry destination")?;
        let (sender, receiver) = sync_channel::<Sample>(1);
        let process_id = std::process::id();
        std::thread::Builder::new()
            .name("nbmcbot-telemetry".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut sequence = 0;
                while let Ok(sample) = receiver.recv() {
                    if let Ok(packets) = encode_frames(
                        &sample.players,
                        process_id,
                        sample.sampled_at_ms,
                        &mut sequence,
                    ) {
                        for packet in packets {
                            // An absent or slow observer must never become a bot failure.
                            let _ = socket.send(&packet);
                        }
                    }
                }
            })
            .context("could not start telemetry sender")?;
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Ok(Self { sender, interval })
    }

    pub(super) fn sample<'a>(&self, sessions: impl IntoIterator<Item = &'a Arc<Mutex<Session>>>) {
        let sampled_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let players = sessions
            .into_iter()
            .filter_map(|shared| {
                let session = shared.try_lock()?;
                snapshot_session(&session)
            })
            .collect::<Vec<_>>();
        if !players.is_empty() {
            // Keep at most one queued snapshot; drop frames instead of building backlog.
            let _ = self.sender.try_send(Sample {
                sampled_at_ms,
                players,
            });
        }
    }
}

pub(super) async fn wait_for_tick(telemetry: &mut Option<Telemetry>) {
    if let Some(telemetry) = telemetry {
        telemetry.interval.tick().await;
    } else {
        pending::<()>().await;
    }
}

fn parse_target(value: Option<&str>) -> Result<Option<SocketAddr>> {
    value
        .map(|value| {
            let target: SocketAddr = value
                .parse()
                .context("NBMCBOT_TELEMETRY_ADDR must be a numeric loopback IP and port")?;
            ensure!(
                target.ip().is_loopback() && target.port() != 0,
                "NBMCBOT_TELEMETRY_ADDR must use loopback and a nonzero port"
            );
            Ok(target)
        })
        .transpose()
}

fn snapshot_session(session: &Session) -> Option<PlayerSnapshot> {
    let uuid = session.account_uuid;
    let mut snapshot = PlayerSnapshot {
        name: session.account_name.clone(),
        uuid: format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            uuid >> 96,
            (uuid >> 80) & 0xffff,
            (uuid >> 64) & 0xffff,
            (uuid >> 48) & 0xffff,
            uuid & 0xffff_ffff_ffff
        ),
        source: "self",
        position: None,
        yaw: None,
        pitch: None,
        velocity: None,
        health: None,
        alive: None,
        connected: false,
        dimension: None,
        task: session.task_name().map(str::to_owned),
        style: None,
        phase: None,
        target: None,
        gliding: None,
        equipment: EquipmentSnapshot::default(),
    };
    if let Some(Task::Duel(duel)) = &session.task {
        let state = duel.telemetry();
        snapshot.style = Some(state.style.to_owned());
        snapshot.phase = Some(state.phase);
        snapshot.target = state.target.map(str::to_owned);
    }
    let Some(bot) = &session.bot else {
        return Some(snapshot);
    };
    let ecs = bot.ecs.try_lock()?;
    let dimension = ecs.get::<InstanceName>(bot.entity);
    snapshot.connected = !session
        .flags
        .paused
        .load(std::sync::atomic::Ordering::Acquire)
        && !session
            .flags
            .quit
            .load(std::sync::atomic::Ordering::Acquire)
        && dimension.is_some()
        && ecs
            .get::<RawConnection>(bot.entity)
            .is_some_and(RawConnection::is_alive);
    if !snapshot.connected {
        return Some(snapshot);
    }
    snapshot.dimension = dimension.map(|dimension| dimension.0.to_string());
    snapshot.position = ecs
        .get::<Position>(bot.entity)
        .and_then(|position| finite_vec(**position));
    if let Some(look) = ecs.get::<LookDirection>(bot.entity) {
        snapshot.yaw = finite(look.y_rot());
        snapshot.pitch = finite(look.x_rot());
    }
    snapshot.velocity = ecs
        .get::<Physics>(bot.entity)
        .and_then(|physics| finite_vec(physics.velocity));
    snapshot.health = ecs
        .get::<Health>(bot.entity)
        .and_then(|health| finite(health.0));
    snapshot.alive = if ecs.get::<Dead>(bot.entity).is_some() {
        Some(false)
    } else {
        snapshot.health.map(|health| health > 0.0)
    };
    snapshot.gliding = ecs.get::<FallFlying>(bot.entity).map(|flying| flying.0);
    if let Some(inventory) = ecs.get::<Inventory>(bot.entity) {
        let item = |slot| {
            inventory
                .inventory_menu
                .slot(slot)
                .map(|item| item.kind().to_string())
        };
        snapshot.equipment = EquipmentSnapshot {
            mainhand: item(36 + usize::from(inventory.selected_hotbar_slot)),
            chest: item(6),
            offhand: item(45),
        };
    }
    Some(snapshot)
}

fn finite(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

fn finite_vec(value: Vec3) -> Option<[f64; 3]> {
    let xyz = [value.x, value.y, value.z];
    xyz.iter().all(|value| value.is_finite()).then_some(xyz)
}

#[derive(Serialize)]
struct Frame<'a> {
    event: &'static str,
    version: u8,
    process_id: u32,
    sampled_at_ms: u64,
    sequence: u64,
    players: &'a [PlayerSnapshot],
}

fn encode_frames(
    players: &[PlayerSnapshot],
    process_id: u32,
    sampled_at_ms: u64,
    sequence: &mut u64,
) -> serde_json::Result<Vec<Vec<u8>>> {
    let mut packets = Vec::new();
    let mut start = 0;
    let mut size = FRAME_OVERHEAD;
    let mut flush = |players: &[PlayerSnapshot]| -> serde_json::Result<()> {
        if !players.is_empty() {
            let bytes = serde_json::to_vec(&Frame {
                event: "telemetry_frame",
                version: 1,
                process_id,
                sampled_at_ms,
                sequence: *sequence,
                players,
            })?;
            if bytes.len() < MAX_PACKET_BYTES {
                packets.push(bytes);
                *sequence = sequence.wrapping_add(1);
            }
        }
        Ok(())
    };
    for (index, player) in players.iter().enumerate() {
        let length = serde_json::to_vec(player)?.len() + 1;
        if size + length >= MAX_PACKET_BYTES {
            flush(&players[start..index])?;
            start = index;
            size = FRAME_OVERHEAD;
        }
        if FRAME_OVERHEAD + length >= MAX_PACKET_BYTES {
            start = index + 1;
        } else {
            size += length;
        }
    }
    flush(&players[start..])?;
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{Flags, Session, Task};
    use azalea::{
        Account, Client, Identifier, Vec3,
        connection::RawConnection,
        ecs::world::World,
        entity::{
            Dead, LookDirection, Physics, Position,
            inventory::Inventory,
            metadata::{FallFlying, Health},
        },
        inventory::ItemStack,
        protocol::packets::ConnectionProtocol,
        registry::builtin::ItemKind,
        world::InstanceName,
    };
    use nbmcbot_plugins::{PluginLimits, PluginRuntime};
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn session() -> Session {
        let account = Account::offline("Mace001");
        Session {
            config: crate::config::Config {
                username: account.username.clone(),
                ..Default::default()
            },
            account_name: account.username.clone(),
            account_uuid: account.uuid_or_offline().as_u128(),
            flags: Arc::new(Flags::default()),
            bot: None,
            ready: false,
            tick: 0,
            task: None,
            modules: crate::automation::Modules::default(),
            plugins: PluginRuntime::new(PluginLimits::default()).unwrap(),
            peak_rss: 0,
            rss: 0,
            last_plugin_error: None,
        }
    }

    fn attach_client(session: &mut Session, health: f32) {
        let mut ecs = World::new();
        let mut physics = Physics::default();
        physics.velocity = Vec3::new(1.0, 2.0, 3.0);
        let mut inventory = Inventory::default();
        for (slot, kind) in [
            (36, ItemKind::Mace),
            (6, ItemKind::Elytra),
            (45, ItemKind::FireworkRocket),
        ] {
            *inventory.inventory_menu.slot_mut(slot).unwrap() = ItemStack::new(kind, 1);
        }
        let entity = ecs
            .spawn((
                Position::new(Vec3::new(3.0, 80.0, -4.0)),
                LookDirection::new(90.0, -30.0),
                physics,
                Health(health),
                FallFlying(true),
                inventory,
                RawConnection::new_networkless(ConnectionProtocol::Game),
                InstanceName(Identifier::new("minecraft:overworld")),
            ))
            .id();
        session.bot = Some(Client {
            entity,
            ecs: Arc::new(Mutex::new(ecs)),
        });
    }

    #[test]
    fn telemetry_target_is_optional_and_only_accepts_numeric_loopback() {
        assert_eq!(parse_target(None).unwrap(), None);
        assert_eq!(
            parse_target(Some("127.0.0.1:4211"))
                .unwrap()
                .unwrap()
                .port(),
            4211
        );
        assert!(parse_target(Some("[::1]:4211")).is_ok());
        for input in [
            "",
            "localhost:4211",
            "127.0.0.1:0",
            "0.0.0.0:4211",
            "192.0.2.1:4211",
            "[::]:4211",
            "[2001:db8::1]:4211",
        ] {
            assert!(parse_target(Some(input)).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn telemetry_missing_client_preserves_identity_without_inventing_state() {
        let session = session();
        let player = snapshot_session(&session).unwrap();
        assert_eq!(player.name, "Mace001");
        assert_eq!(
            player.uuid,
            Account::offline("Mace001").uuid_or_offline().to_string()
        );
        assert!(!player.connected);
        assert_eq!(player.position, None);
        assert_eq!(player.health, None);
        assert_eq!(player.alive, None);
        assert_eq!(player.task, None);
        assert_eq!(player.equipment.chest, None);
    }

    #[test]
    fn telemetry_uses_authenticated_game_name_instead_of_microsoft_cache_key() {
        let mut session = session();
        session.config.auth = "microsoft".into();
        session.config.username = "private-cache-key@example.invalid".into();
        let check = |session: &Session| {
            let player = snapshot_session(session).unwrap();
            assert_eq!(player.name, "Mace001");
            assert_eq!(
                player.uuid,
                Account::offline("Mace001").uuid_or_offline().to_string()
            );
            assert!(
                !serde_json::to_string(&player)
                    .unwrap()
                    .contains(&session.config.username)
            );
        };
        check(&session);
        attach_client(&mut session, 20.0);
        check(&session);
        let bot = session.bot.as_ref().unwrap();
        bot.ecs
            .lock()
            .entity_mut(bot.entity)
            .remove::<RawConnection>();
        check(&session);
    }

    #[test]
    fn telemetry_idle_and_dead_login_are_sampled_without_ready_or_duel() {
        let mut session = session();
        attach_client(&mut session, 20.0);
        let player = snapshot_session(&session).unwrap();
        assert!(player.connected);
        assert_eq!(player.alive, Some(true));
        assert_eq!(player.position, Some([3.0, 80.0, -4.0]));
        assert_eq!(player.yaw, Some(90.0));
        assert_eq!(player.pitch, Some(-30.0));
        assert_eq!(player.velocity, Some([1.0, 2.0, 3.0]));
        assert_eq!(player.gliding, Some(true));
        assert_eq!(player.equipment.mainhand.as_deref(), Some("minecraft:mace"));
        assert_eq!(player.equipment.chest.as_deref(), Some("minecraft:elytra"));
        assert_eq!(player.task, None);
        let bot = session.bot.as_ref().unwrap();
        bot.ecs
            .lock()
            .entity_mut(bot.entity)
            .insert((Health(0.0), Dead));
        let dead = snapshot_session(&session).unwrap();
        assert!(dead.connected);
        assert_eq!(dead.alive, Some(false));
        assert_eq!(dead.health, Some(0.0));
    }

    #[test]
    fn telemetry_disconnection_does_not_publish_retained_coordinates_as_current() {
        let mut session = session();
        attach_client(&mut session, 20.0);
        let identity = snapshot_session(&session).unwrap().uuid;
        let bot = session.bot.as_ref().unwrap();
        bot.ecs
            .lock()
            .entity_mut(bot.entity)
            .remove::<RawConnection>();
        let player = snapshot_session(&session).unwrap();
        assert_eq!(player.uuid, identity);
        assert!(!player.connected);
        assert_eq!(player.position, None);
        assert_eq!(player.gliding, None);
    }

    #[test]
    fn telemetry_invalid_numbers_become_unknown_and_busy_ecs_is_skipped() {
        let mut session = session();
        attach_client(&mut session, f32::NAN);
        let bot = session.bot.as_ref().unwrap();
        bot.ecs.lock().get_mut::<Position>(bot.entity).unwrap().x = f64::INFINITY;
        let player = snapshot_session(&session).unwrap();
        assert_eq!(player.position, None);
        assert_eq!(player.health, None);
        assert_eq!(player.alive, None);
        let _guard = bot.ecs.lock();
        assert!(snapshot_session(&session).is_none());
    }

    #[test]
    fn telemetry_duel_state_is_available_independently_of_observation_logs() {
        let mut session = session();
        attach_client(&mut session, 20.0);
        session.task = Some(Task::Duel(Box::new(crate::tactics::Duel::new(
            "mace".into(),
            "Spear".into(),
        ))));
        let player = snapshot_session(&session).unwrap();
        assert_eq!(player.task.as_deref(), Some("duel"));
        assert_eq!(player.style.as_deref(), Some("mace"));
        assert_eq!(player.phase.as_deref(), Some("Approach"));
        assert_eq!(player.target, None);
    }

    #[test]
    fn telemetry_packets_are_bounded_and_sequences_identify_every_piece() {
        let player = snapshot_session(&session()).unwrap();
        let players = (0..128)
            .map(|i| {
                let mut copy = player.clone();
                copy.name = format!("Bot{i}{}", "x".repeat(1000));
                copy
            })
            .collect::<Vec<_>>();
        let mut sequence = 7;
        let packets = encode_frames(&players, 42, 1_000, &mut sequence).unwrap();
        assert!(packets.len() > 1);
        let mut names = Vec::new();
        for (index, packet) in packets.iter().enumerate() {
            assert!(packet.len() < MAX_PACKET_BYTES);
            let value: serde_json::Value = serde_json::from_slice(packet).unwrap();
            assert_eq!(value["event"], "telemetry_frame");
            assert_eq!(value["version"], 1);
            assert_eq!(value["process_id"], 42);
            assert_eq!(value["sampled_at_ms"], 1_000);
            assert_eq!(value["sequence"], 7 + index as u64);
            names.extend(
                value["players"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p["name"].as_str().unwrap().to_owned()),
            );
        }
        assert_eq!(
            names,
            players.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        );
        assert_eq!(sequence, 7 + packets.len() as u64);
        let mut huge = player;
        huge.name = "x".repeat(MAX_PACKET_BYTES);
        assert!(
            encode_frames(&[huge], 42, 1_000, &mut sequence)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn telemetry_sender_emits_real_udp_after_session_and_ecs_locks_are_released() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let telemetry = Telemetry::new(receiver.local_addr().unwrap()).unwrap();
        let mut active = session();
        attach_client(&mut active, 0.0);
        let active = Arc::new(Mutex::new(active));
        telemetry.sample(std::iter::once(&active));
        // Holding both locks cannot prevent serialization or delivery of the owned sample.
        let guard = active.lock();
        let _ecs_guard = guard.bot.as_ref().unwrap().ecs.lock();
        let mut packet = vec![0; MAX_PACKET_BYTES];
        let length = receiver.recv(&mut packet).unwrap();
        let frame: serde_json::Value = serde_json::from_slice(&packet[..length]).unwrap();
        assert_eq!(frame["process_id"], std::process::id());
        assert_eq!(frame["sequence"], 0);
        assert!(frame["sampled_at_ms"].as_u64().unwrap() > 0);
        assert_eq!(frame["players"][0]["name"], "Mace001");
        assert_eq!(frame["players"][0]["connected"], true);
        assert_eq!(frame["players"][0]["alive"], false);
    }

    #[tokio::test]
    async fn telemetry_busy_session_and_full_sender_queue_drop_frames_without_waiting() {
        let (sender, receiver) = sync_channel(1);
        let telemetry = Telemetry {
            sender,
            interval: tokio::time::interval(Duration::from_millis(100)),
        };
        let shared = Arc::new(Mutex::new(session()));
        let guard = shared.lock();
        telemetry.sample(std::iter::once(&shared));
        assert!(receiver.try_recv().is_err());
        drop(guard);
        telemetry.sample(std::iter::once(&shared));
        telemetry.sample(std::iter::once(&shared));
        assert_eq!(receiver.try_recv().unwrap().players.len(), 1);
        assert!(receiver.try_recv().is_err());
    }
}
