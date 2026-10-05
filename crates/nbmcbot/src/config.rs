use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: String,
    pub username: String,
    pub auth: String,
    pub view_distance: u8,
    pub memory_limit_mb: u32,
    pub reconnect_delay_secs: u64,
    pub max_reconnect_attempts: u32,
    pub plugins: Vec<String>,
    pub modules: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: "127.0.0.1:25565".into(),
            username: "NBMCBot".into(),
            auth: "offline".into(),
            view_distance: 2,
            memory_limit_mb: 256,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
            plugins: Vec::new(),
            modules: Vec::new(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(self.auth.as_str(), "offline" | "microsoft"),
            "auth must be offline or microsoft"
        );
        anyhow::ensure!(
            !self.username.is_empty()
                && self.username.len() <= 254
                && !self.username.chars().any(char::is_whitespace),
            "username must be nonempty and contain no whitespace"
        );
        if self.auth == "offline" {
            anyhow::ensure!(
                self.username.len() <= 16
                    && self
                        .username
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "offline username must contain 1-16 ASCII letters, digits or underscores"
            );
        }
        anyhow::ensure!(
            !self.server.is_empty()
                && self.server.len() <= 255
                && !self.server.chars().any(char::is_whitespace),
            "server must contain 1-255 bytes without whitespace"
        );
        anyhow::ensure!(
            (2..=32).contains(&self.view_distance),
            "view_distance must be 2..32"
        );
        anyhow::ensure!(
            (32..=256).contains(&self.memory_limit_mb),
            "memory_limit_mb must be 32..256"
        );
        anyhow::ensure!(
            self.max_reconnect_attempts <= 100,
            "max_reconnect_attempts must be <=100"
        );
        anyhow::ensure!(
            (1..=300).contains(&self.reconnect_delay_secs),
            "reconnect_delay_secs must be 1..300"
        );
        anyhow::ensure!(
            self.plugins.len() <= 8,
            "at most eight plugins are supported"
        );
        let mut seen = std::collections::HashSet::new();
        for module in &self.modules {
            anyhow::ensure!(
                matches!(
                    module.as_str(),
                    "anti-idle" | "auto-eat" | "auto-totem" | "auto-respawn"
                ),
                "unknown startup module: {module}"
            );
            anyhow::ensure!(seen.insert(module), "duplicate startup module: {module}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn startup_modules_require_known_unique_names() {
        assert!(
            Config {
                modules: vec!["unknown".into()],
                ..Config::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                modules: vec!["anti-idle".into(), "anti-idle".into()],
                ..Config::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                modules: ["anti-idle", "auto-eat", "auto-totem", "auto-respawn"]
                    .map(String::from)
                    .to_vec(),
                ..Config::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            toml::from_str::<Config>("modules = ['anti-idle', 'auto-eat']")
                .unwrap()
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn rejects_malformed_offline_identity_and_auth() {
        for username in ["", "a name", "this_name_is_far_too_long", "bad/name"] {
            let config = Config {
                username: username.into(),
                ..Config::default()
            };
            assert!(config.validate().is_err(), "accepted {username:?}");
        }
        let config = Config {
            auth: "unknown".into(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn prevents_memory_and_view_distance_overcommit() {
        for memory_limit_mb in [0, 257, u32::MAX] {
            let config = Config {
                memory_limit_mb,
                ..Config::default()
            };
            assert!(config.validate().is_err());
        }
        for view_distance in [0, 1, 33, 255] {
            let config = Config {
                view_distance,
                ..Config::default()
            };
            assert!(config.validate().is_err());
        }
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn rejects_configuration_typos() {
        assert!(toml::from_str::<Config>("view_distnace = 2").is_err());
    }

    #[test]
    fn rejects_server_and_retry_limits() {
        for server in ["", "host name", "host\n"] {
            assert!(
                Config {
                    server: server.into(),
                    ..Config::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Config {
                server: "x".repeat(256),
                ..Config::default()
            }
            .validate()
            .is_err()
        );
        for reconnect_delay_secs in [0, 301, u64::MAX] {
            assert!(
                Config {
                    reconnect_delay_secs,
                    ..Config::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Config {
                max_reconnect_attempts: 101,
                ..Config::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                plugins: vec!["x".into(); 9],
                ..Config::default()
            }
            .validate()
            .is_err()
        );
    }
}
