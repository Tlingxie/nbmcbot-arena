use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use azalea::{
    BlockPos,
    auth::game_profile::GameProfile,
    block::BlockState,
    buf::AzaleaWrite,
    core::{
        game_type::{GameMode, OptionalGameType},
        position::Vec3,
    },
    entity::LookDirection,
    protocol::{
        common::movements::{PositionMoveRotation, RelativeMovements},
        connect::Connection,
        packets::{
            ClientIntention,
            common::CommonPlayerSpawnInfo,
            config::{
                ClientboundFinishConfiguration, ClientboundRegistryData, ServerboundConfigPacket,
            },
            game::{
                ClientboundAddEntity, ClientboundBlockChangedAck, ClientboundBlockUpdate,
                ClientboundContainerSetSlot, ClientboundLevelChunkWithLight, ClientboundLogin,
                ClientboundPlayerInfoUpdate, ClientboundPlayerPosition, ClientboundRespawn,
                ClientboundSetChunkCacheCenter, ClientboundSetHealth, ClientboundSetHeldSlot,
                ClientboundTeleportEntity, ServerboundGamePacket,
                c_level_chunk_with_light::ClientboundLevelChunkPacketData,
                c_light_update::ClientboundLightUpdatePacketData,
                c_player_info_update::{ActionEnumSet, PlayerInfoEntry},
                s_client_command::Action as ClientCommandAction,
                s_interact::ActionType,
                s_interact::InteractionHand,
                s_player_action::Action,
            },
            handshake::{ClientboundHandshakePacket, ServerboundHandshakePacket},
            login::{ClientboundLoginFinished, ServerboundLoginPacket},
        },
    },
    registry::{
        DataRegistry,
        builtin::{BlockKind, EntityKind, ItemKind},
        data::{Biome, DimensionKind},
        identifier::Identifier,
    },
    world::{MinecraftEntityId, Section, palette::PalettedContainer},
};
use simdnbt::owned::{NbtCompound, NbtTag};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc::UnboundedSender},
    task::JoinSet,
};
use uuid::Uuid;

#[derive(Debug)]
pub enum FixtureEvent {
    Handshake {
        protocol: i32,
    },
    Login {
        name: String,
    },
    Chat(String),
    Movement {
        x: f64,
        y: f64,
        z: f64,
    },
    BlockBroken {
        x: i32,
        y: i32,
        z: i32,
    },
    Respawn,
    HealthZero,
    TeleportAccepted(u32),
    SelectedHotbar(u16),
    UsedMainHand,
    ReleasedUse,
    ViewDistance(u8),
    TotemSwap {
        source: i16,
        target: u8,
        is_swap: bool,
    },
    Closed,
    CombatAttack {
        target: i32,
        elapsed_ms: u128,
        selected_slot: u16,
        empty_hand: bool,
        distance: f64,
    },
    CombatSwing {
        main_hand: bool,
        elapsed_ms: u128,
    },
}

/// One offline session over the actual 1.21.11 packet codec. No server process is needed.
pub async fn serve(
    listener: TcpListener,
    events: UnboundedSender<FixtureEvent>,
) -> anyhow::Result<()> {
    serve_sessions(listener, events, 1).await
}

pub async fn serve_sessions(
    listener: TcpListener,
    events: UnboundedSender<FixtureEvent>,
    count: usize,
) -> anyhow::Result<()> {
    let world = Arc::new(FixtureWorld::new());
    let mut sessions = JoinSet::new();
    for _ in 0..count {
        let (stream, _) = listener.accept().await?;
        let events = events.clone();
        let world = world.clone();
        sessions.spawn(async move { serve_session(stream, &events, world, 0).await });
    }
    drop(events);
    while let Some(session) = sessions.join_next().await {
        session??;
    }
    Ok(())
}

#[derive(Debug)]
pub struct NamedFixtureEvent {
    pub name: String,
    pub event: FixtureEvent,
}

