use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{Seek, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyWrite, ReplyOpen, Request,
};
use libc::{EIO, ENOENT, O_RDWR, O_WRONLY, O_TRUNC};
use log::{debug, error, info};

use domain::{storage::{DocsEntry, DocsListResp, DocsListQuery}, Result as DomainResult};
use storage::StorageService;

use crate::inode::{InodeManager, FUSE_ROOT_ID};

const TTL: &std::time::Duration = &std::time::Duration::from_secs(1);

pub struct BlobFs<S: StorageService + Send + Sync + 'static> {
    storage: Arc<S>,
    username: String,
    blobs_root: String,
    inode_manager: Arc<std::sync::Mutex<InodeManager>>,
    write_timestamps: Arc<std::sync::Mutex<HashMap<u64, Instant>>>,
    dirty_files: Arc<std::sync::Mutex<HashMap<u64, PathBuf>>>,
}

impl<S: StorageService + Send + Sync + 'static> BlobFs<S> {
    pub fn new(storage: Arc<S>, username: String, blobs_root: String) -> Self {
        Self {
            storage,
            username,
            blobs_root,
            inode_manager: Arc::new(std::sync::Mutex::new(InodeManager::new())),
            write_timestamps: Arc::new(std::sync::Mutex::new(HashMap::new())),
            dirty_files: Arc::new(std::sync::Mutex::new(HashMap::new())),
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
        // Check if file is dirty (CoW)
        if !entry.is_dir {
            let dirty = self.dirty_files.lock().unwrap();
            if let Some(path) = dirty.get(&inode) {
                if let Ok(metadata) = std::fs::metadata(path) {
                    let now = SystemTime::now();
                    return FileAttr {
                        ino: inode,
                        size: metadata.len(),
                        blocks: (metadata.len() + 511) / 512,
                        atime: metadata.accessed().unwrap_or(now),
                        mtime: metadata.modified().unwrap_or(now),
                        ctime: metadata.created().unwrap_or(now),
                        crtime: metadata.created().unwrap_or(now),
                        kind: FileType::RegularFile,
                        perm: 0o755,
                        nlink: 1,
                        uid: 1000,
                        gid: 1000,
                        rdev: 0,
                        flags: 0,
                        blksize: 4096,
                    };
                }
            }
        }

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
    fn init(&mut self, _req: &Request<'_>, _config: &mut fuser::KernelConfig) -> Result<(), libc::c_int> {
        info!("BlobFs: Initialized");
        Ok(())
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        info!("BlobFs: lookup parent={} name={}", parent, name_str);

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
            let list_result = self.storage.list(&self.username, DocsListQuery {
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
        info!("BlobFs: getattr ino={}", ino);

        if ino == FUSE_ROOT_ID {
            let now = SystemTime::now();
            let attr = FileAttr {
                ino: FUSE_ROOT_ID,
                size: 0,
                blocks: 0,
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 4096,
            };
            reply.attr(&TTL, &attr);
            return;
        }

        // Check dirty file first
        let dirty_attr = {
            let dirty = self.dirty_files.lock().unwrap();
            if let Some(path) = dirty.get(&ino) {
                if let Ok(metadata) = std::fs::metadata(path) {
                    let now = SystemTime::now();
                    Some(FileAttr {
                        ino: ino,
                        size: metadata.len(),
                        blocks: (metadata.len() + 511) / 512,
                        atime: metadata.accessed().unwrap_or(now),
                        mtime: metadata.modified().unwrap_or(now),
                        ctime: metadata.created().unwrap_or(now),
                        crtime: metadata.created().unwrap_or(now),
                        kind: FileType::RegularFile,
                        perm: 0o755,
                        nlink: 1,
                        uid: 1000,
                        gid: 1000,
                        rdev: 0,
                        flags: 0,
                        blksize: 4096,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(attr) = dirty_attr {
            reply.attr(&TTL, &attr);
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

        let result: DomainResult<DocsEntry> = rt.block_on(async {
            let blobs_path = self.map_to_blobs_path(&path);
            let parent_dir = std::path::Path::new(&blobs_path).parent().unwrap_or(std::path::Path::new("/")).to_string_lossy();
            let name = std::path::Path::new(&blobs_path).file_name().unwrap().to_string_lossy();
            
            let list_result = self.storage.list(&self.username, DocsListQuery {
                path: Some(parent_dir.to_string()),
                limit: Some(1000), // Increase limit to find file
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

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!("setattr: ino={}, size={:?}", ino, size);

        if let Some(new_size) = size {
            let is_dirty = {
                let dirty = self.dirty_files.lock().unwrap();
                dirty.contains_key(&ino)
            };

            if !is_dirty {
                // Trigger CoW
                 let path_opt = self.get_path_for_inode(ino);
                if let Some(virtual_path) = path_opt {
                    let rt = match tokio::runtime::Handle::try_current() {
                        Ok(rt) => rt,
                        Err(_) => {
                            error!("No Tokio runtime available");
                            reply.error(EIO);
                            return;
                        }
                    };

                    let res: DomainResult<PathBuf> = rt.block_on(async {
                        let blobs_path = self.map_to_blobs_path(&virtual_path);
                        self.storage.get_file_path(&self.username, &blobs_path).await
                    });

                    match res {
                        Ok(blob_path) => {
                            let temp_dir = std::path::Path::new(&self.blobs_root).parent().unwrap_or(std::path::Path::new("/")).join("tmp");
                            if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                                error!("Failed to create temp dir {:?}: {}", temp_dir, e);
                                reply.error(EIO);
                                return;
                            }
                            
                            let temp_path = temp_dir.join(format!("fuse_dirty_{}_{}", ino, SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
                            
                            debug!("CoW (setattr): Copying {:?} to {:?}", blob_path, temp_path);
                            if let Err(e) = std::fs::copy(&blob_path, &temp_path) {
                                error!("Failed to copy blob {:?} to temp {:?}: {}", blob_path, temp_path, e);
                                reply.error(EIO);
                                return;
                            }

                            let mut dirty = self.dirty_files.lock().unwrap();
                            dirty.insert(ino, temp_path);
                        }
                        Err(e) => {
                            error!("Failed to get blob path for CoW: {:?}", e);
                            reply.error(ENOENT);
                            return;
                        }
                    }
                } else {
                    reply.error(ENOENT);
                    return;
                }
            }

            // Truncate dirty file
            let dirty_path = {
                let dirty = self.dirty_files.lock().unwrap();
                dirty.get(&ino).cloned()
            };
            
            if let Some(path) = dirty_path {
                debug!("setattr: Truncating file {:?} to size {}", path, new_size);
                if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) {
                     if let Err(e) = file.set_len(new_size) {
                         error!("Failed to set_len for {:?}: {}", path, e);
                         reply.error(EIO);
                         return;
                     }
                } else {
                    error!("Failed to open dirty file for truncation: {:?}", path);
                    reply.error(EIO);
                    return;
                }
            }
        }

        self.getattr(_req, ino, reply);
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
            self.storage.list(&self.username, DocsListQuery {
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
            self.storage.mkdir(&self.username, domain::storage::DocsMkdirReq {
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
            self.storage.save_file(&self.username, &blobs_parent_path, name_str, bytes::Bytes::new()).await
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

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open: ino={}, flags={}", ino, flags);
        
        let is_write = (flags & (O_WRONLY | O_RDWR)) != 0;
        let is_truncate = (flags & O_TRUNC) != 0;
        
        if is_write {
            // CoW Logic: Check if we already have a dirty copy
            let is_dirty = {
                let dirty = self.dirty_files.lock().unwrap();
                dirty.contains_key(&ino)
            };

            if !is_dirty {
                // Get virtual path from inode
                let path_opt = self.get_path_for_inode(ino);
                if let Some(virtual_path) = path_opt {
                    // Get current physical blob path
                    let rt = match tokio::runtime::Handle::try_current() {
                        Ok(rt) => rt,
                        Err(_) => {
                            error!("No Tokio runtime available");
                            reply.error(EIO);
                            return;
                        }
                    };

                    let res: DomainResult<PathBuf> = rt.block_on(async {
                        let blobs_path = self.map_to_blobs_path(&virtual_path);
                        self.storage.get_file_path(&self.username, &blobs_path).await
                    });

                    match res {
                        Ok(blob_path) => {
                            // Create temp file for writing
                            let temp_dir = std::path::Path::new(&self.blobs_root).parent().unwrap_or(std::path::Path::new("/")).join("tmp");
                            if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                                error!("Failed to create temp dir {:?}: {}", temp_dir, e);
                                reply.error(EIO);
                                return;
                            }
                            
                            let temp_path = temp_dir.join(format!("fuse_dirty_{}_{}", ino, SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
                            
                            if is_truncate {
                                debug!("CoW (Truncate): Creating empty temp file {:?}", temp_path);
                                if let Err(e) = std::fs::File::create(&temp_path) {
                                    error!("Failed to create empty temp file {:?}: {}", temp_path, e);
                                    reply.error(EIO);
                                    return;
                                }
                            } else {
                                debug!("CoW: Copying {:?} to {:?}", blob_path, temp_path);
                                if let Err(e) = std::fs::copy(&blob_path, &temp_path) {
                                    error!("Failed to copy blob {:?} to temp {:?}: {}", blob_path, temp_path, e);
                                    reply.error(EIO);
                                    return;
                                }
                            }

                            // Register dirty file
                            let mut dirty = self.dirty_files.lock().unwrap();
                            dirty.insert(ino, temp_path);
                        }
                        Err(e) => {
                            error!("Failed to get blob path for CoW: {:?}", e);
                            reply.error(ENOENT);
                            return;
                        }
                    }
                } else {
                    reply.error(ENOENT);
                    return;
                }
            } else if is_truncate {
                // Already dirty, truncate it
                let dirty_path = {
                    let dirty = self.dirty_files.lock().unwrap();
                    dirty.get(&ino).cloned()
                };
                if let Some(path) = dirty_path {
                    debug!("CoW (Truncate): Truncating existing dirty file {:?}", path);
                    if let Err(e) = std::fs::File::create(&path) {
                         error!("Failed to truncate dirty file {:?}: {}", path, e);
                         reply.error(EIO);
                         return;
                    }
                }
            }
        }
        
        reply.opened(0, 0);
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData) {
        debug!("read: ino={}, offset={}, size={}", ino, offset, size);

        if ino == FUSE_ROOT_ID {
            reply.error(libc::EISDIR);
            return;
        }

        // Check for dirty file (CoW) first
        let dirty_path = {
            let dirty = self.dirty_files.lock().unwrap();
            dirty.get(&ino).cloned()
        };

        let physical_path = if let Some(path) = dirty_path {
            path
        } else {
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
                self.storage.get_file_path(&self.username, &blobs_path).await
            });

            match result {
                Ok(p) => p,
                Err(e) => {
                    error!("get_file_path failed: {:?}", e);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

        match std::fs::read(&physical_path) {
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
                error!("Failed to read file {:?}: {}", physical_path, e);
                reply.error(EIO);
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

        // Check dirty file first
        let dirty_path = {
            let dirty = self.dirty_files.lock().unwrap();
            dirty.get(&ino).cloned()
        };

        let physical_path = if let Some(p) = dirty_path {
            p
        } else {
            // Trigger CoW if not already dirty (fallback)
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
                self.storage.get_file_path(&self.username, &blobs_path).await
            });

            match result {
                Ok(blob_path) => {
                     let temp_dir = std::path::Path::new(&self.blobs_root).parent().unwrap_or(std::path::Path::new("/")).join("tmp");
                     if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                         error!("Failed to create temp dir: {}", e);
                         reply.error(EIO);
                         return;
                     }
                     let temp_path = temp_dir.join(format!("fuse_dirty_{}_{}", ino, SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
                     if let Err(e) = std::fs::copy(&blob_path, &temp_path) {
                         error!("Failed to copy to temp: {}", e);
                         reply.error(EIO);
                         return;
                     }
                     
                     let mut dirty = self.dirty_files.lock().unwrap();
                     dirty.insert(ino, temp_path.clone());
                     temp_path
                }
                Err(e) => {
                    error!("get_file_path failed: {:?}", e);
                    reply.error(ENOENT);
                    return;
                }
            }
        };

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
                
                // Don't sync_all every write for performance on temp file? 
                // Maybe keep it for safety.
                if let Err(e) = file.sync_all() {
                        error!("Failed to sync file {:?}: {}", physical_path, e);
                }

                // Update timestamps but NOT DB metadata yet
                {
                    let mut timestamps = self.write_timestamps.lock().unwrap();
                    timestamps.insert(ino, Instant::now());
                }

                reply.written(data.len() as u32);
                info!("Finished writing chunk to file: ino={}, size={}", ino, data.len());
            }
            Err(e) => {
                error!("Failed to open file {:?}: {}", physical_path, e);
                reply.error(EIO);
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
            self.storage.delete(&self.username, domain::storage::DocsDeleteQuery {
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
            self.storage.delete(&self.username, domain::storage::DocsDeleteQuery {
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
        
        // Commit dirty file if exists
        let dirty_path = {
            let mut dirty = self.dirty_files.lock().unwrap();
            dirty.remove(&ino)
        };

        if let Some(temp_path) = dirty_path {
             info!("Committing dirty file for ino={}: {:?}", ino, temp_path);
             
             let path = match self.get_path_for_inode(ino) {
                Some(p) => p,
                None => {
                    error!("Cannot find virtual path for ino={}", ino);
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

            let res = rt.block_on(async {
                 self.storage.commit_blob_change(&self.username, &path, &temp_path).await
            });

            if let Err(e) = res {
                error!("Failed to commit blob change: {:?}", e);
                reply.error(EIO);
                return;
            }
        }

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
            self.storage.rename(&self.username, domain::storage::DocsRenameReq {
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
