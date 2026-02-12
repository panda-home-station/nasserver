use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct IdReq {
    pub id: String,
}
