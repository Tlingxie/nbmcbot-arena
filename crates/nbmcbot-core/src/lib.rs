use std::{
    fmt, io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Status,
    Quit,
    Disconnect,
    Reconnect,
    Stop,
    Chat(String),
    Goto { x: i32, y: i32, z: i32 },
    Follow(String),
    Mine { x: i32, y: i32, z: i32 },
    Look { yaw: f32, pitch: f32 },
    Inventory,
    Attack(String),
    Fight(String),
    Duel { style: String, enemy_prefix: String },
    Modules,
    Module { name: String, enabled: bool },
    Plugins,
    PluginLoad(PathBuf),
    PluginUnload(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError(pub String);
impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CommandError {}

pub fn parse_command(input: &str) -> Result<Command, CommandError> {
    let input = input.trim();
    let mut words = input.split_whitespace();
    let verb = words
        .next()
        .ok_or_else(|| CommandError("command is empty; use help".into()))?;
    let args: Vec<_> = words.collect();
    let error = || CommandError(format!("invalid arguments for {verb}; use help"));
    let coordinate = |index: usize, limit: i32| -> Result<i32, CommandError> {
        let value = args
            .get(index)
            .ok_or_else(error)?
            .parse::<i32>()
            .map_err(|_| error())?;
        if !(-limit..=limit).contains(&value) {
            return Err(CommandError(format!(
                "coordinate {} must be within +/-{limit}",
                index + 1
            )));
        }
        Ok(value)
    };
    match (verb, args.as_slice()) {
        ("help", []) => Ok(Command::Help),
        ("status", []) => Ok(Command::Status),
        ("quit", []) => Ok(Command::Quit),
        ("disconnect", []) => Ok(Command::Disconnect),
        ("reconnect", []) => Ok(Command::Reconnect),
        ("stop", []) => Ok(Command::Stop),
        ("inventory", []) => Ok(Command::Inventory),
        ("modules", []) => Ok(Command::Modules),
        ("plugins", []) => Ok(Command::Plugins),
        ("say", [_, ..]) => Ok(Command::Chat(input[verb.len()..].trim().to_owned())),
        ("goto" | "mine", [_, _, _]) => {
            let (x, y, z) = (
                coordinate(0, 30_000_000)?,
                coordinate(1, 2048)?,
                coordinate(2, 30_000_000)?,
            );
            Ok(if verb == "goto" {
                Command::Goto { x, y, z }
            } else {
                Command::Mine { x, y, z }
            })
        }
        ("follow", [name]) => Ok(Command::Follow((*name).into())),
        ("attack", [name]) => Ok(Command::Attack((*name).into())),
        ("fight", [name]) => Ok(Command::Fight((*name).into())),
        ("duel", [style @ ("mace" | "spear"), enemy_prefix])
            if (1..=16).contains(&enemy_prefix.len())
                && enemy_prefix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
        {
            Ok(Command::Duel {
                style: (*style).into(),
                enemy_prefix: (*enemy_prefix).into(),
            })
        }
        ("look", [yaw, pitch]) => {
            let yaw = yaw.parse::<f32>().map_err(|_| error())?;
            let pitch = pitch.parse::<f32>().map_err(|_| error())?;
            if !yaw.is_finite() || !pitch.is_finite() || !(-90.0..=90.0).contains(&pitch) {
                return Err(CommandError(
                    "look requires finite yaw and pitch within -90..90".into(),
                ));
            }
            Ok(Command::Look { yaw, pitch })
        }
        ("module", [name, enabled @ ("on" | "off")]) => Ok(Command::Module {
            name: (*name).into(),
            enabled: *enabled == "on",
        }),
        ("plugin", ["load", path]) => Ok(Command::PluginLoad(PathBuf::from(path))),
        ("plugin", ["unload", name]) => Ok(Command::PluginUnload((*name).into())),
        _ => Err(error()),
    }
}

/// Tracks explicit reservations, independently of OS resident memory.
#[derive(Debug, Clone)]
pub struct MemoryBudget(Arc<BudgetState>);
#[derive(Debug)]
struct BudgetState {
    limit: usize,
    used: AtomicUsize,
}
#[derive(Debug)]
pub struct MemoryLease {
    budget: MemoryBudget,
    bytes: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryError {
    pub requested: usize,
    pub used: usize,
    pub limit: usize,
}
impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot reserve {} bytes: {} of {} bytes already reserved",
            self.requested, self.used, self.limit
        )
    }
}
impl std::error::Error for MemoryError {}
impl MemoryBudget {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(BudgetState {
            limit,
            used: AtomicUsize::new(0),
        }))
    }
    pub fn reserve(&self, bytes: usize) -> Result<MemoryLease, MemoryError> {
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&next| next <= self.0.limit)
            })
            .map_err(|used| MemoryError {
                requested: bytes,
                used,
                limit: self.0.limit,
            })?;
        Ok(MemoryLease {
            budget: self.clone(),
            bytes,
        })
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }
    pub fn limit(&self) -> usize {
        self.0.limit
    }
    pub fn remaining(&self) -> usize {
        self.limit() - self.used()
    }
}
impl MemoryLease {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}
impl Drop for MemoryLease {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemory {
    pub rss_bytes: u64,
    pub peak_rss_bytes: u64,
}

#[cfg(target_os = "linux")]
pub fn process_memory() -> io::Result<ProcessMemory> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let field = |name: &str| -> io::Result<u64> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|kb| kb.checked_mul(1024))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("missing or invalid {name}"),
                )
            })
    };
    Ok(ProcessMemory {
        rss_bytes: field("VmRSS:")?,
        peak_rss_bytes: field("VmHWM:")?,
    })
}

