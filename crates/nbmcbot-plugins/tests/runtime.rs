use nbmcbot_plugins::{BotSnapshot, PluginAction, PluginLimits, PluginRuntime};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixture(text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "plugin-{}-{}.wat",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, text).unwrap();
    path
}
fn snapshot() -> BotSnapshot {
    BotSnapshot {
        tick: 1,
        health: 20.0,
        position: [0.0; 3],
        connected: true,
    }
}
#[test]
fn real_actions_and_unload() {
    let path = fixture(
        r#"(module (import "nbmcbot" "chat" (func $chat (param i32 i32))) (import "nbmcbot" "goto" (func $goto (param i32 i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "hello") (func (export "on_tick") i32.const 0 i32.const 5 call $chat i32.const 1 i32.const 2 i32.const 3 call $goto))"#,
    );
    let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
    let id = rt.load(&path).unwrap();
    assert_eq!(
        rt.tick(snapshot()),
        vec![
            PluginAction::Chat("hello".into()),
            PluginAction::Goto { x: 1, y: 2, z: 3 }
        ]
    );
    assert!(rt.unload(&id));
    assert!(rt.tick(snapshot()).is_empty());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn traps_are_atomic_and_bounded() {
    for body in [
        "(loop br 0)",
        "i32.const 2 memory.grow drop",
        "i32.const 0 i32.const 1 call $chat i32.const 0 i32.const 1 call $chat",
        "unreachable",
    ] {
        let path = fixture(&format!(
            r#"(module (import "nbmcbot" "chat" (func $chat (param i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "x") (func (export "on_tick") i32.const 0 i32.const 1 call $chat {body}))"#
        ));
        let limits = PluginLimits {
            max_memory_bytes: 65536,
            max_actions_per_tick: 2,
            fuel_per_tick: 1000,
            ..PluginLimits::default()
        };
        let mut rt = PluginRuntime::new(limits).unwrap();
        rt.load(&path).unwrap();
        assert!(rt.tick(snapshot()).is_empty());
        assert!(rt.list()[0].last_error.is_some());
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn reject_oversize_and_unknown_import() {
    let mut rt = PluginRuntime::new(PluginLimits {
        max_file_bytes: 64,
        ..PluginLimits::default()
    })
    .unwrap();
    let path = fixture(&"x".repeat(65));
    assert!(rt.load(&path).is_err());
    std::fs::remove_file(path).unwrap();
    let path = fixture(
        r#"(module (import "bad" "bad" (func)) (memory (export "memory") 1) (func (export "on_tick")))"#,
    );
    let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
    assert!(rt.load(&path).is_err());
    assert!(rt.list().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn aggregate_admission_and_cleanup() {
    let source = r#"(module (memory (export "memory") 1) (func (export "on_tick")))"#;
    let first = fixture(source);
    let second = fixture(source);
    let limits = PluginLimits {
        max_memory_bytes: 65536,
        max_total_memory_bytes: 65536,
        ..PluginLimits::default()
    };
    let mut rt = PluginRuntime::new(limits).unwrap();
    let id = rt.load(&first).unwrap();
    assert!(rt.load(&second).is_err());
    assert!(rt.unload(&id));
    assert!(rt.load(&second).is_ok());
    rt.clear();
    assert!(rt.list().is_empty());
    let mut rt = PluginRuntime::new(PluginLimits {
        max_total_module_bytes: source.len(),
        ..PluginLimits::default()
    })
    .unwrap();
    rt.load(&first).unwrap();
    assert!(rt.load(&second).is_err());
    std::fs::remove_file(first).unwrap();
    std::fs::remove_file(second).unwrap();
}

#[test]
fn disconnect_skips_execution_and_reconnect_resumes() {
    let path = fixture(
        r#"(module (import "nbmcbot" "look" (func $look (param f32 f32))) (memory (export "memory") 1) (global $yaw (mut f32) (f32.const 0)) (func (export "on_tick") global.get $yaw f32.const 1 f32.add global.set $yaw global.get $yaw f32.const 0 call $look))"#,
    );
    let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
    rt.load(&path).unwrap();
    assert_eq!(
        rt.tick(snapshot()),
        vec![PluginAction::Look {
            yaw: 1.0,
            pitch: 0.0
        }]
    );
    assert!(
        rt.tick(BotSnapshot {
            connected: false,
            ..snapshot()
        })
        .is_empty()
    );
    assert_eq!(
        rt.tick(snapshot()),
        vec![PluginAction::Look {
            yaw: 2.0,
            pitch: 0.0
        }]
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn recursion_table_growth_memory_count_and_start_are_bounded() {
    for source in [
        r#"(module (memory (export "memory") 1) (func $f (export "on_tick") call $f))"#,
        r#"(module (memory (export "memory") 1) (table 1 funcref) (func (export "on_tick") ref.null func i32.const 100 table.grow drop))"#,
    ] {
        let path = fixture(source);
        let mut rt = PluginRuntime::new(PluginLimits {
            max_recursion_depth: 8,
            max_table_elements: 1,
            ..PluginLimits::default()
        })
        .unwrap();
        rt.load(&path).unwrap();
        assert!(rt.tick(snapshot()).is_empty());
        assert!(rt.list()[0].last_error.is_some());
        std::fs::remove_file(path).unwrap();
    }
    for source in [
        r#"(module (memory (export "memory") 1) (memory 1) (func (export "on_tick")))"#,
        r#"(module (memory (export "memory") 1) (func $start) (start $start) (func (export "on_tick")))"#,
    ] {
        let path = fixture(source);
        let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
        assert!(rt.load(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn runnable_anti_idle_example() {
    let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
    rt.load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/plugins/anti-idle.wat"
    ))
    .unwrap();
    assert!(rt.tick(snapshot()).is_empty());
    assert_eq!(
        rt.tick(BotSnapshot {
            tick: 200,
            ..snapshot()
        }),
        vec![PluginAction::Look {
            yaw: 200.0,
            pitch: 0.0
        }]
    );
}

#[test]
fn invalid_navigation_discards_prior_actions() {
    for (x, y, z) in [
        (30000001, 0, 0),
        (0, 2049, 0),
        (0, 0, -30000001),
        (i32::MIN, 0, 0),
    ] {
        let path = fixture(&format!(
            r#"(module (import "nbmcbot" "goto" (func $goto (param i32 i32 i32))) (memory (export "memory") 1) (func (export "on_tick") i32.const 1 i32.const 2 i32.const 3 call $goto i32.const {x} i32.const {y} i32.const {z} call $goto))"#
        ));
        let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
        rt.load(&path).unwrap();
        assert!(rt.tick(snapshot()).is_empty());
        assert!(rt.list()[0].last_error.is_some());
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn jar_rejection_explains_source_port_requirement() {
    let path = std::env::temp_dir().join("unsupported-plugin.jar");
    let mut rt = PluginRuntime::new(PluginLimits::default()).unwrap();
    let error = rt.load(path).unwrap_err();
    assert!(error.contains("source") && error.contains("Wasm"));
}
