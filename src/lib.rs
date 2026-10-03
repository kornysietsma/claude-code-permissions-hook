//! tool-gate-hook: a PreToolUse hook that gates agent tool use.

pub mod agent;
pub mod auditing;
pub mod config;
pub mod hook_io;
pub mod policy;

pub use agent::Agent;
pub use config::Config;
