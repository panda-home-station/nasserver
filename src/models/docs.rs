use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct DocsListQuery {
    pub path: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Deserialize, Debug)]
pub struct DocsMkdirReq {
    pub path: String,
}

#[derive(Deserialize, Debug)]
pub struct DocsRenameReq {
    pub from: Option<String>,
    pub to: Option<String>,
    pub id: Option<String>,
    pub new_name: Option<String>,
    pub new_dir: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct DocsDownloadQuery {
    pub id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize, Debug)]
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
    pub mime: String,
}

#[derive(Serialize)]
pub struct DocsListResp {
    pub path: String,
    pub entries: Vec<DocsEntry>,
    pub has_more: bool,
    pub next_offset: i64,
}
