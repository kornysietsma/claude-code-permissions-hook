use clap::ValueEnum;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Copilot,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Copilot => "copilot",
        }
    }

    pub fn default_config_path(self, home: &Path) -> PathBuf {
        home.join(".config")
            .join("tool-gate-hook")
            .join(format!("{}.toml", self.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn default_config_path_is_per_agent_under_home() {
        let home = Path::new("/home/someone");
        assert_eq!(
            Agent::Copilot.default_config_path(home),
            PathBuf::from("/home/someone/.config/tool-gate-hook/copilot.toml")
        );
    }
}
