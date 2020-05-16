use std::env::args_os;
use std::ffi::CString;
use std::mem::size_of;

use simplefs_fuse::raw::{__BindgenBitfieldUnit, fuse_main_real, fuse_operations, stat, timespec};

unsafe extern "C" fn getattr(
    _arg1: *const ::std::os::raw::c_char,
    arg2: *mut stat,
) -> ::std::os::raw::c_int {
    (*arg2).st_size = 0;
    0
}

fn main() {
    #[cfg(not(target_os = "macos"))]
    let ops = fuse_operations {
        getattr: Some(getattr),
        readlink: None,
        getdir: None,
        mknod: None,
        mkdir: None,
        unlink: None,
        rmdir: None,
        symlink: None,
        rename: None,
        link: None,
        chmod: None,
        chown: None,
        truncate: None,
        utime: None,
        open: None,
        read: None,
        write: None,
        statfs: None,
        flush: None,
        release: None,
        fsync: None,
        setxattr: None,
        getxattr: None,
        listxattr: None,
        removexattr: None,
        opendir: None,
        readdir: None,
        releasedir: None,
        fsyncdir: None,
        init: None,
        destroy: None,
        access: None,
        create: None,
        ftruncate: None,
        fgetattr: None,
        lock: None,
        utimens: None,
        bmap: None,
        _bitfield_1: __BindgenBitfieldUnit::new([0; 4]),
        ioctl: None,
        poll: None,
        write_buf: None,
        read_buf: None,
        flock: None,
        fallocate: None,
    };

    #[cfg(target_os = "macos")]
    let ops = fuse_operations {
        getattr: Some(getattr),
        readlink: None,
        getdir: None,
        mknod: None,
        mkdir: None,
        unlink: None,
        rmdir: None,
        symlink: None,
        rename: None,
        link: None,
        chmod: None,
        chown: None,
        truncate: None,
        utime: None,
        open: None,
        read: None,
        write: None,
        statfs: None,
        flush: None,
        release: None,
        fsync: None,
        setxattr: None,
        getxattr: None,
        listxattr: None,
        removexattr: None,
        opendir: None,
        readdir: None,
        releasedir: None,
        fsyncdir: None,
        init: None,
        destroy: None,
        access: None,
        create: None,
        ftruncate: None,
        fgetattr: None,
        lock: None,
        utimens: None,
        bmap: None,
        _bitfield_1: __BindgenBitfieldUnit::new([0; 4]),
        ioctl: None,
        poll: None,
        write_buf: None,
        read_buf: None,
        flock: None,
        fallocate: None,
        reserved00: None,
        reserved01: None,
        reserved02: None,
        statfs_x: None,
        setvolname: None,
        exchange: None,
        getxtimes: None,
        setbkuptime: None,
        setchgtime: None,
        setcrtime: None,
        chflags: None,
        setattr_x: None,
        fsetattr_x: None,
    };

    let argc: i32 = args_os().len() as i32;
    let args: Vec<CString> = args_os()
        .into_iter()
        .map(|arg| {
            arg.to_str()
                .and_then(|s| {
                    CString::new(s)
                        .map(|c_string| {
                            dbg!(&c_string);
                            c_string
                        })
                        .ok()
                })
                .expect("Expected valid arg input")
        })
        .collect();

    let mut argv: Vec<*const ::std::os::raw::c_char> =
        args.iter().map(|arg| arg.as_ptr()).collect();

    println!("Argc: {}, argv: {:?}", argc, argv);
    unsafe {
        fuse_main_real(
            argc,
            argv.as_mut_ptr() as *mut *mut ::std::os::raw::c_char,
            &ops,
            size_of::<fuse_operations>() as u64,
            std::ptr::null_mut(),
        );
    }
}
