//! The runtime as a shared image in this process.
//!
//! rustc_codegen_cranelift's JIT mode needs every dependency as a dynamic
//! library and resolves symbols through a lookup function; this runner does
//! the same with the Molt runtime. The CLI links the image from the exact AOT
//! runtime archive, so the image defines exactly the symbols an AOT link
//! resolves against, and generated code imports resolve in it alone.

use std::ffi::{CStr, CString, c_char, c_void};
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
pub(crate) struct RuntimeLaunchApi {
    pub(crate) register_compiled_sources: unsafe extern "C" fn(*const *const c_char, u64),
    pub(crate) set_app_callable_resolver: unsafe extern "C" fn(u64) -> u64,
    pub(crate) module_registry_install: unsafe extern "C" fn(*const u8) -> u64,
    pub(crate) runtime_init: unsafe extern "C" fn() -> u64,
    pub(crate) runtime_ensure_gil: unsafe extern "C" fn(),
    pub(crate) set_argv: unsafe extern "C" fn(i32, *const *const c_char),
    pub(crate) exception_pending: unsafe extern "C" fn() -> u64,
    pub(crate) exception_last_pending: unsafe extern "C" fn() -> u64,
    pub(crate) exception_report_uncaught: unsafe extern "C" fn(u64) -> u64,
    pub(crate) dec_ref_obj: unsafe extern "C" fn(u64),
    pub(crate) runtime_exit: unsafe extern "C" fn(u64) -> u64,
}

impl RuntimeLaunchApi {
    /// Resolve every launch entry point, naming the first one missing.
    pub(crate) fn resolve(image: &RuntimeImage) -> Result<Self, String> {
        Ok(Self {
            register_compiled_sources: entry(image, "molt_register_compiled_sources")?,
            set_app_callable_resolver: entry(image, "molt_set_app_callable_resolver")?,
            module_registry_install: entry(image, "molt_module_registry_install")?,
            runtime_init: entry(image, "molt_runtime_init")?,
            runtime_ensure_gil: entry(image, "molt_runtime_ensure_gil")?,
            set_argv: entry(image, "molt_set_argv")?,
            exception_pending: entry(image, "molt_exception_pending")?,
            exception_last_pending: entry(image, "molt_exception_last_pending")?,
            exception_report_uncaught: entry(image, "molt_exception_report_uncaught")?,
            dec_ref_obj: entry(image, "molt_dec_ref_obj")?,
            runtime_exit: entry(image, "molt_runtime_exit")?,
        })
    }
}

/// One runtime function as the C function pointer type `F`.
///
/// `F` must be the exact `unsafe extern "C" fn` type the runtime exports
/// `name` with, as the AOT main stub declares it.
fn entry<F: Copy>(image: &RuntimeImage, name: &str) -> Result<F, String> {
    const {
        assert!(std::mem::size_of::<F>() == std::mem::size_of::<*const u8>());
    }
    let address = image
        .symbol(name)
        .ok_or_else(|| format!("the runtime image defines no `{name}`"))?;
    // SAFETY: `F` is a function pointer type of pointer size (checked above),
    // and the runtime defines `name` with that C signature.
    Ok(unsafe { std::mem::transmute_copy::<*const u8, F>(&address) })
}
