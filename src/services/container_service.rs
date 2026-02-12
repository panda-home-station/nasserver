use async_trait::async_trait;
use crate::core::Result;
pub use crate::models::container::{ContainerInfo, ImageInfo, VolumeInfo, NetworkInfo};

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
