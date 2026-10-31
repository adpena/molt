//! Pure `stat` module support shared by runtime bridges.
//!
//! The parent runtime owns Python object conversion and exported ABI entrypoints.
//! This module owns POSIX/Windows mode constants, bit tests, and filemode text.

pub const S_IFMT_MASK: i64 = 0o170000;
pub const S_IFSOCK: i64 = 0o140000;
pub const S_IFLNK: i64 = 0o120000;
pub const S_IFREG: i64 = 0o100000;
pub const S_IFBLK: i64 = 0o060000;
pub const S_IFDIR: i64 = 0o040000;
pub const S_IFCHR: i64 = 0o020000;
pub const S_IFIFO: i64 = 0o010000;
pub const S_IFDOOR: i64 = 0;
pub const S_IFPORT: i64 = 0;
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub const S_IFWHT: i64 = 0o160000;
#[cfg(not(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
pub const S_IFWHT: i64 = 0;

pub const S_ISUID: i64 = 0o004000;
pub const S_ISGID: i64 = 0o002000;
pub const S_ISVTX: i64 = 0o001000;
pub const S_IRUSR: i64 = 0o000400;
pub const S_IWUSR: i64 = 0o000200;
pub const S_IXUSR: i64 = 0o000100;
pub const S_IRGRP: i64 = 0o000040;
pub const S_IWGRP: i64 = 0o000020;
pub const S_IXGRP: i64 = 0o000010;
pub const S_IROTH: i64 = 0o000004;
pub const S_IWOTH: i64 = 0o000002;
pub const S_IXOTH: i64 = 0o000001;
pub const S_IMODE_MASK: i64 = 0o7777;

pub const ST_MODE: i64 = 0;
pub const ST_INO: i64 = 1;
pub const ST_DEV: i64 = 2;
pub const ST_NLINK: i64 = 3;
pub const ST_UID: i64 = 4;
pub const ST_GID: i64 = 5;
pub const ST_SIZE: i64 = 6;
pub const ST_ATIME: i64 = 7;
pub const ST_MTIME: i64 = 8;
pub const ST_CTIME: i64 = 9;

pub const UF_NODUMP: i64 = 0x00000001;
pub const UF_IMMUTABLE: i64 = 0x00000002;
pub const UF_APPEND: i64 = 0x00000004;
pub const UF_OPAQUE: i64 = 0x00000008;
pub const UF_NOUNLINK: i64 = 0x00000010;
pub const UF_SETTABLE: i64 = 0x0000ffff;
pub const UF_COMPRESSED: i64 = 0x00000020;
pub const UF_TRACKED: i64 = 0x00000040;
pub const UF_DATAVAULT: i64 = 0x00000080;
pub const UF_HIDDEN: i64 = 0x00008000;
pub const SF_ARCHIVED: i64 = 0x00010000;
pub const SF_IMMUTABLE: i64 = 0x00020000;
pub const SF_APPEND: i64 = 0x00040000;
pub const SF_SETTABLE: i64 = 0x3fff0000;
pub const SF_RESTRICTED: i64 = 0x00080000;
pub const SF_NOUNLINK: i64 = 0x00100000;
pub const SF_SNAPSHOT: i64 = 0x00200000;
pub const SF_FIRMLINK: i64 = 0x00800000;
pub const SF_DATALESS: i64 = 0x40000000;
pub const SF_SUPPORTED: i64 = 0x009f0000;
pub const SF_SYNTHETIC: i64 = 0xc0000000;

pub const FILE_ATTRIBUTE_ARCHIVE: i64 = 32;
pub const FILE_ATTRIBUTE_COMPRESSED: i64 = 2048;
pub const FILE_ATTRIBUTE_DEVICE: i64 = 64;
pub const FILE_ATTRIBUTE_DIRECTORY: i64 = 16;
pub const FILE_ATTRIBUTE_ENCRYPTED: i64 = 16384;
pub const FILE_ATTRIBUTE_HIDDEN: i64 = 2;
pub const FILE_ATTRIBUTE_INTEGRITY_STREAM: i64 = 32768;
pub const FILE_ATTRIBUTE_NORMAL: i64 = 128;
pub const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: i64 = 8192;
pub const FILE_ATTRIBUTE_NO_SCRUB_DATA: i64 = 131072;
pub const FILE_ATTRIBUTE_OFFLINE: i64 = 4096;
pub const FILE_ATTRIBUTE_READONLY: i64 = 1;
pub const FILE_ATTRIBUTE_REPARSE_POINT: i64 = 1024;
pub const FILE_ATTRIBUTE_SPARSE_FILE: i64 = 512;
pub const FILE_ATTRIBUTE_SYSTEM: i64 = 4;
pub const FILE_ATTRIBUTE_TEMPORARY: i64 = 256;
pub const FILE_ATTRIBUTE_VIRTUAL: i64 = 65536;

