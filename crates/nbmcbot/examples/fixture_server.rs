#[path = "../tests/support/mod.rs"]
#[allow(dead_code)]
mod support;
use clap::Parser;

#[derive(Parser)]
struct Options {
    /// Total accepted connections, including reconnections.
    #[arg(long, default_value_t=1, value_parser=clap::value_parser!(u16).range(1..=256))]
    clients: u16,
    #[arg(long, default_value_t=0, value_parser=clap::value_parser!(u8).range(0..=8))]
    chunk_radius: u8,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("{}", listener.local_addr()?);
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(support::serve_many(
        listener,
        sender,
        usize::from(options.clients),
        i32::from(options.chunk_radius),
    ));
    while let Some(support::NamedFixtureEvent { name, event }) = events.recv().await {
        eprintln!("bot={name} event={event:?}");
        match event {
            support::FixtureEvent::Handshake { protocol } => {
                eprintln!("handshake protocol={protocol}")
            }
            support::FixtureEvent::Login { name } => eprintln!("login name={name}"),
            support::FixtureEvent::Chat(message) => eprintln!("chat {message}"),
            support::FixtureEvent::Movement { x, y, z } => eprintln!("movement {x} {y} {z}"),
            support::FixtureEvent::Closed => eprintln!("closed"),
            support::FixtureEvent::Respawn => eprintln!("respawn requested"),
            support::FixtureEvent::HealthZero => eprintln!("health zero"),
            support::FixtureEvent::TeleportAccepted(id) => eprintln!("teleport accepted {id}"),
            support::FixtureEvent::SelectedHotbar(slot) => eprintln!("selected slot {slot}"),
            support::FixtureEvent::UsedMainHand => eprintln!("used main hand"),
            support::FixtureEvent::ReleasedUse => eprintln!("released use"),
            support::FixtureEvent::ViewDistance(distance) => eprintln!("view distance {distance}"),
            support::FixtureEvent::TotemSwap {
                source,
                target,
                is_swap,
            } => eprintln!("totem swap source={source} target={target} swap={is_swap}"),
            support::FixtureEvent::BlockBroken { x, y, z } => eprintln!("block broken {x} {y} {z}"),
            support::FixtureEvent::CombatAttack {
                target,
                elapsed_ms,
                selected_slot,
                empty_hand,
                distance,
            } => eprintln!(
                "combat attack target={target} elapsed_ms={elapsed_ms} selected_slot={selected_slot} empty_hand={empty_hand} distance={distance}"
            ),
            support::FixtureEvent::CombatSwing {
                main_hand,
                elapsed_ms,
            } => eprintln!("combat swing main_hand={main_hand} elapsed_ms={elapsed_ms}"),
        }
    }
    server.await??;
    Ok(())
}
