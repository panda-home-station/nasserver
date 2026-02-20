use std::sync::Arc;
use async_trait::async_trait;
use domain::{
    blobfs::BlobFsService,
    Result as DomainResult,
};
use storage::StorageService;
use crate::filesystem::BlobFs;

pub struct BlobFsServiceImpl<S: StorageService + Send + Sync + 'static> {
    storage: Arc<S>,
    mount_root: String,
}

impl<S: StorageService + Send + Sync + 'static> BlobFsServiceImpl<S> {
    pub fn new(storage: Arc<S>, mount_root: String) -> Self {
        Self { storage, mount_root }
    }
}

#[async_trait]
impl<S: StorageService + Send + Sync + 'static> BlobFsService for BlobFsServiceImpl<S> {
    async fn mount_for_user(&self, username: &str) -> DomainResult<()> {
        let mount_point = format!("{}/User/{}", self.mount_root, username);
        let blobs_root = "/".to_string();
        let username_clone = username.to_string();

        std::fs::create_dir_all(&mount_point)
            .map_err(|e| domain::Error::Io(e.to_string()))?;

        let options = vec![
            fuser::MountOption::RW,
            fuser::MountOption::FSName(format!("fuse_fs_{}", username_clone)),
            // fuser::MountOption::AutoUnmount,
            // fuser::MountOption::AllowOther,
            fuser::MountOption::DefaultPermissions,
        ];

        log::info!("Mounting fuse_fs for user {} to {}", username_clone, mount_point);

        let blob_fs = BlobFs::new(self.storage.clone(), username_clone.clone(), blobs_root);

        // Start FUSE mount in a blocking task
        tokio::task::spawn_blocking(move || {
            let res = fuser::mount2(blob_fs, &mount_point, &options);
            match res {
                Ok(_) => {
                    log::info!("fuse_fs for user {} unmounted successfully.", username_clone);
                }
                Err(e) => {
                    log::error!("Failed to mount fuse_fs for user {}: {}", username_clone, e);
                }
            }
        });

        Ok(())
    }
}
