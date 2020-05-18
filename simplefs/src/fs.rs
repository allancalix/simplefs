use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::alloc::{Bitmap, NextAvailableAllocation};
use crate::io::BlockStorage;
use crate::node::InodeGroup;
use crate::sb::SuperBlock;

#[cfg(target_os = "macos")]
use fuse::ReplyXTimes;
use fuse::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyBmap, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyLock, ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request,
};
use libc::ENOENT;
use thiserror::Error;
use time::Timespec;

const SB_MAGIC: u32 = 0x5346_5342; // SFSB

pub const BLOCK_SIZE: usize = 4096;
const NODE_SIZE: usize = 256;

/// Known locations.
const SUPERBLOCK_INDEX: usize = 0;
const DATA_REGION_BMP: usize = 1;
const INODE_BMP: usize = 2;
const INODE_START: usize = 3;

impl Default for SuperBlock {
    fn default() -> Self {
        let mut sb = SuperBlock::new();
        sb.sb_magic = SB_MAGIC;
        // This is a limited implementation only supporting at most 80 file system
        // objects (files or directories).
        sb.inodes_count = 5 * (BLOCK_SIZE / NODE_SIZE) as u32;
        // Use the remaining space for user data blocks.
        sb.blocks_count = 56;
        sb.reserved_blocks_count = 0;
        sb.free_blocks_count = 0;
        // All inodes are initially free.
        sb.free_inodes_count = sb.inodes_count;
        sb
    }
}

// Encodes open filesystem call options http://man7.org/linux/man-pages/man2/open.2.html.
pub enum OpenMode {
    RO,
    WO,
    RW,
    DIRECTORY,
    CREATE,
}

#[derive(Error, Debug)]
pub enum SFSError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("found no file at path")]
    DoesNotExist,
    #[error("invalid file system block layout")]
    InvalidBlock(#[from] std::io::Error),
}

/// A fixed 64 4k block file system. Currently hard coded for simplicity with
/// one super block, one inode bitmap, one data block bitmap, five inode blocks,
/// and 56 blocks for data storage.
pub struct SFS<T: BlockStorage> {
    dev: T,
    super_block: SuperBlock,
    data_map: Bitmap,
    inodes: InodeGroup,
}

impl<T: BlockStorage> SFS<T> {
    /// Initializes the file system onto owned block storage.
    ///
    /// # Layout
    /// ==============================================================================
    /// | SuperBlock | Bitmap (data region) | Bitmap (inodes) | Inodes | Data Region |
    /// ==============================================================================
    pub fn create(mut dev: T) -> Result<Self, SFSError> {
        // Reusable buffer for writing blocks.
        let mut block_buffer = [0; 4096];

        // Init SuperBlock header.
        let super_block = SuperBlock::default();
        block_buffer[0..28].copy_from_slice(super_block.serialize());
        dev.write_block(SUPERBLOCK_INDEX, &mut block_buffer)?;

        // Init allocation map for data region.
        let data_map = Bitmap::new();
        block_buffer.copy_from_slice(data_map.serialize());
        dev.write_block(DATA_REGION_BMP, &mut block_buffer)?;

        // Initialize inode structure with root node.
        let inodes = InodeGroup::new(Bitmap::new());
        block_buffer.copy_from_slice(inodes.allocations().serialize());
        dev.write_block(INODE_BMP, &mut block_buffer)?;
        dev.write_block(INODE_START, &mut inodes.serialize_block(0))?;
        dev.sync_disk()?;

        Ok(SFS {
            dev,
            inodes,
            data_map,
            super_block,
        })
    }

    pub fn from_block_storage(mut dev: T) -> Result<Self, SFSError> {
        let mut block_buf = vec![0; 4096];

        // Read superblock from first block;
        dev.read_block(SUPERBLOCK_INDEX, &mut block_buf)?;
        let super_block = SuperBlock::parse(&block_buf, SB_MAGIC);

        dev.read_block(DATA_REGION_BMP, &mut block_buf)?;
        let data_map = Bitmap::parse(&block_buf);

        dev.read_block(INODE_BMP, &mut block_buf)?;
        let inode_allocs = Bitmap::parse(&block_buf);
        let mut inodes = InodeGroup::open(inode_allocs);

        for i in INODE_START..INODE_START + 5 {
            dev.read_block(i, &mut block_buf)?;
            // TODO(allancalix): This is a bit ugly. Because the inode group is unaware that's first
            // disk block is at an offset (INODE_START) we have to subtract the offset before loading
            // the block.
            inodes.load_block((i - INODE_START) as u32, &block_buf);
        }

        Ok(SFS {
            dev,
            inodes,
            data_map,
            super_block,
        })
    }

