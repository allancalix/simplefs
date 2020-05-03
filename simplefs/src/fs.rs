use std::io::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;

use crate::alloc::{Bitmap, State};
use crate::io::BlockStorage;
use crate::sb::SuperBlock;

use thiserror::Error;
use zerocopy::{AsBytes, FromBytes};

/// An identifier used to verify a disk has been properly initialized.
const SB_MAGIC: u32 = 0x5346_5342; // "SFSB"

/// The total size of a single file system block in bytes.
pub const BLOCK_SIZE: usize = 4096;
/// The size in bytes of each inode.
const NODE_SIZE: usize = 256;
/// The number of inodes container per file system block.
const NODES_PER_BLOCK: usize = BLOCK_SIZE / NODE_SIZE;

/// # Cheap hacks
/// A cheap hack for checking if there's enough space in a data block for another directory entry.
const SAFE_SPACE: usize = 40;
/// A cheap hack for finding the current end of the meaningful data in a directory data block.
const EOF: char = '\0';

const ROOT_DEFAULT_MODE: u16 = 0x4000;

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

pub struct InodeGroup<T: BlockStorage> {
  /// A storage for reading and writing to data blocks.
    dev: T,
    data_alloc: BlockAllocator,
    /// Tracks and caches all inodes belonging to the group.
    inodes: BTreeMap<u32, Inode>,
    /// Available file system blocks on which to store the inode structure. Total inodes available
    /// can be calculated with a simple formula (BLOCK_SIZE * BLOCK_COUNT) / INODE_SIZE.
    block_count: u32,
    /// Tracks the allocation state of all inodes under control of the group.
    allocations: Bitmap,
}

impl<T: BlockStorage> InodeGroup<T> {
    fn new(dev: T, inodes: BTreeMap<u32, Inode>, alloc: Bitmap, data_alloc: BlockAllocator) -> Self {
        InodeGroup {
            dev,
            data_alloc,
            inodes,
            block_count: 5, // Known value.
            allocations: alloc,
        }
    }

    /// Returns the total number of inodes available in the group.
    fn total_inodes(&self) -> usize {
        BLOCK_SIZE * self.block_count as usize / NODE_SIZE
    }

    /// Returns the number of inodes allocated in the group.
    fn reserved_inodes(&self) -> usize {
        (0..self.total_inodes())
            .map(|i| self.allocations.get(i))
            .filter(|i| i == &State::Used)
            .count()
    }

    pub fn create(&mut self, parent_inum: u32, entry: &str) -> Result<u32, std::io::Error> {
        let mut parent = self.inodes.get(&parent_inum).unwrap();
        // Find free space in the data blocks.
        for (i, block) in parent.blocks.iter().enumerate() {
            let mut buf = Vec::with_capacity(BLOCK_SIZE);
            if *block < 8 {
                let new_block = self.data_alloc.get_free();
                let mut new_region = vec![0;BLOCK_SIZE];
                write!(new_region, "{}:{}\n{}", entry, 2, EOF)?;
                self.dev.write_block(new_block, &mut new_region)?;
                parent.blocks[i] = new_block as u32;
                return Ok(2);
            }

            self.dev.read_block(*block as usize, &mut buf);
            let values = String::from_utf8(buf).unwrap();
            let end = values
                .find(EOF)
                .expect("directory data block not formatted with EOF");

            if end + SAFE_SPACE < BLOCK_SIZE {
                let mut swap_val = values + &format!("{}:{}\n", entry, 2);
                unsafe {
                    self.dev
                        .write_block(*block as usize, swap_val.as_bytes_mut())?;
                }
              self.dev.sync_disk()?;
              return Ok(2);
            }
        }
        panic!(
            r#"The data should have stored or failed to store in one of the available blocks.
            If the code reached this point, it's likely that the directory is out of space to allocate."#
        )
    }

    pub fn find_node(&self, parts: &mut std::path::Components) -> Result<InodeStatus, SFSError> {
        // Get root and recurse until node is found.
        let root = self.get_root();
        // inum starts with root inode, zero.
        self.get_handle(parts, root, 0)
    }

