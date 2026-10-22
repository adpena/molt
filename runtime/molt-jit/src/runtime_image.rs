//! The runtime as a shared image in this process.
//!
//! rustc_codegen_cranelift's JIT mode needs every dependency as a dynamic
//! library and resolves symbols through a lookup function; this runner does
//! the same with the Molt runtime. The CLI links the image from the exact AOT
//! runtime archive, so the image defines exactly the symbols an AOT link
//! resolves against, and generated code imports resolve in it alone.

use std::ffi::{CStr, CString, c_void};
use std::path::Path;

/// A loaded runtime image. It is never unloaded: generated code and the
/// runtime's own threads hold its addresses until the process exits.
pub(crate) struct RuntimeImage {
    handle: *mut c_void,
}

// SAFETY: the handle is an opaque `dlopen` result that is never closed, and
// `dlsym` is thread-safe on every supported platform.
unsafe impl Send for RuntimeImage {}
// SAFETY: as above; lookups take `&self` and mutate nothing.
unsafe impl Sync for RuntimeImage {}

impl RuntimeImage {
    /// Load the image and bind every symbol now, so an image that needs a
    /// missing host symbol fails here instead of in the middle of a run.
    #[cfg(unix)]
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("runtime image path contains NUL: {}", path.display()))?;
        // SAFETY: `name` is NUL-terminated; RTLD_LOCAL keeps the image's
        // symbols out of the global namespace, so only this lookup sees them.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(format!(
                "cannot load runtime image {}: {}",
                path.display(),
                last_dl_error()
            ));
        }
        Ok(Self { handle })
    }

    #[cfg(not(unix))]
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        Err(format!(
            "the JIT runner loads its runtime image with dlopen; this platform is not supported yet ({})",
            path.display()
        ))
    }

    /// The address of `name` in the image, or `None` when it defines none.
    pub(crate) fn symbol(&self, name: &str) -> Option<*const u8> {
        let name = CString::new(name).ok()?;
        #[cfg(unix)]
        {
            // SAFETY: the handle is live for the process; `name` is NUL-terminated.
            let address = unsafe { libc::dlsym(self.handle, name.as_ptr()) };
            (!address.is_null()).then_some(address as *const u8)
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            None
        }
    }
}

#[cfg(unix)]
fn last_dl_error() -> String {
    // SAFETY: `dlerror` returns NULL or a NUL-terminated thread-local string.
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        return "unknown dynamic loader error".to_string();
    }
    // SAFETY: checked non-null above.
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

/// The runtime entry points the AOT main stub calls, resolved in one image.
/// `runtime/native_launch_abi.toml` declares them for the stub and the runner.
pub(crate) use crate::launch_abi::RuntimeLaunchApi;

impl RuntimeImage {
    /// Resolve every launch entry in this image, naming the first one missing.
    pub(crate) fn launch_api(&self) -> Result<RuntimeLaunchApi, String> {
        // SAFETY: the image was linked from the AOT runtime archive, which
        // defines each launch entry with the C signature the stub declares.
        unsafe { RuntimeLaunchApi::resolve(|name| self.symbol(name)) }
    }
}