#[cfg(target_os = "windows")]
pub const IO_REPARSE_TAG_APPEXECLINK: i64 = 0x8000001b;
#[cfg(not(target_os = "windows"))]
pub const IO_REPARSE_TAG_APPEXECLINK: i64 = 0;
#[cfg(target_os = "windows")]
pub const IO_REPARSE_TAG_MOUNT_POINT: i64 = 0xa0000003;
#[cfg(not(target_os = "windows"))]
pub const IO_REPARSE_TAG_MOUNT_POINT: i64 = 0;
#[cfg(target_os = "windows")]
pub const IO_REPARSE_TAG_SYMLINK: i64 = 0xa000000c;
#[cfg(not(target_os = "windows"))]
pub const IO_REPARSE_TAG_SYMLINK: i64 = 0;

pub fn stat_constants_payload(has_313_constants: bool) -> Vec<i64> {
    vec![
        S_IFMT_MASK,
        S_IFSOCK,
        S_IFLNK,
        S_IFREG,
        S_IFBLK,
        S_IFDIR,
        S_IFCHR,
        S_IFIFO,
        S_IFDOOR,
        S_IFPORT,
        S_IFWHT,
        S_ISUID,
        S_ISGID,
        S_ISVTX,
        S_IRUSR,
        S_IWUSR,
        S_IXUSR,
        S_IRGRP,
        S_IWGRP,
        S_IXGRP,
        S_IROTH,
        S_IWOTH,
        S_IXOTH,
        ST_MODE,
        ST_INO,
        ST_DEV,
        ST_NLINK,
        ST_UID,
        ST_GID,
        ST_SIZE,
        ST_ATIME,
        ST_MTIME,
        ST_CTIME,
        UF_NODUMP,
        UF_IMMUTABLE,
        UF_APPEND,
        UF_OPAQUE,
        UF_NOUNLINK,
        UF_COMPRESSED,
        UF_HIDDEN,
        SF_ARCHIVED,
        SF_IMMUTABLE,
        SF_APPEND,
        SF_NOUNLINK,
        SF_SNAPSHOT,
        if has_313_constants { UF_SETTABLE } else { 0 },
        if has_313_constants { UF_TRACKED } else { 0 },
        if has_313_constants { UF_DATAVAULT } else { 0 },
        if has_313_constants { SF_SETTABLE } else { 0 },
        if has_313_constants { SF_RESTRICTED } else { 0 },
        if has_313_constants { SF_FIRMLINK } else { 0 },
        if has_313_constants { SF_DATALESS } else { 0 },
        if has_313_constants { SF_SUPPORTED } else { 0 },
        if has_313_constants { SF_SYNTHETIC } else { 0 },
        FILE_ATTRIBUTE_ARCHIVE,
        FILE_ATTRIBUTE_COMPRESSED,
        FILE_ATTRIBUTE_DEVICE,
        FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_ENCRYPTED,
        FILE_ATTRIBUTE_HIDDEN,
        FILE_ATTRIBUTE_INTEGRITY_STREAM,
        FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
        FILE_ATTRIBUTE_NO_SCRUB_DATA,
        FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_SPARSE_FILE,
        FILE_ATTRIBUTE_SYSTEM,
        FILE_ATTRIBUTE_TEMPORARY,
        FILE_ATTRIBUTE_VIRTUAL,
        IO_REPARSE_TAG_APPEXECLINK,
        IO_REPARSE_TAG_MOUNT_POINT,
        IO_REPARSE_TAG_SYMLINK,
    ]
}

pub fn stat_ifmt(mode: i64) -> i64 {
    mode & S_IFMT_MASK
}

pub fn stat_imode(mode: i64) -> i64 {
    mode & S_IMODE_MASK
}

pub fn stat_isdir(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFDIR
}

pub fn stat_isreg(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFREG
}

pub fn stat_ischr(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFCHR
}

pub fn stat_isblk(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFBLK
}

