use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DocsEntry {
    pub id: String,
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub modified_ts: i64,
    pub mime: String,
}
