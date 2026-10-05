use std::{
    fmt::Debug,
    io::Cursor,
    mem,
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
};

use azalea_crypto::Aes128CfbEnc;
use azalea_protocol::{
    connect::{RawReadConnection, RawWriteConnection},
    packets::{
        ConnectionProtocol, Packet, ProtocolPacket, config::ClientboundConfigPacket,
        game::ClientboundGamePacket, login::ClientboundLoginPacket,
    },
    read::{ReadPacketError, deserialize_packet},
    write::serialize_packet,
};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_tasks::{IoTaskPool, futures_lite::future};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    net::tcp::OwnedWriteHalf,
    sync::mpsc::{self},
};
use tracing::{debug, error, info, trace};

use super::packet::{
    config::ReceiveConfigPacketEvent, game::ReceiveGamePacketEvent, login::ReceiveLoginPacketEvent,
};
use crate::packet::{config, game, login};

pub struct ConnectionPlugin;
impl Plugin for ConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, (poll_all_writer_tasks, read_packets).chain());
    }
}

const MAX_PACKETS_PER_CONNECTION_PER_UPDATE: usize = 256;

#[derive(Default, Resource)]
struct PacketReadCursor(usize);

pub fn read_packets(ecs: &mut World) {
    let mut entity_and_conn_query = ecs.query::<(Entity, &mut RawConnection)>();
    let mut conn_query = ecs.query::<&mut RawConnection>();

    let mut entities_handling_packets = Vec::new();
    let mut entities_with_injected_packets = Vec::new();
    for (entity, mut raw_conn) in entity_and_conn_query.iter_mut(ecs) {
        if !raw_conn.injected_clientbound_packets.is_empty() {
            entities_with_injected_packets.push((
                entity,
                mem::take(&mut raw_conn.injected_clientbound_packets),
            ));
        }

        if raw_conn.network.is_none() {
            // no network connection, don't bother with the normal packet handling
            continue;
        }

        entities_handling_packets.push(entity);
    }

    let mut queued_packet_events = QueuedPacketEvents::default();

    // handle injected packets, see the comment on
    // RawConnection::injected_clientbound_packets for more info
    for (entity, raw_packets) in entities_with_injected_packets {
        for raw_packet in raw_packets {
            let conn = conn_query.get(ecs, entity).unwrap();
            let state = conn.state;

            trace!("Received injected packet with bytes: {raw_packet:?}");
            if let Err(e) =
                handle_raw_packet(ecs, &raw_packet, entity, state, &mut queued_packet_events)
            {
                error!("Error reading injected packet: {e}");
            }
        }
    }

    if !entities_handling_packets.is_empty() {
        let count = entities_handling_packets.len();
        let first = ecs
            .get_resource::<PacketReadCursor>()
            .map_or(0, |cursor| cursor.0)
            % count;
        entities_handling_packets.rotate_left(first);
        ecs.insert_resource(PacketReadCursor((first + 1) % count));
    }

    for entity in entities_handling_packets {
        for _ in 0..MAX_PACKETS_PER_CONNECTION_PER_UPDATE {
            let Ok(mut conn) = conn_query.get_mut(ecs, entity) else {
                break;
            };
            let Some(net_conn) = conn.net_conn() else {
                break;
            };
            // try_read internally polls Tokio AsyncRead. An exhausted parent task budget
            // must not masquerade as an empty socket; the packet cap bounds this work.
            let read_res = future::block_on(tokio::task::unconstrained(async {
                net_conn.reader.try_read()
            }));
            let state = conn.state;
            match read_res {
                Ok(Some(raw_packet)) => {
                    let raw_packet = Arc::<[u8]>::from(raw_packet);
                    if let Err(e) = handle_raw_packet(
                        ecs,
                        &raw_packet,
                        entity,
                        state,
                        &mut queued_packet_events,
                    ) {
                        error!("Error reading packet: {e}");
                    }
                }
                Ok(None) => {
                    // no packets available
                    break;
                }
                Err(err) => {
                    log_for_error(&err);

                    if matches!(
                        &*err,
                        ReadPacketError::IoError { .. } | ReadPacketError::ConnectionClosed
                    ) {
                        info!("Server closed connection");
                        // ungraceful disconnect :(
                        conn.network = None;
                        // setting this will make us send a DisconnectEvent
                        conn.is_alive = false;
                    }

                    break;
                }
            }
        }
        if let Ok(mut conn) = conn_query.get_mut(ecs, entity) {
            conn.poll_writer();
        }
    }

    queued_packet_events.write_messages(ecs);
}

