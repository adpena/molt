use super::*;

pub(super) const TCL_OK: c_int = 0;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclFindExecutableFn = unsafe extern "C" fn(*const c_char);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclCreateInterpFn = unsafe extern "C" fn() -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclDeleteInterpFn = unsafe extern "C" fn(*mut c_void);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclInitFn = unsafe extern "C" fn(*mut c_void) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclEvalExFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclEvalObjvFn = unsafe extern "C" fn(*mut c_void, c_int, *const *mut c_void, c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetStringResultFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclNewStringObjFn = unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclNewListObjFn = unsafe extern "C" fn(c_int, *const *mut c_void) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclListObjAppendElementFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclIncrRefCountFn = unsafe extern "C" fn(*mut c_void);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclDecrRefCountFn = unsafe extern "C" fn(*mut c_void);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclDbIncrRefCountFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclDbDecrRefCountFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclDoOneEventFn = unsafe extern "C" fn(c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclSplitListFn =
    unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int, *mut *mut *const c_char) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclMergeFn = unsafe extern "C" fn(c_int, *const *const c_char) -> *mut c_char;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclFreeFn = unsafe extern "C" fn(*mut c_char);

// --- Typed Tcl_Obj bridge (wantobjects=1 parity, CPython _tkinter.c AsObj/FromObj) ---
//
// `Tcl_WideInt` is `long long` (i64) on every supported platform.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) type TclWideInt = i64;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclNewWideIntObjFn = unsafe extern "C" fn(TclWideInt) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclNewDoubleObjFn = unsafe extern "C" fn(f64) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclNewBooleanObjFn = unsafe extern "C" fn(c_int) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclNewByteArrayObjFn = unsafe extern "C" fn(*const u8, c_int) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetWideIntFromObjFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut TclWideInt) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetDoubleFromObjFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut f64) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetBooleanFromObjFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetStringFromObjFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> *const c_char;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetByteArrayFromObjFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> *const u8;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclListObjGetElementsFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_int, *mut *mut *mut c_void) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetObjResultFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
type TclGetObjTypeFn = unsafe extern "C" fn(*const c_char) -> *const c_void;

/// Prefix of the public `Tcl_Obj` struct, used solely to read `typePtr` — the
/// field CPython's `FromObj` dispatches on. We never touch `internalRep`; all
/// value extraction goes through the `Tcl_Get*FromObj` accessors.
///
/// `typePtr` sits at byte offset 24 on 64-bit for BOTH Tcl 8.x and 9.0, despite
/// the field-width change (verified against tcl.h):
///   8.6: refCount(int@0,4) +pad bytes(ptr@8) length(int@16,4) +pad typePtr@24
///   9.0: refCount(Tcl_Size@0,8) bytes(ptr@8) length(Tcl_Size@16,8) typePtr@24
/// The natural 8-byte pointer alignment makes 8.x's `int+pad` occupy the same
/// 8 bytes as 9.0's `Tcl_Size`, so this `#[repr(C)]` shape — whose
/// `usize`-width head fields force `type_ptr` to offset 24 — is correct on both.
/// A static assertion below pins the offset so an ABI change fails the build.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[repr(C)]
pub(super) struct TclObjHeader {
    // Width-agnostic head: three pointer-sized words cover {refCount, bytes,
    // length} on 9.0 and {refCount+pad, bytes, length+pad} on 8.x alike. These
    // exist purely to position `type_ptr`; they are never read directly.
    #[allow(dead_code)]
    _head0: usize,
    #[allow(dead_code)]
    _head1: usize,
    #[allow(dead_code)]
    _head2: usize,
    pub(super) type_ptr: *const c_void,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
const _: () = {
    // typePtr must land at offset 24 on every supported 64-bit Tcl ABI.
    assert!(std::mem::offset_of!(TclObjHeader, type_ptr) == 24);
};

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[derive(Clone, Copy)]
pub(super) struct TclApi {
    pub(super) find_executable: TclFindExecutableFn,
    pub(super) create_interp: TclCreateInterpFn,
    pub(super) delete_interp: TclDeleteInterpFn,
    pub(super) init: TclInitFn,
    pub(super) eval_ex: TclEvalExFn,
    pub(super) eval_objv: TclEvalObjvFn,
    pub(super) get_string_result: TclGetStringResultFn,
    pub(super) new_string_obj: TclNewStringObjFn,
    pub(super) new_list_obj: TclNewListObjFn,
    pub(super) list_obj_append_element: TclListObjAppendElementFn,
    pub(super) incr_ref_count: Option<TclIncrRefCountFn>,
    pub(super) decr_ref_count: Option<TclDecrRefCountFn>,
    pub(super) db_incr_ref_count: Option<TclDbIncrRefCountFn>,
    pub(super) db_decr_ref_count: Option<TclDbDecrRefCountFn>,
    pub(super) do_one_event: TclDoOneEventFn,
    pub(super) split_list: TclSplitListFn,
    pub(super) merge: TclMergeFn,
    pub(super) free: TclFreeFn,
    // Typed Tcl_Obj bridge (wantobjects=1 parity).
    pub(super) new_wide_int_obj: TclNewWideIntObjFn,
    pub(super) new_double_obj: TclNewDoubleObjFn,
    pub(super) new_boolean_obj: TclNewBooleanObjFn,
    pub(super) new_byte_array_obj: TclNewByteArrayObjFn,
    pub(super) get_wide_int_from_obj: TclGetWideIntFromObjFn,
    pub(super) get_double_from_obj: TclGetDoubleFromObjFn,
    pub(super) get_boolean_from_obj: TclGetBooleanFromObjFn,
    pub(super) get_string_from_obj: TclGetStringFromObjFn,
    pub(super) get_byte_array_from_obj: TclGetByteArrayFromObjFn,
    pub(super) list_obj_get_elements: TclListObjGetElementsFn,
    pub(super) get_obj_result: TclGetObjResultFn,
    pub(super) get_obj_type: TclGetObjTypeFn,
}

/// Cached `Tcl_ObjType*` pointers used by the typed result bridge, captured once
/// per interpreter (per CPython `Tkapp_New`). A null entry means "type not
/// registered in this Tcl build"; the dispatch then simply never matches it.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[derive(Clone, Copy, Default)]
pub(super) struct TclTypePtrs {
    pub(super) int_t: *const c_void,
    pub(super) wide_int_t: *const c_void,
    pub(super) double_t: *const c_void,
    pub(super) boolean_t: *const c_void,
    pub(super) bytearray_t: *const c_void,
    pub(super) list_t: *const c_void,
    pub(super) string_t: *const c_void,
    pub(super) utf32_string_t: *const c_void,
    pub(super) bignum_t: *const c_void,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
unsafe impl Send for TclTypePtrs {}
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
unsafe impl Sync for TclTypePtrs {}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl TclTypePtrs {
    /// Capture the interpreter's internal type pointers. Mirrors CPython
    /// `Tkapp_New`: probe by name via `Tcl_GetObjType`, falling back to reading
    /// a freshly-constructed object's `typePtr` where the named type is no
    /// longer registered (Tcl 9.0 dropped "int" and "bytearray").
    unsafe fn capture(api: &TclApi) -> Self {
        unsafe fn type_by_name(api: &TclApi, name: &[u8]) -> *const c_void {
            unsafe { (api.get_obj_type)(name.as_ptr().cast::<c_char>()) }
        }
        unsafe fn type_of(obj: *mut c_void) -> *const c_void {
            if obj.is_null() {
                return ptr::null();
            }
            unsafe { (*obj.cast::<TclObjHeader>()).type_ptr }
        }
        unsafe fn free_probe(api: &TclApi, obj: *mut c_void) {
            if obj.is_null() {
                return;
            }
            // Probe objects start at refCount 0; incr+decr frees them safely.
            unsafe {
                api.incr_ref_count_obj(obj);
                api.decr_ref_count_obj(obj);
            }
        }
        unsafe {
            let mut t = TclTypePtrs {
                double_t: type_by_name(api, b"double\0"),
                wide_int_t: type_by_name(api, b"wideInt\0"),
                bignum_t: type_by_name(api, b"bignum\0"),
                list_t: type_by_name(api, b"list\0"),
                string_t: type_by_name(api, b"string\0"),
                utf32_string_t: type_by_name(api, b"utf32string\0"),
                ..Default::default()
            };

            // "wideInt" is the canonical integer type. If the build does not
            // register it by name, read it from a fresh wide-int object.
            if t.wide_int_t.is_null() {
                let probe = (api.new_wide_int_obj)(0);
                t.wide_int_t = type_of(probe);
                free_probe(api, probe);
            }
            // "int": registered in Tcl 8.x; dropped in 9.0 where integers carry
            // the wideInt type. Fall back to the wideInt pointer so 9.0 integer
            // results still dispatch to the int branch.
            t.int_t = type_by_name(api, b"int\0");
            if t.int_t.is_null() {
                t.int_t = t.wide_int_t;
            }
            // "boolean": force a string->boolean conversion, then read typePtr.
            {
                let probe = (api.new_string_obj)(c"true".as_ptr(), 4);
                if !probe.is_null() {
                    let mut b: c_int = 0;
                    let _ = (api.get_boolean_from_obj)(ptr::null_mut(), probe, &mut b);
                    t.boolean_t = type_of(probe);
                    free_probe(api, probe);
                }
            }
            // "bytearray": probe an empty byte array obj.
            {
                let probe = (api.new_byte_array_obj)(ptr::null(), 0);
                t.bytearray_t = type_of(probe);
                free_probe(api, probe);
            }
            t
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) const TCL_REFCOUNT_FILE: &[u8] = b"molt-runtime/tk.rs\0";

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl TclApi {
    pub(super) unsafe fn incr_ref_count_obj(&self, obj: *mut c_void) {
        if let Some(incr) = self.incr_ref_count {
            unsafe {
                incr(obj);
            }
            return;
        }
        if let Some(incr) = self.db_incr_ref_count {
            unsafe {
                incr(
                    obj,
                    TCL_REFCOUNT_FILE.as_ptr().cast::<c_char>(),
                    line!() as c_int,
                );
            }
        }
    }

    pub(super) unsafe fn decr_ref_count_obj(&self, obj: *mut c_void) {
        if let Some(decr) = self.decr_ref_count {
            unsafe {
                decr(obj);
            }
            return;
        }
        if let Some(decr) = self.db_decr_ref_count {
            unsafe {
                decr(
                    obj,
                    TCL_REFCOUNT_FILE.as_ptr().cast::<c_char>(),
                    line!() as c_int,
                );
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn bundled_tcl_runtime_lib_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let mut candidates = vec![
        exe_dir.join("../Resources/tcl-tk/lib"),
        exe_dir.join("tcl-tk/lib"),
        exe_dir.join("../lib/tcl-tk"),
        exe_dir.join("lib"),
    ];
    candidates.retain(|path| path.exists());
    candidates.into_iter().next()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn runtime_version_dir(root: &Path, stem: &str) -> Option<PathBuf> {
    ["9.0", "8.7", "8.6"]
        .into_iter()
        .map(|version| root.join(format!("{stem}{version}")))
        .find(|path| path.is_dir())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn configured_tcl_runtime_lib_dir() -> Option<PathBuf> {
    static CONFIGURED: OnceLock<Option<PathBuf>> = OnceLock::new();
    CONFIGURED
        .get_or_init(|| {
            let root = bundled_tcl_runtime_lib_dir()?;
            if let Some(tcl_library) = runtime_version_dir(&root, "tcl") {
                // Process-global env mutation is serialized behind OnceLock init.
                unsafe {
                    std::env::set_var("TCL_LIBRARY", &tcl_library);
                }
            }
            if let Some(tk_library) = runtime_version_dir(&root, "tk") {
                // Process-global env mutation is serialized behind OnceLock init.
                unsafe {
                    std::env::set_var("TK_LIBRARY", &tk_library);
                }
            }
            Some(root)
        })
        .clone()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_library_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("MOLT_TCL_LIB")
        && !path.trim().is_empty()
    {
        candidates.push(PathBuf::from(path));
    }
    if let Some(root) = configured_tcl_runtime_lib_dir() {
        if cfg!(target_os = "macos") {
            candidates.push(root.join("libtcl9.0.dylib"));
            candidates.push(root.join("libtcl8.7.dylib"));
            candidates.push(root.join("libtcl8.6.dylib"));
            candidates.push(root.join("libtcl.dylib"));
        } else if cfg!(target_os = "windows") {
            candidates.push(root.join("tcl87t.dll"));
            candidates.push(root.join("tcl86t.dll"));
            candidates.push(root.join("tcl87.dll"));
            candidates.push(root.join("tcl86.dll"));
        } else {
            candidates.push(root.join("libtcl9.0.so"));
            candidates.push(root.join("libtcl8.7.so.0"));
            candidates.push(root.join("libtcl8.6.so.0"));
            candidates.push(root.join("libtcl8.7.so"));
            candidates.push(root.join("libtcl8.6.so"));
            candidates.push(root.join("libtcl.so"));
        }
    }
    let mut preferred_names: Vec<&'static str> = Vec::new();
    if cfg!(target_os = "macos") {
        preferred_names.extend(["libtcl8.7.dylib", "libtcl8.6.dylib", "libtcl.dylib"]);
        // Prefer Homebrew Tcl over system framework (system Tcl may have
        // macOS version compatibility issues on newer releases).
        candidates.push(PathBuf::from(
            "/opt/homebrew/opt/tcl-tk@8/lib/libtcl8.6.dylib",
        ));
        candidates.push(PathBuf::from(
            "/opt/homebrew/opt/tcl-tk/lib/libtcl8.7.dylib",
        ));
        candidates.push(PathBuf::from(
            "/opt/homebrew/opt/tcl-tk/lib/libtcl8.6.dylib",
        ));
        candidates.push(PathBuf::from(
            "/System/Library/Frameworks/Tcl.framework/Tcl",
        ));
        candidates.push(PathBuf::from("/usr/local/opt/tcl-tk/lib/libtcl8.7.dylib"));
        candidates.push(PathBuf::from("/usr/local/opt/tcl-tk/lib/libtcl8.6.dylib"));
        candidates.push(PathBuf::from("/opt/local/lib/libtcl8.7.dylib"));
        candidates.push(PathBuf::from("/opt/local/lib/libtcl8.6.dylib"));
        candidates.push(PathBuf::from(
            "/Library/Frameworks/Python.framework/Versions/Current/lib/libtcl8.6.dylib",
        ));
        candidates.push(PathBuf::from(
            "/Library/Frameworks/Python.framework/Versions/3.12/lib/libtcl8.6.dylib",
        ));
    } else if cfg!(target_os = "windows") {
        preferred_names.extend(["tcl87t.dll", "tcl86t.dll", "tcl87.dll", "tcl86.dll"]);
        candidates.push(PathBuf::from("tcl87t.dll"));
        candidates.push(PathBuf::from("tcl86t.dll"));
        candidates.push(PathBuf::from("tcl86.dll"));
        candidates.push(PathBuf::from("tcl87.dll"));
    } else {
        preferred_names.extend([
            "libtcl8.7.so.0",
            "libtcl8.6.so.0",
            "libtcl8.7.so",
            "libtcl8.6.so",
            "libtcl.so",
        ]);
        candidates.push(PathBuf::from("libtcl8.7.so.0"));
        candidates.push(PathBuf::from("libtcl8.6.so.0"));
        candidates.push(PathBuf::from("libtcl8.7.so"));
        candidates.push(PathBuf::from("libtcl8.6.so"));
        candidates.push(PathBuf::from("libtcl.so"));
    }
    if let Ok(dir) = std::env::var("MOLT_TCL_LIB_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            let base = PathBuf::from(dir);
            for name in &preferred_names {
                candidates.push(base.join(name));
            }
        }
    }
    candidates
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_find_executable_arg() -> CString {
    let mut candidate_bytes: Vec<Vec<u8>> = Vec::new();
    if let Ok(path) = std::env::current_exe() {
        let bytes = path.to_string_lossy().into_owned().into_bytes();
        if !bytes.is_empty() {
            candidate_bytes.push(bytes);
        }
    }
    candidate_bytes.push(b"molt".to_vec());
    for bytes in candidate_bytes {
        if let Ok(cstr) = CString::new(bytes) {
            return cstr;
        }
    }
    CString::new("molt").expect("literal executable name must be NUL-free")
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn load_tcl_api() -> Result<&'static TclApi, String> {
    static API: OnceLock<Result<TclApi, String>> = OnceLock::new();
    API.get_or_init(|| {
        let mut last_error = String::from("no Tcl library candidate succeeded");
        for path in tcl_library_candidates() {
            let lib = match unsafe { Library::new(&path) } {
                Ok(lib) => lib,
                Err(err) => {
                    last_error = format!("failed to load {}: {err}", path.display());
                    continue;
                }
            };
            let leaked: &'static Library = Box::leak(Box::new(lib));
            unsafe {
                let load = |symbol: &[u8]| -> Result<*const (), String> {
                    leaked
                        .get::<*const ()>(symbol)
                        .map(|sym| *sym)
                        .map_err(|err| {
                            format!(
                                "failed to load symbol {} from {}: {err}",
                                String::from_utf8_lossy(symbol),
                                path.display()
                            )
                        })
                };
                let load_optional = |symbol: &[u8]| -> Option<*const ()> {
                    leaked.get::<*const ()>(symbol).ok().map(|sym| *sym)
                };
                let api = TclApi {
                    find_executable: std::mem::transmute::<*const (), TclFindExecutableFn>(load(
                        b"Tcl_FindExecutable\0",
                    )?),
                    create_interp: std::mem::transmute::<*const (), TclCreateInterpFn>(load(
                        b"Tcl_CreateInterp\0",
                    )?),
                    delete_interp: std::mem::transmute::<*const (), TclDeleteInterpFn>(load(
                        b"Tcl_DeleteInterp\0",
                    )?),
                    init: std::mem::transmute::<*const (), TclInitFn>(load(b"Tcl_Init\0")?),
                    eval_ex: std::mem::transmute::<*const (), TclEvalExFn>(load(b"Tcl_EvalEx\0")?),
                    eval_objv: std::mem::transmute::<*const (), TclEvalObjvFn>(load(
                        b"Tcl_EvalObjv\0",
                    )?),
                    get_string_result: std::mem::transmute::<*const (), TclGetStringResultFn>(
                        load(b"Tcl_GetStringResult\0")?,
                    ),
                    new_string_obj: std::mem::transmute::<*const (), TclNewStringObjFn>(load(
                        b"Tcl_NewStringObj\0",
                    )?),
                    new_list_obj: std::mem::transmute::<*const (), TclNewListObjFn>(load(
                        b"Tcl_NewListObj\0",
                    )?),
                    list_obj_append_element: std::mem::transmute::<
                        *const (),
                        TclListObjAppendElementFn,
                    >(load(
                        b"Tcl_ListObjAppendElement\0",
                    )?),
                    incr_ref_count: load_optional(b"Tcl_IncrRefCount\0")
                        .map(|sym| std::mem::transmute::<*const (), TclIncrRefCountFn>(sym)),
                    decr_ref_count: load_optional(b"Tcl_DecrRefCount\0")
                        .map(|sym| std::mem::transmute::<*const (), TclDecrRefCountFn>(sym)),
                    db_incr_ref_count: load_optional(b"Tcl_DbIncrRefCount\0")
                        .map(|sym| std::mem::transmute::<*const (), TclDbIncrRefCountFn>(sym)),
                    db_decr_ref_count: load_optional(b"Tcl_DbDecrRefCount\0")
                        .map(|sym| std::mem::transmute::<*const (), TclDbDecrRefCountFn>(sym)),
                    do_one_event: std::mem::transmute::<*const (), TclDoOneEventFn>(load(
                        b"Tcl_DoOneEvent\0",
                    )?),
                    split_list: std::mem::transmute::<*const (), TclSplitListFn>(load(
                        b"Tcl_SplitList\0",
                    )?),
                    merge: std::mem::transmute::<*const (), TclMergeFn>(load(b"Tcl_Merge\0")?),
                    free: std::mem::transmute::<*const (), TclFreeFn>(load(b"Tcl_Free\0")?),
                    new_wide_int_obj: std::mem::transmute::<*const (), TclNewWideIntObjFn>(load(
                        b"Tcl_NewWideIntObj\0",
                    )?),
                    new_double_obj: std::mem::transmute::<*const (), TclNewDoubleObjFn>(load(
                        b"Tcl_NewDoubleObj\0",
                    )?),
                    new_boolean_obj: std::mem::transmute::<*const (), TclNewBooleanObjFn>(load(
                        b"Tcl_NewBooleanObj\0",
                    )?),
                    new_byte_array_obj: std::mem::transmute::<*const (), TclNewByteArrayObjFn>(
                        load(b"Tcl_NewByteArrayObj\0")?,
                    ),
                    get_wide_int_from_obj: std::mem::transmute::<*const (), TclGetWideIntFromObjFn>(
                        load(b"Tcl_GetWideIntFromObj\0")?,
                    ),
                    get_double_from_obj: std::mem::transmute::<*const (), TclGetDoubleFromObjFn>(
                        load(b"Tcl_GetDoubleFromObj\0")?,
                    ),
                    get_boolean_from_obj: std::mem::transmute::<*const (), TclGetBooleanFromObjFn>(
                        load(b"Tcl_GetBooleanFromObj\0")?,
                    ),
                    get_string_from_obj: std::mem::transmute::<*const (), TclGetStringFromObjFn>(
                        load(b"Tcl_GetStringFromObj\0")?,
                    ),
                    get_byte_array_from_obj: std::mem::transmute::<
                        *const (),
                        TclGetByteArrayFromObjFn,
                    >(load(b"Tcl_GetByteArrayFromObj\0")?),
                    list_obj_get_elements: std::mem::transmute::<*const (), TclListObjGetElementsFn>(
                        load(b"Tcl_ListObjGetElements\0")?,
                    ),
                    get_obj_result: std::mem::transmute::<*const (), TclGetObjResultFn>(load(
                        b"Tcl_GetObjResult\0",
                    )?),
                    get_obj_type: std::mem::transmute::<*const (), TclGetObjTypeFn>(load(
                        b"Tcl_GetObjType\0",
                    )?),
                };
                return Ok(api);
            }
        }
        Err(last_error)
    })
    .as_ref()
    .map_err(Clone::clone)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[derive(Clone)]
pub(super) enum TclObjKind {
    Scalar(String),
    List(Vec<TclObj>),
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[derive(Clone)]
pub(super) struct TclObj {
    pub(super) kind: TclObjKind,
    pub(super) interp_ptr: usize,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl TclObj {
    pub(super) fn scalar(text: String) -> Self {
        Self {
            kind: TclObjKind::Scalar(text),
            interp_ptr: 0,
        }
    }

    pub(super) fn scalar_from_interp(text: String, interp_ptr: usize) -> Self {
        Self {
            kind: TclObjKind::Scalar(text),
            interp_ptr,
        }
    }

    pub(super) fn new_list<I: IntoIterator<Item = TclObj>>(iter: I) -> Self {
        Self {
            kind: TclObjKind::List(iter.into_iter().collect()),
            interp_ptr: 0,
        }
    }

    pub(super) fn get_elements(&self) -> Result<std::vec::IntoIter<TclObj>, String> {
        match &self.kind {
            TclObjKind::List(items) => Ok(items.clone().into_iter()),
            TclObjKind::Scalar(text) => {
                let interp_addr = self.interp_ptr;
                if interp_addr == 0 {
                    return Err("cannot split Tcl list without interpreter context".to_string());
                }
                let interp = interp_addr as *mut c_void;
                let api = load_tcl_api()?;
                let parts = tcl_split_list(api, interp, text)?;
                Ok(parts
                    .into_iter()
                    .map(|part| TclObj::scalar_from_interp(part, interp_addr))
                    .collect::<Vec<_>>()
                    .into_iter())
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl std::fmt::Display for TclObj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            TclObjKind::Scalar(text) => f.write_str(text),
            TclObjKind::List(items) => {
                let rendered = items
                    .iter()
                    .map(|item| {
                        let s = item.to_string();
                        if s.contains(' ')
                            || s.contains('{')
                            || s.contains('}')
                            || s.contains('"')
                            || s.contains('\\')
                            || s.contains('[')
                            || s.contains(']')
                            || s.contains('$')
                            || s.is_empty()
                        {
                            format!("{{{}}}", s)
                        } else {
                            s
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                f.write_str(&rendered)
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl From<&str> for TclObj {
    fn from(value: &str) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl From<String> for TclObj {
    fn from(value: String) -> Self {
        Self::scalar(value)
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl From<i64> for TclObj {
    fn from(value: i64) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl From<i32> for TclObj {
    fn from(value: i32) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl From<f64> for TclObj {
    fn from(value: f64) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) trait IntoTclCommand {
    fn into_command(self) -> Vec<TclObj>;
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl IntoTclCommand for TclObj {
    fn into_command(self) -> Vec<TclObj> {
        match self.kind {
            TclObjKind::List(items) => items,
            TclObjKind::Scalar(_) => vec![self],
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
macro_rules! impl_into_tcl_command_tuple {
    ($($ty:ident => $var:ident),+ $(,)?) => {
        impl<$($ty),+> IntoTclCommand for ($($ty,)+)
        where
            $($ty: Into<TclObj>,)+
        {
            fn into_command(self) -> Vec<TclObj> {
                let ($($var,)+) = self;
                vec![$($var.into(),)+]
            }
        }
    };
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl_into_tcl_command_tuple!(A => a);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl_into_tcl_command_tuple!(A => a, B => b);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl_into_tcl_command_tuple!(A => a, B => b, C => c);
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl_into_tcl_command_tuple!(A => a, B => b, C => c, D => d);

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_result_string(api: &TclApi, interp: *mut c_void) -> String {
    let ptr = unsafe { (api.get_string_result)(interp) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_split_list(
    api: &TclApi,
    interp: *mut c_void,
    list: &str,
) -> Result<Vec<String>, String> {
    let list_c = CString::new(list.as_bytes())
        .map_err(|_| "Tcl list string contains interior NUL byte".to_string())?;
    let mut argc: c_int = 0;
    let mut argv: *mut *const c_char = ptr::null_mut();
    let rc = unsafe { (api.split_list)(interp, list_c.as_ptr(), &mut argc, &mut argv) };
    if rc != TCL_OK {
        let message = tcl_result_string(api, interp);
        return Err(if message.is_empty() {
            "failed to split Tcl list".to_string()
        } else {
            message
        });
    }
    let mut out = Vec::with_capacity(argc.max(0) as usize);
    if !argv.is_null() {
        for idx in 0..argc {
            let entry_ptr = unsafe { *argv.add(idx as usize) };
            if entry_ptr.is_null() {
                out.push(String::new());
                continue;
            }
            out.push(
                unsafe { CStr::from_ptr(entry_ptr) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        unsafe { (api.free)(argv.cast::<c_char>()) };
    }
    Ok(out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_merge_args(api: &TclApi, args: &[String]) -> Result<Vec<u8>, String> {
    let mut c_args = Vec::with_capacity(args.len());
    for arg in args {
        c_args.push(
            CString::new(arg.as_bytes())
                .map_err(|_| "Tcl argument contains interior NUL byte".to_string())?,
        );
    }
    let ptrs: Vec<*const c_char> = c_args.iter().map(|arg| arg.as_ptr()).collect();
    let merged_ptr = unsafe { (api.merge)(ptrs.len() as c_int, ptrs.as_ptr()) };
    if merged_ptr.is_null() {
        return Err("Tcl_Merge returned null".to_string());
    }
    let merged = unsafe { CStr::from_ptr(merged_ptr) }.to_bytes().to_vec();
    unsafe { (api.free)(merged_ptr) };
    Ok(merged)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) struct TclInterpreter {
    pub(super) interp_addr: usize,
    pub(super) owner_thread: ThreadId,
    pub(super) api: &'static TclApi,
    pub(super) types: TclTypePtrs,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl TclInterpreter {
    pub(super) fn new() -> Result<Self, String> {
        static FIND_EXECUTABLE_ONCE: OnceLock<()> = OnceLock::new();
        static FIND_EXECUTABLE_ARG: OnceLock<CString> = OnceLock::new();
        let _ = configured_tcl_runtime_lib_dir();
        let api = load_tcl_api()?;
        let executable_arg = FIND_EXECUTABLE_ARG.get_or_init(tcl_find_executable_arg);
        FIND_EXECUTABLE_ONCE.get_or_init(|| unsafe {
            (api.find_executable)(executable_arg.as_ptr());
        });
        let interp_ptr = unsafe { (api.create_interp)() };
        if interp_ptr.is_null() {
            return Err("failed to create Tcl interpreter".to_string());
        }
        let rc = unsafe { (api.init)(interp_ptr) };
        if rc != TCL_OK {
            let err = tcl_result_string(api, interp_ptr);
            unsafe { (api.delete_interp)(interp_ptr) };
            return Err(if err.is_empty() {
                "Tcl_Init failed".to_string()
            } else {
                err
            });
        }
        // Capture the interpreter's internal type pointers now that Tcl_Init has
        // registered the core types (needed by the typed result bridge).
        let types = unsafe { TclTypePtrs::capture(api) };
        Ok(Self {
            interp_addr: interp_ptr as usize,
            owner_thread: thread::current().id(),
            api,
            types,
        })
    }

    pub(super) fn interp_ptr(&self) -> *mut c_void {
        self.interp_addr as *mut c_void
    }

    pub(super) fn ensure_owner_thread(&self) -> Result<(), String> {
        if thread::current().id() != self.owner_thread {
            return Err("Tk interpreter used from a different thread".to_string());
        }
        Ok(())
    }

    pub(super) fn eval<C: IntoTclCommand>(&self, command: C) -> Result<TclObj, String> {
        self.ensure_owner_thread()?;
        let parts = command.into_command();
        let mut objv = Vec::with_capacity(parts.len());
        for part in &parts {
            match self.alloc_obj(part) {
                Ok(obj) => {
                    // Immediately own the object (refcount 0→1).
                    unsafe { self.api.incr_ref_count_obj(obj) };
                    objv.push(obj);
                }
                Err(err) => {
                    // Clean up already-allocated objects on partial failure.
                    for &allocated in &objv {
                        unsafe { self.api.decr_ref_count_obj(allocated) };
                    }
                    return Err(err);
                }
            }
        }
        let rc = unsafe {
            let call_rc =
                (self.api.eval_objv)(self.interp_ptr(), objv.len() as c_int, objv.as_ptr(), 0);
            for &obj in &objv {
                self.api.decr_ref_count_obj(obj);
            }
            call_rc
        };
        let result = tcl_result_string(self.api, self.interp_ptr());
        if rc != TCL_OK {
            return Err(if result.is_empty() {
                "Tcl_EvalObjv failed".to_string()
            } else {
                result
            });
        }
        Ok(TclObj::scalar_from_interp(result, self.interp_addr))
    }

    pub(super) fn get(&self, name: &str) -> Result<TclObj, String> {
        self.eval(("set", name))
    }

    pub(super) fn do_one_event(&self, flags: i32) -> Result<bool, String> {
        self.ensure_owner_thread()?;
        Ok(unsafe { (self.api.do_one_event)(flags as c_int) != 0 })
    }

    pub(super) fn render_part(&self, part: &TclObj) -> Result<String, String> {
        match &part.kind {
            TclObjKind::Scalar(text) => Ok(text.clone()),
            TclObjKind::List(list) => {
                let mut rendered = Vec::with_capacity(list.len());
                for nested in list {
                    rendered.push(self.render_part(nested)?);
                }
                let merged = tcl_merge_args(self.api, &rendered)?;
                Ok(String::from_utf8_lossy(&merged).into_owned())
            }
        }
    }

    pub(super) fn alloc_obj(&self, part: &TclObj) -> Result<*mut c_void, String> {
        match &part.kind {
            TclObjKind::Scalar(text) => {
                let bytes = CString::new(text.as_bytes())
                    .map_err(|_| "Tcl string contained interior NUL byte".to_string())?;
                let obj = unsafe { (self.api.new_string_obj)(bytes.as_ptr(), text.len() as c_int) };
                if obj.is_null() {
                    return Err("Tcl_NewStringObj returned null".to_string());
                }
                Ok(obj)
            }
            TclObjKind::List(list) => {
                let list_obj = unsafe { (self.api.new_list_obj)(0, ptr::null()) };
                if list_obj.is_null() {
                    return Err("Tcl_NewListObj returned null".to_string());
                }
                for nested in list {
                    let nested_obj = self.alloc_obj(nested)?;
                    let rc = unsafe {
                        (self.api.list_obj_append_element)(self.interp_ptr(), list_obj, nested_obj)
                    };
                    if rc != TCL_OK {
                        // Safely free refcount-0 objects: incr then decr (0→1→0).
                        unsafe {
                            self.api.incr_ref_count_obj(nested_obj);
                            self.api.decr_ref_count_obj(nested_obj);
                            self.api.incr_ref_count_obj(list_obj);
                            self.api.decr_ref_count_obj(list_obj);
                        }
                        let err = tcl_result_string(self.api, self.interp_ptr());
                        return Err(if err.is_empty() {
                            "Tcl_ListObjAppendElement failed".to_string()
                        } else {
                            err
                        });
                    }
                }
                Ok(list_obj)
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
impl Drop for TclInterpreter {
    fn drop(&mut self) {
        if self.interp_addr != 0 {
            unsafe { (self.api.delete_interp)(self.interp_ptr()) };
            self.interp_addr = 0;
        }
    }
}
