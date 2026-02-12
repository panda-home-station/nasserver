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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DocsListResp {
    pub path: String,
    pub entries: Vec<DocsEntry>,
    pub has_more: bool,
    pub next_offset: i64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DocsListQuery {
    pub path: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DocsMkdirReq {
    pub path: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DocsRenameReq {
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DocsDownloadQuery {
    pub path: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DocsDeleteQuery {
    pub path: Option<String>,
}
