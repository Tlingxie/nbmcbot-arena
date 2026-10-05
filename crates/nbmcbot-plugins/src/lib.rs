//! Bounded source-port addon ABI. This does not load Java or Meteor JARs.
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};
use wasmi::{
    Caller, Config, Engine, Error, Linker, Module, Store, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

#[derive(Debug, Clone, PartialEq)]
pub enum PluginAction {
    Chat(String),
    Goto { x: i32, y: i32, z: i32 },
    Look { yaw: f32, pitch: f32 },
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fixture(source: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "engine-lifecycle-{}-{}.wat",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn engines_are_independent_and_released_on_unload_clear_and_runtime_drop() {
        let source = r#"(module (memory (export "memory") 1) (func (export "on_tick")))"#;
        let first = fixture(source);
        let second = fixture(source);
        let mut runtime = PluginRuntime::new(PluginLimits::default()).unwrap();
        let first_id = runtime.load(&first).unwrap();
        let second_id = runtime.load(&second).unwrap();
        let a = runtime.plugins[&first_id].store.engine().weak();
        let b = runtime.plugins[&second_id].store.engine().weak();
        assert!(!Engine::same(
            runtime.plugins[&first_id].store.engine(),
            runtime.plugins[&second_id].store.engine()
        ));
        assert!(runtime.unload(&first_id));
        assert!(a.upgrade().is_none());
        assert!(b.upgrade().is_some());
        runtime.clear();
        assert!(b.upgrade().is_none());
        let id = runtime.load(&first).unwrap();
        let c = runtime.plugins[&id].store.engine().weak();
        drop(runtime);
        assert!(c.upgrade().is_none());
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }

    #[test]
    fn failed_loads_do_not_keep_or_contaminate_live_engines() {
        let valid = fixture(r#"(module (memory (export "memory") 1) (func (export "on_tick")))"#);
        let invalid =
            fixture(r#"(module (memory (export "memory") 1) (func (export "wrong_callback")))"#);
        let mut runtime = PluginRuntime::new(PluginLimits::default()).unwrap();
        let id = runtime.load(&valid).unwrap();
        let live = runtime.plugins[&id].store.engine().weak();
        for _ in 0..64 {
            assert!(runtime.load(&invalid).is_err());
            assert!(
                runtime
                    .last_load_engine
                    .as_ref()
                    .unwrap()
                    .upgrade()
                    .is_none()
            );
        }
        assert_eq!(runtime.list().len(), 1);
        assert!(runtime.unload(&id));
        assert!(live.upgrade().is_none());
        assert!(runtime.load(&valid).is_ok());
        std::fs::remove_file(valid).unwrap();
        std::fs::remove_file(invalid).unwrap();
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct BotSnapshot {
    pub tick: u64,
    pub health: f32,
    pub position: [f64; 3],
    pub connected: bool,
}
#[derive(Debug, Clone)]
pub struct PluginLimits {
    pub max_plugins: usize,
    pub max_file_bytes: usize,
    pub max_total_module_bytes: usize,
    pub max_memory_bytes: usize,
    pub max_total_memory_bytes: usize,
    pub max_table_elements: usize,
    pub max_recursion_depth: usize,
    pub max_stack_height: usize,
    pub fuel_per_tick: u64,
    pub max_actions_per_tick: usize,
    pub max_message_bytes: usize,
}
impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            max_plugins: 8,
            max_file_bytes: 262144,
            max_total_module_bytes: 1048576,
            max_memory_bytes: 1048576,
            max_total_memory_bytes: 8388608,
            max_table_elements: 1024,
            max_recursion_depth: 64,
            max_stack_height: 4096,
            fuel_per_tick: 100000,
            max_actions_per_tick: 16,
            max_message_bytes: 256,
        }
    }
}
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub module_bytes: usize,
    pub last_error: Option<String>,
}
struct Host {
    limits: StoreLimits,
    snapshot: BotSnapshot,
    actions: Vec<PluginAction>,
    cap: usize,
    message_cap: usize,
}
struct Plugin {
    info: PluginInfo,
    store: Store<Host>,
    callback: TypedFunc<(), ()>,
}
pub struct PluginRuntime {
    config: Config,
    limits: PluginLimits,
    plugins: BTreeMap<String, Plugin>,
    #[cfg(test)]
    last_load_engine: Option<wasmi::EngineWeak>,
}
fn err(message: impl Into<String>) -> Error {
    Error::new(message)
}
fn charge(caller: &mut Caller<'_, Host>, cost: u64) -> Result<(), Error> {
    let remaining = caller
        .get_fuel()?
        .checked_sub(cost)
        .ok_or_else(|| err("host fuel exhausted"))?;
    caller.set_fuel(remaining)
}
fn push(caller: &mut Caller<'_, Host>, action: PluginAction) -> Result<(), Error> {
    charge(caller, 20)?;
    if caller.data().actions.len() >= caller.data().cap {
        return Err(err("action queue limit"));
    }
    caller.data_mut().actions.push(action);
    Ok(())
}
impl PluginRuntime {
    pub fn new(limits: PluginLimits) -> Result<Self, String> {
        if limits.max_stack_height < 1
            || limits.max_recursion_depth < 1
            || limits.max_file_bytes == usize::MAX
            || limits.max_message_bytes > 65536
            || limits.max_actions_per_tick > 4096
            || limits.max_plugins > 128
        {
            return Err("invalid plugin limits".into());
        }
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .allow_start_fn(false)
            .ignore_custom_sections(true)
            .wasm_multi_memory(false)
            .set_min_stack_height(1)
            .set_max_stack_height(limits.max_stack_height)
            .set_max_recursion_depth(limits.max_recursion_depth)
            .set_max_cached_stacks(0)
            .enforced_limits(wasmi::EnforcedLimits::strict());
        Ok(Self {
            config,
            limits,
            plugins: BTreeMap::new(),
            #[cfg(test)]
            last_load_engine: None,
        })
    }
    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<String, String> {
        let path = path.as_ref();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("jar") => {
                return Err("Java/Meteor JARs require a source port to the Wasm addon ABI; load a .wat or .wasm file".into());
            }
            Some("wat" | "wasm") => {}
            _ => return Err("plugin must be a .wat or .wasm file".into()),
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid plugin id")?
            .to_owned();
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid plugin id".into());
        }
        if self.plugins.contains_key(&id) {
            return Err("plugin already loaded".into());
        }
        let count = self.plugins.len().checked_add(1).ok_or("count overflow")?;
        if count > self.limits.max_plugins
            || count
                .checked_mul(self.limits.max_memory_bytes)
                .ok_or("memory overflow")?
                > self.limits.max_total_memory_bytes
        {
            return Err("aggregate plugin limit".into());
        }
        let file = File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > self.limits.max_file_bytes as u64 {
            return Err("plugin file too large".into());
        }
        let mut bytes = Vec::new();
        file.take(self.limits.max_file_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > self.limits.max_file_bytes {
            return Err("plugin file too large".into());
        }
        let total = self
            .plugins
            .values()
            .try_fold(bytes.len(), |sum, p| sum.checked_add(p.info.module_bytes))
            .ok_or("module overflow")?;
        if total > self.limits.max_total_module_bytes {
            return Err("aggregate module limit".into());
        }
        // Engine code storage is append-only. Keep it scoped to one plugin so
        // unloading or failing admission also releases translated code.
        let engine = Engine::new(&self.config);
        #[cfg(test)]
        {
            self.last_load_engine = Some(engine.weak());
        }
        let module = Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
        let mut linker = Linker::<Host>::new(&engine);
        linker
            .func_wrap(
                "nbmcbot",
                "chat",
                |mut c: Caller<'_, Host>, ptr: i32, len: i32| -> Result<(), Error> {
                    let len = usize::try_from(len).map_err(|_| err("negative message length"))?;
                    let ptr = usize::try_from(ptr).map_err(|_| err("negative message pointer"))?;
                    if len > c.data().message_cap || c.data().actions.len() >= c.data().cap {
                        return Err(err("message/action limit"));
                    }
                    charge(&mut c, len as u64)?;
                    let memory = c
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| err("missing memory"))?;
                    let data = memory
                        .data(&c)
                        .get(
                            ptr..ptr
                                .checked_add(len)
                                .ok_or_else(|| err("pointer overflow"))?,
                        )
                        .ok_or_else(|| err("message outside memory"))?;
                    let message = std::str::from_utf8(data)
                        .map_err(|_| err("message must be UTF-8"))?
                        .to_owned();
                    if message.contains(['\n', '\r', '\0']) {
                        return Err(err("invalid chat character"));
                    }
                    push(&mut c, PluginAction::Chat(message))
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap(
                "nbmcbot",
                "goto",
                |mut c: Caller<'_, Host>, x: i32, y: i32, z: i32| {
                    if !(-30_000_000..=30_000_000).contains(&x)
                        || !(-2048..=2048).contains(&y)
                        || !(-30_000_000..=30_000_000).contains(&z)
                    {
                        return Err(err("navigation coordinate out of range"));
                    }
                    push(&mut c, PluginAction::Goto { x, y, z })
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap(
                "nbmcbot",
                "look",
                |mut c: Caller<'_, Host>, yaw: f32, pitch: f32| -> Result<(), Error> {
                    if !yaw.is_finite() || !pitch.is_finite() || !(-90.0..=90.0).contains(&pitch) {
                        return Err(err("invalid look"));
                    }
                    push(&mut c, PluginAction::Look { yaw, pitch })
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap("nbmcbot", "tick", |c: Caller<'_, Host>| {
                c.data().snapshot.tick as i64
            })
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap("nbmcbot", "health", |c: Caller<'_, Host>| {
                c.data().snapshot.health
            })
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap(
                "nbmcbot",
                "position",
                |c: Caller<'_, Host>, axis: i32| -> Result<f64, Error> {
                    c.data()
                        .snapshot
                        .position
                        .get(axis as usize)
                        .copied()
                        .ok_or_else(|| err("invalid position axis"))
                },
            )
            .map_err(|e| e.to_string())?;
        let host = Host {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .table_elements(self.limits.max_table_elements)
                .instances(1)
                .memories(1)
                .tables(1)
                .trap_on_grow_failure(true)
                .build(),
            snapshot: BotSnapshot::default(),
            actions: Vec::new(),
            cap: self.limits.max_actions_per_tick,
            message_cap: self.limits.max_message_bytes,
        };
        let mut store = Store::new(&engine, host);
        store.limiter(|h| &mut h.limits);
        store
            .set_fuel(self.limits.fuel_per_tick)
            .map_err(|e| e.to_string())?;
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| e.to_string())?;
        if instance.get_memory(&store, "memory").is_none() {
            return Err("missing exported memory".into());
        }
        let callback = instance
            .get_typed_func::<(), ()>(&store, "on_tick")
            .map_err(|e| e.to_string())?;
        self.plugins.insert(
            id.clone(),
            Plugin {
                info: PluginInfo {
                    id: id.clone(),
                    module_bytes: bytes.len(),
                    last_error: None,
                },
                store,
                callback,
            },
        );
        Ok(id)
    }
    pub fn unload(&mut self, id: &str) -> bool {
        self.plugins.remove(id).is_some()
    }
    pub fn clear(&mut self) {
        self.plugins.clear();
    }
    pub fn list(&self) -> Vec<PluginInfo> {
        self.plugins.values().map(|p| p.info.clone()).collect()
    }
    pub fn tick(&mut self, snapshot: BotSnapshot) -> Vec<PluginAction> {
        let mut result = Vec::new();
        for plugin in self.plugins.values_mut() {
            plugin.store.data_mut().actions.clear();
            if !snapshot.connected {
                continue;
            }
            plugin.store.data_mut().snapshot = snapshot;
            let outcome = plugin
                .store
                .set_fuel(self.limits.fuel_per_tick)
                .and_then(|()| plugin.callback.call(&mut plugin.store, ()));
            match outcome {
                Ok(()) => {
                    plugin.info.last_error = None;
                    result.append(&mut plugin.store.data_mut().actions);
                }
                Err(e) => {
                    plugin.info.last_error = Some(e.to_string());
                    plugin.store.data_mut().actions.clear();
                }
            }
        }
        result
    }
}
