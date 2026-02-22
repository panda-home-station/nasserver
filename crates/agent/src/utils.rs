use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

pub fn append_chat_log(session_id: &str, content: String) {
    let session_id = session_id.to_string();
    tokio::task::spawn_blocking(move || {
        let log_dir = "logs/chats";
        if !Path::new(log_dir).exists() {
            let _ = std::fs::create_dir_all(log_dir);
        }

        let file_path = format!("{}/{}.log", log_dir, session_id);
        
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path) 
        {
            let _ = writeln!(file, "[{}] {}", timestamp, content);
        }
    });
}
