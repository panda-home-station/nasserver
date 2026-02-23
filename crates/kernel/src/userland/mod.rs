pub const LS_JS: &str = include_str!("ls.js");
pub const CAT_JS: &str = include_str!("cat.js");
pub const MKDIR_JS: &str = include_str!("mkdir.js");
pub const RM_JS: &str = include_str!("rm.js");
pub const TOUCH_JS: &str = include_str!("touch.js");
pub const CP_JS: &str = include_str!("cp.js");
pub const MV_JS: &str = include_str!("mv.js");

pub const COMMANDS: &[&str] = &["ls", "cat", "mkdir", "rm", "touch", "cp", "mv"];

pub fn get_script(name: &str) -> Option<&'static str> {
    match name {
        "ls" => Some(LS_JS),
        "cat" => Some(CAT_JS),
        "mkdir" => Some(MKDIR_JS),
        "rm" => Some(RM_JS),
        "touch" => Some(TOUCH_JS),
        "cp" => Some(CP_JS),
        "mv" => Some(MV_JS),
        _ => None,
    }
}

pub async fn install_userland(storage: std::sync::Arc<dyn domain::storage::StorageService>) -> crate::error::Result<()> {
    // Ensure /bin exists
    // We use "admin" as the user for system operations
    let _ = storage.mkdir("admin", domain::dtos::docs::DocsMkdirReq { path: "/bin".to_string() }).await;

    for cmd in COMMANDS {
        if let Some(script) = get_script(cmd) {
            // Check if file exists to avoid overwriting (or maybe we should overwrite to update?)
            // For now, let's overwrite to ensure latest version
            let _ = storage.save_file(
                "admin", 
                "/bin", 
                cmd, 
                bytes::Bytes::from(script.as_bytes().to_vec())
            ).await;
        }
    }
    Ok(())
}