pub async fn serve_many(
    listener: TcpListener,
    events: UnboundedSender<NamedFixtureEvent>,
    count: usize,
    radius: i32,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        (0..=8).contains(&radius) && (1..=256).contains(&count),
        "invalid fixture dimensions"
    );
    let world = Arc::new(FixtureWorld::new());
    let mut sessions = JoinSet::new();
    for _ in 0..count {
        let (stream, _) = listener.accept().await?;
        let events = events.clone();
        let world = world.clone();
        sessions.spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let forward = tokio::spawn(async move {
                let mut name = None;
                let mut buffered = Vec::new();
                while let Some(event) = rx.recv().await {
                    if let FixtureEvent::Login { name: value } = &event {
                        name = Some(value.clone());
                    }
                    if let Some(name) = &name {
                        for event in buffered.drain(..) {
                            let _ = events.send(NamedFixtureEvent {
                                name: name.clone(),
                                event,
                            });
                        }
                        let _ = events.send(NamedFixtureEvent {
                            name: name.clone(),
                            event,
                        });
                    } else {
                        buffered.push(event);
                    }
                }
            });
            let result = serve_session(stream, &tx, world, radius).await;
            drop(tx);
            forward.await?;
            result
        });
    }
    drop(events);
    while let Some(session) = sessions.join_next().await {
        session??;
    }
    Ok(())
}

struct FixtureWorld {
    identities: Mutex<HashMap<String, i32>>,
    broken: Mutex<HashSet<(i32, i32, i32)>>,
    updates: broadcast::Sender<BlockPos>,
}
impl FixtureWorld {
    fn new() -> Self {
        Self {
            identities: Mutex::new(HashMap::new()),
            broken: Mutex::new(HashSet::new()),
            updates: broadcast::channel(1024).0,
        }
    }
    fn identity(&self, name: &str) -> i32 {
        let mut identities = self.identities.lock().unwrap();
        let next = identities.len() as i32 + 1;
        *identities.entry(name.into()).or_insert(next)
    }
}

fn world_chunks(radius: i32) -> anyhow::Result<Vec<ClientboundLevelChunkWithLight>> {
    let mut chunks = Vec::new();
    for x in -radius..=radius {
        for z in -radius..=radius {
            let sections: Vec<_> = (0..24)
                .map(|_| Section {
                    block_count: 0,
                    states: PalettedContainer::<BlockState>::new(),
                    biomes: PalettedContainer::<Biome>::new(),
                })
                .collect();
            let mut bytes = Vec::new();
            sections.azalea_write(&mut bytes)?;
            chunks.push(ClientboundLevelChunkWithLight {
                x,
                z,
                chunk_data: ClientboundLevelChunkPacketData {
                    heightmaps: Default::default(),
                    data: Arc::new(bytes.into()),
                    block_entities: vec![],
                },
                light_data: ClientboundLightUpdatePacketData::default(),
            });
        }
    }
    Ok(chunks)
}

fn floor_updates(world: &FixtureWorld, radius: i32) -> Vec<ClientboundBlockUpdate> {
    let broken = world.broken.lock().unwrap();
    let mut updates = Vec::new();
    for x in -radius * 16..(radius + 1) * 16 {
        for z in -radius * 16..(radius + 1) * 16 {
            let block_state = if broken.contains(&(x, 63, z)) {
                BlockKind::Air.into()
            } else {
                BlockKind::Stone.into()
            };
            updates.push(ClientboundBlockUpdate {
                pos: BlockPos::new(x, 63, z),
                block_state,
            });
        }
    }
    updates
}