fn poll_all_writer_tasks(mut conn_query: Query<&mut RawConnection>) {
    for mut conn in conn_query.iter_mut() {
        conn.poll_writer();
    }
}

#[derive(Default)]
pub struct QueuedPacketEvents {
    login: Vec<ReceiveLoginPacketEvent>,
    config: Vec<ReceiveConfigPacketEvent>,
    game: Vec<ReceiveGamePacketEvent>,
}
impl QueuedPacketEvents {
    fn write_messages(&mut self, ecs: &mut World) {
        ecs.write_message_batch(self.login.drain(..));
        ecs.write_message_batch(self.config.drain(..));
        ecs.write_message_batch(self.game.drain(..));
    }
}

fn log_for_error(error: &ReadPacketError) {
    if !matches!(*error, ReadPacketError::ConnectionClosed) {
        error!("Error reading packet from Client: {error:?}");
    }
}

/// The client's connection to the server.
#[derive(Component)]
pub struct RawConnection {
    /// The network connection to the server.
    ///
    /// This isn't guaranteed to be present, for example during the main packet
    /// handlers or at all times during tests.
    ///
    /// You shouldn't rely on this. Instead, use the events for sending packets
    /// like [`SendGamePacketEvent`](crate::packet::game::SendGamePacketEvent) /
    /// [`SendConfigPacketEvent`](crate::packet::config::SendConfigPacketEvent)
    /// / [`SendLoginPacketEvent`](crate::packet::login::SendLoginPacketEvent).
    ///
    /// To check if we haven't disconnected from the server, use
    /// [`Self::is_alive`].
    pub(crate) network: Option<NetworkConnection>,
    pub state: ConnectionProtocol,
    pub(crate) is_alive: bool,

    /// This exists for internal testing purposes and probably shouldn't be used
    /// for normal bots.
    ///
    /// It's basically a way to make our client think it received a packet from
    /// the server without needing to interact with the network.
    pub injected_clientbound_packets: Vec<Box<[u8]>>,
}
impl RawConnection {
    fn poll_writer(&mut self) {
        if self
            .network
            .as_mut()
            .is_some_and(|network| network.poll_writer().is_some())
        {
            self.network = None;
            self.is_alive = false;
        }
    }

    pub fn new(
        reader: RawReadConnection,
        writer: RawWriteConnection,
        state: ConnectionProtocol,
    ) -> Self {
        let task_pool = IoTaskPool::get();

        let (network_packet_writer_tx, network_packet_writer_rx) =
            mpsc::unbounded_channel::<Box<[u8]>>();

        let writer_task =
            task_pool.spawn(write_task(network_packet_writer_rx, writer.write_stream));

        let mut conn = Self::new_networkless(state);
        conn.network = Some(NetworkConnection {
            reader,
            enc_cipher: writer.enc_cipher,
            network_packet_writer_tx,
            writer_task,
        });

        conn
    }

    pub fn new_networkless(state: ConnectionProtocol) -> Self {
        Self {
            network: None,
            state,
            is_alive: true,
            injected_clientbound_packets: Vec::new(),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.is_alive
    }

    /// Write a packet to the server without emitting any events.
    ///
    /// This is called by the handlers for [`SendGamePacketEvent`],
    /// [`SendConfigPacketEvent`], and [`SendLoginPacketEvent`].
    ///
    /// [`SendGamePacketEvent`]: crate::packet::game::SendGamePacketEvent
    /// [`SendConfigPacketEvent`]: crate::packet::config::SendConfigPacketEvent
    /// [`SendLoginPacketEvent`]: crate::packet::login::SendLoginPacketEvent
    pub fn write<P: ProtocolPacket + Debug>(
        &mut self,
        packet: impl Packet<P>,
    ) -> Result<(), WritePacketError> {
        if let Some(network) = &mut self.network {
            network.write(packet)?;
        } else {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, atomic::Ordering::Relaxed) {
                debug!(
                    "tried to write packet to the network but there is no NetworkConnection. if you're trying to send a packet from the handler function, use self.write instead"
                );
            }
        }
        Ok(())
    }

    pub fn net_conn(&mut self) -> Option<&mut NetworkConnection> {
        self.network.as_mut()
    }
}