    /// Opens a file descriptor at the path provided. By default, this implementation will return an
    /// error if the file does not exists. Set OpenMode to override the behavior and create a file or
    /// directory.
    pub fn open<P: AsRef<Path>>(&mut self, path: P, mode: OpenMode) -> Result<u32, SFSError> {
        let mut parts = path.as_ref().components();
        if Some(std::path::Component::RootDir) != parts.next() {
            return Err(SFSError::InvalidArgument(
                "path must start with \"/\"".to_string(),
            ));
        }

        let mut inum = 0;
        while let Some(part) = parts.next() {
            let content = self.read_dir(inum)?;
            let node = content.get(part.as_os_str());
            if node.is_none() {
                if parts.peekable().peek().is_some() {
                    return Err(SFSError::InvalidArgument(
                        "Missing subdirectory in path.".to_string(),
                    ));
                }

                match mode {
                    OpenMode::CREATE => break,
                    _ => return Err(SFSError::DoesNotExist),
                }
            }

            inum = *node.unwrap();
        }

        match mode {
            OpenMode::CREATE => {
                let created_file = self.inodes.new_file();
                let mut parent_dir = self.read_dir(inum)?;
                parent_dir.insert(
                    OsString::from(path.as_ref().file_name().unwrap()),
                    created_file,
                );
                self.write_dir(inum, parent_dir)?;
                Ok(created_file)
            }
            OpenMode::RO => Ok(inum),
            // The rest of the modes.
            _ => unimplemented!(),
        }
    }

    fn write_dir(&mut self, dir: u32, entries: HashMap<OsString, u32>) -> Result<(), SFSError> {
        let mut contents: String = entries
            .iter()
            .map(|(k, v)| format!("{}:{}\n", v, k.to_str().unwrap()))
            .collect();
        contents.push('\0');

        let node = self.inodes.get_mut(dir).unwrap();
        let allocated_blocks: Vec<u32> = node
            .blocks
            .iter()
            .filter(|block| *block > &8_u32)
            .copied()
            .collect();

        if allocated_blocks.len() < 1 + (contents.as_bytes().len() / BLOCK_SIZE) {
            let needed = 1 + (contents.as_bytes().len() / BLOCK_SIZE);
            let have = allocated_blocks.len();

            let mut alloc_gen = NextAvailableAllocation::new(self.data_map, None);
            let new_blocks: Vec<u32> = (0..(needed - have))
                // Panics if no free blocks are available.
                .map(|_| alloc_gen.next().unwrap() as u32)
                .collect();
            // Mark new blocks as allocated.
            for &new_block in new_blocks.iter() {
                self.data_map.set_reserved(new_block as usize);
            }
            let mut all_blocks = allocated_blocks.iter().chain(new_blocks.iter());
            let new_blocks = all_blocks.clone().copied().collect::<Vec<u32>>();
            node.blocks[0..new_blocks.len()].copy_from_slice(&new_blocks);

            unsafe {
                contents
                    .as_bytes_mut()
                    .chunks_mut(BLOCK_SIZE)
                    .for_each(|chunk| {
                        self.dev
                            .write_block(*all_blocks.next().unwrap() as usize, chunk)
                            .unwrap();
                    });
            }
            return Ok(());
        }

        info!("Writing content \"{}\" to dir inode {}.", contents, dir);
        let mut blocks = allocated_blocks.iter();
        unsafe {
            contents
                .as_bytes_mut()
                .chunks_mut(BLOCK_SIZE)
                .for_each(|chunk| {
                    self.dev
                        .write_block(*blocks.next().unwrap() as usize, chunk)
                        .unwrap();
                });
        }
        Ok(())
    }