async fn serve_session(
    stream: TcpStream,
    events: &UnboundedSender<FixtureEvent>,
    world: Arc<FixtureWorld>,
    radius: i32,
) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let mut conn =
        Connection::<ServerboundHandshakePacket, ClientboundHandshakePacket>::wrap(stream);
    let ServerboundHandshakePacket::Intention(intention) = conn.read().await?;
    eprintln!("fixture handshake {}", intention.protocol_version);
    anyhow::ensure!(intention.protocol_version == 774, "expected protocol 774");
    anyhow::ensure!(
        intention.intention == ClientIntention::Login,
        "expected login intention"
    );
    let _ = events.send(FixtureEvent::Handshake {
        protocol: intention.protocol_version,
    });
    let mut conn = conn.login();
    let ServerboundLoginPacket::Hello(hello) = conn.read().await? else {
        anyhow::bail!("expected Hello");
    };
    let _ = events.send(FixtureEvent::Login {
        name: hello.name.clone(),
    });
    eprintln!("fixture login {}", hello.name);
    let entity_id = world.identity(&hello.name);
    let uuid = Uuid::from_u128(entity_id as u128);
    conn.write(ClientboundLoginFinished {
        game_profile: GameProfile::new(uuid, hello.name),
    })
    .await?;
    loop {
        if matches!(
            conn.read().await?,
            ServerboundLoginPacket::LoginAcknowledged(_)
        ) {
            break;
        }
    }
    let mut conn = conn.config();
    conn.write(ClientboundRegistryData {
        registry_id: Identifier::new("minecraft:dimension_type"),
        entries: vec![(
            Identifier::new("minecraft:overworld"),
            Some(NbtCompound::from_values(vec![
                ("height".into(), NbtTag::Int(384)),
                ("min_y".into(), NbtTag::Int(-64)),
            ])),
        )]
        .into_iter()
        .collect(),
    })
    .await?;
    // Wait for client settings before finishing configuration so the fixture
    // observes negotiated view distance in both initial and reconnected sessions.
    loop {
        if let ServerboundConfigPacket::ClientInformation(packet) = conn.read().await? {
            eprintln!(
                "fixture configuration view distance {}",
                packet.information.view_distance
            );
            let _ = events.send(FixtureEvent::ViewDistance(packet.information.view_distance));
            break;
        }
    }
    conn.write(ClientboundFinishConfiguration).await?;
    loop {
        match conn.read().await? {
            ServerboundConfigPacket::ClientInformation(packet) => {
                eprintln!(
                    "fixture configuration view distance {}",
                    packet.information.view_distance
                );
                let _ = events.send(FixtureEvent::ViewDistance(packet.information.view_distance));
            }
            ServerboundConfigPacket::FinishConfiguration(_) => break,
            _ => {}
        }
    }
    let mut conn = conn.game();
    eprintln!("fixture entered game");
    let common = CommonPlayerSpawnInfo {
        dimension_type: DimensionKind::new_raw(0),
        dimension: Identifier::new("minecraft:overworld"),
        seed: 0,
        game_type: GameMode::Survival,
        previous_game_type: OptionalGameType(None),
        is_debug: false,
        is_flat: true,
        last_death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
    };
    // Adapted from Azalea's MIT-licensed test_utils packet builders.
    conn.write(ClientboundLogin {
        player_id: MinecraftEntityId(entity_id),
        hardcore: false,
        levels: vec![Identifier::new("minecraft:overworld")],
        max_players: 256,
        chunk_radius: radius.max(2) as u32,
        simulation_distance: 2,
        reduced_debug_info: false,
        show_death_screen: true,
        do_limited_crafting: false,
        common: common.clone(),
        enforces_secure_chat: false,
    })
    .await?;
    conn.write(ClientboundSetChunkCacheCenter { x: 0, z: 0 })
        .await?;
    let mut updates = world.updates.subscribe();
    for chunk in world_chunks(radius)? {
        conn.write(chunk).await?;
    }
    for update in floor_updates(&world, radius) {
        conn.write(update).await?;
    }
    conn.write(ClientboundSetHealth {
        health: 20.0,
        food: 20,
        saturation: 5.0,
    })
    .await?;
    let position = ClientboundPlayerPosition {
        id: 1,
        change: PositionMoveRotation {
            pos: Vec3::new(4.5, 64.0, 4.5),
            delta: Vec3::ZERO,
            look_direction: LookDirection::default(),
        },
        relative: RelativeMovements::all_absolute(),
    };
    conn.write(position.clone()).await?;
    let mut food_ready = false;
    let combat_clock = std::time::Instant::now();
    let mut combat_position = Vec3::new(4.5, 64.0, 4.5);
    let mut target_position = Vec3::new(12.5, 64.0, 4.5);
    let mut combat_selected_slot = 0;
    let mut weapon_ready = false;
    loop {
        let incoming = tokio::select! {
            packet = conn.read() => packet,
            update = updates.recv() => {
                let pos = update?;
                conn.write(ClientboundBlockUpdate {pos, block_state: BlockKind::Air.into()}).await?;
                continue;
            }
        };
        let packet = match incoming {
            Ok(packet) => packet,
            Err(error) => {
                let _ = events.send(FixtureEvent::Closed);
                // An EOF after quit is the normal session terminator.
                use azalea::protocol::read::{FrameSplitterError, ReadPacketError};
                if matches!(
                    &*error,
                    ReadPacketError::ConnectionClosed
                        | ReadPacketError::FrameSplitter {
                            source: FrameSplitterError::ConnectionClosed
                                | FrameSplitterError::ConnectionReset
                        }
                ) {
                    return Ok(());
                }
                return Err(error.into());
            }
        };
        match packet {
            ServerboundGamePacket::PlayerAction(packet) => {
                if packet.action == Action::ReleaseUseItem {
                    let _ = events.send(FixtureEvent::ReleasedUse);
                }
                if packet.action == Action::StopDestroyBlock
                    && packet.pos.y == 63
                    && (-radius * 16..(radius + 1) * 16).contains(&packet.pos.x)
                    && (-radius * 16..(radius + 1) * 16).contains(&packet.pos.z)
                {
                    world
                        .broken
                        .lock()
                        .unwrap()
                        .insert((packet.pos.x, packet.pos.y, packet.pos.z));
                    let _ = world.updates.send(packet.pos);
                    let _ = events.send(FixtureEvent::BlockBroken {
                        x: packet.pos.x,
                        y: packet.pos.y,
                        z: packet.pos.z,
                    });
                }
                conn.write(ClientboundBlockChangedAck { seq: packet.seq })
                    .await?;
            }
            ServerboundGamePacket::Chat(packet) => {
                eprintln!("fixture chat {}", packet.message);
                if packet.message == "fixture-weapon" {
                    conn.write(ClientboundContainerSetSlot {
                        container_id: 0,
                        state_id: 10,
                        slot: 36,
                        item_stack: azalea::inventory::ItemStack::new(ItemKind::DiamondSword, 1),
                    })
                    .await?;
                    conn.write(ClientboundSetHeldSlot { slot: 0 }).await?;
                    combat_selected_slot = 0;
                    weapon_ready = true;
                }
                if packet.message == "fixture-fighter" {
                    conn.write(ClientboundPlayerInfoUpdate {
                        actions: ActionEnumSet {
                            add_player: true,
                            initialize_chat: false,
                            update_game_mode: false,
                            update_listed: true,
                            update_latency: false,
                            update_display_name: false,
                            update_hat: false,
                            update_list_order: false,
                        },
                        entries: vec![PlayerInfoEntry {
                            profile: GameProfile::new(
                                Uuid::from_u128(10000),
                                "SparringPlayer".into(),
                            ),
                            listed: true,
                            ..Default::default()
                        }],
                    })
                    .await?;
                    conn.write(ClientboundAddEntity {
                        id: MinecraftEntityId(10000),
                        uuid: Uuid::from_u128(10000),
                        entity_type: EntityKind::Player,
                        position: target_position,
                        movement: Default::default(),
                        x_rot: 0,
                        y_rot: 0,
                        y_head_rot: 0,
                        data: 0,
                    })
                    .await?;
                }
                if packet.message == "fixture-target-far" {
                    target_position = Vec3::new(14.5, 64.0, 4.5);
                    conn.write(ClientboundTeleportEntity {
                        id: MinecraftEntityId(10000),
                        change: PositionMoveRotation {
                            pos: target_position,
                            delta: Vec3::ZERO,
                            look_direction: LookDirection::default(),
                        },
                        relative: RelativeMovements::all_absolute(),
                        on_ground: true,
                    })
                    .await?;
                }
                if packet.message == "fixture-target-turn" {
                    target_position = Vec3::new(4.5, 64.0, 12.5);
                    conn.write(ClientboundTeleportEntity {
                        id: MinecraftEntityId(10000),
                        change: PositionMoveRotation {
                            pos: target_position,
                            delta: Vec3::ZERO,
                            look_direction: LookDirection::default(),
                        },
                        relative: RelativeMovements::all_absolute(),
                        on_ground: true,
                    })
                    .await?;
                }
                if packet.message == "fixture-die" {
                    conn.write(ClientboundSetHealth {
                        health: 0.0,
                        food: 20,
                        saturation: 5.0,
                    })
                    .await?;
                    let _ = events.send(FixtureEvent::HealthZero);
                }
                if packet.message == "fixture-food" {
                    conn.write(ClientboundContainerSetSlot {
                        container_id: 0,
                        state_id: 1,
                        slot: 36,
                        item_stack: azalea::inventory::ItemStack::new(ItemKind::Stone, 1),
                    })
                    .await?;
                    conn.write(ClientboundContainerSetSlot {
                        container_id: 0,
                        state_id: 2,
                        slot: 38,
                        item_stack: azalea::inventory::ItemStack::new(ItemKind::Bread, 3),
                    })
                    .await?;
                    conn.write(ClientboundSetHeldSlot { slot: 0 }).await?;
                    conn.write(ClientboundSetHealth {
                        health: 20.0,
                        food: 12,
                        saturation: 0.0,
                    })
                    .await?;
                    food_ready = true;
                }
                if packet.message == "fixture-totem" {
                    conn.write(ClientboundContainerSetSlot {
                        container_id: 0,
                        state_id: 3,
                        slot: 10,
                        item_stack: azalea::inventory::ItemStack::new(ItemKind::TotemOfUndying, 1),
                    })
                    .await?;
                }
                let _ = events.send(FixtureEvent::Chat(packet.message));
            }
            ServerboundGamePacket::ClientCommand(packet)
                if packet.action == ClientCommandAction::PerformRespawn =>
            {
                eprintln!("fixture respawn");
                let _ = events.send(FixtureEvent::Respawn);
                conn.write(ClientboundRespawn {
                    common: common.clone(),
                    data_to_keep: 0,
                })
                .await?;
                conn.write(ClientboundSetChunkCacheCenter { x: 0, z: 0 })
                    .await?;
                for chunk in world_chunks(radius)? {
                    conn.write(chunk).await?;
                }
                for update in floor_updates(&world, radius) {
                    conn.write(update).await?;
                }
                conn.write(ClientboundSetHealth {
                    health: 20.0,
                    food: 20,
                    saturation: 5.0,
                })
                .await?;
                let mut position = position.clone();
                position.id = 2;
                conn.write(position).await?;
            }
            ServerboundGamePacket::MovePlayerPos(packet) => {
                combat_position = packet.pos;
                eprintln!("fixture movement {:?}", packet.pos);
                let _ = events.send(FixtureEvent::Movement {
                    x: packet.pos.x,
                    y: packet.pos.y,
                    z: packet.pos.z,
                });
            }
            ServerboundGamePacket::ClientInformation(packet) => {
                eprintln!(
                    "fixture game view distance {}",
                    packet.client_information.view_distance
                );
                let _ = events.send(FixtureEvent::ViewDistance(
                    packet.client_information.view_distance,
                ));
            }
            ServerboundGamePacket::ContainerClick(packet) => {
                if packet.slot_num == 10 && packet.button_num == 40 {
                    anyhow::ensure!(
                        packet.changed_slots.contains_key(&10)
                            && packet.changed_slots.contains_key(&45),
                        "offhand swap must hash menu slots10 and45"
                    );
                    anyhow::ensure!(
                        !packet.changed_slots.contains_key(&40),
                        "offhand swap must not predict menu slot40"
                    );
                    anyhow::ensure!(
                        packet.changed_slots[&10].0.is_none(),
                        "totem source must predict empty after swap"
                    );
                    anyhow::ensure!(
                        packet.changed_slots[&45]
                            .0
                            .as_ref()
                            .is_some_and(
                                |item| item.kind == ItemKind::TotemOfUndying && item.count == 1
                            ),
                        "offhand predicted hash must contain one totem"
                    );
                    conn.write(ClientboundContainerSetSlot {
                        container_id: 0,
                        state_id: 4,
                        slot: 10,
                        item_stack: azalea::inventory::ItemStack::Empty,
                    })
                    .await?;
                    conn.write(ClientboundContainerSetSlot {
                        container_id: 0,
                        state_id: 5,
                        slot: 45,
                        item_stack: azalea::inventory::ItemStack::new(ItemKind::TotemOfUndying, 1),
                    })
                    .await?;
                }
                let _ = events.send(FixtureEvent::TotemSwap {
                    source: packet.slot_num,
                    target: packet.button_num,
                    is_swap: packet.click_type == azalea::inventory::operations::ClickType::Swap,
                });
            }
            ServerboundGamePacket::AcceptTeleportation(packet) => {
                let _ = events.send(FixtureEvent::TeleportAccepted(packet.id));
            }
            ServerboundGamePacket::SetCarriedItem(packet) => {
                combat_selected_slot = packet.slot;
                let _ = events.send(FixtureEvent::SelectedHotbar(packet.slot));
            }
            ServerboundGamePacket::UseItem(packet) if packet.hand == InteractionHand::MainHand => {
                let _ = events.send(FixtureEvent::UsedMainHand);
                if food_ready {
                    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
                    conn.write(ClientboundSetHealth {
                        health: 20.0,
                        food: 20,
                        saturation: 5.0,
                    })
                    .await?;
                    food_ready = false;
                }
            }
            ServerboundGamePacket::MovePlayerPosRot(packet) => {
                combat_position = packet.pos;
                eprintln!("fixture movement {:?}", packet.pos);
                let _ = events.send(FixtureEvent::Movement {
                    x: packet.pos.x,
                    y: packet.pos.y,
                    z: packet.pos.z,
                });
            }
            ServerboundGamePacket::Interact(packet) if packet.action == ActionType::Attack => {
                let eye = combat_position + Vec3::new(0.0, 1.62, 0.0);
                let nearest = Vec3::new(
                    eye.x
                        .clamp(target_position.x - 0.3, target_position.x + 0.3),
                    eye.y.clamp(target_position.y, target_position.y + 1.8),
                    eye.z
                        .clamp(target_position.z - 0.3, target_position.z + 0.3),
                );
                let delta = eye - nearest;
                let _ = events.send(FixtureEvent::CombatAttack {
                    target: packet.entity_id.0,
                    elapsed_ms: combat_clock.elapsed().as_millis(),
                    selected_slot: combat_selected_slot,
                    empty_hand: !weapon_ready || combat_selected_slot != 0,
                    distance: (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt(),
                });
            }
            ServerboundGamePacket::Swing(packet) => {
                let _ = events.send(FixtureEvent::CombatSwing {
                    main_hand: packet.hand == InteractionHand::MainHand,
                    elapsed_ms: combat_clock.elapsed().as_millis(),
                });
            }
            _ => {}
        }
    }
}