pub fn handle_raw_packet(
    ecs: &mut World,
    raw_packet: &[u8],
    entity: Entity,
    state: ConnectionProtocol,
    queued_packet_events: &mut QueuedPacketEvents,
) -> Result<(), Box<ReadPacketError>> {
    let stream = &mut Cursor::new(raw_packet);
    match state {
        ConnectionProtocol::Handshake => {
            unreachable!()
        }
        ConnectionProtocol::Game => {
            let packet = Arc::new(deserialize_packet::<ClientboundGamePacket>(stream)?);
            trace!("Packet: {packet:?}");
            game::process_packet(ecs, entity, packet.as_ref());
            queued_packet_events
                .game
                .push(ReceiveGamePacketEvent { entity, packet });
        }
        ConnectionProtocol::Status => {
            unreachable!()
        }
        ConnectionProtocol::Login => {
            let packet = Arc::new(deserialize_packet::<ClientboundLoginPacket>(stream)?);
            trace!("Packet: {packet:?}");
            login::process_packet(ecs, entity, &packet);
            queued_packet_events
                .login
                .push(ReceiveLoginPacketEvent { entity, packet });
        }
        ConnectionProtocol::Configuration => {
            let packet = Arc::new(deserialize_packet::<ClientboundConfigPacket>(stream)?);
            trace!("Packet: {packet:?}");
            config::process_packet(ecs, entity, &packet);
            queued_packet_events
                .config
                .push(ReceiveConfigPacketEvent { entity, packet });
        }
    };

    Ok(())
}

pub struct NetworkConnection {
    reader: RawReadConnection,
    // compression threshold is in the RawReadConnection
    pub enc_cipher: Option<Aes128CfbEnc>,

    pub writer_task: bevy_tasks::Task<()>,
    /// A queue of raw TCP packets to send.
    ///
    /// These will not be modified further, they should already be serialized
    /// and compressed and encrypted before being added here.
    network_packet_writer_tx: mpsc::UnboundedSender<Box<[u8]>>,
}
impl NetworkConnection {
    pub fn write<P: ProtocolPacket + Debug>(
        &mut self,
        packet: impl Packet<P>,
    ) -> Result<(), WritePacketError> {
        let packet = packet.into_variant();
        let raw_packet = serialize_packet(&packet)?;
        self.write_raw(&raw_packet)?;

        Ok(())
    }

    pub fn write_raw(&mut self, raw_packet: &[u8]) -> Result<(), WritePacketError> {
        let network_packet = azalea_protocol::write::encode_to_network_packet(
            raw_packet,
            self.reader.compression_threshold,
            &mut self.enc_cipher,
        );
        self.network_packet_writer_tx
            .send(network_packet.into_boxed_slice())?;
        Ok(())
    }

    /// Makes sure packets get sent and returns Some(()) if the connection has
    /// closed.
    pub fn poll_writer(&mut self) -> Option<()> {
        let poll_once_res = future::poll_once(&mut self.writer_task);
        future::block_on(poll_once_res)
    }

    pub fn set_compression_threshold(&mut self, threshold: Option<u32>) {
        trace!("Set compression threshold to {threshold:?}");
        self.reader.compression_threshold = threshold;
    }
    /// Set the encryption key that is used to encrypt and decrypt packets.
    ///
    /// The same key is used for both reading and writing.
    pub fn set_encryption_key(&mut self, key: [u8; 16]) {
        trace!("Enabled protocol encryption");
        let (enc_cipher, dec_cipher) = azalea_crypto::create_cipher(&key);
        self.reader.dec_cipher = Some(dec_cipher);
        self.enc_cipher = Some(enc_cipher);
    }
}

async fn write_task(
    mut network_packet_writer_rx: mpsc::UnboundedReceiver<Box<[u8]>>,
    mut write_half: OwnedWriteHalf,
) {
    while let Some(network_packet) = network_packet_writer_rx.recv().await {
        if let Err(e) = write_half.write_all(&network_packet).await {
            debug!("Error writing packet to server: {e}");
            break;
        };
    }

    trace!("write task is done");
}

