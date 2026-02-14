use serde::{Deserialize, Serialize};
use reqwest::Client;
use domain::{Result, Error};

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
}

pub struct SearchService {
    client: Client,
}

impl SearchService {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        // Using a simple DuckDuckGo search (no JS)
        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding::encode(query));
        
        let resp = self.client.get(url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Search request failed: {}", e)))?;

        let html = resp.text().await
            .map_err(|e| Error::Internal(format!("Failed to read search response: {}", e)))?;

        // Simple parsing of DuckDuckGo HTML
        // Note: This is a fragile way to parse HTML, but for a simple implementation it works.
        // In a real scenario, use a proper HTML parser like scraper.
        let mut results = Vec::new();
        
        // Very basic extraction logic
        let parts: Vec<&str> = html.split("<div class=\"result__body\">").collect();
        for part in parts.iter().skip(1).take(5) {
            let title = self.extract_tag(part, "result__a", ">", "</a>");
            let link = self.extract_tag(part, "result__a", "href=\"", "\"");
            let snippet = self.extract_tag(part, "result__snippet", ">", "</a>");

            if !title.is_empty() && !link.is_empty() {
                results.push(SearchResult {
                    title: self.clean_html(&title),
                    link: self.clean_html(&link),
                    snippet: self.clean_html(&snippet),
                });
            }
        }

        Ok(results)
    }

    fn extract_tag(&self, content: &str, class: &str, start_after: &str, end_before: &str) -> String {
        if let Some(pos) = content.find(class) {
            let sub = &content[pos..];
            if let Some(start_pos) = sub.find(start_after) {
                let start = start_pos + start_after.len();
                if let Some(end_pos) = sub[start..].find(end_before) {
                    return sub[start..start + end_pos].to_string();
                }
            }
        }
        String::new()
    }

    fn clean_html(&self, html: &str) -> String {
        // Basic HTML entity decoding and tag removal
        let mut cleaned = html.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'");
        
        // Remove any remaining tags
        while let Some(start) = cleaned.find('<') {
            if let Some(end) = cleaned[start..].find('>') {
                cleaned.replace_range(start..start + end + 1, "");
            } else {
                break;
            }
        }
        
        cleaned.trim().to_string()
    }
}
