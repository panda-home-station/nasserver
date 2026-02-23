use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;
use domain::task::{CreateTaskReq, UpdateTaskReq};

pub struct TaskCommand;

#[async_trait]
impl Command for TaskCommand {
    fn name(&self) -> &str {
        "task"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             return Ok(("".to_string(), "task: missing subcommand\nUsage: task [list|create|update|clear]".to_string(), 1));
        }

        match args[0] {
            "list" => {
                match service.task_service.list_tasks().await {
                    Ok(tasks) => {
                        let mut output = String::from("ID      NAME                            TYPE       STATUS     PROGRESS\n");
                        for task in tasks {
                             output.push_str(&format!("{:<7} {:<31} {:<10} {:<10} {:>3}%\n", 
                                 &task.id.to_string()[..6], 
                                 &task.name.chars().take(30).collect::<String>(), 
                                 task.task_type, 
                                 task.status, 
                                 task.progress
                             ));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error listing tasks: {}", e), 1))
                }
            },
            "create" => {
                // task create <type> <name>
                if args.len() < 3 {
                    return Ok(("".to_string(), "task create: requires type and name".to_string(), 1));
                }
                let task_type = args[1];
                let name = args[2];
                let id = uuid::Uuid::new_v4().to_string();
                
                let req = CreateTaskReq {
                    id: id.clone(),
                    task_type: task_type.to_string(),
                    name: name.to_string(),
                    dir: Some(service.get_user_cwd()),
                    progress: 0,
                    status: "pending".to_string(),
                };
                
                match service.task_service.create_task(req).await {
                    Ok(_) => Ok((format!("Task created: {}\n", id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error creating task: {}", e), 1))
                }
            },
            "update" => {
                // task update <id> <status> [progress]
                if args.len() < 3 {
                     return Ok(("".to_string(), "task update: requires id and status".to_string(), 1));
                }
                let id = args[1];
                let status = args[2];
                let progress = if args.len() > 3 {
                    args[3].parse::<i32>().ok()
                } else {
                    None
                };
                
                let req = UpdateTaskReq {
                    status: Some(status.to_string()),
                    progress,
                };
                
                match service.task_service.update_task(id.to_string(), req).await {
                     Ok(_) => Ok((format!("Task {} updated\n", id), "".to_string(), 0)),
                     Err(e) => Ok(("".to_string(), format!("Error updating task: {}", e), 1))
                }
            },
            "clear" => {
                 match service.task_service.clear_completed_tasks().await {
                     Ok(_) => Ok(("Completed tasks cleared\n".to_string(), "".to_string(), 0)),
                     Err(e) => Ok(("".to_string(), format!("Error clearing tasks: {}", e), 1))
                 }
            },
            _ => Ok(("".to_string(), format!("task: unknown subcommand '{}'", args[0]), 1))
        }
    }
}