    #[inline]
    fn get_root(&self) -> &Inode {
        self.inodes
            .get(&0_u32)
            .expect("File system has no root inode. This should never happen")
    }

    fn get_handle(
        &self,
        parts: &mut std::path::Components,
        node: &Inode,
        inum: u32,
    ) -> Result<InodeStatus, SFSError> {
        let part = parts.next();

        match part {
            Some(_component) => {
                for block in node.blocks.iter() {
                    if *block > 8 {
                        todo!("Add search through data blocks, parsing, and comparing to part.")
                    }
                }
                // The path did not match before reaching the final directory (where the file should exist).
                if parts.peekable().peek().is_some() {
                    return Err(SFSError::DoesNotExist);
                }
                // This means that the inode exists but no file handles belong to it.
                Ok(InodeStatus::NotFound(inum))
            }
            None => Ok(InodeStatus::Found(inum)),
        }
    }
}

#[repr(C)]
#[derive(AsBytes, FromBytes, Copy, Clone)]
pub struct Inode {
    /// The file mode (e.g full access - drwxrwxrwx).
    mode: u16,
    /// The id of the owning user.
    uid: u16,
    /// The id of the owning group.
    gid: u16,
    /// The number of links to this file.
    links_count: u16,
    /// The total size of the file in bytes.
    size: u32,
    /// The time the file was created in milliseconds since epoch.
    create_time: u32,
    /// The time the file was last updated in milliseconds since epoch.
    update_time: u32,
    /// The time the file was last accessed in milliseconds since epoch.
    access_time: u32,
    /// Reserved for future expansion of file attributes up to 256 byte limit.
    padding: [u32; 43],
    /// Pointers for the data blocks that belong to the file. Uses the remaining
    /// space the 256 inode space.
    blocks: [u32; 15],
    // TODO(allancalix): Fill in the rest of the metadata like access time, create
    // time, modification time, symlink information.
}

impl Inode {
    fn new_root() -> Self {
        let mut root = Self {
            mode: ROOT_DEFAULT_MODE,
            uid: 0,
            gid: 0,
            links_count: 0,
            size: 0,
            create_time: 0,
            update_time: 0,
            access_time: 0,
            padding: [0; 43],
            blocks: [0; 15],
        };
        // Set an initial data region for allocations.
        root.blocks[0] = 8;
        root
    }

    fn default() -> Self {
        Self {
            // TODO(allancalix): Probably find another mode.
            mode: ROOT_DEFAULT_MODE,
            uid: 0,
            gid: 0,
            links_count: 0,
            size: 0,
            create_time: 0,
            update_time: 0,
            access_time: 0,
            padding: [0; 43],
            blocks: [0; 15],
        }
    }
}

pub enum InodeStatus {
    /// The entity requested exists.
    Found(u32),
    /// The parent handle if traversal finds parent directory but not terminal entity.
    NotFound(u32),
}

// Encodes open filesystem call options http://man7.org/linux/man-pages/man2/open.2.html.
pub enum OpenMode {
    RO,
    WO,
    RW,
    DIRECTORY,
    CREATE,
}

pub struct BlockAllocator {
  alloc_map: Bitmap,
}

impl BlockAllocator {
  pub fn new(alloc: Bitmap) -> Self {
    Self {
      alloc_map: alloc,
    }
  }

  pub fn get_free(&mut self) -> usize {
    for i in 8..64 {
      if let State::Free = self.alloc_map.get(i) {
        self.alloc_map.set_reserved(i);
        return i;
      }
    }
    panic!("No data region space left to allocate.")
  }
}

