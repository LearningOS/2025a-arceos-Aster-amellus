#![allow(dead_code)]

use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_void};
use axerrno::{AxError, LinuxError};
use axhal::arch::TrapFrame;
use axhal::mem::VirtAddr;
use axhal::paging::MappingFlags;
use axhal::trap::{register_trap_handler, SYSCALL};
use arceos_posix_api as api;
use axtask::current;
use axtask::TaskExtRef;
use memory_addr::{VirtAddrRange, PAGE_SIZE_4K};

const SYS_IOCTL: usize = 29;
const SYS_OPENAT: usize = 56;
const SYS_CLOSE: usize = 57;
const SYS_READ: usize = 63;
const SYS_WRITE: usize = 64;
const SYS_WRITEV: usize = 66;
const SYS_EXIT: usize = 93;
const SYS_EXIT_GROUP: usize = 94;
const SYS_SET_TID_ADDRESS: usize = 96;
const SYS_MMAP: usize = 222;

const AT_FDCWD: i32 = -100;
const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;

/// Macro to generate syscall body
///
/// It will receive a function which return Result<_, LinuxError> and convert it to
/// the type which is specified by the caller.
#[macro_export]
macro_rules! syscall_body {
    ($fn: ident, $($stmt: tt)*) => {{
        #[allow(clippy::redundant_closure_call)]
        let res = (|| -> axerrno::LinuxResult<_> { $($stmt)* })();
        match res {
            Ok(_) | Err(axerrno::LinuxError::EAGAIN) => debug!(concat!(stringify!($fn), " => {:?}"),  res),
            Err(_) => info!(concat!(stringify!($fn), " => {:?}"), res),
        }
        match res {
            Ok(v) => v as _,
            Err(e) => {
                -e.code() as _
            }
        }
    }};
}

bitflags::bitflags! {
    #[derive(Debug)]
    /// permissions for sys_mmap
    ///
    /// See <https://github.com/bminor/glibc/blob/master/bits/mman.h>
    struct MmapProt: i32 {
        /// Page can be read.
        const PROT_READ = 1 << 0;
        /// Page can be written.
        const PROT_WRITE = 1 << 1;
        /// Page can be executed.
        const PROT_EXEC = 1 << 2;
    }
}

impl From<MmapProt> for MappingFlags {
    fn from(value: MmapProt) -> Self {
        let mut flags = MappingFlags::USER;
        if value.contains(MmapProt::PROT_READ) {
            flags |= MappingFlags::READ;
        }
        if value.contains(MmapProt::PROT_WRITE) {
            flags |= MappingFlags::WRITE;
        }
        if value.contains(MmapProt::PROT_EXEC) {
            flags |= MappingFlags::EXECUTE;
        }
        flags
    }
}

bitflags::bitflags! {
    #[derive(Debug)]
    /// flags for sys_mmap
    ///
    /// See <https://github.com/bminor/glibc/blob/master/bits/mman.h>
    struct MmapFlags: i32 {
        /// Share changes
        const MAP_SHARED = 1 << 0;
        /// Changes private; copy pages on write.
        const MAP_PRIVATE = 1 << 1;
        /// Map address must be exactly as requested, no matter whether it is available.
        const MAP_FIXED = 1 << 4;
        /// Don't use a file.
        const MAP_ANONYMOUS = 1 << 5;
        /// Don't check for reservations.
        const MAP_NORESERVE = 1 << 14;
        /// Allocation is for a stack.
        const MAP_STACK = 0x20000;
    }
}

#[register_trap_handler(SYSCALL)]
fn handle_syscall(tf: &TrapFrame, syscall_num: usize) -> isize {
    ax_println!("handle_syscall [{}] ...", syscall_num);
    let ret = match syscall_num {
        SYS_IOCTL => sys_ioctl(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _) as _,
        SYS_SET_TID_ADDRESS => sys_set_tid_address(tf.arg0() as _),
        SYS_OPENAT => sys_openat(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _, tf.arg3() as _),
        SYS_CLOSE => sys_close(tf.arg0() as _),
        SYS_READ => sys_read(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _),
        SYS_WRITE => sys_write(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _),
        SYS_WRITEV => sys_writev(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _),
        SYS_EXIT_GROUP => {
            ax_println!("[SYS_EXIT_GROUP]: system is exiting ..");
            axtask::exit(tf.arg0() as _)
        },
        SYS_EXIT => {
            ax_println!("[SYS_EXIT]: system is exiting ..");
            axtask::exit(tf.arg0() as _)
        },
        SYS_MMAP => sys_mmap(
            tf.arg0() as _,
            tf.arg1() as _,
            tf.arg2() as _,
            tf.arg3() as _,
            tf.arg4() as _,
            tf.arg5() as _,
        ),
        _ => {
            ax_println!("Unimplemented syscall: {}", syscall_num);
            -LinuxError::ENOSYS.code() as _
        }
    };
    ret
}

