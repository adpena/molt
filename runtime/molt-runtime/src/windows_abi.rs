#![allow(dead_code, non_snake_case)]

use std::ffi::c_void;

pub(crate) const FILE_TYPE_CHAR: u32 = 0x0002;
pub(crate) const HANDLE_FLAG_INHERIT: u32 = 0x00000001;
pub(crate) const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;
pub(crate) const FILE_NAME_NORMALIZED: u32 = 0x00000000;
pub(crate) const VOLUME_NAME_DOS: u32 = 0x00000000;
pub(crate) const WSAENOTSOCK: i32 = 10038;
pub(crate) const WSAESHUTDOWN: i32 = 10058;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub(crate) fn GetCurrentProcess() -> *mut c_void;
    pub(crate) fn GetFileType(hFile: *mut c_void) -> u32;
    pub(crate) fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
    pub(crate) fn GetHandleInformation(hObject: *mut c_void, lpdwFlags: *mut u32) -> i32;
    pub(crate) fn SetHandleInformation(hObject: *mut c_void, dwMask: u32, dwFlags: u32) -> i32;
    pub(crate) fn DuplicateHandle(
        hSourceProcessHandle: *mut c_void,
        hSourceHandle: *mut c_void,
        hTargetProcessHandle: *mut c_void,
        lpTargetHandle: *mut *mut c_void,
        dwDesiredAccess: u32,
        bInheritHandle: i32,
        dwOptions: u32,
    ) -> i32;
    pub(crate) fn GetFinalPathNameByHandleW(
        hFile: *mut c_void,
        lpszFilePath: *mut u16,
        cchFilePath: u32,
        dwFlags: u32,
    ) -> u32;
    pub(crate) fn CloseHandle(hObject: *mut c_void) -> i32;
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    pub(crate) fn closesocket(socket: usize) -> i32;
    pub(crate) fn WSAGetLastError() -> i32;
}

unsafe extern "C" {
    #[link_name = "_mktime64"]
    pub(crate) fn mktime64(tm: *mut libc::tm) -> libc::time_t;

    pub(crate) fn strftime(
        s: *mut libc::c_char,
        maxsize: usize,
        format: *const libc::c_char,
        timeptr: *const libc::tm,
    ) -> usize;
}