#[cfg(target_os = "macos")]
pub fn process_memory() -> io::Result<ProcessMemory> {
    // proc_pidinfo reports current resident bytes; getrusage reports peak bytes on Darwin.
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>();
        let result = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size as i32,
        );
        if result != size as i32 {
            return Err(io::Error::last_os_error());
        }
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ProcessMemory {
            rss_bytes: info.pti_resident_size,
            peak_rss_bytes: (usage.ru_maxrss as u64).max(info.pti_resident_size),
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_memory() -> io::Result<ProcessMemory> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "RSS sampling supports macOS and Linux",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duel_accepts_weapon_styles_and_minecraft_name_prefixes() {
        for input in [
            "duel mace Spear_",
            "duel spear Mace01",
            "duel mace A",
            "duel spear abcdefghijklmnop",
        ] {
            let words: Vec<_> = input.split_whitespace().collect();
            assert_eq!(
                parse_command(input).unwrap(),
                Command::Duel {
                    style: words[1].into(),
                    enemy_prefix: words[2].into(),
                },
                "{input}"
            );
        }
    }

    #[test]
    fn duel_rejects_invalid_styles_prefixes_and_argument_counts() {
        for input in [
            "duel",
            "duel mace",
            "duel mace Enemy extra",
            "duel sword Enemy",
            "duel MACE Enemy",
            "duel mace *",
            "duel mace Enemy-",
            "duel mace Enemy.",
            "duel mace abcdefghijklmnopq",
            "duel mace \u{4eba}",
        ] {
            assert!(parse_command(input).is_err(), "{input}");
        }
    }

    #[test]
    fn parses_every_command_shape() {
        let examples = [
            ("help", Command::Help),
            ("status", Command::Status),
            ("quit", Command::Quit),
            ("disconnect", Command::Disconnect),
            ("reconnect", Command::Reconnect),
            ("stop", Command::Stop),
            ("follow Alex", Command::Follow("Alex".into())),
            ("attack Zombie", Command::Attack("Zombie".into())),
            ("fight nearest", Command::Fight("nearest".into())),
            ("mine 1 2 3", Command::Mine { x: 1, y: 2, z: 3 }),
            (
                "look -180 -90",
                Command::Look {
                    yaw: -180.0,
                    pitch: -90.0,
                },
            ),
            ("inventory", Command::Inventory),
            ("modules", Command::Modules),
            (
                "module guard on",
                Command::Module {
                    name: "guard".into(),
                    enabled: true,
                },
            ),
            (
                "module guard off",
                Command::Module {
                    name: "guard".into(),
                    enabled: false,
                },
            ),
            ("plugins", Command::Plugins),
            (
                "plugin load test.wasm",
                Command::PluginLoad("test.wasm".into()),
            ),
            ("plugin unload test", Command::PluginUnload("test".into())),
        ];
        for (input, expected) in examples {
            assert_eq!(parse_command(input).unwrap(), expected);
        }
    }
    #[test]
    fn maximum_budget_does_not_wrap() {
        let budget = MemoryBudget::new(usize::MAX);
        let lease = budget.reserve(usize::MAX).unwrap();
        assert!(budget.reserve(1).is_err());
        assert_eq!(budget.used(), usize::MAX);
        let moved = lease;
        drop(moved);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn command_boundaries() {
        assert_eq!(
            parse_command("say hello world").unwrap(),
            Command::Chat("hello world".into())
        );
        assert_eq!(
            parse_command("goto -30000000 2048 30000000").unwrap(),
            Command::Goto {
                x: -30000000,
                y: 2048,
                z: 30000000
            }
        );
        for input in [
            "",
            "say",
            "goto 1 2",
            "goto 30000001 0 0",
            "mine 0 -2049 0",
            "look NaN 0",
            "look inf 0",
            "look 0 91",
            "status extra",
            "module foo maybe",
            "plugin unload",
            "follow a b",
            "fight",
            "fight one two",
        ] {
            assert!(parse_command(input).is_err(), "{input}");
        }
    }
    #[test]
    fn leases_release_and_fail_without_charging() {
        let budget = MemoryBudget::new(10);
        let lease = budget.reserve(7).unwrap();
        assert_eq!(budget.used(), 7);
        assert!(budget.reserve(4).is_err());
        assert!(budget.reserve(usize::MAX).is_err());
        assert_eq!(budget.remaining(), 3);
        drop(lease);
        assert_eq!(budget.used(), 0);
        drop(budget.reserve(0).unwrap());
        assert_eq!(budget.limit(), 10);
        assert!(MemoryBudget::new(0).reserve(1).is_err());
    }
    #[test]
    fn concurrent_reservations_are_bounded() {
        let budget = MemoryBudget::new(100);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let budget = budget.clone();
                scope.spawn(move || {
                    for _ in 0..1000 {
                        if let Ok(lease) = budget.reserve(30) {
                            assert!(budget.used() <= 100);
                            drop(lease);
                        }
                    }
                });
            }
        });
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn samples_actual_process_memory() {
        let memory = process_memory().unwrap();
        assert!(memory.rss_bytes > 0);
        assert!(memory.peak_rss_bytes >= memory.rss_bytes);
        eprintln!(
            "process RSS={} bytes, peak={} bytes",
            memory.rss_bytes, memory.peak_rss_bytes
        );
    }
}
