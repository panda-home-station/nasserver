use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;

pub struct EchoCommand;

#[async_trait]
impl Command for EchoCommand {
    fn name(&self) -> &str {
        "echo"
    }

    async fn execute(&self, _service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut no_newline = false;
        let mut enable_escapes = false;
        let mut processed_args = Vec::new();

        for arg in args {
            if *arg == "-n" {
                no_newline = true;
            } else if *arg == "-e" {
                enable_escapes = true;
            } else {
                processed_args.push(*arg);
            }
        }

        let mut output = processed_args.join(" ");

        if enable_escapes {
            output = output.replace("\\n", "\n")
                           .replace("\\t", "\t")
                           .replace("\\r", "\r")
                           .replace("\\\\", "\\")
                           .replace("\\\"", "\"");
        }

        if !no_newline {
            output.push('\n');
        }

        Ok((output, "".to_string(), 0))
    }
}
