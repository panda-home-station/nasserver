use std::process::Command;

/// Abstraction for environment where command runs (Host vs Docker)
pub trait Sandbox: Send + Sync {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()>;
}

/// A simple No-Op sandbox that runs commands directly on host
pub struct NoopSandbox;

impl Sandbox for NoopSandbox {
    fn wrap_command(&self, _cmd: &mut Command) -> std::io::Result<()> {
        Ok(())
    }
}
