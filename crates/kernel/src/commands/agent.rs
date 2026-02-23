use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;
use domain::dtos::agent::{TaskRequest, ChatRequest, ChatMessage};
use futures_util::StreamExt;

use uuid::Uuid;

pub struct AgentCommand;

#[async_trait]
impl Command for AgentCommand {
    fn name(&self) -> &str {
        "agent"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             return Ok(("".to_string(), "agent: missing subcommand\nUsage: agent [task|chat|session|search|exec]".to_string(), 1));
        }

        match args[0] {
            "task" => self.handle_task(service, &args[1..]).await,
            "chat" => self.handle_chat(service, &args[1..]).await,
            "session" => self.handle_session(service, &args[1..]).await,
            "search" => self.handle_search(service, &args[1..]).await,
            "exec" => self.handle_exec(service, &args[1..]).await,
            _ => Ok(("".to_string(), format!("agent: unknown subcommand '{}'", args[0]), 1))
        }
    }
}

impl AgentCommand {
    async fn handle_task(&self, service: &TerminalService, args: &[&str]) -> Result<(String, String, i32)> {
        let agent_service = match &service.agent_service {
            Some(s) => s,
            None => return Ok(("".to_string(), "Agent service not available".to_string(), 1)),
        };

        if args.is_empty() {
            return Ok(("".to_string(), "agent task: missing subcommand [create|get]".to_string(), 1));
        }
        match args[0] {
            "create" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "agent task create: requires query".to_string(), 1));
                }
                let query = args[1..].join(" ");
                let req = TaskRequest {
                    query,
                    config: None,
                };
                match agent_service.create_task(req).await {
                    Ok(resp) => {
                        let mut output = format!("Task Created: {}\nStatus: {}\nPlan:\n", resp.task_id, resp.status);
                        for step in resp.plan {
                            output.push_str(&format!("  - [{}] {}\n", step.status, step.description));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error creating task: {}", e), 1))
                }
            },
            "get" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "agent task get: requires task_id".to_string(), 1));
                }
                let task_id = args[1];
                match agent_service.get_task(task_id).await {
                     Ok(resp) => {
                        let mut output = format!("Task: {}\nStatus: {}\nPlan:\n", resp.task_id, resp.status);
                        for step in resp.plan {
                            output.push_str(&format!("  - [{}] {}\n", step.status, step.description));
                        }
                        output.push_str("Logs:\n");
                        for log in resp.logs {
                            output.push_str(&format!("  {}\n", log));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error getting task: {}", e), 1))
                }
            },
            _ => Ok(("".to_string(), format!("agent task: unknown subcommand '{}'", args[0]), 1))
        }
    }

    async fn handle_chat(&self, service: &TerminalService, args: &[&str]) -> Result<(String, String, i32)> {
        let agent_service = match &service.agent_service {
            Some(s) => s,
            None => return Ok(("".to_string(), "Agent service not available".to_string(), 1)),
        };

        if args.is_empty() {
            return Ok(("".to_string(), "agent chat: requires message or session_id".to_string(), 1));
        }
        
        // Simple heuristic: if first arg is a UUID, treat as session_id
        let (session_id, message) = if let Ok(uuid) = Uuid::parse_str(args[0]) {
            if args.len() < 2 {
                return Ok(("".to_string(), "agent chat <session_id>: requires message".to_string(), 1));
            }
            (Some(uuid), args[1..].join(" "))
        } else {
            (None, args.join(" "))
        };

        let req = ChatRequest {
            messages: vec![ChatMessage { role: "user".to_string(), content: message }],
            model: None,
            endpoint: None,
            session_id: session_id.map(|u| u.to_string()),
            user_id: None, // Service should infer or we can fetch from auth
            agent_id: None,
        };
        
        match agent_service.chat(req).await {
            Ok(mut stream) => {
                let mut output = String::new();
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(event) => {
                             // Format SSE event for terminal
                             output.push_str(&format!("{:?}\n", event));
                        },
                        Err(e) => {
                            output.push_str(&format!("Error in stream: {}\n", e));
                        }
                    }
                }
                Ok((output, "".to_string(), 0))
            },
            Err(e) => Ok(("".to_string(), format!("Error starting chat: {}", e), 1))
        }
    }

    async fn handle_session(&self, service: &TerminalService, args: &[&str]) -> Result<(String, String, i32)> {
        let agent_service = match &service.agent_service {
            Some(s) => s,
            None => return Ok(("".to_string(), "Agent service not available".to_string(), 1)),
        };

        if args.is_empty() {
            return Ok(("".to_string(), "agent session: missing subcommand [list|create|delete]".to_string(), 1));
        }

        // We need user_id. For now, let's assume we can look it up or use a placeholder if not available.
        let user_uuid = match Uuid::parse_str(&service.current_user) {
            Ok(uuid) => uuid,
            Err(_) => {
                 match service.auth_service.get_user_by_id(&service.current_user).await {
                     Ok(user_json) => {
                         if let Some(id_str) = user_json.get("id").and_then(|v| v.as_str()) {
                             match Uuid::parse_str(id_str) {
                                 Ok(u) => u,
                                 Err(_) => return Ok(("".to_string(), format!("Invalid UUID in user record: {}", id_str), 1)),
                             }
                         } else {
                             return Ok(("".to_string(), "User record missing 'id' field".to_string(), 1));
                         }
                     },
                     Err(e) => return Ok(("".to_string(), format!("Failed to resolve user '{}': {}", service.current_user, e), 1)),
                 }
            }
        };

        match args[0] {
            "list" => {
                match agent_service.list_sessions(user_uuid).await {
                    Ok(sessions) => {
                        let mut output = String::from("ID                                     TITLE\n");
                        for session in sessions {
                            output.push_str(&format!("{:<38} {}\n", session.id, session.title));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error listing sessions: {}", e), 1))
                }
            },
            "create" => {
                // usage: agent session create <agent_id> <title>
                if args.len() < 3 {
                    return Ok(("".to_string(), "agent session create: requires agent_id and title".to_string(), 1));
                }
                let agent_id = args[1].to_string();
                let title = args[2..].join(" ");
                match agent_service.create_session(user_uuid, agent_id, title).await {
                    Ok(session) => Ok((format!("Session created: {} ({})\n", session.id, session.title), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error creating session: {}", e), 1))
                }
            },
            "delete" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "agent session delete: requires session_id".to_string(), 1));
                }
                let session_id = match Uuid::parse_str(args[1]) {
                    Ok(id) => id,
                    Err(_) => return Ok(("".to_string(), "Invalid session UUID".to_string(), 1)),
                };
                match agent_service.delete_session(session_id).await {
                    Ok(_) => Ok((format!("Session {} deleted\n", session_id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error deleting session: {}", e), 1))
                }
            },
             _ => Ok(("".to_string(), format!("agent session: unknown subcommand '{}'", args[0]), 1))
        }
    }

    async fn handle_search(&self, service: &TerminalService, args: &[&str]) -> Result<(String, String, i32)> {
        let agent_service = match &service.agent_service {
            Some(s) => s,
            None => return Ok(("".to_string(), "Agent service not available".to_string(), 1)),
        };

        if args.is_empty() {
            return Ok(("".to_string(), "agent search: requires query".to_string(), 1));
        }
        let query = args.join(" ");
        match agent_service.search(&query).await {
            Ok(val) => {
                match serde_json::to_string_pretty(&val) {
                    Ok(s) => Ok((s, "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error formatting search result: {}", e), 1))
                }
            },
            Err(e) => Ok(("".to_string(), format!("Error searching: {}", e), 1))
        }
    }

    async fn handle_exec(&self, service: &TerminalService, args: &[&str]) -> Result<(String, String, i32)> {
        let agent_service = match &service.agent_service {
            Some(s) => s,
            None => return Ok(("".to_string(), "Agent service not available".to_string(), 1)),
        };

        if args.is_empty() {
            return Ok(("".to_string(), "agent exec: requires command string".to_string(), 1));
        }
        let command = args.join(" ");
        match agent_service.execute_command(None, None, command).await {
             Ok(val) => {
                match serde_json::to_string_pretty(&val) {
                    Ok(s) => Ok((s, "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error formatting exec result: {}", e), 1))
                }
            },
            Err(e) => Ok(("".to_string(), format!("Error executing command: {}", e), 1))
        }
    }
}