fn sys_mmap(
    addr: *mut usize,
    length: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    _offset: isize,
) -> isize {
    syscall_body!(sys_mmap, {
        // Align as 4 KiB, both length and offset can't be 0.
        if length == 0 {
            return Err(LinuxError::EINVAL);
        }
        if _offset < 0 {
            return Err(LinuxError::EINVAL);
        }

        // Parse w/r bit in MmapProt and map it into MappingFlags
        let prot_bits = MmapProt::from_bits(prot).ok_or(LinuxError::EINVAL)?;
        let flag_bits = MmapFlags::from_bits(flags).ok_or(LinuxError::EINVAL)?;
        if !flag_bits.contains(MmapFlags::MAP_ANONYMOUS) && fd < 0 { // In anomynous map, fd must le 0
            return Err(LinuxError::EBADF);
        }

        let len_aligned = length
            .checked_add(PAGE_SIZE_4K - 1)
            .ok_or(LinuxError::ENOMEM)?
            & !(PAGE_SIZE_4K - 1);
        if len_aligned == 0 {
            return Err(LinuxError::EINVAL);
        }

        let offset = _offset as usize;
        if offset % PAGE_SIZE_4K != 0 {
            return Err(LinuxError::EINVAL);
        }

        let mut map_flags: MappingFlags = prot_bits.into();
        if !map_flags.contains(MappingFlags::USER) {
            map_flags |= MappingFlags::USER;
        }
        let populate = !flag_bits.contains(MmapFlags::MAP_NORESERVE);

        let curr = current();
        let task_ext = curr.task_ext();
        let mut aspace = task_ext.aspace.lock();

        let base = aspace.base();
        let limit = VirtAddrRange::from_start_size(base, aspace.size());
        let req_addr = addr as usize;

        let target = if flag_bits.contains(MmapFlags::MAP_FIXED) {
            if addr.is_null() {
                return Err(LinuxError::EINVAL);
            }
            if req_addr % PAGE_SIZE_4K != 0 {
                return Err(LinuxError::EINVAL);
            }
            let vaddr = VirtAddr::from(req_addr);
            if !aspace.contains_range(vaddr, len_aligned) {
                return Err(LinuxError::EINVAL);
            }
            vaddr
        } else {
            let hint = if addr.is_null() {
                base
            } else {
                VirtAddr::from(req_addr & !(PAGE_SIZE_4K - 1))
            };
            aspace
                .find_free_area(hint, len_aligned, limit)
                .or_else(|| {
                    if hint != base {
                        aspace.find_free_area(base, len_aligned, limit)
                    } else {
                        None
                    }
                })
                .ok_or(LinuxError::ENOMEM)?
        };

        aspace
            .map_alloc(target, len_aligned, map_flags, populate)
            .map_err(ax_err_to_linux)?;

        if !flag_bits.contains(MmapFlags::MAP_ANONYMOUS) {
            let saved = syscall_ret_to_isize(api::sys_lseek(fd, 0, SEEK_CUR) as isize)?;
            syscall_ret_to_isize(api::sys_lseek(fd, offset as _, SEEK_SET) as isize)?;

            let mut buf: Vec<u8> = Vec::with_capacity(length);
            buf.resize(length, 0);
            let read_len = syscall_ret_to_isize(
                api::sys_read(fd, buf.as_mut_ptr() as *mut c_void, length) as isize,
            )? as usize;

            if read_len > 0 {
                aspace
                    .write(target, &buf[..read_len])
                    .map_err(ax_err_to_linux)?;
            }

            syscall_ret_to_isize(api::sys_lseek(fd, saved as _, SEEK_SET) as isize)?;
        }

        Ok(target.as_usize())
    })
}

fn sys_openat(dfd: c_int, fname: *const c_char, flags: c_int, mode: api::ctypes::mode_t) -> isize {
    assert_eq!(dfd, AT_FDCWD);
    api::sys_open(fname, flags, mode) as isize
}

fn sys_close(fd: i32) -> isize {
    api::sys_close(fd) as isize
}

fn sys_read(fd: i32, buf: *mut c_void, count: usize) -> isize {
    api::sys_read(fd, buf, count)
}

fn sys_write(fd: i32, buf: *const c_void, count: usize) -> isize {
    api::sys_write(fd, buf, count)
}

fn sys_writev(fd: i32, iov: *const api::ctypes::iovec, iocnt: i32) -> isize {
    unsafe { api::sys_writev(fd, iov, iocnt) }
}

fn sys_set_tid_address(tid_ptd: *const i32) -> isize {
    let curr = current();
    curr.task_ext().set_clear_child_tid(tid_ptd as _);
    curr.id().as_u64() as isize
}

fn sys_ioctl(_fd: i32, _op: usize, _argp: *mut c_void) -> i32 {
    ax_println!("Ignore SYS_IOCTL");
    0
}

fn ax_err_to_linux(err: AxError) -> LinuxError {
    match err {
        AxError::NoMemory => LinuxError::ENOMEM,
        AxError::InvalidInput | AxError::BadState => LinuxError::EINVAL,
        AxError::BadAddress => LinuxError::EFAULT,
        AxError::AlreadyExists => LinuxError::EEXIST,
        AxError::NotFound => LinuxError::ENOENT,
        AxError::PermissionDenied => LinuxError::EACCES,
        AxError::WouldBlock => LinuxError::EAGAIN,
        _ => LinuxError::EFAULT,
    }
}

fn syscall_ret_to_isize(ret: isize) -> Result<isize, LinuxError> {
    if ret < 0 {
        let errno = (-ret) as i32;
        Err(LinuxError::try_from(errno).unwrap_or(LinuxError::EINVAL))
    } else {
        Ok(ret)
    }
}
