use crate::{Tool, ToolContext};
use async_trait::async_trait;
use loci_core::error::{AppError, Result};
use serde_json::{json, Value};
use tokio::process::Command;

/// Patterns that require explicit confirmation before execution
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm ",
    "rm\t",
    "rmdir",
    "dd ",
    "mkfs",
    "format",
    "DROP ",
    "DELETE FROM",
    "TRUNCATE",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "chmod 777",
    "chmod -R 777",
    "> /dev/",
    "| sudo",
];

pub struct ShellExec;

#[async_trait]
impl Tool for ShellExec {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout/stderr. \
         Dangerous commands (rm, DROP, shutdown, etc.) are blocked unless pre-approved."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run" },
                "timeout_secs": { "type": "integer", "default": 30 },
                "confirmed": { "type": "boolean", "default": false,
                    "description": "Set true to confirm execution of dangerous commands" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<Value> {
        let cmd = params["command"]
            .as_str()
            .ok_or_else(|| AppError::Tool("missing command".into()))?;
        let confirmed = params["confirmed"].as_bool().unwrap_or(false);

        // Safety check
        let cmd_upper = cmd.to_uppercase();
        let dangerous = DANGEROUS_PATTERNS
            .iter()
            .any(|p| cmd_upper.contains(&p.to_uppercase()));
        if dangerous && !confirmed {
            return Ok(json!({
                "blocked": true,
                "reason": format!("Command '{}' matches dangerous pattern. Set confirmed=true to proceed.", cmd),
                "command": cmd
            }));
        }

        // Audit log
        let log_entry = format!("[{}] EXEC: {}\n", chrono::Utc::now().to_rfc3339(), cmd);
        if let Some(dir) = &ctx.working_dir {
            let log_path = std::path::Path::new(dir).join(".bs/audit.log");
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map(|mut f| {
                    use std::io::Write;
                    let _ = f.write_all(log_entry.as_bytes());
                });
        }

        let timeout = params["timeout_secs"].as_u64().unwrap_or(30);
        let mut builder = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.arg("/C").arg(cmd);
            command
        } else {
            let mut command = Command::new("sh");
            command.arg("-c").arg(cmd);
            command
        };
        if let Some(dir) = &ctx.working_dir {
            builder.current_dir(dir);
        }

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(timeout), builder.output())
                .await
                .map_err(|_| AppError::Tool(format!("command timed out after {}s", timeout)))?
                .map_err(|e| AppError::Tool(e.to_string()))?;

        Ok(json!({
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "exit_code": output.status.code(),
            "confirmed": confirmed
        }))
    }
}