    fn read_dir(&mut self, inum: u32) -> Result<HashMap<OsString, u32>, SFSError> {
        let content = self.read_file(inum)?;
        let contents_parsed = String::from_utf8(content).unwrap();

        let mut dir_contents = HashMap::new();
        for line in contents_parsed.lines() {
            let mut contents = line.split(':');
            let entry_inum = contents.next().unwrap().parse::<u32>().unwrap();
            let entry_name = OsString::from(contents.next().unwrap());
            dir_contents.insert(entry_name, entry_inum);
        }

        Ok(dir_contents)
    }

    fn read_file(&mut self, inum: u32) -> Result<Vec<u8>, SFSError> {
        let node = self.inodes.get(inum);
        if node.is_none() {
            return Err(SFSError::DoesNotExist);
        }
        let allocated_blocks: Vec<u32> = node
            .unwrap()
            .blocks
            .iter()
            .filter(|block| *block > &(self.super_block.inodes_count + 3))
            .copied()
            .collect();

        let mut content = vec![0; allocated_blocks.len()];
        for (i, &block) in allocated_blocks.iter().enumerate() {
            let start = i * BLOCK_SIZE;
            let end = start + BLOCK_SIZE;
            self.dev
                .read_block(block as usize, &mut content[start..end])?;
        }
        Ok(content)
    }
}

impl<T: BlockStorage> Filesystem for SFS<T> {
    fn init(&mut self, _req: &Request) -> Result<(), i32> {
        info!("Filesystem mount requested.");
        Ok(())
    }

    fn destroy(&mut self, _req: &Request) {
        unimplemented!()
    }

    fn lookup(&mut self, _req: &Request, _parent: u64, _name: &OsStr, _reply: ReplyEntry) {
        unimplemented!()
    }

