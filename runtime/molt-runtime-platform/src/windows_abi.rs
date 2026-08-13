#![allow(dead_code, non_snake_case)]

use std::ffi::c_void;

pub const FILE_TYPE_CHAR: u32 = 0x0002;
pub const HANDLE_FLAG_INHERIT: u32 = 0x00000001;
pub const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;
pub const FILE_NAME_NORMALIZED: u32 = 0x00000000;
pub const VOLUME_NAME_DOS: u32 = 0x00000000;
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
    pub fn GetCurrentProcess() -> *mut c_void;
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
