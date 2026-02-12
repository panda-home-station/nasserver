use async_trait::async_trait;
use crate::Result;
pub use crate::entities::container::{ContainerInfo, ImageInfo, VolumeInfo, NetworkInfo, NetworkIpam, NetworkIpamConfig};
pub use crate::dtos::container::IdReq;
pub use crate::entities::app::{App, AppStatus, AppType};

#[async_trait]
pub trait ContainerService: Send + Sync {
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>>;
    async fn start_container(&self, id: &str) -> Result<()>;
    async fn stop_container(&self, id: &str) -> Result<()>;
    async fn restart_container(&self, id: &str) -> Result<()>;
    async fn remove_container(&self, id: &str) -> Result<()>;
    
    async fn list_images(&self) -> Result<Vec<ImageInfo>>;
    async fn remove_image(&self, id: &str) -> Result<()>;
    
    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>>;
    async fn list_networks(&self) -> Result<Vec<NetworkInfo>>;
}

#[async_trait]
pub trait AppManager: Send + Sync {
    async fn list_apps(&self) -> Result<Vec<App>>;
    async fn get_app(&self, id: &str) -> Result<App>;
    async fn install_app(&self, app_config: App) -> Result<()>;
    async fn uninstall_app(&self, id: &str) -> Result<()>;
    async fn start_app(&self, id: &str) -> Result<()>;
    async fn stop_app(&self, id: &str) -> Result<()>;
    async fn get_app_status(&self, id: &str) -> Result<AppStatus>;
}
