#[allow(dead_code)]
mod support;
use anyhow::Context;
use std::{process::Stdio, time::Duration};
use support::FixtureEvent;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    time::timeout,
};

async fn output_event(
    rx: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    kind: &str,
) -> anyhow::Result<serde_json::Value> {
    loop {
        let value = rx.recv().await.context("CLI exited")?;
        anyhow::ensure!(
            !matches!(
                value["event"].as_str(),
                Some("error" | "command_error" | "task_error")
            ),
            "CLI error: {value}"
        );
        if value["event"] == kind {
            return Ok(value);
        }
    }
}

#[tokio::test]
async fn fight_redirects_mid_chase_instead_of_finishing_old_path() -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (sender, mut events) = mpsc::unbounded_channel();
        let server = tokio::spawn(support::serve(listener, sender));
        let mut child = Command::new(env!("CARGO_BIN_EXE_nbmcbot"))
            .args(["run", "--server", &address.to_string(), "--username", "RedirectBot", "--auth", "offline", "--no-reconnect"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move { let mut text = String::new(); stderr.read_to_string(&mut text).await?; Ok::<_, std::io::Error>(text) });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (sender, mut output) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                if sender.send(serde_json::from_str::<serde_json::Value>(&line)?).is_err() { break; }
            }
            anyhow::Ok(())
        });
        output_event(&mut output, "spawn").await?;
        while !matches!(events.recv().await.context("fixture closed before teleport")?, FixtureEvent::TeleportAccepted(1)) {}
        stdin.write_all(b"fight SparringPlayer\nsay fixture-fighter\n").await?;
        let redirected_at = loop {
            match events.recv().await.context("fixture closed before chase")? {
                FixtureEvent::Movement { x, .. } if x >= 6.0 => break x,
                FixtureEvent::CombatAttack { .. } => anyhow::bail!("old target was reached before redirect stimulus"),
                _ => {}
            }
        };
        stdin.write_all(b"say fixture-target-turn\n").await?;
        fixture_chat(&mut events, "fixture-target-turn").await?;
        let mut max_x = redirected_at;
        let mut turned = false;
        loop {
            match events.recv().await.context("fixture closed during redirect")? {
                FixtureEvent::Movement { x, z, .. } => {
                    max_x = max_x.max(x);
                    turned |= z > 6.0;
                    // Allow a full ten-tick update interval and braking inertia,
                    // but reject completing the old eastward path near x=10.5.
                    anyhow::ensure!(max_x < 9.5, "bot kept following old path after target turned: start={redirected_at}, max_x={max_x}, z={z}");
                }
                FixtureEvent::CombatAttack { empty_hand, distance, .. } => {
                    assert!(empty_hand); assert!(distance <= 3.2); assert!(turned, "did not follow target around the turn"); break;
                }
                FixtureEvent::Closed => anyhow::bail!("disconnected during redirect"),
                _ => {}
            }
        }
        eprintln!("redirect start_x={redirected_at} max_x={max_x}, reached turned target");
        stdin.write_all(b"stop\nquit\n").await?;
        assert!(child.wait().await?.success());
        let errors = errors.await??; assert!(!errors.contains("panicked"), "{errors}");
        reader.await??; server.await??; anyhow::Ok(())
    }).await??;
    Ok(())
}

