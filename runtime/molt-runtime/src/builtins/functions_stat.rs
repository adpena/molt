use molt_obj_model::MoltObject;

use crate::{alloc_string, alloc_tuple, obj_from_bits, raise_exception, to_i64};

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        fn stat_target_minor() -> i64 {
            // Check explicit env-var overrides first.  Do NOT read from
            // state.sys_version_info — that field is set by
            // molt_sys_set_version_info which bakes in the *host* Python
            // version (e.g. 3.13) used to run the compiler.  The stat
            // intrinsic must use molt's *target* version (3.12) unless the
            // user explicitly overrides via an env var.
            if let Ok(raw) = std::env::var("MOLT_PYTHON_VERSION")
                && let Some((major_raw, minor_raw)) = raw.split_once('.')
                && major_raw.trim() == "3"
                && let Ok(minor) = minor_raw.trim().parse::<i64>()
            {
                return minor;
            }
            if let Ok(raw) = std::env::var("MOLT_SYS_VERSION_INFO") {
                let mut parts = raw.split(',');
                if let (Some(major_raw), Some(minor_raw)) = (parts.next(), parts.next())
                    && major_raw.trim() == "3"
                    && let Ok(minor) = minor_raw.trim().parse::<i64>()
                {
                    return minor;
                }
            }
            12
        }

        let has_313_constants = stat_target_minor() >= 13;
        const S_IFMT_MASK: i64 = 0o170000;
        const S_IFSOCK: i64 = 0o140000;
        const S_IFLNK: i64 = 0o120000;
        const S_IFREG: i64 = 0o100000;
        const S_IFBLK: i64 = 0o060000;
        const S_IFDIR: i64 = 0o040000;
        const S_IFCHR: i64 = 0o020000;
        const S_IFIFO: i64 = 0o010000;
        const S_IFDOOR: i64 = 0;
        const S_IFPORT: i64 = 0;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        const S_IFWHT: i64 = 0o160000;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        const S_IFWHT: i64 = 0;
        const S_ISUID: i64 = 0o004000;
        const S_ISGID: i64 = 0o002000;
        const S_ISVTX: i64 = 0o001000;
        const S_IRUSR: i64 = 0o000400;
        const S_IWUSR: i64 = 0o000200;
        const S_IXUSR: i64 = 0o000100;
        const S_IRGRP: i64 = 0o000040;
        const S_IWGRP: i64 = 0o000020;
        const S_IXGRP: i64 = 0o000010;
        const S_IROTH: i64 = 0o000004;
        const S_IWOTH: i64 = 0o000002;
        const S_IXOTH: i64 = 0o000001;
        const ST_MODE: i64 = 0;
        const ST_INO: i64 = 1;
        const ST_DEV: i64 = 2;
        const ST_NLINK: i64 = 3;
        const ST_UID: i64 = 4;
        const ST_GID: i64 = 5;
        const ST_SIZE: i64 = 6;
        const ST_ATIME: i64 = 7;
        const ST_MTIME: i64 = 8;
        const ST_CTIME: i64 = 9;
        const UF_NODUMP: i64 = 0x00000001;
        const UF_IMMUTABLE: i64 = 0x00000002;
        const UF_APPEND: i64 = 0x00000004;
        const UF_OPAQUE: i64 = 0x00000008;
        const UF_NOUNLINK: i64 = 0x00000010;
        const UF_SETTABLE: i64 = 0x0000ffff;
        const UF_COMPRESSED: i64 = 0x00000020;
        const UF_TRACKED: i64 = 0x00000040;
        const UF_DATAVAULT: i64 = 0x00000080;
        const UF_HIDDEN: i64 = 0x00008000;
        const SF_ARCHIVED: i64 = 0x00010000;
        const SF_IMMUTABLE: i64 = 0x00020000;
        const SF_APPEND: i64 = 0x00040000;
        const SF_SETTABLE: i64 = 0x3fff0000;
        const SF_RESTRICTED: i64 = 0x00080000;
        const SF_NOUNLINK: i64 = 0x00100000;
        const SF_SNAPSHOT: i64 = 0x00200000;
        const SF_FIRMLINK: i64 = 0x00800000;
        const SF_DATALESS: i64 = 0x40000000;
        const SF_SUPPORTED: i64 = 0x009f0000;
        const SF_SYNTHETIC: i64 = 0xc0000000;
        const FILE_ATTRIBUTE_ARCHIVE: i64 = 32;
        const FILE_ATTRIBUTE_COMPRESSED: i64 = 2048;
        const FILE_ATTRIBUTE_DEVICE: i64 = 64;
        const FILE_ATTRIBUTE_DIRECTORY: i64 = 16;
        const FILE_ATTRIBUTE_ENCRYPTED: i64 = 16384;
        const FILE_ATTRIBUTE_HIDDEN: i64 = 2;
        const FILE_ATTRIBUTE_INTEGRITY_STREAM: i64 = 32768;
        const FILE_ATTRIBUTE_NORMAL: i64 = 128;
        const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: i64 = 8192;
        const FILE_ATTRIBUTE_NO_SCRUB_DATA: i64 = 131072;
        const FILE_ATTRIBUTE_OFFLINE: i64 = 4096;
        const FILE_ATTRIBUTE_READONLY: i64 = 1;
        const FILE_ATTRIBUTE_REPARSE_POINT: i64 = 1024;
        const FILE_ATTRIBUTE_SPARSE_FILE: i64 = 512;
        const FILE_ATTRIBUTE_SYSTEM: i64 = 4;
        const FILE_ATTRIBUTE_TEMPORARY: i64 = 256;
        const FILE_ATTRIBUTE_VIRTUAL: i64 = 65536;
        let payload = [
            MoltObject::from_int(S_IFMT_MASK).bits(),
            MoltObject::from_int(S_IFSOCK).bits(),
            MoltObject::from_int(S_IFLNK).bits(),
            MoltObject::from_int(S_IFREG).bits(),
            MoltObject::from_int(S_IFBLK).bits(),
            MoltObject::from_int(S_IFDIR).bits(),
            MoltObject::from_int(S_IFCHR).bits(),
            MoltObject::from_int(S_IFIFO).bits(),
            MoltObject::from_int(S_IFDOOR).bits(),
            MoltObject::from_int(S_IFPORT).bits(),
            MoltObject::from_int(S_IFWHT).bits(),
            MoltObject::from_int(S_ISUID).bits(),
            MoltObject::from_int(S_ISGID).bits(),
            MoltObject::from_int(S_ISVTX).bits(),
            MoltObject::from_int(S_IRUSR).bits(),
            MoltObject::from_int(S_IWUSR).bits(),
            MoltObject::from_int(S_IXUSR).bits(),
            MoltObject::from_int(S_IRGRP).bits(),
            MoltObject::from_int(S_IWGRP).bits(),
            MoltObject::from_int(S_IXGRP).bits(),
            MoltObject::from_int(S_IROTH).bits(),
            MoltObject::from_int(S_IWOTH).bits(),
            MoltObject::from_int(S_IXOTH).bits(),
            MoltObject::from_int(ST_MODE).bits(),
            MoltObject::from_int(ST_INO).bits(),
            MoltObject::from_int(ST_DEV).bits(),
            MoltObject::from_int(ST_NLINK).bits(),
            MoltObject::from_int(ST_UID).bits(),
            MoltObject::from_int(ST_GID).bits(),
            MoltObject::from_int(ST_SIZE).bits(),
            MoltObject::from_int(ST_ATIME).bits(),
            MoltObject::from_int(ST_MTIME).bits(),
            MoltObject::from_int(ST_CTIME).bits(),
            MoltObject::from_int(UF_NODUMP).bits(),
            MoltObject::from_int(UF_IMMUTABLE).bits(),
            MoltObject::from_int(UF_APPEND).bits(),
            MoltObject::from_int(UF_OPAQUE).bits(),
            MoltObject::from_int(UF_NOUNLINK).bits(),
            MoltObject::from_int(UF_COMPRESSED).bits(),
            MoltObject::from_int(UF_HIDDEN).bits(),
            MoltObject::from_int(SF_ARCHIVED).bits(),
            MoltObject::from_int(SF_IMMUTABLE).bits(),
            MoltObject::from_int(SF_APPEND).bits(),
            MoltObject::from_int(SF_NOUNLINK).bits(),
            MoltObject::from_int(SF_SNAPSHOT).bits(),
            MoltObject::from_int(if has_313_constants { UF_SETTABLE } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { UF_TRACKED } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { UF_DATAVAULT } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_SETTABLE } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_RESTRICTED } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_FIRMLINK } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_DATALESS } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_SUPPORTED } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_SYNTHETIC } else { 0 }).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_ARCHIVE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_COMPRESSED).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_DEVICE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_DIRECTORY).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_ENCRYPTED).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_HIDDEN).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_INTEGRITY_STREAM).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_NORMAL).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_NOT_CONTENT_INDEXED).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_NO_SCRUB_DATA).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_OFFLINE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_READONLY).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_REPARSE_POINT).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_SPARSE_FILE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_SYSTEM).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_TEMPORARY).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_VIRTUAL).bits(),
        ];
        let tuple_ptr = alloc_tuple(_py, &payload);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

