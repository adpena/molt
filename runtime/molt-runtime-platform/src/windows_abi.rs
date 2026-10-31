#![allow(dead_code, non_snake_case)]

use std::ffi::c_void;

pub const FILE_TYPE_CHAR: u32 = 0x0002;
pub const HANDLE_FLAG_INHERIT: u32 = 0x00000001;
pub const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;
pub const FILE_NAME_NORMALIZED: u32 = 0x00000000;
pub const VOLUME_NAME_DOS: u32 = 0x00000000;
pub const OPEN_EXISTING: u32 = 3;
pub const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_SHARE_DELETE: u32 = 4;
const FILE_ATTRIBUTE_TAG_INFO_CLASS: i32 = 9;
const FILE_ID_INFO_CLASS: i32 = 18;
pub const WSAENOTSOCK: i32 = 10038;
pub const WSAESHUTDOWN: i32 = 10058;

/// Translate a Win32 error code to the canonical C/Python `errno` value.
///
/// This is the single runtime authority for the mapping used by CPython 3.12's
/// `PC/errmap.h`.  Keep the complete table here, below the platform boundary,
/// so filesystem, socket, and exception consumers cannot grow partial copies.
#[inline]
pub fn winerror_to_errno(mut winerror: i32) -> i32 {
    // HRESULT_FROM_WIN32 values retain the original Win32 code in the low word.
    let bits = winerror as u32;
    if bits & 0xffff_0000 == 0x8007_0000 {
        winerror = (bits & 0x0000_ffff) as i32;
    }

    // Winsock's selected errno-compatible values are offset by 10,000.
    if (10_000..12_000).contains(&winerror) {
        return match winerror {
            10_004 | 10_009 | 10_013 | 10_014 | 10_022 | 10_024 => winerror - 10_000,
            _ => winerror,
        };
    }

    match winerror {
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => libc::ENOENT,
        10 => libc::E2BIG,
        11 | 188..=202 => libc::ENOEXEC,
        6 | 114 | 130 => libc::EBADF,
        128 | 129 => libc::ECHILD,
        89 | 164 | 215 => libc::EAGAIN,
        7 | 8 | 9 | 1816 => libc::ENOMEM,
        5 | 16 | 19..=34 | 35 | 36 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => libc::EACCES,
        80 | 183 => libc::EEXIST,
        17 => libc::EXDEV,
        267 => libc::ENOTDIR,
        4 => libc::EMFILE,
        112 => libc::ENOSPC,
        109 | 232 => libc::EPIPE,
        145 => libc::ENOTEMPTY,
        1113 => libc::EILSEQ,
        _ => libc::EINVAL,
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn NeedCurrentDirectoryForExePathW(exe_name: *const u16) -> i32;
    pub fn GetCurrentProcess() -> *mut c_void;
    pub fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn GetFileInformationByHandleEx(
        handle: *mut c_void,
        info_class: i32,
        output: *mut c_void,
        size: u32,
    ) -> i32;
    fn GetFileInformationByHandle(handle: *mut c_void, output: *mut c_void) -> i32;
    pub fn GetFileType(hFile: *mut c_void) -> u32;
    pub fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
    pub fn GetHandleInformation(hObject: *mut c_void, lpdwFlags: *mut u32) -> i32;
    pub fn SetHandleInformation(hObject: *mut c_void, dwMask: u32, dwFlags: u32) -> i32;
    pub fn DuplicateHandle(
        hSourceProcessHandle: *mut c_void,
        hSourceHandle: *mut c_void,
        hTargetProcessHandle: *mut c_void,
        lpTargetHandle: *mut *mut c_void,
        dwDesiredAccess: u32,
        bInheritHandle: i32,
        dwOptions: u32,
    ) -> i32;
    pub fn GetFinalPathNameByHandleW(
        hFile: *mut c_void,
        lpszFilePath: *mut u16,
        cchFilePath: u32,
        dwFlags: u32,
    ) -> u32;
    pub fn CloseHandle(hObject: *mut c_void) -> i32;
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    pub fn closesocket(socket: usize) -> i32;
    pub fn WSAGetLastError() -> i32;
}

unsafe extern "C" {
    pub fn _wgetenv(name: *const u16) -> *mut u16;
    pub fn __p__wenviron() -> *mut *mut *mut u16;
    pub fn _wputenv(assignment: *const u16) -> i32;
    pub fn _errno() -> *mut i32;
    #[link_name = "_mktime64"]
    pub fn mktime64(tm: *mut libc::tm) -> libc::time_t;

    pub fn strftime(
        s: *mut libc::c_char,
        maxsize: usize,
        format: *const libc::c_char,
        timeptr: *const libc::tm,
    ) -> usize;
}

#[cfg(test)]
mod tests {
    use super::{WSAENOTSOCK, winerror_to_errno};

    #[test]
    fn cpython_312_winerror_mapping_is_complete() {
        let groups: &[(&[i32], i32)] = &[
            (&[2, 3, 15, 18, 53, 67, 161, 206], libc::ENOENT),
            (&[10], libc::E2BIG),
            (
                &[
                    11, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202,
                ],
                libc::ENOEXEC,
            ),
            (&[6, 114, 130], libc::EBADF),
            (&[128, 129], libc::ECHILD),
            (&[89, 164, 215], libc::EAGAIN),
            (&[7, 8, 9, 1816], libc::ENOMEM),
            (
                &[
                    5, 16, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
                    65, 82, 83, 108, 132, 158, 167,
                ],
                libc::EACCES,
            ),
            (&[80, 183], libc::EEXIST),
            (&[17], libc::EXDEV),
            (&[267], libc::ENOTDIR),
            (&[4], libc::EMFILE),
            (&[112], libc::ENOSPC),
            (&[109, 232], libc::EPIPE),
            (&[145], libc::ENOTEMPTY),
            (&[1113], libc::EILSEQ),
            (&[1, 12, 13, 87, 131, 9999], libc::EINVAL),
        ];
        for &(codes, expected) in groups {
            for &code in codes {
                assert_eq!(winerror_to_errno(code), expected, "Win32 error {code}");
            }
        }
    }

    #[test]
    fn cpython_312_winerror_mapping_handles_hresult_and_winsock() {
        assert_eq!(winerror_to_errno(0x8007_0004u32 as i32), libc::EMFILE);
        for code in [10_004, 10_009, 10_013, 10_014, 10_022, 10_024] {
            assert_eq!(winerror_to_errno(code), code - 10_000);
        }
        assert_eq!(winerror_to_errno(WSAENOTSOCK), WSAENOTSOCK);
    }
}

pub const LOCALE_USER_DEFAULT: u32 = 0x0400;
pub const LOCALE_IDEFAULTLANGUAGE: u32 = 0x0009;
pub const LOCALE_SISO639LANGNAME: u32 = 0x0059;
pub const LOCALE_SISO3166CTRYNAME: u32 = 0x005a;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetACP() -> u32;
    pub fn GetLocaleInfoA(locale: u32, kind: u32, data: *mut u8, capacity: i32) -> i32;
}
unsafe extern "C" {
    pub fn _wsetlocale(category: libc::c_int, locale: *const u16) -> *mut u16;
    pub fn _wputenv_s(name: *const u16, value: *const u16) -> libc::c_int;
}