async fn fixture_chat(
    events: &mut mpsc::UnboundedReceiver<FixtureEvent>,
    text: &str,
) -> anyhow::Result<()> {
    loop {
        match events
            .recv()
            .await
            .context("fixture closed before chat barrier")?
        {
            FixtureEvent::Chat(message) if message == text => return Ok(()),
            FixtureEvent::Closed => anyhow::bail!("client disconnected before chat barrier"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn fight_waits_chases_uses_empty_hand_respects_cooldown_and_stops() -> anyhow::Result<()> {
    timeout(Duration::from_secs(40),async{
        let listener=TcpListener::bind("127.0.0.1:0").await?;let address=listener.local_addr()?;
        let(tx,mut events)=mpsc::unbounded_channel();let server=tokio::spawn(support::serve(listener,tx));
        let mut child=Command::new(env!("CARGO_BIN_EXE_nbmcbot")).args(["run","--server",&address.to_string(),"--username","CombatBot","--auth","offline","--no-reconnect"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
        let mut stdin=child.stdin.take().unwrap();let mut stderr=child.stderr.take().unwrap();
        let errors=tokio::spawn(async move{let mut text=String::new();stderr.read_to_string(&mut text).await?;Ok::<_,std::io::Error>(text)});
        let mut lines=BufReader::new(child.stdout.take().unwrap()).lines();let(tx,mut output)=mpsc::unbounded_channel();
        let reader=tokio::spawn(async move{while let Some(line)=lines.next_line().await?{let value:serde_json::Value=serde_json::from_str(&line)?;if tx.send(value).is_err(){break;}}anyhow::Ok(())});
        output_event(&mut output,"spawn").await?;
        stdin.write_all(b"fight SparringPlayer\nstatus\n").await?;
        let waiting=output_event(&mut output,"status").await?;assert_eq!(waiting["task"],"fight");
        let quiet=tokio::time::sleep(Duration::from_millis(300));tokio::pin!(quiet);
        loop{tokio::select!{_=&mut quiet=>break,event=events.recv()=>{anyhow::ensure!(event.is_some(),"fixture closed");assert!(!matches!(event,Some(FixtureEvent::CombatAttack{..})),"attacked before player appeared");}}}
        stdin.write_all(b"say fixture-weapon\nsay fixture-fighter\n").await?;
        let mut times=Vec::new();let mut swings=0;let mut selected=false;let mut chased=false;
        while times.len()<6 || swings<6{
            match events.recv().await.context("fixture closed during combat")?{
                FixtureEvent::Movement{x,..} if x>7.0=>chased=true,
                FixtureEvent::SelectedHotbar(slot) if slot!=0=>selected=true,
                FixtureEvent::CombatAttack{target,elapsed_ms,selected_slot,empty_hand,distance}=>{
                    assert_eq!(target,10000);assert!(selected,"must synchronize empty slot before attack");assert_ne!(selected_slot,0);assert!(empty_hand,"weapon was held");assert!(distance<=3.2,"attacked out of reach: {distance}");times.push(elapsed_ms);
                }
                FixtureEvent::CombatSwing{main_hand,..}=>{assert!(main_hand);swings+=1;},
                FixtureEvent::Closed=>anyhow::bail!("bot disconnected during combat"),_=>{}
            }
        }
        assert!(chased,"bot must chase target eight blocks away");
        for pair in times.windows(2){assert!(pair[1]-pair[0]>=150,"server attacks arrived too quickly: {times:?}");}
        let mut ticks=Vec::new();while ticks.len()<6{let attack=output_event(&mut output,"combat_attack").await?;assert_eq!(attack["empty_hand"],true);assert_eq!(attack["cooldown_remaining_ticks"],0);ticks.push(attack["tick"].as_u64().context("combat_attack missing tick")?);}
        for pair in ticks.windows(2){assert!(pair[1]-pair[0]>=5,"client cooldown less than five ticks: {ticks:?}");}
        eprintln!("combat server_attack_ms={times:?} client_attack_ticks={ticks:?}");
        stdin.write_all(b"say fixture-target-far\n").await?;
        fixture_chat(&mut events,"fixture-target-far").await?;
        let mut chased_again = false;
        loop {
            match events.recv().await.context("fixture closed after target movement")? {
                FixtureEvent::Movement{x,..} if x>10.5 => chased_again=true,
                FixtureEvent::CombatAttack{empty_hand,distance,..} => {
                    assert!(empty_hand); assert!(distance<=3.2,"attacked after target moved out of reach");
                    assert!(chased_again,"target moved away but bot did not chase again"); break;
                }
                _ => {}
            }
        }
        stdin.write_all(b"stop\nsay combat-stop-barrier\n").await?;
        fixture_chat(&mut events,"combat-stop-barrier").await?;
        let quiet=tokio::time::sleep(Duration::from_millis(600));tokio::pin!(quiet);
        loop{tokio::select!{_=&mut quiet=>break,event=events.recv()=>{anyhow::ensure!(event.is_some(),"fixture closed after stop");assert!(!matches!(event,Some(FixtureEvent::CombatAttack{..})),"attack continued after stop");}}}
        stdin.write_all(b"quit\n").await?;assert!(child.wait().await?.success());let errors=errors.await??;assert!(!errors.contains("panicked"),"{errors}");reader.await??;server.await??;anyhow::Ok(())
    }).await??;
    Ok(())
}