pub fn stat_isfifo(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFIFO
}

pub fn stat_islnk(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFLNK
}

pub fn stat_issock(mode: i64) -> bool {
    stat_ifmt(mode) == S_IFSOCK
}

pub fn stat_isdoor(mode: i64) -> bool {
    S_IFDOOR != 0 && stat_ifmt(mode) == S_IFDOOR
}

pub fn stat_isport(mode: i64) -> bool {
    S_IFPORT != 0 && stat_ifmt(mode) == S_IFPORT
}

pub fn stat_iswht(mode: i64) -> bool {
    S_IFWHT != 0 && stat_ifmt(mode) == S_IFWHT
}

pub fn stat_filemode(mode: i64) -> String {
    let file_type = stat_ifmt(mode);
    let mut out = String::with_capacity(10);
    let type_char = if file_type == S_IFLNK {
        'l'
    } else if file_type == S_IFSOCK {
        's'
    } else if file_type == S_IFREG {
        '-'
    } else if file_type == S_IFBLK {
        'b'
    } else if file_type == S_IFDIR {
        'd'
    } else if file_type == S_IFCHR {
        'c'
    } else if file_type == S_IFIFO {
        'p'
    } else if S_IFDOOR != 0 && file_type == S_IFDOOR {
        'D'
    } else if S_IFPORT != 0 && file_type == S_IFPORT {
        'P'
    } else if S_IFWHT != 0 && file_type == S_IFWHT {
        'w'
    } else {
        '?'
    };
    out.push(type_char);
    out.push(if (mode & S_IRUSR) != 0 { 'r' } else { '-' });
    out.push(if (mode & S_IWUSR) != 0 { 'w' } else { '-' });
    out.push(match ((mode & S_IXUSR) != 0, (mode & S_ISUID) != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });
    out.push(if (mode & S_IRGRP) != 0 { 'r' } else { '-' });
    out.push(if (mode & S_IWGRP) != 0 { 'w' } else { '-' });
    out.push(match ((mode & S_IXGRP) != 0, (mode & S_ISGID) != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });
    out.push(if (mode & S_IROTH) != 0 { 'r' } else { '-' });
    out.push(if (mode & S_IWOTH) != 0 { 'w' } else { '-' });
    out.push(match ((mode & S_IXOTH) != 0, (mode & S_ISVTX) != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_payload_preserves_order_and_version_gate() {
        let pre_313 = stat_constants_payload(false);
        let py_313 = stat_constants_payload(true);
        assert_eq!(pre_313.len(), py_313.len());
        assert_eq!(pre_313[0], S_IFMT_MASK);
        assert_eq!(pre_313[3], S_IFREG);
        assert_eq!(pre_313[5], S_IFDIR);
        assert_eq!(pre_313[45], 0);
        assert_eq!(py_313[45], UF_SETTABLE);
        assert_eq!(pre_313[54], FILE_ATTRIBUTE_ARCHIVE);
    }

    #[test]
    fn masks_and_predicates_share_one_authority() {
        let dir_mode = S_IFDIR | 0o755;
        let reg_mode = S_IFREG | 0o644;
        assert_eq!(stat_ifmt(dir_mode), S_IFDIR);
        assert_eq!(stat_imode(dir_mode), 0o755);
        assert!(stat_isdir(dir_mode));
        assert!(!stat_isreg(dir_mode));
        assert!(stat_isreg(reg_mode));
        assert!(stat_islnk(S_IFLNK));
        assert!(stat_issock(S_IFSOCK));
        assert!(!stat_isdoor(S_IFREG));
        assert!(!stat_isport(S_IFREG));
    }

    #[test]
    fn filemode_formats_type_and_special_permission_bits() {
        assert_eq!(
            stat_filemode(S_IFREG | S_IRUSR | S_IWUSR | S_IRGRP | S_IROTH),
            "-rw-r--r--"
        );
        assert_eq!(
            stat_filemode(S_IFLNK | S_IRUSR | S_IWUSR | S_IXUSR),
            "lrwx------"
        );
        assert_eq!(stat_filemode(S_IFREG | S_ISUID | S_IXUSR), "---s------");
        assert_eq!(stat_filemode(S_IFREG | S_ISGID | S_IXGRP), "------s---");
        assert_eq!(stat_filemode(S_IFDIR | S_ISVTX | S_IXOTH), "d--------t");
    }

    #[cfg(any(unix, windows))]
    fn host_stat(path: &std::path::Path) -> PathMetadata {
        #[cfg(unix)]
        {
            PathMetadata::from_host(&std::fs::metadata(path).unwrap())
        }
        #[cfg(windows)]
        {
            PathMetadata::from_windows_path(path, true).unwrap()
        }
    }

    /// CPython's samestat compares (st_ino, st_dev); hard links share them and
    /// count each other in st_nlink, and a distinct file differs.
    #[cfg(any(unix, windows))]
    #[test]
    fn host_stat_reports_the_file_identity_and_link_count() {
        let dir = std::env::temp_dir().join(format!("molt-stat-identity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first");
        let linked = dir.join("linked");
        let other = dir.join("other");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&other, b"a").unwrap();
        std::fs::hard_link(&first, &linked).unwrap();
        let (a, b, c) = (host_stat(&first), host_stat(&linked), host_stat(&other));
        assert_ne!(a.ino(), 0);
        assert_eq!((a.ino(), a.fields[2]), (b.ino(), b.fields[2]));
        assert_ne!((a.ino(), a.fields[2]), (c.ino(), c.fields[2]));
        assert_eq!((a.fields[3], b.fields[3], c.fields[3]), (2, 2, 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Exact signed timestamp shared by the route/query and all projections.
/// Nanoseconds fit i128 for every native i64 second and VFS u64 second input.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatTime(i128);

impl StatTime {
    pub const fn from_parts(seconds: i128, nanoseconds: i128) -> Self {
        Self(seconds * 1_000_000_000 + nanoseconds)
    }
    pub fn from_system_time(time: std::io::Result<std::time::SystemTime>) -> Self {
        match time {
            Ok(time) => match time.duration_since(std::time::UNIX_EPOCH) {
                Ok(value) => Self::from_parts(
                    i128::from(value.as_secs()),
                    i128::from(value.subsec_nanos()),
                ),
                Err(error) => {
                    let value = error.duration();
                    Self(
                        -Self::from_parts(
                            i128::from(value.as_secs()),
                            i128::from(value.subsec_nanos()),
                        )
                        .0,
                    )
                }
            },
            Err(_) => Self::default(),
        }
    }
    pub const fn nanoseconds(self) -> i128 {
        self.0
    }
    pub fn seconds(self) -> i128 {
        self.0.div_euclid(1_000_000_000)
    }
    pub fn as_float(self) -> f64 {
        self.seconds() as f64 + self.0.rem_euclid(1_000_000_000) as f64 * 1e-9
    }
}

/// Fixed metadata projection shared by direct and satellite filesystem callers.
/// Virtual files expose only backend-owned facts; zero identity fields are
/// unknown and must never be used by samefile. All time views derive from times.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PathMetadata {
    pub fields: [i128; 7],
    pub times: [StatTime; 3],
    pub virtual_file: bool,
    #[cfg(any(unix, target_os = "wasi"))]
    pub block_size: i64,
    #[cfg(any(unix, target_os = "wasi"))]
    pub blocks: i64,
    #[cfg(any(unix, target_os = "wasi"))]
    pub rdev: u64,
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    pub flags: i64,
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    pub generation: i64,
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        windows
    ))]
    pub birthtime: StatTime,
    #[cfg(windows)]
    pub file_attributes: u32,
    #[cfg(windows)]
    pub reparse_tag: u32,
    #[cfg(any(target_os = "solaris", target_os = "illumos"))]
    pub fstype: [u8; 16],
}

/// CPython changed st_rdev from PyLong_FromLong to _PyLong_FromDev in 3.13.
/// The device acquisition retains C's unsigned-long-long view; the Python
/// record owner selects the required publication convention.
#[cfg(any(unix, target_os = "wasi"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceNumberConvention {
    CLong,
    PythonDev,
}

#[cfg(any(unix, target_os = "wasi"))]
#[inline]
#[allow(clippy::unnecessary_cast)] // Native dev_t can be signed or unsigned.
pub fn device_number_integer(value: u64, convention: DeviceNumberConvention) -> i128 {
    match convention {
        DeviceNumberConvention::CLong => i128::from(value as libc::c_long),
        DeviceNumberConvention::PythonDev => {
            // The BSD sys/param.h NODEV declaration is (dev_t)(-1), as in
            // Apple's SDK and FreeBSD's native headers. CPython preserves its
            // signed-long-long view, and unsigned-casts every other device.
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            {
                let nodev: libc::dev_t = !0;
                if value == nodev as u64 {
                    return i128::from(value as i64);
                }
            }
            i128::from(value)
        }
    }
}

impl PathMetadata {
    pub fn is_dir(&self) -> bool {
        stat_isdir(self.fields[ST_MODE as usize] as i64)
    }
    pub fn is_file(&self) -> bool {
        stat_isreg(self.fields[ST_MODE as usize] as i64)
    }
    pub fn is_symlink(&self) -> bool {
        stat_islnk(self.fields[ST_MODE as usize] as i64)
    }

    #[cfg(unix)]
    // CPython's posixmodule projects these fields through C long, whose width
    // differs from the native stat fields on supported Unix architectures.
    #[allow(clippy::unnecessary_cast)]
    pub fn from_unix_stat(st: &libc::stat) -> Self {
        #[cfg(not(target_os = "netbsd"))]
        let times = [
            StatTime::from_parts(st.st_atime as i128, st.st_atime_nsec as i128),
            StatTime::from_parts(st.st_mtime as i128, st.st_mtime_nsec as i128),
            StatTime::from_parts(st.st_ctime as i128, st.st_ctime_nsec as i128),
        ];
        #[cfg(target_os = "netbsd")]
        let times = [
            StatTime::from_parts(st.st_atime as i128, st.st_atimensec as i128),
            StatTime::from_parts(st.st_mtime as i128, st.st_mtimensec as i128),
            StatTime::from_parts(st.st_ctime as i128, st.st_ctimensec as i128),
        ];
        Self {
            fields: [
                st.st_mode as i128,
                st.st_ino as i128,
                device_number_integer(st.st_dev as u64, DeviceNumberConvention::PythonDev),
                (st.st_nlink as libc::c_long) as i128,
                i128::from(crate::utils::uid_integer(st.st_uid)),
                i128::from(crate::utils::gid_integer(st.st_gid)),
                st.st_size as i128,
            ],
            times,
            virtual_file: false,
            block_size: (st.st_blksize as libc::c_long) as i64,
            blocks: (st.st_blocks as libc::c_long) as i64,
            rdev: st.st_rdev as u64,
            #[cfg(any(target_os = "solaris", target_os = "illumos"))]
            fstype: st.st_fstype.map(|byte| byte as u8),
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            flags: (st.st_flags as libc::c_long) as i64,
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            generation: (st.st_gen as libc::c_long) as i64,
            #[cfg(any(target_vendor = "apple", target_os = "freebsd", target_os = "openbsd"))]
            birthtime: StatTime::from_parts(st.st_birthtime as i128, st.st_birthtime_nsec as i128),
            #[cfg(target_os = "netbsd")]
            birthtime: StatTime::from_parts(st.st_birthtime as i128, st.st_birthtimensec as i128),
        }
    }

    #[cfg(not(windows))]
    pub fn from_host(metadata: &std::fs::Metadata) -> Self {
        Self::from_host_base(metadata)
    }

    /// CPython's `win32_xstat_slow_impl` order: open the path for metadata and
    /// read every fact, identity included, from that one handle. Only when the
    /// open is refused for access or sharing does CPython answer from the
    /// parent directory entry, which carries no identity facts (zero `st_ino`,
    /// `st_dev` and `st_nlink`).
    #[cfg(windows)]
    pub fn from_windows_path(path: &std::path::Path, follow: bool) -> std::io::Result<Self> {
        match crate::windows_abi::open_metadata_file(path, follow) {
            Ok(file) => Self::from_windows_file(&file),
            Err(error) if crate::windows_abi::metadata_open_reads_directory_entry(&error) => {
                let metadata = if follow {
                    std::fs::metadata(path)
                } else {
                    std::fs::symlink_metadata(path)
                }?;
                Ok(Self::from_host_base(&metadata))
            }
            Err(error) => Err(error),
        }
    }

    /// Borrow an admitted descriptor or owned path-query handle. A tag-query
    /// error remains an OS error; it cannot become a fabricated zero tag.
    #[cfg(windows)]
    pub fn from_windows_file(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::windows::fs::MetadataExt;
        let metadata = file.metadata()?;
        let mut result = Self::from_host_base(&metadata);
        let facts = crate::windows_abi::file_identity_facts(file)?;
        // The ID is 128 bits; `ino` reads this field back unsigned.
        result.fields[1] = facts.file_id as i128;
        result.fields[2] = i128::from(facts.volume_serial);
        result.fields[3] = i128::from(facts.links);
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT as u32 != 0 {
            (result.file_attributes, result.reparse_tag) =
                crate::windows_abi::file_attribute_tag(file)?;
        }
        Ok(result)
    }

    /// `st_ino`: the file index, unsigned. A Windows file ID fills all 128 bits
    /// of `fields[1]`; a POSIX inode fits in its low 64.
    pub fn ino(&self) -> u128 {
        self.fields[1] as u128
    }

    // Keep the same intermediate C long conversion as from_unix_stat; a cast
    // that is redundant on one host can be the required narrowing on another.
    #[allow(clippy::unnecessary_cast)]
    fn from_host_base(metadata: &std::fs::Metadata) -> Self {
        // This adapter never adds a query: the admitted route owns acquisition.
        let mut result = Self::from_virtual(
            metadata.is_file(),
            metadata.is_dir(),
            metadata.permissions().readonly(),
            metadata.len(),
            0,
        );
        result.virtual_file = false;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            result.fields = [
                metadata.mode() as i128,
                metadata.ino() as i128,
                device_number_integer(metadata.dev(), DeviceNumberConvention::PythonDev),
                (metadata.nlink() as libc::c_long) as i128,
                i128::from(crate::utils::uid_integer(metadata.uid())),
                i128::from(crate::utils::gid_integer(metadata.gid())),
                metadata.size() as i128,
            ];
            result.times = [
                StatTime::from_parts(metadata.atime() as i128, metadata.atime_nsec() as i128),
                StatTime::from_parts(metadata.mtime() as i128, metadata.mtime_nsec() as i128),
                StatTime::from_parts(metadata.ctime() as i128, metadata.ctime_nsec() as i128),
            ];
            result.block_size = (metadata.blksize() as libc::c_long) as i64;
            result.blocks = (metadata.blocks() as libc::c_long) as i64;
            result.rdev = metadata.rdev();
        }
        #[cfg(target_vendor = "apple")]
        {
            use std::os::macos::fs::MetadataExt;
            result.flags = (metadata.st_flags() as libc::c_long) as i64;
            result.generation = (metadata.st_gen() as libc::c_long) as i64;
        }
        #[cfg(not(unix))]
        {
            if metadata.file_type().is_symlink() {
                result.fields[0] =
                    (result.fields[0] & !i128::from(S_IFMT_MASK)) | i128::from(S_IFLNK);
            }
            result.times = [
                StatTime::from_system_time(metadata.accessed()),
                StatTime::from_system_time(metadata.modified()),
                StatTime::from_system_time(metadata.created()),
            ];
        }
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            windows
        ))]
        {
            result.birthtime = StatTime::from_system_time(metadata.created());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            result.file_attributes = metadata.file_attributes();
        }
        result
    }

    pub fn from_virtual(
        is_file: bool,
        is_dir: bool,
        readonly: bool,
        size: u64,
        mtime: u64,
    ) -> Self {
        let kind = if is_dir {
            S_IFDIR
        } else if is_file {
            S_IFREG
        } else {
            0
        };
        let permissions = if readonly {
            if is_dir { 0o555 } else { 0o444 }
        } else if is_dir {
            0o777
        } else {
            0o666
        };
        Self {
            fields: [
                i128::from(kind | permissions),
                0,
                0,
                0,
                0,
                0,
                i128::from(size),
            ],
            times: [
                StatTime::default(),
                StatTime::from_parts(i128::from(mtime), 0),
                StatTime::default(),
            ],
            virtual_file: true,
            #[cfg(any(unix, target_os = "wasi"))]
            block_size: 0,
            #[cfg(any(unix, target_os = "wasi"))]
            blocks: 0,
            #[cfg(any(unix, target_os = "wasi"))]
            rdev: 0,
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            flags: 0,
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            generation: 0,
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                windows
            ))]
            birthtime: StatTime::default(),
            #[cfg(windows)]
            file_attributes: 0,
            #[cfg(windows)]
            reparse_tag: 0,
            #[cfg(any(target_os = "solaris", target_os = "illumos"))]
            fstype: [0; 16],
        }
    }
}

#[cfg(test)]
#[path = "stat_support/metadata_tests.rs"]
mod metadata_time_tests;
