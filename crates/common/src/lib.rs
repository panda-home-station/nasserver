pub mod core;

use std::collections::HashMap;
use std::sync::Mutex;
use chrono::Utc;
use once_cell::sync::Lazy;

pub static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
pub static START_TIME: Lazy<chrono::DateTime<Utc>> = Lazy::new(|| Utc::now());
