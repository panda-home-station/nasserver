use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct DocsListQuery {
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct DocsMkdirReq {
    pub path: String,
}

#[derive(Deserialize)]
pub struct DocsRenameReq {
    pub from: Option<String>,
    pub to: Option<String>,
    pub id: Option<String>,
    pub new_name: Option<String>,
    pub new_dir: Option<String>,
}

#[derive(Deserialize)]
pub struct DocsDownloadQuery {
    pub id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct DocsDeleteQuery {
    pub id: Option<String>,
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct DocsEntry {
    pub id: String,
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub modified_ts: i64,
}

#[derive(Serialize)]
pub struct DocsListResp {
    pub path: String,
    pub entries: Vec<DocsEntry>,
}
