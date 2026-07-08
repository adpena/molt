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
}
