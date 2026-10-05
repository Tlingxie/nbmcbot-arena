#[allow(dead_code)]
mod support;
use anyhow::Context;
use std::{collections::HashSet, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    time::timeout,
};

async fn event(
    output: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    kind: &str,
    bot: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    loop {
        let value = output
            .recv()
            .await
            .context("swarm exited before expected event")?;
        anyhow::ensure!(
            !matches!(
                value["event"].as_str(),
                Some("error" | "command_error" | "task_error" | "plugin_error")
            ),
            "swarm error: {value}"
        );
        if value["event"] == kind && bot.is_none_or(|bot| value["bot"] == bot) {
            return Ok(value);
        }
    }
}

#[tokio::test]
async fn two_accounts_share_chunks_and_isolate_disconnect_reconnect() -> anyhow::Result<()> {
    timeout(Duration::from_secs(60), async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (sender, mut events) = mpsc::unbounded_channel();
        let fixture = tokio::spawn(support::serve_sessions(listener, sender, 3));
        let mut child = Command::new(env!("CARGO_BIN_EXE_nbmcbot"))
            .args([
                "swarm",
                "--server",
                &address.to_string(),
                "--usernames",
                "Bot01,Bot02",
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
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            stderr.read_to_string(&mut text).await?;
            Ok::<_, std::io::Error>(text)
        });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (tx, mut output) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                let value: serde_json::Value = serde_json::from_str(&line)?;
                eprintln!("swarm {value}");
                if tx.send(value).is_err() {
                    break;
                }
            }
            anyhow::Ok(())
        });
        let mut spawned = HashSet::new();
        while spawned.len() < 2 {
            let value = event(&mut output, "spawn", None).await?;
            spawned.insert(
                value["bot"]
                    .as_str()
                    .context("spawn missing bot identity")?
                    .to_owned(),
            );
        }
        assert_eq!(
            spawned,
            HashSet::from(["Bot01".to_owned(), "Bot02".to_owned()])
        );
        stdin.write_all(b"status\n").await?;
        let shared = event(&mut output, "swarm_status", None).await?;
        assert_eq!(shared["connected"], 2);
        assert_eq!(shared["unique_worlds"], 1);
        assert!(shared["unique_chunks"].as_u64().unwrap() > 0);
        assert!(
            shared["unique_chunks"].as_u64().unwrap()
                < shared["chunk_references"].as_u64().unwrap()
        );
        stdin.write_all(b"@Bot01 disconnect\n").await?;
        event(&mut output, "disconnected", Some("Bot01")).await?;
        let directory = tempfile::tempdir()?;
        let plugin = directory.path().join("swarm-action.wat");
        std::fs::write(
            &plugin,
            r#"(module
            (import "nbmcbot" "chat" (func $chat (param i32 i32)))
            (memory (export "memory") 1 1)
            (data (i32.const 0) "swarm-plugin")
            (global $sent (mut i32) (i32.const 0))
            (func (export "on_tick") global.get $sent i32.eqz
                if i32.const 0 i32.const 12 call $chat i32.const 1 global.set $sent end))"#,
        )?;
        stdin
            .write_all(
                format!(
                    "@Bot02 goto 7 64 4\n@Bot02 plugin load {}\n",
                    plugin.display()
                )
                .as_bytes(),
            )
            .await?;
        // The load acknowledgement precedes the navigation completion.
        event(&mut output, "plugin_loaded", Some("Bot02")).await?;
        loop {
            match events
                .recv()
                .await
                .context("fixture closed before plugin action")?
            {
                support::FixtureEvent::Chat(message) if message == "swarm-plugin" => break,
                _ => {}
            }
        }
        let finished = event(&mut output, "task_finished", Some("Bot02")).await?;
        assert_eq!(finished["task"], "goto");
        stdin.write_all(b"@Bot02 status\n").await?;
        let arrived = event(&mut output, "status", Some("Bot02")).await?;
        assert_eq!(arrived["connected"], true);
        let position = arrived["position"]
            .as_array()
            .context("status missing position")?;
        assert_eq!(
            position
                .iter()
                .map(|value| value.as_f64().unwrap().floor() as i32)
                .collect::<Vec<_>>(),
            [7, 64, 4]
        );
        stdin.write_all(b"@Bot01 reconnect\n").await?;
        event(&mut output, "spawn", Some("Bot01")).await?;
        stdin.write_all(b"status\n").await?;
        let rejoined = event(&mut output, "swarm_status", None).await?;
        assert_eq!(rejoined["connected"], 2);
        assert_eq!(rejoined["unique_worlds"], 1);
        assert!(
            rejoined["unique_chunks"].as_u64().unwrap()
                < rejoined["chunk_references"].as_u64().unwrap()
        );
        stdin
            .write_all(b"@Bot02 plugin unload swarm-action\nquit\n")
            .await?;
        assert!(child.wait().await?.success());
        let errors = errors.await??;
        assert!(!errors.contains("panicked"), "swarm panic: {errors}");
        reader.await??;
        fixture.await??;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
