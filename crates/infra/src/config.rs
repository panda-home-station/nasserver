use std::io::{BufRead, BufReader};
use std::fs::File;

pub fn read_env_var_from_file(var_name: &str) -> Result<String, std::io::Error> {
    let file = File::open(".env")?;
    let reader = BufReader::new(file);
    
    for line in reader.lines() {
        let line = line?;
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        
        if let Some(pos) = line.find('=') {
            let key = line[..pos].trim();
            if key == var_name {
                let value = line[pos + 1..].trim();
                let value = if value.starts_with('"') && value.ends_with('"') && value.len() > 1 {
                    &value[1..value.len() - 1]
                } else if value.starts_with('\'') && value.ends_with('\'') && value.len() > 1 {
                    &value[1..value.len() - 1]
                } else {
                    value
                };
                return Ok(value.to_string());
            }
        }
    }
    
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("Variable {} not found in .env file", var_name)))
}
