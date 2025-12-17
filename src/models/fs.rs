use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct FsListQuery {
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct FsDeleteQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct FsRenameReq {
    pub from: String,
    pub to: String,
}

#[derive(Deserialize)]
pub struct FsDownloadQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct FsMkdirReq {
    pub path: String,
}

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ts: i64,
}

#[derive(Serialize)]
pub struct FsListResp {
    pub base: String,
    pub path: String,
    pub entries: Vec<FsEntry>,
}
