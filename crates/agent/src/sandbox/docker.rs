use crate::traits::Sandbox;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DockerSandbox {
    image: String,
}

impl Default for DockerSandbox {
    fn default() -> Self {
        Self {
            image: "alpine:latest".to_string(),
        }
    }
}

impl DockerSandbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_image(image: String) -> Self {
        Self { image }
    }
}

impl Sandbox for DockerSandbox {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()> {
        let program = cmd.get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        let mut docker_cmd = Command::new("docker");
        docker_cmd.args([
            "run",
            "--rm",
            "--network",
            "none", // Disable network by default for safety
            "-i",   // Interactive
        ]);
        docker_cmd.arg(&self.image);
        docker_cmd.arg(program);
        docker_cmd.args(args);

        *cmd = docker_cmd;
        Ok(())
    }
}
