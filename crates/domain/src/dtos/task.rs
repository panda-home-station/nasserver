use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct CreateTaskReq {
    pub id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub name: String,
    pub dir: Option<String>,
    pub progress: i32,
    pub status: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateTaskReq {
    pub progress: Option<i32>,
    pub status: Option<String>,
}