fn nul_terminated_path(path: &std::ffi::OsStr) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path.encode_wide().collect();
    if wide.contains(&0) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    wide.push(0);
    Ok(wide)
}

/// Open the already-admitted path for metadata, without requesting data access.
/// The returned file owns exactly this handle. No-follow applies to the final
/// component; directory and junction handles require backup semantics.
pub fn open_metadata_file(path: &std::path::Path, follow: bool) -> std::io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let wide = nul_terminated_path(path.as_os_str())?;
    let flags = FILE_FLAG_BACKUP_SEMANTICS
        | if follow {
            0
        } else {
            FILE_FLAG_OPEN_REPARSE_POINT
        };
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if raw as isize == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_handle(raw) })
}

/// The volume serial number and file ID of an open handle, the pair CPython's
/// `_Py_attribute_data_to_stat` (`Python/fileutils.c`) reports as `st_dev` and
/// `st_ino`. A file system without `FileIdInfo` reports the 64-bit file index
/// of `BY_HANDLE_FILE_INFORMATION`, as CPython falls back to it.
pub fn file_id(file: &std::fs::File) -> std::io::Result<(u64, u128)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }
    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        times: [u32; 6],
        volume_serial_number: u32,
        size: [u32; 2],
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    const _: () = assert!(std::mem::size_of::<FileIdInfo>() == 24);
    const _: () = assert!(std::mem::size_of::<ByHandleFileInformation>() == 52);
    let mut id = std::mem::MaybeUninit::<FileIdInfo>::uninit();
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_ID_INFO_CLASS,
            id.as_mut_ptr().cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    };
    if success != 0 {
        let id = unsafe { id.assume_init() };
        return Ok((id.volume_serial_number, u128::from_le_bytes(id.file_id)));
    }
    let mut info = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr().cast()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.index_high) << 32) | u64::from(info.index_low);
    Ok((u64::from(info.volume_serial_number), u128::from(index)))
}

/// Query the actual tag of the same handle used for the rest of the metadata.
/// Attributes only select this query; they never determine the reparse kind.
pub fn file_attribute_tag(file: &std::fs::File) -> std::io::Result<(u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct AttributeTag {
        attributes: u32,
        tag: u32,
    }
    const _: () = assert!(std::mem::size_of::<AttributeTag>() == 8);
    let mut result = std::mem::MaybeUninit::<AttributeTag>::uninit();
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_ATTRIBUTE_TAG_INFO_CLASS,
            result.as_mut_ptr().cast(),
            std::mem::size_of::<AttributeTag>() as u32,
        )
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let result = unsafe { result.assume_init() };
    Ok((result.attributes, result.tag))
}
