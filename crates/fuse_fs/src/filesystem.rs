use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{Seek, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyWrite, Request,
};
use libc::{EIO, ENOENT};
use log::{debug, error, info};

use domain::{storage::{DocsEntry, DocsListResp, DocsListQuery}, Result as DomainResult};
use storage::StorageService;

use crate::inode::{InodeManager, FUSE_ROOT_ID};

const TTL: &std::time::Duration = &std::time::Duration::from_secs(1);

pub struct BlobFs<S: StorageService + Send + Sync + 'static> {
    storage: Arc<S>,
    blobs_root: String,
    inode_manager: Arc<std::sync::Mutex<InodeManager>>,
    write_timestamps: Arc<std::sync::Mutex<HashMap<u64, Instant>>>,
}

impl<S: StorageService + Send + Sync + 'static> BlobFs<S> {
    pub fn new(storage: Arc<S>, blobs_root: String) -> Self {
        Self {
            storage,
            blobs_root,
            inode_manager: Arc::new(std::sync::Mutex::new(InodeManager::new())),
            write_timestamps: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// 将用户路径映射到 blobs 存储路径
    /// 例如: /home/user/file.txt -> /blobs/file.txt
    fn map_to_blobs_path(&self, user_path: &str) -> String {
        // 移除前导的 home 路径部分，只保留相对路径
        let relative_path = user_path.trim_start_matches('/');
        if relative_path.is_empty() {
            self.blobs_root.clone()
        } else {
            format!("{}/{}", self.blobs_root, relative_path)
        }
    }

    fn get_path_for_inode(&self, inode: u64) -> Option<String> {
        let manager = self.inode_manager.lock().unwrap();
        manager.get_path(inode).cloned()
    }

    fn get_or_create_inode_for_path(&self, path: &str) -> u64 {
        let mut manager = self.inode_manager.lock().unwrap();
        manager.get_or_create_inode(path)
    }

    fn file_attr_from_docs_entry(&self, entry: &DocsEntry, inode: u64) -> FileAttr {
        let now = SystemTime::now();
        FileAttr {
            ino: inode,
            size: entry.size as u64,
            blocks: ((entry.size + 511) / 512) as u64,
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind: if entry.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm: 0o755,
            nlink: if entry.is_dir { 2 } else { 1 },
            uid: 1000,
            gid: 1000,
            rdev: 0,
            flags: 0,
            blksize: 4096,
        }
    }
}

impl<S: StorageService + Send + Sync + 'static> Filesystem for BlobFs<S> {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        debug!("lookup: parent={}, name={}", parent, name_str);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<DocsEntry> = rt.block_on(async {
            let blobs_parent_path = self.map_to_blobs_path(&parent_path);
            let list_result = self.storage.list(&self.blobs_root, DocsListQuery {
                path: Some(blobs_parent_path),
                limit: Some(100),
                offset: Some(0),
            }).await?;
            
            list_result.entries.into_iter()
                .find(|entry| entry.name == name_str)
                .ok_or_else(|| domain::Error::NotFound(format!("File not found: {}", full_path)))
        });

        match result {
            Ok(entry) => {
                let inode = self.get_or_create_inode_for_path(&full_path);
                let attr = self.file_attr_from_docs_entry(&entry, inode);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!("getattr: ino={}", ino);

        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<DocsEntry> = rt.block_on(async {
            let path_buf = PathBuf::from(&path);
            let parent_path = if path == "/" { "" } else {
                path_buf.parent()
                    .and_then(|p| p.to_str())
                    .unwrap_or("")
            };
            let name = path_buf.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            
            let blobs_parent_path = self.map_to_blobs_path(parent_path);
            let list_result = self.storage.list(&self.blobs_root, DocsListQuery {
                path: Some(blobs_parent_path),
                limit: Some(100),
                offset: Some(0),
            }).await?;
            
            list_result.entries.into_iter()
                .find(|entry| entry.name == name)
                .ok_or_else(|| domain::Error::NotFound(format!("File not found: {}", path)))
        });

        match result {
            Ok(entry) => {
                let attr = self.file_attr_from_docs_entry(&entry, ino);
                reply.attr(&TTL, &attr);
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir: ino={}, offset={}", ino, offset);

        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<DocsListResp> = rt.block_on(async {
            let blobs_path = self.map_to_blobs_path(&path);
            self.storage.list(&self.blobs_root, DocsListQuery {
                path: Some(blobs_path),
                limit: None,
                offset: None,
            }).await
        });

        match result {
            Ok(list_resp) => {
                let entries = vec![
                    (ino, FileType::Directory, "."),
                    (self.get_or_create_inode_for_path("/"), FileType::Directory, ".."),
                ];

                for (i, (inode, kind, name)) in entries.iter().enumerate() {
                    if (i as i64) >= offset {
                        if reply.add(*inode, (i + 1) as i64, *kind, name) {
                            break;
                        }
                    }
                }

                let current_offset = entries.len() as i64;
                for (i, entry) in list_resp.entries.iter().enumerate() {
                    if (current_offset + i as i64) >= offset {
                        let kind = if entry.is_dir {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        };
                        let full_path = if path == "/" {
                            format!("/{}", entry.name)
                        } else {
                            format!("{}/{}", path, entry.name)
                        };
                        let inode = self.get_or_create_inode_for_path(&full_path);

                        if reply.add(inode, current_offset + i as i64 + 1, kind, &entry.name) {
                            break;
                        }
                    }
                }

                reply.ok();
            }
            Err(e) => {
                error!("readdir failed: {:?}", e);
                reply.error(EIO);
            }
        }
    }

    fn mkdir(&mut self, _req: &Request, parent: u64, name: &OsStr, mode: u32, _umask: u32, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        debug!("mkdir: parent={}, name={}, mode={}", parent, name_str, mode);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<()> = rt.block_on(async {
            let blobs_full_path = self.map_to_blobs_path(&full_path);
            self.storage.mkdir(&self.blobs_root, domain::storage::DocsMkdirReq {
                path: blobs_full_path,
            }).await
        });

        match result {
            Ok(()) => {
                let inode = self.get_or_create_inode_for_path(&full_path);
                let now = SystemTime::now();
                let attr = FileAttr {
                    ino: inode,
                    size: 0,
                    blocks: 0,
                    atime: now,
                    mtime: now,
                    ctime: now,
                    crtime: now,
                    kind: FileType::Directory,
                    perm: mode as u16,
                    nlink: 2,
                    uid: 1000,
                    gid: 1000,
                    rdev: 0,
                    flags: 0,
                    blksize: 4096,
                };
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                error!("mkdir failed: {:?}", e);
                reply.error(EIO);
            }
        }
    }

    fn create(&mut self, _req: &Request, parent: u64, name: &OsStr, mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        debug!("create: parent={}, name={}, mode={}", parent, name_str, mode);
        info!("Starting file upload (create): parent_ino={}, name={}", parent, name_str);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        // Create empty file using storage service
        let result: DomainResult<()> = rt.block_on(async {
            let blobs_parent_path = self.map_to_blobs_path(&parent_path);
            self.storage.save_file(&self.blobs_root, &blobs_parent_path, name_str, bytes::Bytes::new()).await
        });

        match result {
            Ok(()) => {
                let inode = self.get_or_create_inode_for_path(&full_path);
                let now = SystemTime::now();
                let attr = FileAttr {
                    ino: inode,
                    size: 0,
                    blocks: 0,
                    atime: now,
                    mtime: now,
                    ctime: now,
                    crtime: now,
                    kind: FileType::RegularFile,
                    perm: mode as u16,
                    nlink: 1,
                    uid: 1000,
                    gid: 1000,
                    rdev: 0,
                    flags: 0,
                    blksize: 4096,
                };
                reply.created(&TTL, &attr, 0, inode, 0);
            }
            Err(e) => {
                error!("create failed: {:?}", e);
                reply.error(EIO);
            }
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData) {
        debug!("read: ino={}, offset={}, size={}", ino, offset, size);

        if ino == FUSE_ROOT_ID {
            reply.error(libc::EISDIR);
            return;
        }

        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<PathBuf> = rt.block_on(async {
            let blobs_path = self.map_to_blobs_path(&path);
            self.storage.get_file_path(&self.blobs_root, &blobs_path).await
        });

        match result {
            Ok(path) => {
                match std::fs::read(&path) {
                    Ok(data) => {
                        let start = offset as usize;
                        let end = std::cmp::min(start + size as usize, data.len());
                        if start < data.len() {
                            reply.data(&data[start..end]);
                        } else {
                            reply.data(&[]);
                        }
                    }
                    Err(e) => {
                        error!("Failed to read file {:?}: {}", path, e);
                        reply.error(EIO);
                    }
                }
            }
            Err(e) => {
                error!("get_file_path failed: {:?}", e);
                reply.error(ENOENT);
            }
        }
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyWrite) {
        info!("Writing data to file: ino={}, offset={}, data_len={}", ino, offset, data.len());
        debug!("write: ino={}, offset={}, data_len={}", ino, offset, data.len());

        if ino == FUSE_ROOT_ID {
            reply.error(libc::EISDIR);
            return;
        }

        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<PathBuf> = rt.block_on(async {
            let blobs_path = self.map_to_blobs_path(&path);
            self.storage.get_file_path(&self.blobs_root, &blobs_path).await
        });

        match result {
            Ok(physical_path) => {
                match std::fs::OpenOptions::new().write(true).create(true).open(&physical_path) {
                    Ok(mut file) => {
                        if let Err(e) = file.seek(std::io::SeekFrom::Start(offset as u64)) {
                            error!("Failed to seek file {:?}: {}", physical_path, e);
                            reply.error(EIO);
                            return;
                        }
                        if let Err(e) = file.write_all(data) {
                            error!("Failed to write file {:?}: {}", physical_path, e);
                            reply.error(EIO);
                            return;
                        }

                        rt.block_on(async {
                            if let Err(e) = self.storage.sync_external_change(&physical_path).await {
                                error!("Failed to sync external change for {:?}: {:?}", physical_path, e);
                            }
                        });

                        reply.written(data.len() as u32);
                        info!("Finished writing chunk to file: ino={}, size={}", ino, data.len());
                        {
                            let mut timestamps = self.write_timestamps.lock().unwrap();
                            timestamps.insert(ino, Instant::now());
                        }
                    }
                    Err(e) => {
                        error!("Failed to open file {:?}: {}", physical_path, e);
                        reply.error(EIO);
                    }
                }
            }
            Err(e) => {
                error!("get_file_path failed: {:?}", e);
                reply.error(ENOENT);
            }
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        debug!("unlink: parent={}, name={}", parent, name_str);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<()> = rt.block_on(async {
            let blobs_full_path = self.map_to_blobs_path(&full_path);
            self.storage.delete(&self.blobs_root, domain::storage::DocsDeleteQuery {
                path: Some(blobs_full_path),
            }).await
        });

        match result {
            Ok(()) => {
                // Remove from inode manager
                {
                    let mut manager = self.inode_manager.lock().unwrap();
                    manager.remove_path(&full_path);
                }
                reply.ok();
            }
            Err(e) => {
                error!("unlink failed: {:?}", e);
                reply.error(EIO);
            }
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        debug!("rmdir: parent={}, name={}", parent, name_str);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<()> = rt.block_on(async {
            let blobs_full_path = self.map_to_blobs_path(&full_path);
            self.storage.delete(&self.blobs_root, domain::storage::DocsDeleteQuery {
                path: Some(blobs_full_path),
            }).await
        });

        match result {
            Ok(()) => {
                // Remove from inode manager
                {
                    let mut manager = self.inode_manager.lock().unwrap();
                    manager.remove_path(&full_path);
                }
                reply.ok();
            }
            Err(e) => {
                error!("rmdir failed: {:?}", e);
                reply.error(EIO);
            }
        }
    }

    fn release(&mut self, _req: &Request, ino: u64, _fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        info!("File release/close: ino={}", ino);
        {
            let mut timestamps = self.write_timestamps.lock().unwrap();
            if let Some(last_write_time) = timestamps.remove(&ino) {
                let finalization_duration = last_write_time.elapsed();
                info!(
                    "File finalization took: {:.2}s (from last write to release)",
                    finalization_duration.as_secs_f64()
                );
            }
        }

        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        info!("File upload completed and closed: path={}", path);
        reply.ok();
    }

    fn rename(&mut self, _req: &Request, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr, _flags: u32, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let newname_str = match newname.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        debug!("rename: parent={}, name={}, newparent={}, newname={}", parent, name_str, newparent, newname_str);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let newparent_path = match self.get_path_for_inode(newparent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let from_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let to_path = if newparent_path == "/" {
            format!("/{}", newname_str)
        } else {
            format!("{}/{}", newparent_path, newname_str)
        };

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => {
                error!("No Tokio runtime available");
                reply.error(EIO);
                return;
            }
        };

        let result: DomainResult<()> = rt.block_on(async {
            let blobs_from_path = self.map_to_blobs_path(&from_path);
            let blobs_to_path = self.map_to_blobs_path(&to_path);
            self.storage.rename(&self.blobs_root, domain::storage::DocsRenameReq {
                from: Some(blobs_from_path),
                to: Some(blobs_to_path),
            }).await
        });

        match result {
            Ok(()) => {
                // Update inode manager
                {
                    let mut manager = self.inode_manager.lock().unwrap();
                    manager.rename_path(&from_path, &to_path);
                }
                reply.ok();
            }
            Err(e) => {
                error!("rename failed: {:?}", e);
                reply.error(EIO);
            }
        }
    }
}
