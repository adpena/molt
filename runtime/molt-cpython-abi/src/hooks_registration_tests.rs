use super::{
    RUNTIME_HOOKS_ABI_MAGIC, RUNTIME_HOOKS_ABI_VERSION, RuntimeHooks,
    molt_cpython_abi_register_hooks,
};

// An old producer may allocate only its ABI prefix. Rejection must never read
// beyond that allocation or construct function pointers from absent bytes.
#[repr(C)]
struct RejectedTable {
    magic: u64,
    version: u32,
    size: u32,
}

fn reject_prefix(prefix: RejectedTable) {
    let pointer = std::ptr::addr_of!(prefix).cast::<RuntimeHooks>();
    assert_eq!(unsafe { molt_cpython_abi_register_hooks(pointer) }, -1);
}

#[test]
fn incompatible_hook_tables_are_rejected_before_reading_callbacks() {
    reject_prefix(RejectedTable {
        magic: RUNTIME_HOOKS_ABI_MAGIC,
        version: RUNTIME_HOOKS_ABI_VERSION - 1,
        size: std::mem::size_of::<RejectedTable>() as u32,
    });
    reject_prefix(RejectedTable {
        magic: RUNTIME_HOOKS_ABI_MAGIC,
        version: RUNTIME_HOOKS_ABI_VERSION,
        size: std::mem::size_of::<RejectedTable>() as u32,
    });
    reject_prefix(RejectedTable {
        magic: 0,
        version: RUNTIME_HOOKS_ABI_VERSION,
        size: std::mem::size_of::<RuntimeHooks>() as u32,
    });
    assert_eq!(
        unsafe { molt_cpython_abi_register_hooks(std::ptr::null()) },
        -1
    );
}

#[test]
fn incompatible_unaligned_hook_prefix_is_rejected() {
    let mut storage = [0u8; 1 + std::mem::size_of::<RejectedTable>()];
    let pointer = unsafe { storage.as_mut_ptr().add(1) };
    // Byte construction avoids depending on the test struct's alignment or
    // padding while exercising the public entrypoint's unaligned contract.
    storage[1..9].copy_from_slice(&RUNTIME_HOOKS_ABI_MAGIC.to_ne_bytes());
    storage[9..13].copy_from_slice(&RUNTIME_HOOKS_ABI_VERSION.to_ne_bytes());
    storage[13..17].copy_from_slice(&(std::mem::size_of::<RejectedTable>() as u32).to_ne_bytes());
    assert_eq!(
        unsafe { molt_cpython_abi_register_hooks(pointer.cast()) },
        -1
    );
}

// A mapped stack prefix cannot distinguish header-first admission from a full
// vtable overread. Put its final byte against an inaccessible page instead.
#[cfg(any(unix, windows))]
#[test]
fn rejected_hook_prefix_does_not_cross_guard_page() {
    use std::ffi::c_void;

    struct GuardedPages {
        address: *mut c_void,
        page_size: usize,
    }

    impl GuardedPages {
        fn new() -> Self {
            #[cfg(unix)]
            unsafe {
                let size = libc::sysconf(libc::_SC_PAGESIZE);
                assert!(size > 0);
                let page_size = size as usize;
                let address = libc::mmap(
                    std::ptr::null_mut(),
                    2 * page_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON,
                    -1,
                    0,
                );
                assert_ne!(address, libc::MAP_FAILED);
                let pages = Self { address, page_size };
                assert_eq!(
                    libc::mprotect(
                        address.cast::<u8>().add(page_size).cast(),
                        page_size,
                        libc::PROT_NONE
                    ),
                    0
                );
                pages
            }
            #[cfg(windows)]
            unsafe {
                use windows_sys::Win32::System::{
                    Memory::{
                        MEM_COMMIT, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
                        VirtualProtect,
                    },
                    SystemInformation::{GetSystemInfo, SYSTEM_INFO},
                };
                let mut info: SYSTEM_INFO = std::mem::zeroed();
                GetSystemInfo(&mut info);
                let page_size = info.dwPageSize as usize;
                assert!(page_size > 0);
                let address = VirtualAlloc(
                    std::ptr::null(),
                    2 * page_size,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                );
                assert!(!address.is_null());
                let pages = Self { address, page_size };
                let mut previous = 0;
                assert_ne!(
                    VirtualProtect(
                        address.cast::<u8>().add(page_size).cast(),
                        page_size,
                        PAGE_NOACCESS,
                        &mut previous
                    ),
                    0
                );
                pages
            }
        }
    }

    impl Drop for GuardedPages {
        fn drop(&mut self) {
            #[cfg(unix)]
            unsafe {
                libc::munmap(self.address, 2 * self.page_size);
            }
            #[cfg(windows)]
            unsafe {
                use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
                VirtualFree(self.address, 0, MEM_RELEASE);
            }
        }
    }

    let pages = GuardedPages::new();
    for unaligned in [false, true] {
        let length = std::mem::size_of::<RejectedTable>();
        let pointer = unsafe {
            pages
                .address
                .cast::<u8>()
                .add(pages.page_size - length - usize::from(unaligned))
        };
        let prefix = RejectedTable {
            magic: RUNTIME_HOOKS_ABI_MAGIC,
            version: RUNTIME_HOOKS_ABI_VERSION,
            size: length as u32,
        };
        unsafe {
            pointer.cast::<RejectedTable>().write_unaligned(prefix);
        }
        assert_eq!(
            unsafe { molt_cpython_abi_register_hooks(pointer.cast()) },
            -1
        );
    }
}