#[derive(Debug, Error)]
pub enum WritePacketError {
    #[error("Wrong protocol state: expected {expected:?}, got {got:?}")]
    WrongState {
        expected: ConnectionProtocol,
        got: ConnectionProtocol,
    },
    #[error(transparent)]
    Encoding(#[from] azalea_protocol::write::PacketEncodeError),
    #[error(transparent)]
    SendError {
        #[from]
        #[backtrace]
        source: mpsc::error::SendError<Box<[u8]>>,
    },
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use crate::packet::game::KeepAliveEvent;
    use azalea_protocol::packets::game::ClientboundKeepAlive;
    use std::task::{Context, Poll, Waker};

    fn keepalive_frame(id: u64) -> Vec<u8> {
        let packet = ClientboundGamePacket::KeepAlive(ClientboundKeepAlive { id });
        azalea_protocol::write::encode_to_network_packet(
            &serialize_packet(&packet).unwrap(),
            None,
            &mut None,
        )
    }

    #[tokio::test]
    async fn busy_connection_yields_to_other_clients_and_next_round_rotates() {
        IoTaskPool::get_or_init(|| bevy_tasks::TaskPoolBuilder::new().num_threads(1).build());
        let mut world = World::new();
        world.init_resource::<Messages<KeepAliveEvent>>();
        world.init_resource::<Messages<ReceiveGamePacketEvent>>();
        world.init_resource::<Messages<ReceiveConfigPacketEvent>>();
        world.init_resource::<Messages<ReceiveLoginPacketEvent>>();
        let mut entities = Vec::new();
        let mut peers = Vec::new();
        for (count, id) in [(1_024, 1), (1, 2)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let stream = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (peer, _) = listener.accept().await.unwrap();
            let (reader, writer) = stream.into_split();
            let buffered_packets = keepalive_frame(id).repeat(count);
            entities.push(
                world
                    .spawn(RawConnection::new(
                        RawReadConnection {
                            read_stream: reader,
                            buffer: Cursor::new(buffered_packets),
                            compression_threshold: None,
                            dec_cipher: None,
                        },
                        RawWriteConnection {
                            write_stream: writer,
                            compression_threshold: None,
                            enc_cipher: None,
                        },
                        ConnectionProtocol::Game,
                    ))
                    .id(),
            );
            peers.push(peer);
        }
        let mut cursor = world.resource::<Messages<KeepAliveEvent>>().get_cursor();
        read_packets(&mut world);
        let first: Vec<_> = cursor
            .read(world.resource::<Messages<KeepAliveEvent>>())
            .map(|event| (event.entity, event.id))
            .collect();
        assert_eq!(
            first.iter().filter(|event| event.0 == entities[0]).count(),
            256
        );
        assert_eq!(first.last(), Some(&(entities[1], 2)));

        world
            .get_mut::<RawConnection>(entities[1])
            .unwrap()
            .net_conn()
            .unwrap()
            .reader
            .buffer
            .get_mut()
            .extend_from_slice(&keepalive_frame(3));
        read_packets(&mut world);
        let second: Vec<_> = cursor
            .read(world.resource::<Messages<KeepAliveEvent>>())
            .map(|event| (event.entity, event.id))
            .collect();
        assert_eq!(second.first(), Some(&(entities[1], 3)));
        assert_eq!(
            second.iter().filter(|event| event.0 == entities[0]).count(),
            256
        );
        drop(peers);
    }

    #[tokio::test]
    async fn readable_keepalive_survives_exhausted_tokio_cooperative_budget() {
        tokio::spawn(async {
            IoTaskPool::get_or_init(|| bevy_tasks::TaskPoolBuilder::new().num_threads(1).build());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let stream = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (mut peer, _) = listener.accept().await.unwrap();
            let (reader, writer) = stream.into_split();
            let packet = ClientboundGamePacket::KeepAlive(ClientboundKeepAlive { id: 73 });
            azalea_protocol::write::write_raw_packet(
                &serialize_packet(&packet).unwrap(),
                &mut peer,
                None,
                &mut None,
            )
            .await
            .unwrap();
            reader.readable().await.unwrap();

            let mut world = World::new();
            world.init_resource::<Messages<KeepAliveEvent>>();
            world.init_resource::<Messages<ReceiveGamePacketEvent>>();
            world.init_resource::<Messages<ReceiveConfigPacketEvent>>();
            world.init_resource::<Messages<ReceiveLoginPacketEvent>>();
            let entity = world
                .spawn(RawConnection::new(
                    RawReadConnection {
                        read_stream: reader,
                        buffer: Cursor::new(Vec::new()),
                        compression_threshold: None,
                        dec_cipher: None,
                    },
                    RawWriteConnection {
                        write_stream: writer,
                        compression_threshold: None,
                        enc_cipher: None,
                    },
                    ConnectionProtocol::Game,
                ))
                .id();

            let mut context = Context::from_waker(Waker::noop());
            for _ in 0..128 {
                if let Poll::Ready(budget) = tokio::task::coop::poll_proceed(&mut context) {
                    budget.made_progress();
                }
            }
            assert!(!tokio::task::coop::has_budget_remaining());
            read_packets(&mut world);
            let events = world.resource::<Messages<KeepAliveEvent>>();
            let mut cursor = events.get_cursor();
            let received = cursor.read(events).collect::<Vec<_>>();
            assert_eq!(
                received.len(),
                1,
                "a readable heartbeat was mistaken for an empty socket"
            );
            assert_eq!((received[0].entity, received[0].id), (entity, 73));
        })
        .await
        .unwrap();
    }
}