    fn forget(&mut self, _req: &Request, _ino: u64, _nlookup: u64) {
        unimplemented!()
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        info!("Getting attributes for ino={}.", ino);
        let zero_time = Timespec::new(0, 0);
        let attr = FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: zero_time.clone(),
            mtime: zero_time.clone(),
            ctime: zero_time.clone(),
            crtime: zero_time.clone(),
            kind: FileType::Directory,
            perm: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0,
        };
        reply.attr(&Timespec::new(0, 0), &attr);
    }

    fn setattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<Timespec>,
        _mtime: Option<Timespec>,
        _fh: Option<u64>,
        _crtime: Option<Timespec>,
        _chgtime: Option<Timespec>,
        _bkuptime: Option<Timespec>,
        _flags: Option<u32>,
        _reply: ReplyAttr,
    ) {
        unimplemented!()
    }

    fn readlink(&mut self, _req: &Request, _ino: u64, _reply: ReplyData) {
        unimplemented!()
    }

    fn mknod(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _rdev: u32,
        _reply: ReplyEntry,
    ) {
        unimplemented!()
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _reply: ReplyEntry,
    ) {
        unimplemented!()
    }

    fn unlink(&mut self, _req: &Request, _parent: u64, _name: &OsStr, _reply: ReplyEmpty) {
        unimplemented!()
    }

    fn rmdir(&mut self, _req: &Request, _parent: u64, _name: &OsStr, _reply: ReplyEmpty) {
        unimplemented!()
    }

    fn symlink(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _link: &Path,
        _reply: ReplyEntry,
    ) {
        unimplemented!()
    }

    fn rename(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _reply: ReplyEmpty,
    ) {
        unimplemented!()
    }

    fn link(
        &mut self,
        _req: &Request,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        _reply: ReplyEntry,
    ) {
        unimplemented!()
    }

    fn open(&mut self, _req: &Request, _ino: u64, _flags: u32, _reply: ReplyOpen) {
        unimplemented!()
    }

    fn read(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _size: u32,
        _reply: ReplyData,
    ) {
        unimplemented!()
    }

    fn write(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _data: &[u8],
        _flags: u32,
        _reply: ReplyWrite,
    ) {
        unimplemented!()
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, _reply: ReplyEmpty) {
        unimplemented!()
    }

    fn release(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
        _reply: ReplyEmpty,
    ) {
        unimplemented!()
    }

    fn fsync(&mut self, _req: &Request, _ino: u64, _fh: u64, _datasync: bool, _reply: ReplyEmpty) {
        unimplemented!()
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        //TODO(allancalix): The fuse crate starts inodes at 1, translate down to 0 internally.
        let ino = ino - 1;
        info!("Reading directory inode={}.", ino);
        let contents = self.read_dir(ino as u32);
        if contents.is_err() {
            warn!("Error reading inode={}.", ino);
            return reply.error(ENOENT);
        }

        if offset == 2 {
            return reply.ok();
        }

        debug!("Pulled contents for directory {:?}.", contents);
        // Add self.
        reply.add(1, 1, FileType::Directory, ".");
        // Add parent dir.
        reply.add(1, 2, FileType::Directory, "..");
        info!("Serving canned response.");
        reply.ok()
    }

    fn access(&mut self, _req: &Request, _ino: u64, _mask: u32, _reply: ReplyEmpty) {
        unimplemented!()
    }

    fn create(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _flags: u32,
        _reply: ReplyCreate,
    ) {
        unimplemented!()
    }

    #[cfg(target_os = "macos")]
    fn getlk(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        _start: u64,
        _end: u64,
        _typ: u32,
        _pid: u32,
        _reply: ReplyLock,
    ) {
        unimplemented!()
    }

    #[cfg(target_os = "macos")]
    fn setlk(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        _start: u64,
        _end: u64,
        _typ: u32,
        _pid: u32,
        _sleep: bool,
        _reply: ReplyEmpty,
    ) {
        unimplemented!()
    }

    #[cfg(target_os = "macos")]
    fn bmap(&mut self, _req: &Request, _ino: u64, _blocksize: u32, _idx: u64, _reply: ReplyBmap) {
        unimplemented!()
    }

    #[cfg(target_os = "macos")]
    fn setvolname(&mut self, _req: &Request, _name: &OsStr, _reply: ReplyEmpty) {
        unimplemented!()
    }

    #[cfg(target_os = "macos")]
    fn exchange(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _options: u64,
        _reply: ReplyEmpty,
    ) {
        unimplemented!()
    }

    #[cfg(target_os = "macos")]
    fn getxtimes(&mut self, _req: &Request, _ino: u64, _reply: ReplyXTimes) {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{FileBlockEmulator, FileBlockEmulatorBuilder};

    fn create_test_device() -> FileBlockEmulator {
        let dev = tempfile::tempfile().unwrap();
        FileBlockEmulatorBuilder::from(dev)
            .with_block_size(64)
            .build()
            .expect("Could not initialize disk emulator.")
    }

    #[test]
    fn root_dir_returns_root_fd() {
        let dev = create_test_device();
        let mut fs = SFS::create(dev).unwrap();
        assert_eq!(fs.open("/", OpenMode::RO).unwrap(), 0);
    }

    #[test]
    fn file_not_found_without_create_returns_error() {
        let dev = create_test_device();
        let mut fs = SFS::create(dev).unwrap();

        let result = fs.open("/foo", OpenMode::RO);
        match result.unwrap_err() {
            SFSError::DoesNotExist => (),
            _ => assert!(false, "Unexpected error type."),
        }
    }

    #[test]
    fn create_non_existent_file_returns_handle() {
        let dev = create_test_device();

        let mut fs = SFS::create(dev).unwrap();

        assert_eq!(fs.open("/foo", OpenMode::CREATE).unwrap(), 1);
    }

    #[test]
    fn create_non_existent_file_with_missing_subdirectory_returns_error() {
        let dev = create_test_device();

        let mut fs = SFS::create(dev).unwrap();

        assert!(fs.open("/foo/bar", OpenMode::CREATE).is_err());
    }

    #[test]
    fn can_create_and_reopen_initialized_filesystem() {
        let disk = tempfile::NamedTempFile::new().unwrap();
        let dev = FileBlockEmulatorBuilder::from(disk.reopen().unwrap())
            .with_block_size(64)
            .build()
            .unwrap();
        // Initialize the filesystem.
        SFS::create(dev).unwrap();

        let dev = FileBlockEmulatorBuilder::from(disk.reopen().unwrap())
            .with_block_size(64)
            // Don't reset initialized disk.
            .clear_medium(false)
            .build()
            .unwrap();
        let fs: SFS<FileBlockEmulator> = SFS::from_block_storage(dev).unwrap();
        assert_eq!(fs.inodes.total_nodes(), 1);
    }
}
