use async_trait::async_trait;
use crate::models::domain::app::{App, AppStatus};
use crate::core::Result;

#[async_trait]
pub trait AppManager: Send + Sync {
    async fn list_apps(&self) -> Result<Vec<App>>;
    #[allow(dead_code)]
    async fn get_app(&self, id: &str) -> Result<App>;
    async fn install_app(&self, app_config: App) -> Result<()>;
    async fn uninstall_app(&self, id: &str) -> Result<()>;
    async fn start_app(&self, id: &str) -> Result<()>;
    async fn stop_app(&self, id: &str) -> Result<()>;
    #[allow(dead_code)]
    async fn get_app_status(&self, id: &str) -> Result<AppStatus>;
}