fn parse_stat_mode(_py: &crate::PyToken<'_>, mode_bits: u64) -> Result<i64, u64> {
    let Some(mode) = to_i64(obj_from_bits(mode_bits)) else {
        return Err(raise_exception::<_>(_py, "TypeError", "mode must be int"));
    };
    Ok(mode)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_ifmt(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFMT_MASK: i64 = 0o170000;
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(mode & S_IFMT_MASK).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_imode(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IMODE_MASK: i64 = 0o7777;
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(mode & S_IMODE_MASK).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isdir(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o040000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isreg(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o100000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_ischr(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o020000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isblk(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o060000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isfifo(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o010000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_islnk(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o120000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_issock(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o140000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isdoor(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFDOOR: i64 = 0;
        if S_IFDOOR == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == S_IFDOOR).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isport(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFPORT: i64 = 0;
        if S_IFPORT == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == S_IFPORT).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_iswht(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        const S_IFWHT: i64 = 0o160000;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        const S_IFWHT: i64 = 0;
        if S_IFWHT == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == S_IFWHT).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_filemode(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFMT_MASK: i64 = 0o170000;
        const S_IFSOCK: i64 = 0o140000;
        const S_IFLNK: i64 = 0o120000;
        const S_IFREG: i64 = 0o100000;
        const S_IFBLK: i64 = 0o060000;
        const S_IFDIR: i64 = 0o040000;
        const S_IFCHR: i64 = 0o020000;
        const S_IFIFO: i64 = 0o010000;
        const S_IFDOOR: i64 = 0;
        const S_IFPORT: i64 = 0;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        const S_IFWHT: i64 = 0o160000;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        const S_IFWHT: i64 = 0;
        const S_ISUID: i64 = 0o004000;
        const S_ISGID: i64 = 0o002000;
        const S_ISVTX: i64 = 0o001000;
        const S_IRUSR: i64 = 0o000400;
        const S_IWUSR: i64 = 0o000200;
        const S_IXUSR: i64 = 0o000100;
        const S_IRGRP: i64 = 0o000040;
        const S_IWGRP: i64 = 0o000020;
        const S_IXGRP: i64 = 0o000010;
        const S_IROTH: i64 = 0o000004;
        const S_IWOTH: i64 = 0o000002;
        const S_IXOTH: i64 = 0o000001;
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        let file_type = mode & S_IFMT_MASK;
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
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
