use std::collections::HashMap;

pub const FUSE_ROOT_ID: u64 = 1;

pub struct InodeManager {
    next_inode: u64,
    inode_to_path: HashMap<u64, String>,
    path_to_inode: HashMap<String, u64>,
}

impl InodeManager {
    pub fn new() -> Self {
        let mut manager = Self {
            next_inode: FUSE_ROOT_ID + 1,
            inode_to_path: HashMap::new(),
            path_to_inode: HashMap::new(),
        };
        // Add root directory
        manager.inode_to_path.insert(FUSE_ROOT_ID, "/".to_string());
        manager.path_to_inode.insert("/".to_string(), FUSE_ROOT_ID);
        manager
    }

    pub fn get_or_create_inode(&mut self, path: &str) -> u64 {
        if let Some(&inode) = self.path_to_inode.get(path) {
            return inode;
        }

        let inode = self.next_inode;
        self.next_inode += 1;
        self.inode_to_path.insert(inode, path.to_string());
        self.path_to_inode.insert(path.to_string(), inode);
        inode
    }

    pub fn get_inode(&self, path: &str) -> Option<u64> {
        self.path_to_inode.get(path).cloned()
    }

    pub fn get_path(&self, inode: u64) -> Option<&String> {
        self.inode_to_path.get(&inode)
    }

    pub fn remove_path(&mut self, path: &str) {
        if let Some(inode) = self.path_to_inode.remove(path) {
            self.inode_to_path.remove(&inode);
        }
    }

    pub fn rename_path(&mut self, old_path: &str, new_path: &str) {
        if let Some(inode) = self.path_to_inode.remove(old_path) {
            self.inode_to_path.insert(inode, new_path.to_string());
            self.path_to_inode.insert(new_path.to_string(), inode);
        }

        // Rename children recursively
        let old_prefix = format!("{}/", old_path);
        let new_prefix = format!("{}/", new_path);
        
        let mut updates = Vec::new();
        
        for (inode, path) in self.inode_to_path.iter() {
            if path.starts_with(&old_prefix) {
                let suffix = &path[old_prefix.len()..];
                let new_child_path = format!("{}{}", new_prefix, suffix);
                updates.push((*inode, path.clone(), new_child_path));
            }
        }
        
        for (inode, old_child_path, new_child_path) in updates {
            self.path_to_inode.remove(&old_child_path);
            self.path_to_inode.insert(new_child_path.clone(), inode);
            self.inode_to_path.insert(inode, new_child_path);
        }
    }
}
