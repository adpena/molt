//! POSIX user/group database custody shared by pwd, pathlib and shutil.
//!
//! No Python values or permission policy cross this boundary. Reentrant lookups
//! own their scratch storage; enumeration serializes the POSIX process cursor and
//! copies every borrowed field before advancing it or releasing the cursor.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::Mutex;

#[derive(Debug)]
pub struct Passwd {
    pub name: Option<Vec<u8>>,
    pub passwd: Option<Vec<u8>>,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
    pub gecos: Option<Vec<u8>>,
    pub dir: Option<Vec<u8>>,
    pub shell: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug)]
pub struct AllocationError;

type Result<T> = std::result::Result<T, AllocationError>;

unsafe fn copy_field(pointer: *const libc::c_char) -> Result<Option<Vec<u8>>> {
    if pointer.is_null() {
        return Ok(None);
    }
    let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
    let mut value = Vec::new();
    value
        .try_reserve_exact(bytes.len())
        .map_err(|_| AllocationError)?;
    value.extend_from_slice(bytes);
    Ok(Some(value))
}

impl Passwd {
    unsafe fn copy_from(value: &libc::passwd) -> Result<Self> {
        Ok(Self {
            name: unsafe { copy_field(value.pw_name)? },
            #[cfg(not(target_os = "android"))]
            passwd: unsafe { copy_field(value.pw_passwd)? },
            #[cfg(target_os = "android")]
            passwd: Some(Vec::new()),
            uid: value.pw_uid,
            gid: value.pw_gid,
            #[cfg(not(target_os = "android"))]
            gecos: unsafe { copy_field(value.pw_gecos)? },
            #[cfg(target_os = "android")]
            gecos: Some(Vec::new()),
            dir: unsafe { copy_field(value.pw_dir)? },
            shell: unsafe { copy_field(value.pw_shell)? },
        })
    }
}

fn scratch(size_key: libc::c_int) -> Result<Vec<u8>> {
    let suggested = unsafe { libc::sysconf(size_key) };
    let size = if suggested <= 0 {
        1024
    } else {
        suggested as usize
    };
    let mut value = Vec::new();
    value.try_reserve_exact(size).map_err(|_| AllocationError)?;
    value.resize(size, 0);
    Ok(value)
}

fn grow(value: &mut Vec<u8>) -> Result<()> {
    let size = value
        .len()
        .checked_mul(2)
        .filter(|size| *size <= isize::MAX as usize)
        .ok_or(AllocationError)?;
    value
        .try_reserve_exact(size - value.len())
        .map_err(|_| AllocationError)?;
    value.resize(size, 0);
    Ok(())
}

fn lookup<Record, Owned>(
    size_key: libc::c_int,
    mut call: impl FnMut(*mut Record, *mut libc::c_char, usize, *mut *mut Record) -> libc::c_int,
    copy: unsafe fn(&Record) -> Result<Owned>,
) -> Result<Option<Owned>> {
    let mut buffer = scratch(size_key)?;
    loop {
        let mut value = MaybeUninit::<Record>::uninit();
        let mut result = std::ptr::null_mut();
        let status = call(
            value.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        );
        if status == libc::ERANGE {
            grow(&mut buffer)?;
            continue;
        }
        // CPython's pwd/grp wrappers report every non-ERANGE database error as a
        // missing entry. Allocation failures remain a distinct error above.
        return if status != 0 || result.is_null() {
            Ok(None)
        } else {
            unsafe { copy(&*result).map(Some) }
        };
    }
}

pub fn by_name(name: &CStr) -> Result<Option<Passwd>> {
    lookup(
        libc::_SC_GETPW_R_SIZE_MAX,
        |entry, buffer, size, result| unsafe {
            libc::getpwnam_r(name.as_ptr(), entry, buffer, size, result)
        },
        Passwd::copy_from,
    )
}

pub fn by_uid(uid: libc::uid_t) -> Result<Option<Passwd>> {
    lookup(
        libc::_SC_GETPW_R_SIZE_MAX,
        |entry, buffer, size, result| unsafe { libc::getpwuid_r(uid, entry, buffer, size, result) },
        Passwd::copy_from,
    )
}

/// Internal leaf projections copy only their returned field. They share the
/// same reentrant lookup/storage owner as full pwd results and add no policy.
pub fn uid_by_name(name: &CStr) -> Result<Option<libc::uid_t>> {
    unsafe fn copy(entry: &libc::passwd) -> Result<libc::uid_t> {
        Ok(entry.pw_uid)
    }
    lookup(
        libc::_SC_GETPW_R_SIZE_MAX,
        |entry, buffer, size, result| unsafe {
            libc::getpwnam_r(name.as_ptr(), entry, buffer, size, result)
        },
        copy,
    )
}

pub fn user_name_by_uid(uid: libc::uid_t) -> Result<Option<Option<Vec<u8>>>> {
    unsafe fn copy(entry: &libc::passwd) -> Result<Option<Vec<u8>>> {
        unsafe { copy_field(entry.pw_name) }
    }
    lookup(
        libc::_SC_GETPW_R_SIZE_MAX,
        |entry, buffer, size, result| unsafe { libc::getpwuid_r(uid, entry, buffer, size, result) },
        copy,
    )
}

pub fn gid_by_name(name: &CStr) -> Result<Option<libc::gid_t>> {
    unsafe fn copy(entry: &libc::group) -> Result<libc::gid_t> {
        Ok(entry.gr_gid)
    }
    lookup(
        libc::_SC_GETGR_R_SIZE_MAX,
        |entry, buffer, size, result| unsafe {
            libc::getgrnam_r(name.as_ptr(), entry, buffer, size, result)
        },
        copy,
    )
}

pub fn group_name_by_gid(gid: libc::gid_t) -> Result<Option<Option<Vec<u8>>>> {
    unsafe fn copy(entry: &libc::group) -> Result<Option<Vec<u8>>> {
        unsafe { copy_field(entry.gr_name) }
    }
    lookup(
        libc::_SC_GETGR_R_SIZE_MAX,
        |entry, buffer, size, result| unsafe { libc::getgrgid_r(gid, entry, buffer, size, result) },
        copy,
    )
}

static ENUMERATION: Mutex<()> = Mutex::new(());

struct Cursor;
impl Drop for Cursor {
    fn drop(&mut self) {
        unsafe { libc::endpwent() };
    }
}

pub fn all() -> Result<Vec<Passwd>> {
    let _lock = ENUMERATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe { libc::setpwent() };
    let _cursor = Cursor;
    let mut entries = Vec::new();
    // POSIX does not specify getpwent_r; macOS has only the cursor API. The
    // shared cursor lock and immediate owned copy are also CPython 3.15's
    // required free-threaded discipline. No Python allocation runs here.
    loop {
        let pointer = unsafe { libc::getpwent() };
        if pointer.is_null() {
            return Ok(entries);
        }
        let entry = unsafe { Passwd::copy_from(&*pointer)? };
        entries.try_reserve(1).map_err(|_| AllocationError)?;
        entries.push(entry);
    }
}

#[cfg(test)]
#[path = "userdb/tests.rs"]
mod tests;
