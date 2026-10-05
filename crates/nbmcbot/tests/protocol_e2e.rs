#[allow(dead_code)]
mod support;
use anyhow::Context;

use std::{process::Stdio, time::Duration};
use support::FixtureEvent;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    time::timeout,
};

async fn stdout_event(
    output: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    expected: &str,
) -> anyhow::Result<serde_json::Value> {
    loop {
        let event = output
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("CLI exited before {expected}"))?;
        if event["event"] == "error" || event["event"] == "command_error" {
            anyhow::bail!("CLI error: {event}");
        }
        if event["event"] == expected {
            return Ok(event);
        }
    }
}

#[tokio::test]
async fn offline_login_chat_navigation_and_quit_over_real_tcp() -> anyhow::Result<()> {
    timeout(Duration::from_secs(40), async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let fixture = tokio::spawn(support::serve(listener, events_tx));
        let mut child = Command::new(env!("CARGO_BIN_EXE_nbmcbot"))
            .args([
                "run",
                "--server",
                &address.to_string(),
                "--username",
                "TestBot",
                "--auth",
                "offline",
                "--no-reconnect",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
        let stderr_reader = tokio::spawn(async move {
            let mut captured = String::new();
            while let Some(line) = stderr.next_line().await? {
                eprintln!("cli stderr {line}");
                captured.push_str(&line);
                captured.push('\n');
            }
            Ok::<_, std::io::Error>(captured)
        });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (output_tx, mut output) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                let event: serde_json::Value = serde_json::from_str(&line)?;
                eprintln!("cli {event}");
                if output_tx.send(event).is_err() { break; }
            }
            anyhow::Ok(())
        });
        loop {
            let event = output
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("CLI exited before spawn event"))?;
            if event["event"] == "spawn" {
                let position = event["position"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("spawn must report its position"))?;
                assert_eq!(position.len(), 3);
                for (actual, expected) in position.iter().zip([4.5, 64.0, 4.5]) {
                    assert!((actual.as_f64().unwrap() - expected).abs() < 0.01);
                }
                break;
            }
        }
        stdin.write_all(b"say e2e-hello\ngoto 7 64 4\nstatus\n").await?;
        let (mut handshake, mut login, mut chat, mut moved) = (false, false, false, false);
        while !(handshake && login && chat && moved) {
            match events
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("fixture closed before expected effects"))?
            {
                FixtureEvent::Handshake { protocol } => {
                    assert_eq!(protocol, 774);
                    handshake = true;
                }
                FixtureEvent::Login { name } => {
                    assert_eq!(name, "TestBot");
                    login = true;
                }
                FixtureEvent::Chat(message) => {
                    assert_eq!(message, "e2e-hello");
                    chat = true;
                }
                FixtureEvent::Movement { x, y, z } => {
                    if (7.0..8.0).contains(&x) && (y - 64.0).abs() < 0.1 && (4.0..5.0).contains(&z) {
                        moved = true;
                    }
                }
                FixtureEvent::Closed => anyhow::bail!("client disconnected early"),
                FixtureEvent::BlockBroken { x, y, z } => {
                    anyhow::bail!("unexpected block destruction at {x} {y} {z}")
                }
                FixtureEvent::Respawn | FixtureEvent::HealthZero => anyhow::bail!("unexpected death during navigation"),
                FixtureEvent::TeleportAccepted(_) | FixtureEvent::SelectedHotbar(_) | FixtureEvent::UsedMainHand | FixtureEvent::ReleasedUse | FixtureEvent::ViewDistance(_) => {},
                FixtureEvent::TotemSwap { .. } => anyhow::bail!("unexpected inventory swap before enabling module"),
                FixtureEvent::CombatAttack { .. } | FixtureEvent::CombatSwing { .. } => anyhow::bail!("unexpected combat during navigation"),
            }
        }
        let finished = stdout_event(&mut output, "task_finished").await?;
        assert_eq!(finished["task"], "goto");
        stdin.write_all(b"mine 8 63 4\n").await?;
        loop {
            match events.recv().await.context("fixture closed during mining")? {
                FixtureEvent::BlockBroken { x, y, z } => { assert_eq!((x,y,z), (8,63,4)); break; }
                FixtureEvent::Closed => anyhow::bail!("disconnected during mining"),
                _ => {}
            }
        }
        let directory = tempfile::tempdir()?;
        let plugin = directory.path().join("e2e-chat.wat");
        std::fs::write(&plugin, r#"(module
            (import "nbmcbot" "chat" (func $chat (param i32 i32)))
            (memory (export "memory") 1 1)
            (data (i32.const 0) "plugin-e2e")
            (global $sent (mut i32) (i32.const 0))
            (func (export "on_tick")
                global.get $sent i32.eqz
                if i32.const 0 i32.const 10 call $chat i32.const 1 global.set $sent end))"#)?;
        stdin.write_all(format!("plugin load {}\n", plugin.display()).as_bytes()).await?;
        let loaded = stdout_event(&mut output, "plugin_loaded").await?;
        let id = loaded["id"].as_str().context("missing plugin id")?;
        loop {
            match events.recv().await.context("fixture closed during plugin action")? {
                FixtureEvent::Chat(message) if message == "plugin-e2e" => break,
                FixtureEvent::Closed => anyhow::bail!("disconnected during plugin action"),
                _ => {}
            }
        }
        stdin.write_all(format!("plugin unload {id}\nmodule auto-respawn off\nmodules\n").as_bytes()).await?;
        stdout_event(&mut output, "plugin_unloaded").await?;
        let changed = stdout_event(&mut output, "module_changed").await?;
        assert_eq!(changed["name"], "auto-respawn");
        assert_eq!(changed["enabled"], false);
        let listed = stdout_event(&mut output, "modules").await?;
        assert!(listed["modules"].as_array().unwrap().iter().any(|entry| entry[0] == "auto-respawn" && entry[1] == false));
        stdin.write_all(b"say fixture-die\n").await?;
        while !matches!(events.recv().await.context("fixture closed before death")?, FixtureEvent::HealthZero) {}
        let quiet = tokio::time::sleep(Duration::from_millis(250));
        tokio::pin!(quiet);
        loop {
            tokio::select! {
                _ = &mut quiet => break,
                event = events.recv() => {
                    anyhow::ensure!(!matches!(event, Some(FixtureEvent::Respawn)), "disabled module sent respawn");
                    anyhow::ensure!(event.is_some(), "fixture closed during disabled module check");
                }
            }
        }
        stdin.write_all(b"module auto-respawn on\n").await?;
        let changed = stdout_event(&mut output, "module_changed").await?;
        assert_eq!(changed["enabled"], true);
        while !matches!(events.recv().await.context("fixture closed before respawn")?, FixtureEvent::Respawn) {}
        while !matches!(events.recv().await.context("fixture closed before respawn teleport ack")?, FixtureEvent::TeleportAccepted(2)) {}
        stdin.write_all(b"status\n").await?;
        let respawned = stdout_event(&mut output, "status").await?;
        assert_eq!(respawned["position"], serde_json::json!([4.5,64.0,4.5]));
        assert_eq!(respawned["health"], 20.0);
        stdin.write_all(b"module auto-eat on\nsay fixture-food\n").await?;
        let mut order = Vec::new();
        loop {
            match events.recv().await.context("fixture closed during eating")? {
                FixtureEvent::SelectedHotbar(2) => order.push("select food"),
                FixtureEvent::UsedMainHand => order.push("use"),
                FixtureEvent::ReleasedUse => order.push("release"),
                FixtureEvent::SelectedHotbar(0) if !order.is_empty() => { order.push("restore"); break; }
                FixtureEvent::Closed => anyhow::bail!("disconnected during eating"),
                _ => {},
            }
        }
        assert_eq!(order, ["select food", "use", "release", "restore"]);
        stdin.write_all(b"module auto-eat off\n").await?;
        stdin.write_all(b"module auto-totem on\nsay fixture-totem\n").await?;
        loop {
            match events.recv().await.context("fixture closed during totem swap")? {
                FixtureEvent::TotemSwap { source, target, is_swap } => { assert_eq!((source,target,is_swap), (10,40,true)); break; }
                FixtureEvent::Closed => anyhow::bail!("disconnected during totem swap"),
                _ => {},
            }
        }
        let mut confirmed = false;
        let mut last_inventory = serde_json::Value::Null;
        for _ in 0..20 {
            stdin.write_all(b"inventory\n").await?;
            last_inventory = stdout_event(&mut output, "inventory").await?;
            let totems: Vec<_> = last_inventory["items"].as_array().unwrap().iter().filter(|item| item["item"] == "TotemOfUndying").collect();
            if totems.len() == 1 && totems[0]["slot"] == 45 && totems[0]["count"] == 1 {
                confirmed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(confirmed, "totem confirmation must leave exactly one totem in offhand, with no slot40 ghost: {last_inventory}");
        stdin.write_all(b"quit\n").await?;
        assert!(child.wait().await?.success());
        let stderr = stderr_reader.await??;
        assert!(!stderr.contains("panicked"), "CLI panicked despite its exit status: {stderr}");
        reader.await??;
        fixture.await??;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn disconnect_and_immediate_manual_reconnect_restore_session() -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (sender, mut events) = mpsc::unbounded_channel();
        let fixture = tokio::spawn(support::serve_sessions(listener, sender, 2));
        let mut child = Command::new(env!("CARGO_BIN_EXE_nbmcbot"))
            .args([
                "run",
                "--server",
                &address.to_string(),
                "--username",
                "ReconnectBot",
                "--auth",
                "offline",
                "--no-reconnect",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (sender, mut output) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                let event: serde_json::Value = serde_json::from_str(&line)?;
                eprintln!("reconnect cli {event}");
                if sender.send(event).is_err() {
                    break;
                }
            }
            anyhow::Ok(())
        });
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut text = String::new();
            stderr.read_to_string(&mut text).await?;
            Ok::<_, std::io::Error>(text)
        });
        stdout_event(&mut output, "spawn").await?;
        stdin.write_all(b"disconnect\nreconnect\n").await?;
        stdout_event(&mut output, "disconnected").await?;
        let spawn = stdout_event(&mut output, "spawn").await?;
        assert_eq!(spawn["position"], serde_json::json!([4.5, 64.0, 4.5]));
        stdin.write_all(b"say second-session\n").await?;
        let (mut handshakes, mut logins, mut view_distance, mut chat) = (0, 0, false, false);
        while handshakes != 2 || logins != 2 || !view_distance || !chat {
            match events
                .recv()
                .await
                .context("fixture stopped during reconnect")?
            {
                FixtureEvent::Handshake { protocol } => {
                    assert_eq!(protocol, 774);
                    handshakes += 1;
                }
                FixtureEvent::Login { name } => {
                    assert_eq!(name, "ReconnectBot");
                    logins += 1;
                }
                FixtureEvent::ViewDistance(distance) if handshakes == 2 => {
                    assert_eq!(distance, 2);
                    view_distance = true;
                }
                FixtureEvent::Chat(message) if handshakes == 2 => {
                    assert_eq!(message, "second-session");
                    chat = true;
                }
                _ => {}
            }
        }
        stdin.write_all(b"quit\n").await?;
        assert!(child.wait().await?.success());
        let errors = errors.await??;
        assert!(!errors.contains("panicked"), "CLI panic: {errors}");
        reader.await??;
        fixture.await??;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
