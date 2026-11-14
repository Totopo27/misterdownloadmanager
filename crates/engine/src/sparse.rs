//! Sparse part files (D-008 amendment, 2026-10-29).
//!
//! Extending a file with `set_len` and then writing far into it makes NTFS zero-fill everything
//! before the write, synchronously: one 1 MiB write at 2 GiB took 4.3 s on the user's D: drive
//! (and doubles the bytes written). The planner writes at the middle of the file within the
//! first second, so every large download paid it. A sparse file has no such fill.
//!
//! Linux's own NTFS driver (`ntfs3`) fills the same way: a download to an NTFS drive wrote
//! gigabytes of zeros before its first byte, stood at 0% meanwhile and held up every other
//! program's disk (14 GB waiting to be written, a third of the time spent waiting on the disk).
//! It takes the same sparse attribute through its `system.ntfs_attrib` extended attribute.
//! Elsewhere (`ftruncate` already makes sparse files) this is a no-op.
//!
//! The engine's unsafe code: one `DeviceIoControl` call here (and, in its test, one
//! `GetCompressedFileSizeW`), and the free-space probe's calls in `crate::disk`: two on Windows,
//! one on macOS (with its CoreFoundation key, and the release of what it returns).
#![allow(unsafe_code)]

use std::fs::File;

/// Smaller files aren't worth it: the fill is short.
pub const MIN_SIZE: u64 = 5 << 20;

/// Mark `file` sparse (before `set_len`) or, once every byte is written, normal again (clearing
/// allocates any holes still there; Linux's ntfs3 can't and leaves it sparse). False where the
/// file system can't (FAT, exFAT, and every file system but NTFS on Linux): the download then
/// works as before.
pub fn set(file: &File, sparse: bool) -> bool {
    imp::set(file, sparse)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{FILE_SET_SPARSE_BUFFER, FSCTL_SET_SPARSE};

    pub fn set(file: &std::fs::File, sparse: bool) -> bool {
        let input = FILE_SET_SPARSE_BUFFER { SetSparse: sparse };
        let mut returned = 0u32;
        // SAFETY: `file` owns the handle and outlives the call; the input pointer and size
        // describe `input`, a FILE_SET_SPARSE_BUFFER on the stack; there is no output buffer;
        // `returned` is a valid u32; no OVERLAPPED, so the call completes synchronously.
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_SPARSE,
                (&raw const input).cast(),
                size_of::<FILE_SET_SPARSE_BUFFER>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        ok != 0
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use rustix::fs::{XattrFlags, fgetxattr, fsetxattr};

    /// ntfs3's view of the file's NTFS attributes (a native-endian u32).
    const NTFS_ATTRIB: &str = "system.ntfs_attrib";
    const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;

    /// Other file systems have no such attribute and say so: nothing to do there. ntfs3 can't
    /// clear it again (EOPNOTSUPP), and needn't: a sparse file whose every byte is written
    /// reads and behaves as any other.
    pub fn set(file: &std::fs::File, sparse: bool) -> bool {
        if !sparse {
            return false;
        }
        let mut buf = [0u8; 4];
        let Ok(4) = fgetxattr(file, NTFS_ATTRIB, &mut buf) else {
            return false;
        };
        let attrib = u32::from_ne_bytes(buf);
        if attrib & FILE_ATTRIBUTE_SPARSE_FILE != 0 {
            return true;
        }
        let new = (attrib | FILE_ATTRIBUTE_SPARSE_FILE).to_ne_bytes();
        fsetxattr(file, NTFS_ATTRIB, &new, XattrFlags::empty()).is_ok()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub fn set(_: &std::fs::File, _: bool) -> bool {
        false
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::windows::fs::MetadataExt;

    use super::*;

    const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;

    fn is_sparse(f: &File) -> bool {
        f.metadata().unwrap().file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE != 0
    }

    /// The bytes the file takes on the drive (for a sparse file, only its written ranges).
    fn on_drive(path: &std::path::Path) -> u64 {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut high = 0u32;
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call; `high` is a
        // valid u32 for the size's upper half.
        let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
        if low == u32::MAX {
            // Also a valid low half: only the error code tells.
            let e = std::io::Error::last_os_error();
            assert_eq!(e.raw_os_error(), Some(0), "GetCompressedFileSizeW: {e}");
        }
        (u64::from(high) << 32) | u64::from(low)
    }

    #[test]
    fn a_sparse_part_file_is_written_far_in_without_a_fill() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.bin");
        let mut f = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        assert!(set(&f, true), "NTFS temp folder");
        assert!(is_sparse(&f));
        f.set_len(4 << 30).unwrap();
        f.seek(SeekFrom::Start(2 << 30)).unwrap();
        f.write_all(&[7u8; 1 << 20]).unwrap();
        f.sync_data().unwrap();
        // A fill would have written the 2 GiB before the write (0.5–4 s on this machine), and
        // a file that isn't sparse takes its whole 4 GiB on the drive. This one holds only what
        // was written: measured on the drive, not timed (#201: a 400 ms budget failed on a busy
        // CI runner and on a starved CPU).
        let used = on_drive(&path);
        assert!(
            ((1 << 20)..(64 << 20)).contains(&used),
            "{used} bytes on the drive"
        );
        // Clearing allocates any holes left, so only a fully written file is cleared: shrink
        // this mostly empty one first.
        f.set_len(0).unwrap();
        assert!(set(&f, false));
        assert!(!is_sparse(&f));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use std::os::unix::fs::{FileExt, MetadataExt};

    use super::*;

    fn part_file(dir: &std::path::Path) -> File {
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.join("p.bin"))
            .unwrap()
    }

    #[test]
    fn other_file_systems_are_left_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let f = part_file(dir.path());
        assert!(!set(&f, true));
        assert!(!set(&f, false));
    }

    /// An NTFS folder mounted with ntfs3: `MDM_TEST_NTFS_DIR=/mnt/ntfs cargo test -p mdm-engine
    /// sparse -- --ignored`.
    #[test]
    #[ignore = "needs an NTFS folder (ntfs3) in MDM_TEST_NTFS_DIR"]
    fn an_ntfs3_part_file_is_written_far_in_without_a_fill() {
        let root = std::env::var_os("MDM_TEST_NTFS_DIR").expect("MDM_TEST_NTFS_DIR");
        let dir = tempfile::tempdir_in(root).unwrap();
        let f = part_file(dir.path());
        assert!(set(&f, true), "ntfs3 takes the sparse attribute");
        assert!(set(&f, true), "and says so again");
        f.set_len(4 << 30).unwrap();
        f.write_all_at(&[7u8; 1 << 20], 2 << 30).unwrap();
        f.sync_data().unwrap();
        // Filled, the file would take its 2 GiB before the write on the drive.
        let used = f.metadata().unwrap().blocks() * 512;
        assert!(
            ((1 << 20)..(64 << 20)).contains(&used),
            "{used} bytes on the drive"
        );
    }
}