#[derive(Error, Debug)]
pub enum SFSError {
    #[error("found no file at path")]
    DoesNotExist,
    #[error("invalid file system block layout")]
    InvalidBlock(#[from] std::io::Error),
}
/// A fixed 64 4k block file system. Currently hard coded for simplicity with
/// one super block, one inode bitmap, one data block bitmap, five inode blocks,
/// and 56 blocks for data storage.
pub struct SFS<T: BlockStorage> {
    super_block: SuperBlock,
    inodes: InodeGroup<T>,
    // TODO(allancalix): inode structure.
}

impl<T: BlockStorage> SFS<T> {
    /// Initializes the file system onto owned block storage.
    ///
    /// # Layout
    ///
    /// | Superblock | Bitmap (data region) | Bitmap (inodes) | Inodes |
    pub fn create(mut dev: T) -> Result<Self, SFSError> {
        let sb = SuperBlock::default();

        let mut block_buffer = [0; 4096];
        &block_buffer[0..28].copy_from_slice(sb.serialize());
        dev.write_block(0, &mut block_buffer)?;

        let mut data_map = Bitmap::new();
        // Reserve the first block for the root directory.
        data_map.set_reserved(8);
        &block_buffer.copy_from_slice(data_map.serialize());
        dev.write_block(1, &mut block_buffer)?;

        let root = Inode::new_root();
        let mut inodes = BTreeMap::new();
        &block_buffer[0..256].copy_from_slice(root.as_bytes());
        inodes.insert(0, root);
        dev.write_block(3, &mut block_buffer)?;

        // Create inode allocation tracker and set root block to reserved.
        let mut inode_map = Bitmap::new();
        inode_map.set_reserved(0);
        &block_buffer.copy_from_slice(inode_map.serialize());
        dev.write_block(2, &mut block_buffer)?;

        Ok(SFS {
            inodes: InodeGroup::new(dev, inodes, inode_map, BlockAllocator::new(data_map)),
            super_block: sb,
        })
    }

    /// Opens a file descriptor at the path provided. By default, this implementation will return an
    /// error if the file does not exists. Set OpenMode to override the behavior and create a file or
    /// directory.
    pub fn open_file<P: AsRef<Path>>(&mut self, path: P, mode: OpenMode) -> Result<u32, SFSError> {
        let mut parts = path.as_ref().components();
        assert_eq!(
            parts.next(),
            Some(std::path::Component::RootDir),
            "Path must begin with a leading slash - \"/\"."
        );

        let handle = self.inodes.find_node(&mut parts).unwrap();
        match handle {
            InodeStatus::NotFound(i) => {
                match mode {
                    OpenMode::RO | OpenMode::RW | OpenMode::WO => Err(SFSError::DoesNotExist),
                    OpenMode::CREATE => {
                      Ok(self.inodes.create(i, path.as_ref().file_name().unwrap().to_str().unwrap()).unwrap())
                    },
                    OpenMode::DIRECTORY => unimplemented!(),
                    // TODO(allancalix): The rest.
                }
            }
            InodeStatus::Found(i) => Ok(i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::FileBlockEmulatorBuilder;

    #[test]
    fn root_dir_returns_root_fd() {
        let dev = tempfile::tempfile().unwrap();
        let dev = FileBlockEmulatorBuilder::from(dev)
            .with_block_size(64)
            .build()
            .expect("Could not initialize disk emulator.");

        let mut fs = SFS::create(dev).unwrap();
        assert_eq!(fs.open_file("/", OpenMode::RO).unwrap(), 0);
    }

    #[test]
    fn file_not_found_with_create_returns_handle() {
        let dev = tempfile::tempfile().unwrap();
        let dev = FileBlockEmulatorBuilder::from(dev)
            .with_block_size(64)
            .build()
            .expect("Could not initialize disk emulator.");

        let mut fs = SFS::create(dev).unwrap();
        assert_eq!(fs.open_file("/foo", OpenMode::CREATE).unwrap(), 1);
    }

    #[test]
    #[should_panic]
    fn inodes_not_including_data_return_none() {
        let dev = tempfile::tempfile().unwrap();
        let dev = FileBlockEmulatorBuilder::from(dev)
            .with_block_size(64)
            .build()
            .expect("Could not initialize disk emulator.");

        let mut fs = SFS::create(dev).unwrap();
        fs.open_file("/foo/bar", OpenMode::RO).unwrap();
    }

    #[test]
    fn returns_file_descriptor_of_known_file() {
        let dev = tempfile::tempfile().unwrap();
        let dev = FileBlockEmulatorBuilder::from(dev)
            .with_block_size(64)
            .build()
            .expect("Could not initialize disk emulator.");

        let mut fs = SFS::create(dev).unwrap();
        assert_eq!(fs.open_file("/foo/bar", OpenMode::RO).unwrap(), 4);
    }
}
