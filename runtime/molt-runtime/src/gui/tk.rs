#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
use crate::object::ops::format_obj_str;
use crate::{
    MoltObject, PyToken, call_callable0, dec_ref_bits, decode_value_list, exception_pending,
    has_capability, inc_ref_bits, is_truthy, obj_from_bits, raise_exception, string_obj_to_owned,
    to_f64, to_i64,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
use libloading::Library;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    ptr,
    thread::{self, ThreadId},
};

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
const TCL_OK: c_int = 0;

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclFindExecutableFn = unsafe extern "C" fn(*const c_char);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclCreateInterpFn = unsafe extern "C" fn() -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclDeleteInterpFn = unsafe extern "C" fn(*mut c_void);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclInitFn = unsafe extern "C" fn(*mut c_void) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclEvalExFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclEvalObjvFn =
    unsafe extern "C" fn(*mut c_void, c_int, *const *mut c_void, c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclGetStringResultFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclNewStringObjFn = unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclNewListObjFn = unsafe extern "C" fn(c_int, *const *mut c_void) -> *mut c_void;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclListObjAppendElementFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclIncrRefCountFn = unsafe extern "C" fn(*mut c_void);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclDecrRefCountFn = unsafe extern "C" fn(*mut c_void);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclDbIncrRefCountFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclDbDecrRefCountFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclDoOneEventFn = unsafe extern "C" fn(c_int) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclSplitListFn =
    unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int, *mut *mut *const c_char) -> c_int;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclMergeFn = unsafe extern "C" fn(c_int, *const *const c_char) -> *mut c_char;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
type TclFreeFn = unsafe extern "C" fn(*mut c_char);

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
#[derive(Clone, Copy)]
struct TclApi {
    find_executable: TclFindExecutableFn,
    create_interp: TclCreateInterpFn,
    delete_interp: TclDeleteInterpFn,
    init: TclInitFn,
    eval_ex: TclEvalExFn,
    eval_objv: TclEvalObjvFn,
    get_string_result: TclGetStringResultFn,
    new_string_obj: TclNewStringObjFn,
    new_list_obj: TclNewListObjFn,
    list_obj_append_element: TclListObjAppendElementFn,
    incr_ref_count: Option<TclIncrRefCountFn>,
    decr_ref_count: Option<TclDecrRefCountFn>,
    db_incr_ref_count: Option<TclDbIncrRefCountFn>,
    db_decr_ref_count: Option<TclDbDecrRefCountFn>,
    do_one_event: TclDoOneEventFn,
    split_list: TclSplitListFn,
    merge: TclMergeFn,
    free: TclFreeFn,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
const TCL_REFCOUNT_FILE: &[u8] = b"molt-runtime/tk.rs\0";

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl TclApi {
    unsafe fn incr_ref_count_obj(&self, obj: *mut c_void) {
        if let Some(incr) = self.incr_ref_count {
            unsafe {
                incr(obj);
            }
            return;
        }
        if let Some(incr) = self.db_incr_ref_count {
            unsafe {
                incr(obj, TCL_REFCOUNT_FILE.as_ptr().cast::<c_char>(), line!() as c_int);
            }
        }
    }

    unsafe fn decr_ref_count_obj(&self, obj: *mut c_void) {
        if let Some(decr) = self.decr_ref_count {
            unsafe {
                decr(obj);
            }
            return;
        }
        if let Some(decr) = self.db_decr_ref_count {
            unsafe {
                decr(obj, TCL_REFCOUNT_FILE.as_ptr().cast::<c_char>(), line!() as c_int);
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn bundled_tcl_runtime_lib_dir() -> Option<PathBuf> {
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn runtime_version_dir(root: &Path, stem: &str) -> Option<PathBuf> {
    ["9.0", "8.7", "8.6"]
        .into_iter()
        .map(|version| root.join(format!("{stem}{version}")))
        .find(|path| path.is_dir())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn configured_tcl_runtime_lib_dir() -> Option<PathBuf> {
    static CONFIGURED: OnceLock<Option<PathBuf>> = OnceLock::new();
    CONFIGURED
        .get_or_init(|| {
            let Some(root) = bundled_tcl_runtime_lib_dir() else {
                return None;
            };
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_library_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("MOLT_TCL_LIB") {
        if !path.trim().is_empty() {
            candidates.push(PathBuf::from(path));
        }
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_find_executable_arg() -> CString {
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn load_tcl_api() -> Result<&'static TclApi, String> {
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
                    find_executable: std::mem::transmute(load(b"Tcl_FindExecutable\0")?),
                    create_interp: std::mem::transmute(load(b"Tcl_CreateInterp\0")?),
                    delete_interp: std::mem::transmute(load(b"Tcl_DeleteInterp\0")?),
                    init: std::mem::transmute(load(b"Tcl_Init\0")?),
                    eval_ex: std::mem::transmute(load(b"Tcl_EvalEx\0")?),
                    eval_objv: std::mem::transmute(load(b"Tcl_EvalObjv\0")?),
                    get_string_result: std::mem::transmute(load(b"Tcl_GetStringResult\0")?),
                    new_string_obj: std::mem::transmute(load(b"Tcl_NewStringObj\0")?),
                    new_list_obj: std::mem::transmute(load(b"Tcl_NewListObj\0")?),
                    list_obj_append_element: std::mem::transmute(
                        load(b"Tcl_ListObjAppendElement\0")?,
                    ),
                    incr_ref_count: load_optional(b"Tcl_IncrRefCount\0")
                        .map(|sym| unsafe { std::mem::transmute(sym) }),
                    decr_ref_count: load_optional(b"Tcl_DecrRefCount\0")
                        .map(|sym| unsafe { std::mem::transmute(sym) }),
                    db_incr_ref_count: load_optional(b"Tcl_DbIncrRefCount\0")
                        .map(|sym| unsafe { std::mem::transmute(sym) }),
                    db_decr_ref_count: load_optional(b"Tcl_DbDecrRefCount\0")
                        .map(|sym| unsafe { std::mem::transmute(sym) }),
                    do_one_event: std::mem::transmute(load(b"Tcl_DoOneEvent\0")?),
                    split_list: std::mem::transmute(load(b"Tcl_SplitList\0")?),
                    merge: std::mem::transmute(load(b"Tcl_Merge\0")?),
                    free: std::mem::transmute(load(b"Tcl_Free\0")?),
                };
                return Ok(api);
            }
        }
        Err(last_error)
    })
    .as_ref()
    .map_err(Clone::clone)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
#[derive(Clone)]
enum TclObjKind {
    Scalar(String),
    List(Vec<TclObj>),
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
#[derive(Clone)]
struct TclObj {
    kind: TclObjKind,
    interp_ptr: usize,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl TclObj {
    fn scalar(text: String) -> Self {
        Self {
            kind: TclObjKind::Scalar(text),
            interp_ptr: 0,
        }
    }

    fn scalar_from_interp(text: String, interp_ptr: usize) -> Self {
        Self {
            kind: TclObjKind::Scalar(text),
            interp_ptr,
        }
    }

    fn new_list<I: IntoIterator<Item = TclObj>>(iter: I) -> Self {
        Self {
            kind: TclObjKind::List(iter.into_iter().collect()),
            interp_ptr: 0,
        }
    }

    fn to_string(&self) -> String {
        match &self.kind {
            TclObjKind::Scalar(text) => text.clone(),
            TclObjKind::List(items) => items
                .iter()
                .map(TclObj::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    fn get_elements(&self) -> Result<std::vec::IntoIter<TclObj>, String> {
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl From<&str> for TclObj {
    fn from(value: &str) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl From<String> for TclObj {
    fn from(value: String) -> Self {
        Self::scalar(value)
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl From<i64> for TclObj {
    fn from(value: i64) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl From<i32> for TclObj {
    fn from(value: i32) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl From<f64> for TclObj {
    fn from(value: f64) -> Self {
        Self::scalar(value.to_string())
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
trait IntoTclCommand {
    fn into_command(self) -> Vec<TclObj>;
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl IntoTclCommand for TclObj {
    fn into_command(self) -> Vec<TclObj> {
        match self.kind {
            TclObjKind::List(items) => items,
            TclObjKind::Scalar(_) => vec![self],
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl_into_tcl_command_tuple!(A => a);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl_into_tcl_command_tuple!(A => a, B => b);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl_into_tcl_command_tuple!(A => a, B => b, C => c);
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl_into_tcl_command_tuple!(A => a, B => b, C => c, D => d);

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_result_string(api: &TclApi, interp: *mut c_void) -> String {
    let ptr = unsafe { (api.get_string_result)(interp) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_split_list(api: &TclApi, interp: *mut c_void, list: &str) -> Result<Vec<String>, String> {
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_merge_args(api: &TclApi, args: &[String]) -> Result<Vec<u8>, String> {
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
struct TclInterpreter {
    interp_addr: usize,
    owner_thread: ThreadId,
    api: &'static TclApi,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl TclInterpreter {
    fn new() -> Result<Self, String> {
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
        Ok(Self {
            interp_addr: interp_ptr as usize,
            owner_thread: thread::current().id(),
            api,
        })
    }

    fn interp_ptr(&self) -> *mut c_void {
        self.interp_addr as *mut c_void
    }

    fn ensure_owner_thread(&self) -> Result<(), String> {
        if thread::current().id() != self.owner_thread {
            return Err("Tk interpreter used from a different thread".to_string());
        }
        Ok(())
    }

    fn eval<C: IntoTclCommand>(&self, command: C) -> Result<TclObj, String> {
        self.ensure_owner_thread()?;
        let parts = command.into_command();
        let mut objv = Vec::with_capacity(parts.len());
        for part in &parts {
            objv.push(self.alloc_obj(part)?);
        }
        let rc = unsafe {
            for &obj in &objv {
                self.api.incr_ref_count_obj(obj);
            }
            let call_rc = (self.api.eval_objv)(
                self.interp_ptr(),
                objv.len() as c_int,
                objv.as_ptr(),
                0,
            );
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

    fn get(&self, name: &str) -> Result<TclObj, String> {
        self.eval(("set", name))
    }

    fn do_one_event(&self, flags: i32) -> Result<bool, String> {
        self.ensure_owner_thread()?;
        Ok(unsafe { (self.api.do_one_event)(flags as c_int) != 0 })
    }

    fn render_part(&self, part: &TclObj) -> Result<String, String> {
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

    fn alloc_obj(&self, part: &TclObj) -> Result<*mut c_void, String> {
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
                        unsafe {
                            self.api.decr_ref_count_obj(nested_obj);
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

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
impl Drop for TclInterpreter {
    fn drop(&mut self) {
        if self.interp_addr != 0 {
            unsafe { (self.api.delete_interp)(self.interp_ptr()) };
            self.interp_addr = 0;
        }
    }
}

const TK_UNAVAILABLE_LABEL: &str = "tkinter runtime support is not implemented yet";
const TK_CAPABILITY_GUI_WINDOW: &str = "gui.window";
const TK_CAPABILITY_PROCESS_SPAWN: &str = "process.spawn";
const TK_BLOCKER_WASM_TARGET: &str = "target.wasm32";
const TK_BLOCKER_BACKEND_UNIMPLEMENTED: &str = "backend.not_implemented";
const TK_BLOCKER_CAP_GUI_WINDOW: &str = "capability.gui.window";
const TK_BLOCKER_CAP_PROCESS_SPAWN: &str = "capability.process.spawn";
const TK_BLOCKER_PLATFORM_LINUX_DISPLAY: &str = "platform.linux.display";
const TK_BLOCKER_PLATFORM_MACOS_MAIN_THREAD: &str = "platform.macos.main_thread";
const TK_FILE_EVENT_READABLE: i64 = 2;
const TK_FILE_EVENT_WRITABLE: i64 = 4;
const TK_FILE_EVENT_EXCEPTION: i64 = 8;
const TK_DONT_WAIT_FLAG: i32 = 2;
const TK_BIND_SUBST_FORMAT_STR: &str = "%# %b %f %h %k %s %t %w %x %y %A %E %K %N %W %T %X %Y %D";

#[cfg(target_arch = "wasm32")]
const TK_BACKEND_IMPLEMENTED: bool = false;
#[cfg(not(target_arch = "wasm32"))]
const TK_BACKEND_IMPLEMENTED: bool = true;

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tk_runtime_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::panic::catch_unwind(|| {
            let Ok(interp) = TclInterpreter::new() else {
                return false;
            };
            interp.eval(("package", "require", "Tk")).is_ok()
        })
        .unwrap_or(false)
    })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_runtime_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE
        .get_or_init(|| std::panic::catch_unwind(|| TclInterpreter::new().is_ok()).unwrap_or(false))
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn tcl_runtime_available() -> bool {
    true
}

#[cfg(target_arch = "wasm32")]
fn tcl_runtime_available() -> bool {
    false
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn tk_runtime_available() -> bool {
    // Headless phase-0 path remains available without compiling Tcl bindings.
    true
}

#[cfg(target_arch = "wasm32")]
fn tk_runtime_available() -> bool {
    false
}

#[derive(Clone, Copy)]
enum TkOperation {
    AvailabilityProbe,
    AppNew,
    Quit,
    Mainloop,
    DoOneEvent,
    After,
    AfterIdle,
    AfterCancel,
    AfterInfo,
    Call,
    BindCommand,
    UnbindCommand,
    FileHandlerCreate,
    FileHandlerDelete,
    DestroyWidget,
    LastError,
    DialogShow,
    CommonDialogShow,
    MessageBoxShow,
    FileDialogShow,
    SimpleDialogQuery,
}

impl TkOperation {
    const fn symbol(self) -> &'static str {
        match self {
            Self::AvailabilityProbe => "molt_tk_available",
            Self::AppNew => "molt_tk_app_new",
            Self::Quit => "molt_tk_quit",
            Self::Mainloop => "molt_tk_mainloop",
            Self::DoOneEvent => "molt_tk_do_one_event",
            Self::After => "molt_tk_after",
            Self::AfterIdle => "molt_tk_after_idle",
            Self::AfterCancel => "molt_tk_after_cancel",
            Self::AfterInfo => "molt_tk_after_info",
            Self::Call => "molt_tk_call",
            Self::BindCommand => "molt_tk_bind_command",
            Self::UnbindCommand => "molt_tk_unbind_command",
            Self::FileHandlerCreate => "molt_tk_filehandler_create",
            Self::FileHandlerDelete => "molt_tk_filehandler_delete",
            Self::DestroyWidget => "molt_tk_destroy_widget",
            Self::LastError => "molt_tk_last_error",
            Self::DialogShow => "molt_tk_dialog_show",
            Self::CommonDialogShow => "molt_tk_commondialog_show",
            Self::MessageBoxShow => "molt_tk_messagebox_show",
            Self::FileDialogShow => "molt_tk_filedialog_show",
            Self::SimpleDialogQuery => "molt_tk_simpledialog_query",
        }
    }

    const fn requires_gui_window(self) -> bool {
        matches!(
            self,
            Self::AppNew
                | Self::Quit
                | Self::Mainloop
                | Self::DoOneEvent
                | Self::After
                | Self::AfterIdle
                | Self::AfterCancel
                | Self::AfterInfo
                | Self::Call
                | Self::BindCommand
                | Self::UnbindCommand
                | Self::FileHandlerCreate
                | Self::FileHandlerDelete
                | Self::DestroyWidget
                | Self::DialogShow
                | Self::CommonDialogShow
                | Self::MessageBoxShow
                | Self::FileDialogShow
                | Self::SimpleDialogQuery
        )
    }

    const fn requires_process_spawn(self) -> bool {
        matches!(self, Self::AppNew)
    }
}

#[derive(Default)]
struct TkGateState {
    wasm_unsupported: bool,
    backend_unimplemented: bool,
    missing_gui_window: bool,
    missing_process_spawn: bool,
    missing_linux_display: bool,
    missing_macos_main_thread: bool,
}

#[derive(Default)]
struct TkRegistry {
    next_handle: i64,
    apps: HashMap<i64, TkAppState>,
}

#[derive(Default)]
struct TkAppState {
    callbacks: HashMap<String, u64>,
    one_shot_callbacks: HashSet<String>,
    filehandlers: HashMap<i64, TkFileHandlerRegistration>,
    filehandler_commands: HashMap<String, TkFileHandlerCommand>,
    event_queue: VecDeque<TkEvent>,
    variables: HashMap<String, u64>,
    variable_versions: HashMap<String, u64>,
    next_variable_version: u64,
    widgets: HashMap<String, TkWidgetState>,
    images: HashMap<String, TkImageState>,
    fonts: HashMap<String, TkFontState>,
    tix_options: HashMap<String, u64>,
    option_db: HashMap<String, u64>,
    strict_motif: bool,
    ttk_style: TkTtkStyleState,
    bind_scripts: HashMap<String, HashMap<String, String>>,
    bindtags: HashMap<String, Vec<String>>,
    virtual_events: HashMap<String, Vec<String>>,
    traces: HashMap<String, Vec<TkTraceRegistration>>,
    next_trace_order: u64,
    pack_slaves: Vec<String>,
    grid_slaves: Vec<String>,
    place_slaves: Vec<String>,
    pack_propagate: HashMap<String, bool>,
    grid_propagate: HashMap<String, bool>,
    focus_widget: Option<String>,
    grab_widget: Option<String>,
    grab_is_global: bool,
    clipboard_text: String,
    selection_text: String,
    selection_owner: Option<String>,
    after_command_tokens: HashMap<String, String>,
    after_command_kinds: HashMap<String, String>,
    after_due_at_ms: HashMap<String, u64>,
    after_clock_ms: u64,
    wm: TkWmState,
    atoms_by_name: HashMap<String, i64>,
    atoms_by_id: HashMap<i64, String>,
    next_atom_id: i64,
    last_error: Option<String>,
    next_after_id: u64,
    next_callback_command_id: u64,
    quit_requested: bool,
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    interpreter: Option<TclInterpreter>,
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    tk_loaded: bool,
}

#[derive(Default)]
struct TkImageState {
    kind: String,
    options: HashMap<String, u64>,
}

#[derive(Default)]
struct TkFontState {
    options: HashMap<String, u64>,
}

#[derive(Default)]
struct TkMenuEntryState {
    item_type: String,
    options: HashMap<String, u64>,
}

#[derive(Default)]
struct TkWidgetState {
    widget_command: String,
    options: HashMap<String, u64>,
    treeview: Option<TkTreeviewState>,
    ttk_state: HashSet<String>,
    ttk_values: HashMap<String, u64>,
    ttk_items: Vec<String>,
    ttk_item_options: HashMap<String, HashMap<String, u64>>,
    ttk_sash_positions: HashMap<i64, i64>,
    manager: Option<String>,
    pack_options: HashMap<String, u64>,
    grid_options: HashMap<String, u64>,
    place_options: HashMap<String, u64>,
    grid_columnconfigure: HashMap<String, HashMap<String, u64>>,
    grid_rowconfigure: HashMap<String, HashMap<String, u64>>,
    tag_bindings: HashMap<String, HashMap<String, String>>,
    text_tag_ranges: HashMap<String, Vec<(usize, usize)>>,
    text_tag_options: HashMap<String, HashMap<String, String>>,
    text_tag_order: Vec<String>,
    text_marks: HashMap<String, usize>,
    text_mark_gravity: HashMap<String, String>,
    text_value: String,
    list_items: Vec<u64>,
    list_item_options: HashMap<usize, HashMap<String, u64>>,
    list_selection: HashSet<usize>,
    list_active_index: Option<usize>,
    menu_entries: Vec<TkMenuEntryState>,
    menu_active_index: Option<usize>,
    menu_posted_at: Option<(i64, i64)>,
    pane_children: Vec<String>,
    pane_child_options: HashMap<String, HashMap<String, u64>>,
    selection_anchor: Option<usize>,
    selection_range: Option<(usize, usize)>,
    insert_cursor: usize,
    text_edit_modified: bool,
    next_item_id: i64,
}

struct TkFileHandlerRegistration {
    callback_bits: u64,
    file_obj_bits: u64,
    commands: HashMap<i64, String>,
}

#[derive(Clone, Copy)]
struct TkFileHandlerCommand {
    fd: i64,
    mask: i64,
}

#[derive(Clone)]
struct TkTraceRegistration {
    mode_name: String,
    callback_name: String,
    order: u64,
}

#[derive(Default)]
struct TkTtkStyleState {
    configure: HashMap<String, HashMap<String, u64>>,
    style_map: HashMap<String, HashMap<String, u64>>,
    layouts: HashMap<String, u64>,
    elements: HashSet<String>,
    element_options: HashMap<String, Vec<String>>,
    themes: HashSet<String>,
    current_theme: Option<String>,
}

struct TkWmState {
    title: String,
    geometry: String,
    state: String,
    attributes: HashMap<String, u64>,
    aspect: Option<(i64, i64, i64, i64)>,
    client: String,
    colormapwindows: Vec<String>,
    command: Vec<String>,
    focusmodel: String,
    frame: String,
    grid: Option<(i64, i64, i64, i64)>,
    group: Option<String>,
    iconbitmap: String,
    iconmask: String,
    resizable_width: bool,
    resizable_height: bool,
    minsize: (i64, i64),
    maxsize: (i64, i64),
    overrideredirect: bool,
    transient: Option<String>,
    iconname: String,
    iconposition: Option<(i64, i64)>,
    iconwindow: Option<String>,
    positionfrom: String,
    sizefrom: String,
    protocols: HashMap<String, String>,
}

impl Default for TkWmState {
    fn default() -> Self {
        Self {
            title: String::new(),
            geometry: "1x1+0+0".to_string(),
            state: "normal".to_string(),
            attributes: HashMap::new(),
            aspect: None,
            client: String::new(),
            colormapwindows: Vec::new(),
            command: Vec::new(),
            focusmodel: "passive".to_string(),
            frame: ".".to_string(),
            grid: None,
            group: None,
            iconbitmap: String::new(),
            iconmask: String::new(),
            resizable_width: true,
            resizable_height: true,
            minsize: (1, 1),
            maxsize: (32767, 32767),
            overrideredirect: false,
            transient: None,
            iconname: String::new(),
            iconposition: None,
            iconwindow: None,
            positionfrom: String::new(),
            sizefrom: String::new(),
            protocols: HashMap::new(),
        }
    }
}

#[derive(Default)]
struct TkTreeviewState {
    items: HashMap<String, TkTreeviewItem>,
    root_children: Vec<String>,
    selection: Vec<String>,
    focus: Option<String>,
    columns: HashMap<String, HashMap<String, u64>>,
    headings: HashMap<String, HashMap<String, u64>>,
    tags: HashMap<String, TkTreeTagState>,
    next_auto_id: u64,
}

#[derive(Default)]
struct TkTreeviewItem {
    parent: String,
    children: Vec<String>,
    options: HashMap<String, u64>,
    values: HashMap<String, u64>,
}

#[derive(Default)]
struct TkTreeTagState {
    options: HashMap<String, u64>,
    bindings: HashMap<String, String>,
}

enum TkEvent {
    Callback {
        token: String,
    },
    Script {
        token: String,
        commands: Vec<Vec<String>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TkExprLiteral {
    Int(i64),
    Float(f64),
}

fn has_gui_window_capability(py: &PyToken<'_>) -> bool {
    has_capability(py, TK_CAPABILITY_GUI_WINDOW) || has_capability(py, "gui")
}

fn has_process_spawn_capability(py: &PyToken<'_>) -> bool {
    has_capability(py, TK_CAPABILITY_PROCESS_SPAWN) || has_capability(py, "process")
}

#[cfg(target_os = "linux")]
fn env_var_non_empty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.to_string_lossy().trim().is_empty())
}

#[cfg(target_os = "linux")]
fn linux_display_available() -> bool {
    env_var_non_empty("DISPLAY") || env_var_non_empty("WAYLAND_DISPLAY")
}

#[cfg(not(target_os = "linux"))]
fn linux_display_available() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn macos_on_main_thread() -> bool {
    unsafe { libc::pthread_main_np() == 1 }
}

#[cfg(not(target_os = "macos"))]
fn macos_on_main_thread() -> bool {
    true
}

fn tk_gate_state(py: &PyToken<'_>, op: TkOperation) -> TkGateState {
    let wasm_unsupported = cfg!(target_arch = "wasm32");
    let requires_gui_window = op.requires_gui_window();
    let backend_unimplemented = !wasm_unsupported
        && match op {
            TkOperation::AvailabilityProbe => !TK_BACKEND_IMPLEMENTED || !tk_runtime_available(),
            TkOperation::AppNew => !TK_BACKEND_IMPLEMENTED || !tcl_runtime_available(),
            _ => false,
        };
    let missing_gui_window =
        !wasm_unsupported && requires_gui_window && !has_gui_window_capability(py);
    let missing_process_spawn =
        !wasm_unsupported && op.requires_process_spawn() && !has_process_spawn_capability(py);
    let missing_linux_display =
        !wasm_unsupported && requires_gui_window && !linux_display_available();
    let missing_macos_main_thread =
        !wasm_unsupported && requires_gui_window && !macos_on_main_thread();
    TkGateState {
        wasm_unsupported,
        backend_unimplemented,
        missing_gui_window,
        missing_process_spawn,
        missing_linux_display,
        missing_macos_main_thread,
    }
}

fn tk_registry() -> &'static Mutex<TkRegistry> {
    static REGISTRY: OnceLock<Mutex<TkRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(TkRegistry {
            next_handle: 1,
            apps: HashMap::new(),
        })
    })
}

fn tk_blockers(state: &TkGateState) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if state.wasm_unsupported {
        blockers.push(TK_BLOCKER_WASM_TARGET);
    }
    if state.backend_unimplemented {
        blockers.push(TK_BLOCKER_BACKEND_UNIMPLEMENTED);
    }
    if state.missing_gui_window {
        blockers.push(TK_BLOCKER_CAP_GUI_WINDOW);
    }
    if state.missing_process_spawn {
        blockers.push(TK_BLOCKER_CAP_PROCESS_SPAWN);
    }
    if state.missing_linux_display {
        blockers.push(TK_BLOCKER_PLATFORM_LINUX_DISPLAY);
    }
    if state.missing_macos_main_thread {
        blockers.push(TK_BLOCKER_PLATFORM_MACOS_MAIN_THREAD);
    }
    blockers
}

fn has_platform_preflight_blockers(state: &TkGateState) -> bool {
    state.missing_linux_display || state.missing_macos_main_thread
}

fn format_tk_unavailable_message(op: TkOperation, state: &TkGateState) -> String {
    let blockers = tk_blockers(state);
    if blockers.is_empty() {
        format!("{TK_UNAVAILABLE_LABEL} ({})", op.symbol())
    } else {
        format!(
            "{TK_UNAVAILABLE_LABEL} ({}) [blockers: {}]",
            op.symbol(),
            blockers.join(", ")
        )
    }
}

fn format_permission_error_message(state: &TkGateState) -> String {
    let mut missing = Vec::new();
    if state.missing_gui_window {
        missing.push(TK_CAPABILITY_GUI_WINDOW);
    }
    if state.missing_process_spawn {
        missing.push(TK_CAPABILITY_PROCESS_SPAWN);
    }
    debug_assert!(!missing.is_empty());
    if missing.len() == 1 {
        format!("missing {} capability", missing[0])
    } else {
        format!("missing capabilities: {}", missing.join(", "))
    }
}

fn raise_tk_gate_error(py: &PyToken<'_>, op: TkOperation, state: &TkGateState) -> u64 {
    if state.wasm_unsupported {
        return raise_exception::<u64>(
            py,
            "NotImplementedError",
            &format_tk_unavailable_message(op, state),
        );
    }
    if state.backend_unimplemented {
        return raise_exception::<u64>(
            py,
            "RuntimeError",
            &format_tk_unavailable_message(op, state),
        );
    }
    if state.missing_gui_window || state.missing_process_spawn {
        return raise_exception::<u64>(
            py,
            "PermissionError",
            &format_permission_error_message(state),
        );
    }
    if has_platform_preflight_blockers(state) {
        return raise_exception::<u64>(
            py,
            "RuntimeError",
            &format_tk_unavailable_message(op, state),
        );
    }
    raise_exception::<u64>(
        py,
        "RuntimeError",
        &format!("internal tkinter gate error ({})", op.symbol()),
    )
}

fn require_tk_operation(py: &PyToken<'_>, op: TkOperation) -> Result<(), u64> {
    let state = tk_gate_state(py, op);
    if state.wasm_unsupported
        || state.backend_unimplemented
        || state.missing_gui_window
        || state.missing_process_spawn
        || has_platform_preflight_blockers(&state)
    {
        return Err(raise_tk_gate_error(py, op, &state));
    }
    Ok(())
}

fn require_tk_app_new(py: &PyToken<'_>, _use_tk: bool) -> Result<(), u64> {
    let state = tk_gate_state(py, TkOperation::AppNew);
    if state.wasm_unsupported
        || state.backend_unimplemented
        || state.missing_gui_window
        || state.missing_process_spawn
        || has_platform_preflight_blockers(&state)
    {
        return Err(raise_tk_gate_error(py, TkOperation::AppNew, &state));
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    if _use_tk && !tk_runtime_available() {
        let unavailable = TkGateState {
            backend_unimplemented: true,
            ..TkGateState::default()
        };
        return Err(raise_tk_gate_error(py, TkOperation::AppNew, &unavailable));
    }
    Ok(())
}

fn raise_tcl_error(py: &PyToken<'_>, message: &str) -> u64 {
    raise_exception::<u64>(py, "RuntimeError", &format!("TclError: {message}"))
}

fn alloc_string_bits(py: &PyToken<'_>, value: &str) -> Result<u64, u64> {
    let ptr = crate::alloc_string(py, value.as_bytes());
    if ptr.is_null() {
        return Err(raise_exception::<u64>(
            py,
            "MemoryError",
            "failed to allocate tkinter string",
        ));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn app_tcl_error_locked(py: &PyToken<'_>, app: &mut TkAppState, message: impl Into<String>) -> u64 {
    let message = message.into();
    app.last_error = Some(message.clone());
    raise_tcl_error(py, &message)
}

fn raise_invalid_handle_error(py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(py, "ValueError", "invalid tkinter app handle")
}

fn parse_app_handle(py: &PyToken<'_>, app_bits: u64) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(app_bits)) else {
        return Err(raise_invalid_handle_error(py));
    };
    if handle <= 0 {
        return Err(raise_invalid_handle_error(py));
    }
    Ok(handle)
}

fn app_mut_from_registry<'a>(
    py: &PyToken<'_>,
    registry: &'a mut TkRegistry,
    handle: i64,
) -> Result<&'a mut TkAppState, u64> {
    registry
        .apps
        .get_mut(&handle)
        .ok_or_else(|| raise_invalid_handle_error(py))
}

fn clear_value_map_refs(py: &PyToken<'_>, values: &mut HashMap<String, u64>) {
    for bits in values.drain().map(|(_, bits)| bits) {
        dec_ref_bits(py, bits);
    }
}

fn clear_nested_value_map_refs(
    py: &PyToken<'_>,
    values: &mut HashMap<String, HashMap<String, u64>>,
) {
    for mut nested in values.drain().map(|(_, nested)| nested) {
        clear_value_map_refs(py, &mut nested);
    }
}

fn value_map_set_bits(py: &PyToken<'_>, values: &mut HashMap<String, u64>, key: String, bits: u64) {
    inc_ref_bits(py, bits);
    if let Some(old_bits) = values.insert(key, bits) {
        dec_ref_bits(py, old_bits);
    }
}

fn clear_treeview_refs(py: &PyToken<'_>, treeview: &mut TkTreeviewState) {
    for item in treeview.items.values_mut() {
        clear_value_map_refs(py, &mut item.options);
        clear_value_map_refs(py, &mut item.values);
    }
    for options in treeview.columns.values_mut() {
        clear_value_map_refs(py, options);
    }
    for options in treeview.headings.values_mut() {
        clear_value_map_refs(py, options);
    }
    for tag in treeview.tags.values_mut() {
        clear_value_map_refs(py, &mut tag.options);
        tag.bindings.clear();
    }
    treeview.items.clear();
    treeview.root_children.clear();
    treeview.selection.clear();
    treeview.focus = None;
}

fn clear_widget_refs(py: &PyToken<'_>, widget: TkWidgetState) {
    let mut options = widget.options;
    clear_value_map_refs(py, &mut options);
    for bits in widget.list_items {
        dec_ref_bits(py, bits);
    }
    for mut item_options in widget.list_item_options.into_values() {
        clear_value_map_refs(py, &mut item_options);
    }
    for mut menu_entry in widget.menu_entries {
        clear_value_map_refs(py, &mut menu_entry.options);
    }
    let mut ttk_values = widget.ttk_values;
    clear_value_map_refs(py, &mut ttk_values);
    for mut item_options in widget.ttk_item_options.into_values() {
        clear_value_map_refs(py, &mut item_options);
    }
    if let Some(mut treeview) = widget.treeview {
        clear_treeview_refs(py, &mut treeview);
    }
    let mut pack_options = widget.pack_options;
    clear_value_map_refs(py, &mut pack_options);
    let mut grid_options = widget.grid_options;
    clear_value_map_refs(py, &mut grid_options);
    let mut place_options = widget.place_options;
    clear_value_map_refs(py, &mut place_options);
    let mut grid_columnconfigure = widget.grid_columnconfigure;
    clear_nested_value_map_refs(py, &mut grid_columnconfigure);
    let mut grid_rowconfigure = widget.grid_rowconfigure;
    clear_nested_value_map_refs(py, &mut grid_rowconfigure);
    let mut pane_child_options = widget.pane_child_options;
    clear_nested_value_map_refs(py, &mut pane_child_options);
}

fn clear_ttk_style_refs(py: &PyToken<'_>, style: &mut TkTtkStyleState) {
    for options in style.configure.values_mut() {
        clear_value_map_refs(py, options);
    }
    style.configure.clear();
    for options in style.style_map.values_mut() {
        clear_value_map_refs(py, options);
    }
    style.style_map.clear();
    for bits in style.layouts.drain().map(|(_, bits)| bits) {
        dec_ref_bits(py, bits);
    }
    style.elements.clear();
    style.element_options.clear();
    style.themes.clear();
    style.current_theme = None;
}

fn clear_wm_refs(py: &PyToken<'_>, wm: &mut TkWmState) {
    clear_value_map_refs(py, &mut wm.attributes);
}

fn drop_app_state_refs(py: &PyToken<'_>, app: &mut TkAppState) {
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    unregister_all_tcl_callback_procs(app);
    clear_value_map_refs(py, &mut app.variables);
    let filehandler_fds: Vec<i64> = app.filehandlers.keys().copied().collect();
    for fd in filehandler_fds {
        let _ = clear_filehandler_registration_locked(py, app, fd);
    }
    app.filehandler_commands.clear();
    for callback_bits in app.callbacks.drain().map(|(_, bits)| bits) {
        dec_ref_bits(py, callback_bits);
    }
    app.one_shot_callbacks.clear();
    for event in app.event_queue.drain(..) {
        match event {
            TkEvent::Callback { .. } => {}
            TkEvent::Script { .. } => {}
        }
    }
    for widget in app.widgets.drain().map(|(_, widget)| widget) {
        clear_widget_refs(py, widget);
    }
    for image in app.images.values_mut() {
        clear_value_map_refs(py, &mut image.options);
    }
    app.images.clear();
    for font in app.fonts.values_mut() {
        clear_value_map_refs(py, &mut font.options);
    }
    app.fonts.clear();
    clear_value_map_refs(py, &mut app.tix_options);
    clear_value_map_refs(py, &mut app.option_db);
    app.strict_motif = false;
    clear_ttk_style_refs(py, &mut app.ttk_style);
    clear_wm_refs(py, &mut app.wm);
    app.bind_scripts.clear();
    app.bindtags.clear();
    app.virtual_events.clear();
    app.traces.clear();
    app.next_trace_order = 0;
    app.pack_slaves.clear();
    app.grid_slaves.clear();
    app.place_slaves.clear();
    app.pack_propagate.clear();
    app.grid_propagate.clear();
    app.focus_widget = None;
    app.grab_widget = None;
    app.grab_is_global = false;
    app.clipboard_text.clear();
    app.selection_text.clear();
    app.selection_owner = None;
    app.after_command_tokens.clear();
    app.after_command_kinds.clear();
    app.after_due_at_ms.clear();
    app.after_clock_ms = 0;
    app.variable_versions.clear();
    app.next_variable_version = 0;
    app.atoms_by_name.clear();
    app.atoms_by_id.clear();
    app.next_atom_id = 0;
    app.last_error = None;
    app.next_callback_command_id = 0;
    app.quit_requested = true;
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    {
        app.interpreter = None;
        app.tk_loaded = false;
    }
}

fn next_after_token(next_after_id: &mut u64) -> String {
    *next_after_id = next_after_id.saturating_add(1);
    format!("after#{}", *next_after_id)
}

fn after_callback_name_from_token(token: &str) -> String {
    let suffix = token.strip_prefix("after#").unwrap_or(token);
    format!("::__molt_after_callback_{suffix}")
}

fn next_callback_command_name(app: &mut TkAppState, prefix: &str) -> String {
    loop {
        app.next_callback_command_id = app.next_callback_command_id.saturating_add(1);
        if app.next_callback_command_id == 0 {
            app.next_callback_command_id = 1;
        }
        let command_name = format!("::__molt_{prefix}_{}", app.next_callback_command_id);
        if !app.callbacks.contains_key(&command_name)
            && !app.filehandler_commands.contains_key(&command_name)
        {
            return command_name;
        }
    }
}

fn callback_is_callable(callback_bits: u64) -> bool {
    let callable_check = crate::molt_is_callable(callback_bits);
    to_i64(obj_from_bits(callable_check)) == Some(1)
}

fn register_callback_command(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    command_name: &str,
    callback_bits: u64,
    callback_label: &str,
) -> Result<(), u64> {
    if app.callbacks.contains_key(command_name)
        || app.filehandler_commands.contains_key(command_name)
    {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("{callback_label} name collision for \"{command_name}\""),
        ));
    }
    if let Err(err) = register_tcl_callback_proc(app, command_name) {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("failed to register {callback_label} \"{command_name}\": {err}"),
        ));
    }
    inc_ref_bits(py, callback_bits);
    if let Some(old_bits) = app
        .callbacks
        .insert(command_name.to_string(), callback_bits)
    {
        dec_ref_bits(py, old_bits);
    }
    app.one_shot_callbacks.remove(command_name);
    Ok(())
}

fn unregister_callback_command(py: &PyToken<'_>, app: &mut TkAppState, command_name: &str) {
    app.one_shot_callbacks.remove(command_name);
    unregister_tcl_callback_proc(app, command_name);
    if let Some(callback_bits) = app.callbacks.remove(command_name) {
        dec_ref_bits(py, callback_bits);
    }
}

fn trace_callback_is_referenced(app: &TkAppState, callback_name: &str) -> bool {
    app.traces.values().any(|registrations| {
        registrations
            .iter()
            .any(|registration| registration.callback_name == callback_name)
    })
}

fn remove_trace_registration(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    variable_name: &str,
    mode_name: &str,
    callback_name: &str,
) {
    if let Some(registrations) = app.traces.get_mut(variable_name) {
        registrations.retain(|registration| {
            !(registration.mode_name == mode_name && registration.callback_name == callback_name)
        });
        if registrations.is_empty() {
            app.traces.remove(variable_name);
        }
    }
    if !trace_callback_is_referenced(app, callback_name) {
        unregister_callback_command(py, app, callback_name);
    }
}

fn clear_trace_registrations_for_variable(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    variable_name: &str,
) {
    let Some(registrations) = app.traces.remove(variable_name) else {
        return;
    };
    let callbacks: HashSet<String> = registrations
        .into_iter()
        .map(|registration| registration.callback_name)
        .collect();
    for callback_name in callbacks {
        if !trace_callback_is_referenced(app, callback_name.as_str()) {
            unregister_callback_command(py, app, callback_name.as_str());
        }
    }
}

fn normalize_bind_add_prefix(py: &PyToken<'_>, add_bits: u64) -> Result<String, u64> {
    let add_obj = obj_from_bits(add_bits);
    if add_obj.is_none() {
        return Ok(String::new());
    }
    if add_obj.is_bool() {
        return Ok(if add_obj.as_bool().unwrap_or(false) {
            "+".to_string()
        } else {
            String::new()
        });
    }
    if let Some(value) = to_i64(add_obj) {
        return match value {
            0 => Ok(String::new()),
            1 => Ok("+".to_string()),
            _ => Err(raise_exception::<u64>(
                py,
                "TypeError",
                "bind add must be one of: None, '', False, True, or '+'",
            )),
        };
    }
    if let Some(value) = string_obj_to_owned(add_obj) {
        return match value.as_str() {
            "" => Ok(String::new()),
            "+" => Ok("+".to_string()),
            _ => Err(raise_exception::<u64>(
                py,
                "TypeError",
                "bind add must be one of: None, '', False, True, or '+'",
            )),
        };
    }
    Err(raise_exception::<u64>(
        py,
        "TypeError",
        "bind add must be one of: None, '', False, True, or '+'",
    ))
}

fn register_after_command_token(app: &mut TkAppState, token: &str, command_name: &str, kind: &str) {
    app.after_command_tokens
        .insert(token.to_string(), command_name.to_string());
    app.after_command_kinds
        .insert(token.to_string(), kind.to_string());
}

fn unregister_after_command_token(app: &mut TkAppState, token: &str) {
    app.after_command_tokens.remove(token);
    app.after_command_kinds.remove(token);
    app.after_due_at_ms.remove(token);
}

fn lookup_after_command_for_token(app: &TkAppState, token: &str) -> Option<String> {
    app.after_command_tokens.get(token).cloned()
}

fn lookup_after_kind_for_token(app: &TkAppState, token: &str) -> Option<String> {
    app.after_command_kinds.get(token).cloned()
}

fn parse_after_token_id(token: &str) -> Option<u64> {
    token.strip_prefix("after#")?.parse::<u64>().ok()
}

fn sort_after_info_tokens(tokens: &mut [String]) {
    tokens.sort_by(
        |left, right| match (parse_after_token_id(left), parse_after_token_id(right)) {
            (Some(left_id), Some(right_id)) => right_id.cmp(&left_id).then_with(|| left.cmp(right)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.cmp(right),
        },
    );
}

fn alloc_after_info_all(py: &PyToken<'_>, app: &TkAppState) -> Result<u64, u64> {
    let mut tokens: Vec<String> = app.after_command_tokens.keys().cloned().collect();
    sort_after_info_tokens(&mut tokens);
    alloc_tuple_from_strings(py, tokens.as_slice(), "failed to allocate after info tuple")
}

fn alloc_after_info_token(py: &PyToken<'_>, app: &mut TkAppState, token: &str) -> Result<u64, u64> {
    let Some(command_name) = lookup_after_command_for_token(app, token) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("event \"{token}\" doesn't exist"),
        ));
    };
    let kind = lookup_after_kind_for_token(app, token).unwrap_or_else(|| {
        if command_name.starts_with("::__molt_after_callback_") {
            "timer".to_string()
        } else {
            "idle".to_string()
        }
    });
    let info = [command_name, kind];
    alloc_tuple_from_strings(py, &info, "failed to allocate after info token tuple")
}

fn remove_after_events_for_tokens(app: &mut TkAppState, tokens: &HashSet<String>) {
    app.event_queue.retain(|event| match event {
        TkEvent::Callback { token } => !tokens.contains(token),
        TkEvent::Script { token, .. } => !tokens.contains(token),
    });
}

fn schedule_after_timer_token(app: &mut TkAppState, token: &str, delay_ms: i64) {
    if delay_ms <= 0 {
        app.after_due_at_ms
            .insert(token.to_string(), app.after_clock_ms);
        return;
    }
    let delay = u64::try_from(delay_ms).unwrap_or(u64::MAX);
    let due_at = app.after_clock_ms.saturating_add(delay);
    app.after_due_at_ms.insert(token.to_string(), due_at);
}

fn cleanup_after_tokens(py: &PyToken<'_>, app: &mut TkAppState, tokens: &HashSet<String>) {
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    if let Some(interp) = app.interpreter.as_ref() {
        for token in tokens {
            let _ = interp.eval(("after", "cancel", token.clone()));
        }
    }
    for token in tokens {
        let command_name = lookup_after_command_for_token(app, token);
        unregister_after_command_token(app, token);
        let internal_name = command_name
            .clone()
            .unwrap_or_else(|| after_callback_name_from_token(token));
        if internal_name.starts_with("::__molt_after_callback_") {
            app.one_shot_callbacks.remove(&internal_name);
            if let Some(bits) = app.callbacks.remove(&internal_name) {
                dec_ref_bits(py, bits);
            }
            unregister_tcl_callback_proc(app, &internal_name);
        }
    }
    remove_after_events_for_tokens(app, tokens);
}

fn tokens_for_after_command(app: &TkAppState, command_name: &str) -> HashSet<String> {
    app.after_command_tokens
        .iter()
        .filter_map(|(token, mapped)| (mapped == command_name).then_some(token.clone()))
        .collect()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn filehandler_event_name(mask: i64) -> Option<&'static str> {
    match mask {
        TK_FILE_EVENT_READABLE => Some("readable"),
        TK_FILE_EVENT_WRITABLE => Some("writable"),
        TK_FILE_EVENT_EXCEPTION => Some("exception"),
        _ => None,
    }
}

fn filehandler_command_name(fd: i64, event_name: &str) -> String {
    format!("::__molt_filehandler_{fd}_{event_name}")
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn filehandler_poll_events(registration: &TkFileHandlerRegistration) -> libc::c_short {
    let mut events: libc::c_short = 0;
    if registration.commands.contains_key(&TK_FILE_EVENT_READABLE) {
        events |= libc::POLLIN;
    }
    if registration.commands.contains_key(&TK_FILE_EVENT_WRITABLE) {
        events |= libc::POLLOUT;
    }
    if registration.commands.contains_key(&TK_FILE_EVENT_EXCEPTION) {
        events |= libc::POLLPRI;
    }
    events
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn filehandler_revents_to_mask(revents: libc::c_short) -> i64 {
    let mut mask = 0_i64;
    if (revents & libc::POLLIN) != 0 || (revents & libc::POLLHUP) != 0 {
        mask |= TK_FILE_EVENT_READABLE;
    }
    if (revents & libc::POLLOUT) != 0 {
        mask |= TK_FILE_EVENT_WRITABLE;
    }
    if (revents & libc::POLLPRI) != 0
        || (revents & libc::POLLERR) != 0
        || (revents & libc::POLLNVAL) != 0
    {
        mask |= TK_FILE_EVENT_EXCEPTION;
    }
    mask
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn next_ready_filehandler_commands(py: &PyToken<'_>, handle: i64) -> Result<Vec<String>, u64> {
    let mut pollfds: Vec<libc::pollfd> = Vec::new();
    let mut fd_commands: Vec<Vec<(i64, String)>> = Vec::new();
    {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        for (fd, registration) in &app.filehandlers {
            let Ok(fd_native) = libc::c_int::try_from(*fd) else {
                continue;
            };
            let events = filehandler_poll_events(registration);
            if events == 0 {
                continue;
            }
            let mut commands: Vec<(i64, String)> = registration
                .commands
                .iter()
                .map(|(mask, command_name)| (*mask, command_name.clone()))
                .collect();
            commands.sort_unstable_by(|left, right| left.1.cmp(&right.1));
            pollfds.push(libc::pollfd {
                fd: fd_native,
                events,
                revents: 0,
            });
            fd_commands.push(commands);
        }
    }

    if pollfds.is_empty() {
        return Ok(Vec::new());
    }

    let poll_out = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, 0) };
    if poll_out <= 0 {
        return Ok(Vec::new());
    }

    let mut ready_commands = Vec::new();
    for (idx, pollfd) in pollfds.iter().enumerate() {
        if pollfd.revents == 0 {
            continue;
        }
        let ready_mask = filehandler_revents_to_mask(pollfd.revents);
        if ready_mask == 0 {
            continue;
        }
        for (mask, command_name) in &fd_commands[idx] {
            if (ready_mask & *mask) != 0 {
                ready_commands.push(command_name.clone());
            }
        }
    }
    ready_commands.sort_unstable();
    ready_commands.dedup();
    Ok(ready_commands)
}

#[cfg(any(not(unix), target_arch = "wasm32", feature = "molt_tk_native"))]
fn next_ready_filehandler_commands(_py: &PyToken<'_>, _handle: i64) -> Result<Vec<String>, u64> {
    Ok(Vec::new())
}

fn clear_filehandler_registration_locked(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    fd: i64,
) -> Result<(), u64> {
    let Some(registration) = app.filehandlers.remove(&fd) else {
        return Ok(());
    };
    for (_mask, command_name) in &registration.commands {
        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        if let Some(event_name) = filehandler_event_name(*_mask) {
            let clear_result = app_interp_eval_list(
                py,
                app,
                vec![
                    "fileevent".to_string(),
                    fd.to_string(),
                    event_name.to_string(),
                    String::new(),
                ],
            );
            if let Err(bits) = clear_result {
                dec_ref_bits(py, registration.callback_bits);
                dec_ref_bits(py, registration.file_obj_bits);
                return Err(bits);
            }
            unregister_tcl_callback_proc(app, command_name);
        }
        app.filehandler_commands.remove(command_name);
    }
    dec_ref_bits(py, registration.callback_bits);
    dec_ref_bits(py, registration.file_obj_bits);
    Ok(())
}

fn rollback_filehandler_registration_locked(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    _fd: i64,
    registration: &mut TkFileHandlerRegistration,
) {
    let installed_commands: Vec<(i64, String)> = registration.commands.drain().collect();
    for (mask, command_name) in installed_commands {
        app.filehandler_commands.remove(&command_name);
        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        let _ = mask;
        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        if let Some(event_name) = filehandler_event_name(mask) {
            let _ = app_interp_eval_list(
                py,
                app,
                vec![
                    "fileevent".to_string(),
                    _fd.to_string(),
                    event_name.to_string(),
                    String::new(),
                ],
            );
            unregister_tcl_callback_proc(app, &command_name);
        }
    }
    dec_ref_bits(py, registration.callback_bits);
    dec_ref_bits(py, registration.file_obj_bits);
}

fn invoke_filehandler_command(
    py: &PyToken<'_>,
    handle: i64,
    command_name: &str,
) -> Result<Option<u64>, u64> {
    let (callback_bits, file_obj_bits, mask) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(command) = app.filehandler_commands.get(command_name).copied() else {
            return Ok(None);
        };
        let Some(registration) = app.filehandlers.get(&command.fd) else {
            return Ok(None);
        };
        inc_ref_bits(py, registration.callback_bits);
        inc_ref_bits(py, registration.file_obj_bits);
        (
            registration.callback_bits,
            registration.file_obj_bits,
            command.mask,
        )
    };

    let out_bits = invoke_callback(
        py,
        callback_bits,
        &[file_obj_bits, MoltObject::from_int(mask).bits()],
    );
    dec_ref_bits(py, callback_bits);
    dec_ref_bits(py, file_obj_bits);
    if exception_pending(py) {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
        set_last_error(handle, "tkinter filehandler callback raised an exception");
        return Err(MoltObject::none().bits());
    }
    clear_last_error(handle);
    Ok(Some(out_bits))
}

fn parse_tcl_script_commands(script: &str) -> Vec<Vec<String>> {
    fn push_word(words: &mut Vec<String>, current_word: &mut String) {
        if !current_word.is_empty() {
            words.push(std::mem::take(current_word));
        }
    }

    fn push_command(
        commands: &mut Vec<Vec<String>>,
        words: &mut Vec<String>,
        current_word: &mut String,
    ) {
        push_word(words, current_word);
        if !words.is_empty() {
            commands.push(std::mem::take(words));
        }
    }

    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut current_word = String::new();

    let mut in_quote = false;
    let mut brace_depth = 0usize;
    let mut escaped = false;
    let mut command_start = true;
    let mut in_comment = false;

    for ch in script.chars() {
        if in_comment {
            if ch == '\n' || ch == '\r' {
                in_comment = false;
                push_command(&mut commands, &mut words, &mut current_word);
                command_start = true;
            }
            continue;
        }

        if escaped {
            if ch != '\n' && ch != '\r' {
                current_word.push(ch);
            }
            escaped = false;
            command_start = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            command_start = false;
            continue;
        }

        if brace_depth > 0 {
            match ch {
                '{' => {
                    brace_depth = brace_depth.saturating_add(1);
                    current_word.push('{');
                }
                '}' => {
                    brace_depth = brace_depth.saturating_sub(1);
                    if brace_depth > 0 {
                        current_word.push('}');
                    }
                }
                _ => current_word.push(ch),
            }
            command_start = false;
            continue;
        }

        if in_quote {
            if ch == '"' {
                in_quote = false;
            } else {
                current_word.push(ch);
            }
            command_start = false;
            continue;
        }

        if command_start && ch == '#' {
            in_comment = true;
            continue;
        }

        match ch {
            '{' if current_word.is_empty() => {
                brace_depth = 1;
                command_start = false;
            }
            '"' => {
                in_quote = true;
                command_start = false;
            }
            ';' | '\n' | '\r' => {
                push_command(&mut commands, &mut words, &mut current_word);
                command_start = true;
            }
            _ if ch.is_whitespace() => {
                push_word(&mut words, &mut current_word);
                command_start = words.is_empty();
            }
            _ => {
                current_word.push(ch);
                command_start = false;
            }
        }
    }

    if escaped {
        current_word.push('\\');
    }
    push_command(&mut commands, &mut words, &mut current_word);
    commands
}

fn parse_expr_literal(expression: &str) -> Option<TkExprLiteral> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(TkExprLiteral::Int(value));
    }
    if let Ok(value) = trimmed.parse::<f64>()
        && value.is_finite()
    {
        return Some(TkExprLiteral::Float(value));
    }
    None
}

fn alloc_tuple_bits(py: &PyToken<'_>, elems: &[u64], alloc_context: &str) -> Result<u64, u64> {
    let ptr = crate::object::builders::alloc_tuple(py, elems);
    if ptr.is_null() {
        return Err(raise_exception::<u64>(py, "MemoryError", alloc_context));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn alloc_tuple_from_strings(
    py: &PyToken<'_>,
    values: &[String],
    alloc_context: &str,
) -> Result<u64, u64> {
    let mut bits = Vec::with_capacity(values.len());
    for value in values {
        match alloc_string_bits(py, value) {
            Ok(value_bits) => bits.push(value_bits),
            Err(err_bits) => {
                for value_bits in bits {
                    dec_ref_bits(py, value_bits);
                }
                return Err(err_bits);
            }
        }
    }
    let tuple_bits = alloc_tuple_bits(py, bits.as_slice(), alloc_context);
    for value_bits in bits {
        dec_ref_bits(py, value_bits);
    }
    tuple_bits
}

fn normalize_widget_option_name(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("-{name}")
    }
}

fn parse_widget_option_name_arg(
    py: &PyToken<'_>,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    let name = get_string_arg(py, handle, bits, label)?;
    Ok(normalize_widget_option_name(&name))
}

fn parse_widget_option_pairs(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
    start: usize,
    label: &str,
) -> Result<Vec<(String, u64)>, u64> {
    if start >= args.len() {
        return Ok(Vec::new());
    }
    if !(args.len() - start).is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be key/value pairs"),
        ));
    }
    let mut option_names = Vec::with_capacity((args.len() - start) / 2);
    for idx in (start..args.len()).step_by(2) {
        option_names.push(parse_widget_option_name_arg(
            py,
            handle,
            args[idx],
            "widget option name",
        )?);
    }
    let mut out = Vec::with_capacity(option_names.len());
    for (idx, option_name) in option_names.into_iter().enumerate() {
        let value_bits = args[start + idx * 2 + 1];
        if obj_from_bits(value_bits).is_none() {
            continue;
        }
        out.push((option_name, value_bits));
    }
    Ok(out)
}

fn option_map_to_tuple(
    py: &PyToken<'_>,
    values: &HashMap<String, u64>,
    alloc_context: &str,
) -> Result<u64, u64> {
    let mut keys: Vec<String> = values.keys().cloned().collect();
    keys.sort_unstable();
    let mut tuple_elems = Vec::with_capacity(keys.len() * 2);
    for key in keys {
        let Some(value_bits) = values.get(&key).copied() else {
            continue;
        };
        let key_bits = alloc_string_bits(py, &key)?;
        tuple_elems.push(key_bits);
        tuple_elems.push(value_bits);
    }
    let out = alloc_tuple_bits(py, tuple_elems.as_slice(), alloc_context);
    for bits in tuple_elems {
        dec_ref_bits(py, bits);
    }
    out
}

fn option_map_query_or_empty(
    py: &PyToken<'_>,
    values: &HashMap<String, u64>,
    option_name: &str,
) -> Result<u64, u64> {
    if let Some(value_bits) = values.get(option_name).copied() {
        inc_ref_bits(py, value_bits);
        return Ok(value_bits);
    }
    alloc_string_bits(py, "")
}

fn set_to_sorted_tuple(
    py: &PyToken<'_>,
    values: &HashSet<String>,
    alloc_context: &str,
) -> Result<u64, u64> {
    let mut items: Vec<String> = values.iter().cloned().collect();
    items.sort_unstable();
    alloc_tuple_from_strings(py, &items, alloc_context)
}

fn parse_bool_text(value: &str) -> Option<bool> {
    let lowered = value.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return None;
    }
    match lowered.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            let truthy = ["true", "yes", "on"];
            let falsy = ["false", "no", "off"];
            let truthy_match = truthy
                .iter()
                .filter(|candidate| candidate.starts_with(lowered.as_str()))
                .count();
            let falsy_match = falsy
                .iter()
                .filter(|candidate| candidate.starts_with(lowered.as_str()))
                .count();
            match (truthy_match, falsy_match) {
                (1, 0) => Some(true),
                (0, 1) => Some(false),
                _ => None,
            }
        }
    }
}

fn parse_bool_arg(py: &PyToken<'_>, handle: i64, bits: u64, label: &str) -> Result<bool, u64> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Ok(value != 0);
    }
    if let Some(text) = string_obj_to_owned(obj)
        && let Some(value) = parse_bool_text(&text)
    {
        return Ok(value);
    }
    Err(raise_tcl_for_handle(
        py,
        handle,
        format!("{label} must be a boolean-compatible value"),
    ))
}

fn parse_i64_arg(py: &PyToken<'_>, handle: i64, bits: u64, label: &str) -> Result<i64, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be an integer"),
        ));
    };
    Ok(value)
}

fn alloc_int_tuple2_bits(
    py: &PyToken<'_>,
    first: i64,
    second: i64,
    alloc_context: &str,
) -> Result<u64, u64> {
    let values = vec![
        MoltObject::from_int(first).bits(),
        MoltObject::from_int(second).bits(),
    ];
    alloc_tuple_bits(py, values.as_slice(), alloc_context)
}

fn remove_widget_from_layout_lists(app: &mut TkAppState, widget_path: &str) {
    app.pack_slaves.retain(|name| name != widget_path);
    app.grid_slaves.retain(|name| name != widget_path);
    app.place_slaves.retain(|name| name != widget_path);
}

fn ensure_layout_membership(app: &mut TkAppState, manager: &str, widget_path: &str) {
    remove_widget_from_layout_lists(app, widget_path);
    match manager {
        "pack" => app.pack_slaves.push(widget_path.to_string()),
        "grid" => app.grid_slaves.push(widget_path.to_string()),
        "place" => app.place_slaves.push(widget_path.to_string()),
        _ => {}
    }
}

fn tk_widget_class_name(widget_command: &str) -> String {
    match widget_command {
        "button" => "Button".to_string(),
        "canvas" => "Canvas".to_string(),
        "checkbutton" => "Checkbutton".to_string(),
        "entry" => "Entry".to_string(),
        "frame" => "Frame".to_string(),
        "label" => "Label".to_string(),
        "labelframe" => "Labelframe".to_string(),
        "listbox" => "Listbox".to_string(),
        "menu" => "Menu".to_string(),
        "menubutton" => "Menubutton".to_string(),
        "message" => "Message".to_string(),
        "panedwindow" => "Panedwindow".to_string(),
        "radiobutton" => "Radiobutton".to_string(),
        "scale" => "Scale".to_string(),
        "scrollbar" => "Scrollbar".to_string(),
        "spinbox" => "Spinbox".to_string(),
        "text" => "Text".to_string(),
        "toplevel" => "Toplevel".to_string(),
        "ttk::button" => "TButton".to_string(),
        "ttk::checkbutton" => "TCheckbutton".to_string(),
        "ttk::combobox" => "TCombobox".to_string(),
        "ttk::entry" => "TEntry".to_string(),
        "ttk::frame" => "TFrame".to_string(),
        "ttk::label" => "TLabel".to_string(),
        "ttk::labelframe" => "TLabelframe".to_string(),
        "ttk::menubutton" => "TMenubutton".to_string(),
        "ttk::notebook" => "TNotebook".to_string(),
        "ttk::panedwindow" => "TPanedwindow".to_string(),
        "ttk::progressbar" => "TProgressbar".to_string(),
        "ttk::radiobutton" => "TRadiobutton".to_string(),
        "ttk::scale" => "TScale".to_string(),
        "ttk::scrollbar" => "TScrollbar".to_string(),
        "ttk::separator" => "TSeparator".to_string(),
        "ttk::sizegrip" => "TSizegrip".to_string(),
        "ttk::spinbox" => "TSpinbox".to_string(),
        "ttk::treeview" => "Treeview".to_string(),
        _ => widget_command
            .rsplit("::")
            .next()
            .unwrap_or(widget_command)
            .to_string(),
    }
}

fn value_bits_to_i64_default(bits: u64, default: i64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return value;
    }
    if let Some(text) = string_obj_to_owned(obj)
        && let Ok(value) = text.trim().parse::<i64>()
    {
        return value;
    }
    default
}

fn widget_option_i64_default(options: &HashMap<String, u64>, key: &str, default: i64) -> i64 {
    options
        .get(key)
        .copied()
        .map(|bits| value_bits_to_i64_default(bits, default))
        .unwrap_or(default)
}

fn parse_winfo_rgb_components(color: &str) -> (i64, i64, i64) {
    let trimmed = color.trim();
    if trimmed.len() == 7 && trimmed.starts_with('#') {
        let r = i64::from_str_radix(&trimmed[1..3], 16).unwrap_or(0) * 257;
        let g = i64::from_str_radix(&trimmed[3..5], 16).unwrap_or(0) * 257;
        let b = i64::from_str_radix(&trimmed[5..7], 16).unwrap_or(0) * 257;
        return (r, g, b);
    }
    if trimmed.len() == 4 && trimmed.starts_with('#') {
        let r = i64::from_str_radix(&trimmed[1..2], 16).unwrap_or(0) * 0x1111;
        let g = i64::from_str_radix(&trimmed[2..3], 16).unwrap_or(0) * 0x1111;
        let b = i64::from_str_radix(&trimmed[3..4], 16).unwrap_or(0) * 0x1111;
        return (r, g, b);
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "red" => (65535, 0, 0),
        "green" => (0, 65535, 0),
        "blue" => (0, 0, 65535),
        "white" => (65535, 65535, 65535),
        "black" => (0, 0, 0),
        _ => (0, 0, 0),
    }
}

fn parse_treeview_index_strict(value: &str, len: usize) -> Option<usize> {
    if value.eq_ignore_ascii_case("end") {
        return Some(len);
    }
    value.trim().parse::<i64>().ok().map(|parsed| {
        if parsed <= 0 {
            0
        } else {
            (parsed as usize).min(len)
        }
    })
}

fn parse_ttk_insert_index_strict(value: &str, len: usize) -> Option<usize> {
    parse_treeview_index_strict(value, len)
}

fn parse_notebook_index_strict(value: &str, len: usize) -> Result<i64, String> {
    if value.eq_ignore_ascii_case("end") {
        return Ok(len as i64);
    }
    if let Ok(parsed) = value.parse::<i64>() {
        if parsed < 0 || (parsed as usize) >= len {
            return Err(format!("Slave index {parsed} out of bounds"));
        }
        return Ok(parsed);
    }
    Err(format!("invalid tab identifier \"{value}\""))
}

fn treeview_subcommand_is_noop_generic_fallback(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "add"
            | "addtag"
            | "dtag"
            | "scan"
            | "itemconfigure"
            | "entryconfigure"
            | "paneconfigure"
            | "image_configure"
            | "window_configure"
            | "activate"
            | "tk_popup"
            | "post"
            | "unpost"
            | "invoke"
            | "edit"
            | "replace"
            | "dump"
    )
}

fn first_missing_treeview_item<'a>(
    treeview: &TkTreeviewState,
    items: &'a [String],
) -> Option<&'a str> {
    items
        .iter()
        .find(|item| !treeview.items.contains_key(item.as_str()))
        .map(String::as_str)
}

fn treeview_visible_items(treeview: &TkTreeviewState) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = treeview.root_children.iter().rev().cloned().collect();
    let mut visited = HashSet::new();
    while let Some(item_id) = stack.pop() {
        if !visited.insert(item_id.clone()) {
            continue;
        }
        let Some(item) = treeview.items.get(&item_id) else {
            continue;
        };
        out.push(item_id);
        for child in item.children.iter().rev() {
            stack.push(child.clone());
        }
    }
    out
}

fn treeview_hit_item_by_y(treeview: &TkTreeviewState, y: i64) -> Option<String> {
    if y < 0 {
        return None;
    }
    let row = (y as usize) / 20;
    treeview_visible_items(treeview).get(row).cloned()
}

fn parse_treeview_column_offset(spec: &str) -> Option<i64> {
    let normalized = spec.trim();
    let suffix = normalized.strip_prefix('#')?;
    let column = suffix.parse::<i64>().ok()?;
    (column >= 0).then_some(column * 120)
}

const TREEVIEW_COLUMN_OPTIONS: &[&str] = &["-id", "-anchor", "-minwidth", "-stretch", "-width"];
const TREEVIEW_HEADING_OPTIONS: &[&str] = &["-text", "-image", "-anchor", "-command", "-state"];
const TREEVIEW_ITEM_OPTIONS: &[&str] = &["-text", "-image", "-values", "-open", "-tags"];
const TREEVIEW_TAG_OPTIONS: &[&str] = &["-foreground", "-background", "-font", "-image"];
const TTK_NOTEBOOK_TAB_OPTIONS: &[&str] = &[
    "-state",
    "-sticky",
    "-padding",
    "-text",
    "-image",
    "-compound",
    "-underline",
];
const TTK_PANEDWINDOW_PANE_OPTIONS: &[&str] = &["-weight"];

fn option_allowed(option_name: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|candidate| option_name == *candidate)
}

fn clamp_index_i64(value: i64, upper: usize) -> usize {
    if value <= 0 {
        0
    } else {
        (value as usize).min(upper)
    }
}

fn text_char_count(text: &str) -> usize {
    text.chars().count()
}

fn char_index_to_byte_index(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_index)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

fn parse_simple_end_or_int_index(spec: &str, upper: usize) -> Option<usize> {
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") {
        return Some(upper);
    }
    trimmed
        .parse::<i64>()
        .ok()
        .map(|value| clamp_index_i64(value, upper))
}

fn parse_simple_end_or_int_index_bits(bits: u64, upper: usize) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, upper));
    }
    let spec = string_obj_to_owned(obj)?;
    parse_simple_end_or_int_index(&spec, upper)
}

fn parse_listbox_index_bits(bits: u64, len: usize, for_insert: bool) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        let upper = if for_insert {
            len
        } else {
            len.saturating_sub(1)
        };
        return Some(clamp_index_i64(value, upper));
    }
    let spec = string_obj_to_owned(obj)?;
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") {
        return if for_insert {
            Some(len)
        } else if len == 0 {
            Some(0)
        } else {
            Some(len - 1)
        };
    }
    trimmed.parse::<i64>().ok().map(|value| {
        clamp_index_i64(
            value,
            if for_insert {
                len
            } else {
                len.saturating_sub(1)
            },
        )
    })
}

fn parse_menu_existing_index_bits(
    bits: u64,
    len: usize,
    active_index: Option<usize>,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, len - 1));
    }
    let spec = string_obj_to_owned(obj)?;
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") || trimmed.eq_ignore_ascii_case("last") {
        return Some(len - 1);
    }
    if trimmed.eq_ignore_ascii_case("active") {
        return active_index.filter(|idx| *idx < len);
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    trimmed
        .parse::<i64>()
        .ok()
        .map(|value| clamp_index_i64(value, len - 1))
}

fn parse_menu_insert_index_bits(bits: u64, len: usize) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, len));
    }
    let spec = string_obj_to_owned(obj)?;
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") {
        return Some(len);
    }
    trimmed
        .parse::<i64>()
        .ok()
        .map(|value| clamp_index_i64(value, len))
}

fn menu_item_type_supported(item_type: &str) -> bool {
    matches!(
        item_type,
        "cascade" | "checkbutton" | "command" | "radiobutton" | "separator"
    )
}

fn parse_command_words(command: &str) -> Vec<String> {
    let parsed = parse_tcl_script_commands(command);
    if let Some(first) = parsed.into_iter().find(|words| !words.is_empty()) {
        return first;
    }
    vec![command.to_string()]
}

fn run_tk_word_commands(
    py: &PyToken<'_>,
    handle: i64,
    commands: &[Vec<String>],
) -> Result<(), u64> {
    for words in commands {
        let out_bits = call_tk_command_from_strings(py, handle, words)?;
        release_result_bits(py, out_bits);
    }
    Ok(())
}

fn parse_entry_like_index_bits(
    bits: u64,
    text_len: usize,
    insert_cursor: usize,
    selection_anchor: Option<usize>,
) -> Option<usize> {
    if let Some(index) = parse_simple_end_or_int_index_bits(bits, text_len) {
        return Some(index.min(text_len));
    }
    let spec = string_obj_to_owned(obj_from_bits(bits))?;
    let normalized = spec.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "insert" => Some(insert_cursor.min(text_len)),
        "anchor" => Some(selection_anchor.unwrap_or(0).min(text_len)),
        _ => None,
    }
}

fn parse_text_end_delta(spec: &str) -> Option<i64> {
    let compact: String = spec.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return Some(0);
    }
    let (sign, tail) = if let Some(rest) = compact.strip_prefix('+') {
        (1, rest)
    } else if let Some(rest) = compact.strip_prefix('-') {
        (-1, rest)
    } else {
        return None;
    };
    let tail = tail
        .strip_suffix('c')
        .or_else(|| tail.strip_suffix('C'))
        .unwrap_or(tail);
    if tail.is_empty() {
        return None;
    }
    let delta = tail.parse::<i64>().ok()?;
    Some(sign * delta)
}

fn parse_text_line_column_index(spec: &str, text: &str) -> Option<usize> {
    let (line_part, column_part) = spec.split_once('.')?;
    let line = line_part.trim().parse::<i64>().ok()?;
    let column = column_part.trim().parse::<i64>().ok()?;
    let line_number = line.max(1) as usize;
    let column = column.max(0) as usize;

    let mut line_starts = vec![0usize];
    for (char_idx, ch) in text.chars().enumerate() {
        if ch == '\n' {
            line_starts.push(char_idx + 1);
        }
    }

    let total_chars = text_char_count(text);
    let Some(&line_start) = line_starts.get(line_number.saturating_sub(1)) else {
        return Some(total_chars);
    };
    let line_end = line_starts
        .get(line_number)
        .copied()
        .map(|next_start| next_start.saturating_sub(1))
        .unwrap_or(total_chars);
    let line_len = line_end.saturating_sub(line_start);
    Some((line_start + column).min(line_start + line_len))
}

fn parse_text_index_spec(spec: &str, text: &str) -> Option<usize> {
    let trimmed = spec.trim();
    let total_chars = text_char_count(text);
    if trimmed.eq_ignore_ascii_case("end") {
        return Some(total_chars);
    }
    if let Some(rest) = trimmed.strip_prefix("end")
        && let Some(delta) = parse_text_end_delta(rest)
    {
        let index = (total_chars as i64).saturating_add(delta);
        return Some(clamp_index_i64(index, total_chars));
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(clamp_index_i64(value, total_chars));
    }
    parse_text_line_column_index(trimmed, text)
}

fn parse_text_index_bits(bits: u64, text: &str) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, text_char_count(text)));
    }
    let spec = string_obj_to_owned(obj)?;
    parse_text_index_spec(&spec, text)
}

fn parse_treeview_item_list_arg(
    py: &PyToken<'_>,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<Vec<String>, u64> {
    if let Some(raw_items) = decode_value_list(obj_from_bits(bits)) {
        let mut out = Vec::with_capacity(raw_items.len());
        for item_bits in raw_items {
            out.push(get_string_arg(py, handle, item_bits, label)?);
        }
        return Ok(out);
    }
    Ok(vec![get_string_arg(py, handle, bits, label)?])
}

fn parse_treeview_tags(item: &TkTreeviewItem) -> Vec<String> {
    let Some(tags_bits) = item.options.get("-tags").copied() else {
        return Vec::new();
    };
    if let Some(raw) = decode_value_list(obj_from_bits(tags_bits)) {
        let mut out = Vec::with_capacity(raw.len());
        for tag_bits in raw {
            if let Some(tag) = string_obj_to_owned(obj_from_bits(tag_bits)) {
                out.push(tag);
            }
        }
        return out;
    }
    let value = obj_from_bits(tags_bits);
    if let Some(tag) = string_obj_to_owned(value) {
        if tag.trim().is_empty() {
            return Vec::new();
        }
        return tag.split_whitespace().map(str::to_string).collect();
    }
    Vec::new()
}

fn treeview_item_is_descendant_of(
    treeview: &TkTreeviewState,
    item_id: &str,
    ancestor_id: &str,
) -> bool {
    if item_id == ancestor_id {
        return true;
    }
    let mut cursor = treeview.items.get(item_id).map(|item| item.parent.clone());
    while let Some(parent) = cursor {
        if parent.is_empty() {
            return false;
        }
        if parent == ancestor_id {
            return true;
        }
        cursor = treeview.items.get(&parent).map(|item| item.parent.clone());
    }
    false
}

fn treeview_remove_from_parent(treeview: &mut TkTreeviewState, item_id: &str) {
    if let Some(parent_name) = treeview.items.get(item_id).map(|item| item.parent.clone()) {
        if parent_name.is_empty() {
            treeview.root_children.retain(|child| child != item_id);
            return;
        }
        if let Some(parent) = treeview.items.get_mut(&parent_name) {
            parent.children.retain(|child| child != item_id);
        }
    } else {
        treeview.root_children.retain(|child| child != item_id);
    }
}

fn treeview_insert_into_parent(
    treeview: &mut TkTreeviewState,
    parent_id: &str,
    index: usize,
    item_id: String,
) {
    if parent_id.is_empty() {
        let idx = index.min(treeview.root_children.len());
        treeview.root_children.insert(idx, item_id);
        return;
    }
    if let Some(parent) = treeview.items.get_mut(parent_id) {
        let idx = index.min(parent.children.len());
        parent.children.insert(idx, item_id);
    }
}

fn treeview_remove_item(py: &PyToken<'_>, treeview: &mut TkTreeviewState, item_id: &str) {
    let Some(mut item) = treeview.items.remove(item_id) else {
        return;
    };
    let children = std::mem::take(&mut item.children);
    for child in children {
        treeview_remove_item(py, treeview, &child);
    }
    clear_value_map_refs(py, &mut item.options);
    clear_value_map_refs(py, &mut item.values);
    treeview.selection.retain(|selected| selected != item_id);
    if treeview.focus.as_deref() == Some(item_id) {
        treeview.focus = None;
    }
}

fn treeview_set_pairs_to_tuple(py: &PyToken<'_>, item: &TkTreeviewItem) -> Result<u64, u64> {
    let mut keys: Vec<String> = item.values.keys().cloned().collect();
    keys.sort_unstable();
    let mut tuple_elems = Vec::with_capacity(keys.len() * 2);
    for column in keys {
        let Some(value_bits) = item.values.get(&column).copied() else {
            continue;
        };
        let column_bits = alloc_string_bits(py, &column)?;
        tuple_elems.push(column_bits);
        tuple_elems.push(value_bits);
    }
    let out = alloc_tuple_bits(
        py,
        tuple_elems.as_slice(),
        "failed to allocate treeview set tuple",
    );
    for bits in tuple_elems {
        dec_ref_bits(py, bits);
    }
    out
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn option_use_tk(py: &PyToken<'_>, options_bits: u64) -> bool {
    let obj = obj_from_bits(options_bits);
    let Some(dict_ptr) = obj.as_ptr() else {
        return true;
    };
    if unsafe { crate::object_type_id(dict_ptr) } != crate::TYPE_ID_DICT {
        return true;
    }
    let entries = unsafe { crate::dict_order(dict_ptr) }.clone();
    for pair in entries.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let Some(key) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            continue;
        };
        if key == "useTk" {
            return crate::is_truthy(py, obj_from_bits(pair[1]));
        }
    }
    true
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_obj_from_bits(py: &PyToken<'_>, bits: u64) -> TclObj {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return TclObj::from("");
    }
    if let Some(i) = to_i64(obj) {
        return TclObj::from(i);
    }
    if let Some(f) = to_f64(obj) {
        return TclObj::from(f);
    }
    if let Some(s) = string_obj_to_owned(obj) {
        return TclObj::from(s);
    }
    // Handle tuples/lists as Tcl lists (e.g., font tuples like ("SF Pro Display", 18, "bold"))
    if let Some(elements) = decode_value_list(obj) {
        let tcl_elements: Vec<TclObj> = elements
            .iter()
            .map(|&elem_bits| tcl_obj_from_bits(py, elem_bits))
            .collect();
        return TclObj::new_list(tcl_elements.into_iter());
    }
    // Use str() instead of repr() for widget objects and other types.
    // Widget.__str__ returns the Tcl widget path (e.g., ".!frame24"),
    // while repr() returns "<tkinter.Frame object .!frame24>" which Tcl rejects.
    // Clear pending exceptions first — they cause molt_str_from_obj to bail.
    if exception_pending(py) {
        crate::builtins::exceptions::clear_exception(py);
    }
    let str_bits = crate::molt_str_from_obj(bits);
    if !obj_from_bits(str_bits).is_none() {
        if let Some(s) = string_obj_to_owned(obj_from_bits(str_bits)) {
            dec_ref_bits(py, str_bits);
            return TclObj::from(s);
        }
    }
    dec_ref_bits(py, str_bits);
    TclObj::from(format_obj_str(py, obj))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_result_to_bits(py: &PyToken<'_>, value: TclObj) -> u64 {
    let text = value.to_string();
    match alloc_string_bits(py, &text) {
        Ok(bits) => bits,
        Err(bits) => bits,
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn register_tcl_callback_proc(app: &mut TkAppState, name: &str) -> Result<(), String> {
    let Some(interp) = app.interpreter.as_ref() else {
        return Ok(());
    };
    interp
        .eval((
            "proc",
            name.to_string(),
            "args",
            "lappend ::__molt_pending_callbacks [info level 0]; return {}",
        ))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn register_tcl_callback_proc(_app: &mut TkAppState, _name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn register_tcl_callback_proc(_app: &mut TkAppState, _name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn unregister_tcl_callback_proc(app: &mut TkAppState, name: &str) {
    let Some(interp) = app.interpreter.as_ref() else {
        return;
    };
    let _ = interp.eval(("rename", name.to_string(), ""));
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn unregister_tcl_callback_proc(_app: &mut TkAppState, _name: &str) {}

#[cfg(target_arch = "wasm32")]
fn unregister_tcl_callback_proc(_app: &mut TkAppState, _name: &str) {}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn unregister_all_tcl_callback_procs(app: &mut TkAppState) {
    let mut callback_names: Vec<String> = app.callbacks.keys().cloned().collect();
    callback_names.extend(app.filehandler_commands.keys().cloned());
    for callback_name in callback_names {
        unregister_tcl_callback_proc(app, &callback_name);
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn init_tcl_pending_callbacks(interp: &TclInterpreter) -> Result<(), String> {
    interp
        .eval((
            "set",
            "::__molt_pending_callbacks",
            TclObj::new_list(std::iter::empty::<TclObj>()),
        ))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn build_native_tk_app(py: &PyToken<'_>, use_tk: bool) -> Result<TkAppState, u64> {
    let mut app = TkAppState::default();
    let interp = match std::panic::catch_unwind(TclInterpreter::new) {
        Ok(Ok(interp)) => interp,
        Ok(Err(err)) => {
            return Err(raise_tcl_error(
                py,
                &format!("failed to create Tcl interpreter: {err}"),
            ));
        }
        Err(_) => {
            return Err(raise_tcl_error(
                py,
                "failed to create Tcl interpreter: panic in tcl initialization",
            ));
        }
    };
    init_tcl_pending_callbacks(&interp).map_err(|err| {
        raise_tcl_error(
            py,
            &format!("failed to initialize tkinter callback queue: {err}"),
        )
    })?;
    if use_tk {
        interp
            .eval(("package", "require", "Tk"))
            .map_err(|err| raise_tcl_error(py, &format!("failed to load Tk package: {err}")))?;
        app.tk_loaded = true;
    }
    app.interpreter = Some(interp);
    Ok(app)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn run_tcl_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    let mut command = Vec::with_capacity(args.len());
    for &bits in args {
        command.push(tcl_obj_from_bits(py, bits));
    }
    let script = TclObj::new_list(command.into_iter());

    // Trace Tcl commands for debugging layout issues
    if std::env::var("MOLT_TRACE_TCL").is_ok() {
        eprintln!("[tcl] {}", script.to_string());
    }

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(interp) = app.interpreter.as_ref() else {
        return Err(app_tcl_error_locked(
            py,
            app,
            "tk runtime interpreter is unavailable",
        ));
    };
    match interp.eval(script) {
        Ok(result) => {
            app.last_error = None;
            Ok(tcl_result_to_bits(py, result))
        }
        Err(err) => Err(app_tcl_error_locked(
            py,
            app,
            format!("tk command failed: {err}"),
        )),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn take_pending_tcl_callbacks(py: &PyToken<'_>, handle: i64) -> Result<Vec<Vec<String>>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(interp) = app.interpreter.as_ref() else {
        return Ok(Vec::new());
    };

    let pending_obj = match interp.get("::__molt_pending_callbacks") {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    let _ = interp.eval((
        "set",
        "::__molt_pending_callbacks",
        TclObj::new_list(std::iter::empty::<TclObj>()),
    ));

    let mut calls = Vec::new();
    let Ok(pending_iter) = pending_obj.get_elements() else {
        return Ok(calls);
    };
    for pending_call in pending_iter {
        if let Ok(parts) = pending_call.get_elements() {
            calls.push(parts.map(|obj| obj.to_string()).collect());
        } else {
            calls.push(vec![pending_call.to_string()]);
        }
    }
    Ok(calls)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn dispatch_named_callback_from_strings(
    py: &PyToken<'_>,
    handle: i64,
    argv: Vec<String>,
) -> Result<bool, u64> {
    if argv.is_empty() {
        return Ok(false);
    }
    let command_name = argv[0].clone();
    if let Some(out_bits) = invoke_filehandler_command(py, handle, &command_name)? {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
        return Ok(true);
    }

    let (callback_bits, oneshot) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(bits) = app.callbacks.get(&command_name).copied() else {
            return Ok(false);
        };
        inc_ref_bits(py, bits);
        let oneshot = app.one_shot_callbacks.remove(&command_name);
        if oneshot {
            if let Some(old_bits) = app.callbacks.remove(&command_name) {
                debug_assert_eq!(old_bits, bits);
            }
            let oneshot_tokens = tokens_for_after_command(app, &command_name);
            for token in &oneshot_tokens {
                unregister_after_command_token(app, token);
            }
            remove_after_events_for_tokens(app, &oneshot_tokens);
            unregister_tcl_callback_proc(app, &command_name);
        }
        (bits, oneshot)
    };

    let mut arg_bits = Vec::new();
    for arg in argv.iter().skip(1) {
        match alloc_string_bits(py, arg) {
            Ok(bits) => arg_bits.push(bits),
            Err(bits) => {
                dec_ref_bits(py, callback_bits);
                for allocated in arg_bits {
                    dec_ref_bits(py, allocated);
                }
                return Err(bits);
            }
        }
    }

    let out_bits = invoke_callback(py, callback_bits, &arg_bits);
    dec_ref_bits(py, callback_bits);
    for allocated in arg_bits {
        dec_ref_bits(py, allocated);
    }
    if exception_pending(py) {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
        set_last_error(handle, "bound tkinter command raised an exception");
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(py, out_bits);
    }
    if oneshot {
        clear_last_error(handle);
        return Ok(true);
    }
    clear_last_error(handle);
    Ok(true)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn pump_tcl_events(py: &PyToken<'_>, handle: i64, flags: i32) -> Result<bool, u64> {
    let event_handled = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(interp) = app.interpreter.as_ref() else {
            return Ok(false);
        };
        match interp.do_one_event(flags) {
            Ok(handled) => handled,
            Err(err) => return Err(app_tcl_error_locked(py, app, err)),
        }
    };
    let pending_callbacks = take_pending_tcl_callbacks(py, handle)?;
    let mut callback_handled = false;
    for callback_argv in pending_callbacks {
        if dispatch_named_callback_from_strings(py, handle, callback_argv)? {
            callback_handled = true;
        }
    }
    Ok(event_handled || callback_handled)
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
fn pump_tcl_events(_py: &PyToken<'_>, _handle: i64, _flags: i32) -> Result<bool, u64> {
    Ok(false)
}

#[cfg(target_arch = "wasm32")]
fn pump_tcl_events(_py: &PyToken<'_>, _handle: i64, _flags: i32) -> Result<bool, u64> {
    Ok(false)
}

fn clear_last_error(handle: i64) {
    let mut registry = tk_registry().lock().unwrap();
    if let Some(app) = registry.apps.get_mut(&handle) {
        app.last_error = None;
    }
}

fn set_last_error(handle: i64, message: impl Into<String>) {
    let mut registry = tk_registry().lock().unwrap();
    if let Some(app) = registry.apps.get_mut(&handle) {
        app.last_error = Some(message.into());
    }
}

fn raise_tcl_for_handle(py: &PyToken<'_>, handle: i64, message: impl Into<String>) -> u64 {
    let message = message.into();
    set_last_error(handle, message.clone());
    raise_tcl_error(py, &message)
}

fn get_string_arg(py: &PyToken<'_>, handle: i64, bits: u64, label: &str) -> Result<String, u64> {
    string_obj_to_owned(obj_from_bits(bits)).ok_or_else(|| {
        raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be str in tkinter command"),
        )
    })
}

fn get_string_arg_allow_none(
    py: &PyToken<'_>,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(String::new());
    }
    get_string_arg(py, handle, bits, label)
}

fn get_text_arg(py: &PyToken<'_>, handle: i64, bits: u64, label: &str) -> Result<String, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(String::new());
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(text);
    }
    if let Some(value) = to_i64(obj) {
        return Ok(value.to_string());
    }
    if let Some(value) = to_f64(obj) {
        return Ok(value.to_string());
    }
    Err(raise_tcl_for_handle(
        py,
        handle,
        format!("{label} must be str/int/float in tkinter command"),
    ))
}

fn parse_optional_i64_arg(
    py: &PyToken<'_>,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<Option<i64>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(value) = to_i64(obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be an integer"),
        ));
    };
    Ok(Some(value))
}

fn parse_optional_f64_arg(
    py: &PyToken<'_>,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<Option<f64>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(value) = to_f64(obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be a real number"),
        ));
    };
    Ok(Some(value))
}

fn normalize_commondialog_option_name(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("-{name}")
    }
}

fn parse_commondialog_options(
    py: &PyToken<'_>,
    handle: i64,
    options_bits: u64,
) -> Result<Vec<(String, u64)>, u64> {
    let options_obj = obj_from_bits(options_bits);
    if options_obj.is_none() {
        return Ok(Vec::new());
    }

    if let Some(dict_ptr) = options_obj.as_ptr()
        && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
    {
        let entries = unsafe { crate::dict_order(dict_ptr) }.clone();
        let mut options = Vec::with_capacity(entries.len() / 2);
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let name = get_string_arg(py, handle, pair[0], "commondialog option name")?;
            let value_bits = pair[1];
            if obj_from_bits(value_bits).is_none() {
                continue;
            }
            options.push((normalize_commondialog_option_name(&name), value_bits));
        }
        return Ok(options);
    }

    let Some(raw_items) = decode_value_list(options_obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "commondialog options must be a dict or list/tuple",
        ));
    };
    if !raw_items.len().is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "commondialog option list must contain key/value pairs",
        ));
    }

    let mut options = Vec::with_capacity(raw_items.len() / 2);
    for idx in (0..raw_items.len()).step_by(2) {
        let name = get_string_arg(py, handle, raw_items[idx], "commondialog option name")?;
        let value_bits = raw_items[idx + 1];
        if obj_from_bits(value_bits).is_none() {
            continue;
        }
        options.push((normalize_commondialog_option_name(&name), value_bits));
    }
    Ok(options)
}

fn commondialog_option_value_bits(options: &[(String, u64)], key: &str) -> Option<u64> {
    options
        .iter()
        .rev()
        .find_map(|(name, bits)| name.eq_ignore_ascii_case(key).then_some(*bits))
}

fn commondialog_is_supported_command(command: &str) -> bool {
    matches!(
        command,
        "tk_messageBox"
            | "tk_getOpenFile"
            | "tk_getSaveFile"
            | "tk_chooseDirectory"
            | "tk_chooseColor"
    )
}

fn commondialog_supports_parent(command: &str) -> bool {
    commondialog_is_supported_command(command)
}

fn commondialog_allowed_options(command: &str) -> &'static [&'static str] {
    match command {
        "tk_messageBox" => &[
            "-command", "-default", "-detail", "-icon", "-message", "-parent", "-title", "-type",
        ],
        "tk_getOpenFile" => &[
            "-defaultextension",
            "-filetypes",
            "-initialdir",
            "-initialfile",
            "-multiple",
            "-parent",
            "-title",
            "-typevariable",
        ],
        "tk_getSaveFile" => &[
            "-confirmoverwrite",
            "-defaultextension",
            "-filetypes",
            "-initialdir",
            "-initialfile",
            "-parent",
            "-title",
            "-typevariable",
        ],
        "tk_chooseDirectory" => &["-initialdir", "-mustexist", "-parent", "-title"],
        "tk_chooseColor" => &["-initialcolor", "-parent", "-title"],
        _ => &[],
    }
}

fn validate_commondialog_options(
    py: &PyToken<'_>,
    handle: i64,
    command: &str,
    options: &[(String, u64)],
) -> Result<(), u64> {
    let allowed = commondialog_allowed_options(command);
    for (option_name, _) in options {
        let is_allowed = allowed
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(option_name));
        if !is_allowed {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                format!("unknown option \"{option_name}\" for {command}"),
            ));
        }
    }
    Ok(())
}

fn raise_unsupported_commondialog_command(py: &PyToken<'_>, handle: i64, command: &str) -> u64 {
    raise_tcl_for_handle(
        py,
        handle,
        format!("unsupported commondialog command \"{command}\""),
    )
}

fn commondialog_option_text(
    py: &PyToken<'_>,
    handle: i64,
    options: &[(String, u64)],
    key: &str,
    label: &str,
) -> Result<Option<String>, u64> {
    let Some(value_bits) = commondialog_option_value_bits(options, key) else {
        return Ok(None);
    };
    Ok(Some(get_text_arg(py, handle, value_bits, label)?))
}

fn commondialog_option_bool(
    py: &PyToken<'_>,
    handle: i64,
    options: &[(String, u64)],
    key: &str,
    label: &str,
) -> Result<Option<bool>, u64> {
    let Some(value_bits) = commondialog_option_value_bits(options, key) else {
        return Ok(None);
    };
    Ok(Some(parse_bool_arg(py, handle, value_bits, label)?))
}

fn messagebox_buttons_for_type(dialog_type: &str) -> Option<&'static [&'static str]> {
    match dialog_type {
        "ok" => Some(&["ok"]),
        "okcancel" => Some(&["ok", "cancel"]),
        "yesno" => Some(&["yes", "no"]),
        "yesnocancel" => Some(&["yes", "no", "cancel"]),
        "retrycancel" => Some(&["retry", "cancel"]),
        "abortretryignore" => Some(&["abort", "retry", "ignore"]),
        _ => None,
    }
}

fn normalize_dialog_choice_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn resolve_messagebox_selection(
    dialog_type_raw: &str,
    default_raw: Option<&str>,
) -> Result<String, String> {
    let dialog_type = normalize_dialog_choice_name(dialog_type_raw);
    let Some(buttons) = messagebox_buttons_for_type(&dialog_type) else {
        return Err(format!(
            "bad -type value \"{dialog_type_raw}\": must be abortretryignore, ok, okcancel, retrycancel, yesno, or yesnocancel"
        ));
    };
    if let Some(default_name_raw) = default_raw {
        let default_name = normalize_dialog_choice_name(default_name_raw);
        if buttons.iter().any(|candidate| *candidate == default_name) {
            return Ok(default_name);
        }
        return Err(format!(
            "bad -default value \"{default_name_raw}\" for dialog type \"{dialog_type}\""
        ));
    }
    Ok(buttons[0].to_string())
}

fn messagebox_icon_is_supported(icon: &str) -> bool {
    matches!(
        normalize_dialog_choice_name(icon).as_str(),
        "error" | "info" | "question" | "warning"
    )
}

fn join_dialog_path(initial_dir: &str, initial_file: &str) -> String {
    if initial_file.is_empty() {
        return initial_dir.to_string();
    }
    if initial_dir.is_empty() {
        return initial_file.to_string();
    }
    if initial_dir.ends_with('/') || initial_dir.ends_with('\\') {
        return format!("{initial_dir}{initial_file}");
    }
    if initial_dir.ends_with(':') {
        return format!("{initial_dir}\\{initial_file}");
    }
    let sep = if initial_dir.contains('\\') && !initial_dir.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{initial_dir}{sep}{initial_file}")
}

fn apply_default_extension(path: &str, default_extension: &str) -> String {
    let trimmed_ext = default_extension.trim();
    if path.is_empty() || trimmed_ext.is_empty() {
        return path.to_string();
    }
    if std::path::Path::new(path).extension().is_some() {
        return path.to_string();
    }
    if trimmed_ext.starts_with('.') {
        format!("{path}{trimmed_ext}")
    } else {
        format!("{path}.{trimmed_ext}")
    }
}

fn normalize_color_literal(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && trimmed.len() == 4 {
        let mut chars = trimmed.chars();
        let _ = chars.next();
        let red = chars.next()?;
        let green = chars.next()?;
        let blue = chars.next()?;
        if !(red.is_ascii_hexdigit() && green.is_ascii_hexdigit() && blue.is_ascii_hexdigit()) {
            return None;
        }
        return Some(format!("#{}{}{}{}{}{}", red, red, green, green, blue, blue));
    }
    if trimmed.starts_with('#') && trimmed.len() == 7 {
        if !trimmed[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        return Some(trimmed.to_string());
    }
    Some(trimmed.to_string())
}

fn parse_commondialog_command_options(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
) -> Result<Vec<(String, u64)>, u64> {
    parse_widget_option_pairs(py, handle, args, 1, "commondialog options")
}

fn headless_commondialog_result(
    py: &PyToken<'_>,
    handle: i64,
    command: &str,
    options: &[(String, u64)],
) -> Result<u64, u64> {
    match command {
        "tk_messageBox" => {
            let dialog_type =
                commondialog_option_text(py, handle, options, "-type", "messagebox type option")?
                    .unwrap_or_else(|| "ok".to_string());
            let default_choice = commondialog_option_text(
                py,
                handle,
                options,
                "-default",
                "messagebox default option",
            )?;
            if let Some(icon_name) =
                commondialog_option_text(py, handle, options, "-icon", "messagebox icon option")?
                && !messagebox_icon_is_supported(icon_name.as_str())
            {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!(
                        "bad -icon value \"{icon_name}\": must be error, info, question, or warning"
                    ),
                ));
            }
            let selection = resolve_messagebox_selection(&dialog_type, default_choice.as_deref())
                .map_err(|message| raise_tcl_for_handle(py, handle, message))?;
            clear_last_error(handle);
            alloc_string_bits(py, &selection)
        }
        "tk_getOpenFile" => {
            let initial_dir = commondialog_option_text(
                py,
                handle,
                options,
                "-initialdir",
                "filedialog initialdir option",
            )?
            .unwrap_or_default();
            let initial_file = commondialog_option_text(
                py,
                handle,
                options,
                "-initialfile",
                "filedialog initialfile option",
            )?
            .unwrap_or_default();
            let default_extension = commondialog_option_text(
                py,
                handle,
                options,
                "-defaultextension",
                "filedialog defaultextension option",
            )?
            .unwrap_or_default();
            let selected = apply_default_extension(
                join_dialog_path(initial_dir.as_str(), initial_file.as_str()).as_str(),
                default_extension.as_str(),
            );
            let multiple = commondialog_option_bool(
                py,
                handle,
                options,
                "-multiple",
                "filedialog multiple option",
            )?
            .unwrap_or(false);
            clear_last_error(handle);
            if multiple {
                let values = if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![selected]
                };
                alloc_tuple_from_strings(
                    py,
                    values.as_slice(),
                    "failed to allocate open-file selection tuple",
                )
            } else {
                alloc_string_bits(py, &selected)
            }
        }
        "tk_getSaveFile" => {
            let initial_dir = commondialog_option_text(
                py,
                handle,
                options,
                "-initialdir",
                "filedialog initialdir option",
            )?
            .unwrap_or_default();
            let initial_file = commondialog_option_text(
                py,
                handle,
                options,
                "-initialfile",
                "filedialog initialfile option",
            )?
            .unwrap_or_default();
            let default_extension = commondialog_option_text(
                py,
                handle,
                options,
                "-defaultextension",
                "filedialog defaultextension option",
            )?
            .unwrap_or_default();
            let selected = apply_default_extension(
                join_dialog_path(initial_dir.as_str(), initial_file.as_str()).as_str(),
                default_extension.as_str(),
            );
            clear_last_error(handle);
            alloc_string_bits(py, &selected)
        }
        "tk_chooseDirectory" => {
            let initial_dir = commondialog_option_text(
                py,
                handle,
                options,
                "-initialdir",
                "directory dialog initialdir option",
            )?
            .unwrap_or_default();
            let must_exist = commondialog_option_bool(
                py,
                handle,
                options,
                "-mustexist",
                "directory dialog mustexist option",
            )?
            .unwrap_or(false);
            let selected = if must_exist
                && !initial_dir.is_empty()
                && !std::path::Path::new(initial_dir.as_str()).is_dir()
            {
                String::new()
            } else {
                initial_dir
            };
            clear_last_error(handle);
            alloc_string_bits(py, &selected)
        }
        "tk_chooseColor" => {
            let initial_color = commondialog_option_text(
                py,
                handle,
                options,
                "-initialcolor",
                "color chooser initialcolor option",
            )?;
            let selected = if let Some(color_name) = initial_color.as_deref() {
                let Some(normalized) = normalize_color_literal(color_name) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("invalid color name \"{color_name}\""),
                    ));
                };
                normalized
            } else {
                String::new()
            };
            clear_last_error(handle);
            alloc_string_bits(py, &selected)
        }
        _ => Err(raise_unsupported_commondialog_command(py, handle, command)),
    }
}

fn handle_headless_commondialog_command(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    let command = get_string_arg(py, handle, args[0], "commondialog command")?;
    if !commondialog_is_supported_command(command.as_str()) {
        return Err(raise_unsupported_commondialog_command(
            py,
            handle,
            command.as_str(),
        ));
    }
    let options = parse_commondialog_command_options(py, handle, args)?;
    validate_commondialog_options(py, handle, command.as_str(), &options)?;
    headless_commondialog_result(py, handle, command.as_str(), &options)
}

fn clamp_dialog_selection(default_index: i64, button_count: usize) -> i64 {
    if button_count == 0 {
        return 0;
    }
    let max_index = (button_count - 1) as i64;
    default_index.clamp(0, max_index)
}

fn handle_headless_tk_dialog_command(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 6 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tk_dialog expects window, title, text, bitmap, default, and optional button labels",
        ));
    }
    let default_index = parse_i64_arg(py, handle, args[5], "tk_dialog default index")?;
    let selected = clamp_dialog_selection(default_index, args.len().saturating_sub(6));
    clear_last_error(handle);
    Ok(MoltObject::from_int(selected).bits())
}

fn filedialog_is_supported_command(command: &str) -> bool {
    matches!(
        command,
        "tk_getOpenFile" | "tk_getSaveFile" | "tk_chooseDirectory"
    )
}

fn raise_unsupported_filedialog_command(py: &PyToken<'_>, handle: i64, command: &str) -> u64 {
    raise_tcl_for_handle(
        py,
        handle,
        format!("unsupported filedialog command \"{command}\""),
    )
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn ensure_native_tk_loaded_for_commondialog(py: &PyToken<'_>, handle: i64) -> Result<(), u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if app.tk_loaded {
        return Ok(());
    }
    let Some(interp) = app.interpreter.as_ref() else {
        return Err(app_tcl_error_locked(
            py,
            app,
            "tk runtime interpreter is unavailable",
        ));
    };
    match interp.eval(("package", "require", "Tk")) {
        Ok(_) => {
            app.tk_loaded = true;
            Ok(())
        }
        Err(err) => Err(app_tcl_error_locked(
            py,
            app,
            format!("failed to load Tk package: {err}"),
        )),
    }
}

#[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
fn ensure_native_tk_loaded_for_commondialog(_py: &PyToken<'_>, _handle: i64) -> Result<(), u64> {
    Ok(())
}

fn dispatch_commondialog_via_tk_call(
    py: &PyToken<'_>,
    handle: i64,
    master_path: &str,
    command: &str,
    options: &[(String, u64)],
) -> Result<u64, u64> {
    validate_commondialog_options(py, handle, command, options)?;
    ensure_native_tk_loaded_for_commondialog(py, handle)?;

    let inject_parent = !master_path.is_empty()
        && commondialog_supports_parent(command)
        && commondialog_option_value_bits(options, "-parent").is_none();
    let mut argv = Vec::with_capacity(1 + options.len() * 2 + usize::from(inject_parent) * 2);
    let mut allocated = Vec::with_capacity(1 + options.len() + usize::from(inject_parent) * 2);

    let alloc_and_push =
        |value: &str, allocated: &mut Vec<u64>, argv: &mut Vec<u64>| -> Result<(), u64> {
            let bits = alloc_string_bits(py, value)?;
            allocated.push(bits);
            argv.push(bits);
            Ok(())
        };

    if let Err(bits) = alloc_and_push(command, &mut allocated, &mut argv) {
        for owned_bits in allocated {
            dec_ref_bits(py, owned_bits);
        }
        return Err(bits);
    }

    if inject_parent {
        if let Err(bits) = alloc_and_push("-parent", &mut allocated, &mut argv) {
            for owned_bits in allocated {
                dec_ref_bits(py, owned_bits);
            }
            return Err(bits);
        }
        if let Err(bits) = alloc_and_push(master_path, &mut allocated, &mut argv) {
            for owned_bits in allocated {
                dec_ref_bits(py, owned_bits);
            }
            return Err(bits);
        }
    }

    for (name, value_bits) in options {
        if let Err(bits) = alloc_and_push(name, &mut allocated, &mut argv) {
            for owned_bits in allocated {
                dec_ref_bits(py, owned_bits);
            }
            return Err(bits);
        }
        argv.push(*value_bits);
    }

    let out = tk_call_dispatch(py, handle, &argv);
    for bits in allocated {
        dec_ref_bits(py, bits);
    }
    out
}

fn parse_simpledialog_i64(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok()
}

fn parse_simpledialog_f64(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn app_interp_eval_list(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    words: Vec<String>,
) -> Result<TclObj, u64> {
    let eval_result = {
        let Some(interp) = app.interpreter.as_ref() else {
            return Err(app_tcl_error_locked(
                py,
                app,
                "tk runtime interpreter is unavailable",
            ));
        };
        interp.eval(TclObj::new_list(words.into_iter().map(TclObj::from)))
    };
    eval_result.map_err(|err| app_tcl_error_locked(py, app, format!("tk command failed: {err}")))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn cleanup_native_simpledialog(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    dialog_path: &str,
    state_var: &str,
) {
    let _ = app_interp_eval_list(
        py,
        app,
        vec![
            "grab".to_string(),
            "release".to_string(),
            dialog_path.to_string(),
        ],
    );
    let _ = app_interp_eval_list(
        py,
        app,
        vec!["destroy".to_string(), dialog_path.to_string()],
    );
    let _ = app_interp_eval_list(py, app, vec!["unset".to_string(), state_var.to_string()]);
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tk_dispatch_string_command(py: &PyToken<'_>, handle: i64, args: &[String]) -> Result<u64, u64> {
    let mut arg_bits = Vec::with_capacity(args.len());
    for arg in args {
        match alloc_string_bits(py, arg) {
            Ok(bits) => arg_bits.push(bits),
            Err(bits) => {
                for allocated in arg_bits {
                    dec_ref_bits(py, allocated);
                }
                return Err(bits);
            }
        }
    }
    let out = tk_call_dispatch(py, handle, &arg_bits);
    for allocated in arg_bits {
        dec_ref_bits(py, allocated);
    }
    out
}

fn invoke_callback(py: &PyToken<'_>, callback_bits: u64, args: &[u64]) -> u64 {
    if args.is_empty() {
        return unsafe { call_callable0(py, callback_bits) };
    }
    let builder_bits = crate::molt_callargs_new(args.len() as u64, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    for &arg in args {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
    }
    crate::molt_call_bind(callback_bits, builder_bits)
}

fn run_event_callback(py: &PyToken<'_>, handle: i64, event: TkEvent) -> Result<(), u64> {
    match event {
        TkEvent::Callback { token } => {
            let callback_name = after_callback_name_from_token(&token);
            let callback_bits = {
                let mut registry = tk_registry().lock().unwrap();
                let app = app_mut_from_registry(py, &mut registry, handle)?;
                unregister_after_command_token(app, &token);
                app.one_shot_callbacks.remove(&callback_name);
                let Some(bits) = app.callbacks.remove(&callback_name) else {
                    app.last_error = None;
                    return Ok(());
                };
                unregister_tcl_callback_proc(app, &callback_name);
                app.last_error = None;
                bits
            };
            let out_bits = invoke_callback(py, callback_bits, &[]);
            dec_ref_bits(py, callback_bits);
            if exception_pending(py) {
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(py, out_bits);
                }
                set_last_error(handle, "tkinter callback raised an exception");
                return Err(MoltObject::none().bits());
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(py, out_bits);
            }
            clear_last_error(handle);
            Ok(())
        }
        TkEvent::Script { token, commands } => {
            {
                let mut registry = tk_registry().lock().unwrap();
                let app = app_mut_from_registry(py, &mut registry, handle)?;
                unregister_after_command_token(app, &token);
            }
            if commands.is_empty() {
                clear_last_error(handle);
                return Ok(());
            }
            for words in commands {
                let out_bits = call_tk_command_from_strings(py, handle, &words)?;
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(py, out_bits);
                }
            }
            clear_last_error(handle);
            Ok(())
        }
    }
}

fn lookup_bound_callback(py: &PyToken<'_>, handle: i64, name: &str) -> Result<Option<u64>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if let Some(bits) = app.callbacks.get(name).copied() {
        inc_ref_bits(py, bits);
        Ok(Some(bits))
    } else {
        Ok(None)
    }
}

fn normalize_trace_mode_name(mode_name: &str) -> Result<String, String> {
    let mut has_array = false;
    let mut has_read = false;
    let mut has_write = false;
    let mut has_unset = false;
    let mut saw_token = false;
    for token in mode_name
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
    {
        saw_token = true;
        match token.to_ascii_lowercase().as_str() {
            "array" => has_array = true,
            "read" | "r" => has_read = true,
            "write" | "w" => has_write = true,
            "unset" | "u" => has_unset = true,
            _ => {
                return Err(format!(
                    "bad operation \"{token}\": must be array, read, unset, or write"
                ));
            }
        }
    }
    if !saw_token {
        return Err(format!(
            "bad operation \"{mode_name}\": must be array, read, unset, or write"
        ));
    }
    let mut normalized = Vec::with_capacity(4);
    if has_array {
        normalized.push("array");
    }
    if has_read {
        normalized.push("read");
    }
    if has_write {
        normalized.push("write");
    }
    if has_unset {
        normalized.push("unset");
    }
    Ok(normalized.join(" "))
}

fn trace_mode_matches(mode_name: &str, op: &str) -> bool {
    mode_name
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .any(|part| part == op)
}

fn split_array_variable_reference(variable_name: &str) -> (String, Option<String>) {
    let Some(open_idx) = variable_name.find('(') else {
        return (variable_name.to_string(), None);
    };
    if open_idx == 0 || !variable_name.ends_with(')') {
        return (variable_name.to_string(), None);
    }
    let close_idx = variable_name.len().saturating_sub(1);
    if open_idx + 1 > close_idx {
        return (variable_name.to_string(), None);
    }
    let base = variable_name[..open_idx].to_string();
    let index_text = variable_name[open_idx + 1..close_idx].to_string();
    if index_text.is_empty() {
        return (variable_name.to_string(), None);
    }
    (base, Some(index_text))
}

fn collect_trace_callbacks_for_operation(
    app: &TkAppState,
    variable_name: &str,
    op: &str,
    index: Option<&str>,
) -> Vec<(String, String)> {
    let mut ordered: Vec<&TkTraceRegistration> = Vec::new();
    if let Some(registrations) = app.traces.get(variable_name) {
        ordered.extend(registrations.iter());
    }
    let (base_name, _) = split_array_variable_reference(variable_name);
    if base_name != variable_name
        && let Some(registrations) = app.traces.get(base_name.as_str())
    {
        ordered.extend(registrations.iter());
    }
    ordered.sort_by_key(|registration| registration.order);
    let mut callbacks: Vec<(String, String)> = Vec::new();
    for registration in ordered {
        if trace_mode_matches(&registration.mode_name, op) {
            callbacks.push((registration.callback_name.clone(), op.to_string()));
        } else if index.is_some() && trace_mode_matches(&registration.mode_name, "array") {
            callbacks.push((registration.callback_name.clone(), "array".to_string()));
        }
    }
    callbacks
}

fn bump_variable_version(app: &mut TkAppState, variable_name: &str) {
    app.next_variable_version = app.next_variable_version.saturating_add(1);
    if app.next_variable_version == 0 {
        app.next_variable_version = 1;
    }
    app.variable_versions
        .insert(variable_name.to_string(), app.next_variable_version);
}

fn bump_variable_versions_for_reference(app: &mut TkAppState, variable_name: &str) {
    bump_variable_version(app, variable_name);
    let (base_name, index) = split_array_variable_reference(variable_name);
    if index.is_some() && base_name != variable_name {
        bump_variable_version(app, &base_name);
    }
}

fn variable_version(app: &TkAppState, variable_name: &str) -> u64 {
    app.variable_versions
        .get(variable_name)
        .copied()
        .unwrap_or_default()
}

fn call_tk_command_from_strings(
    py: &PyToken<'_>,
    handle: i64,
    argv: &[String],
) -> Result<u64, u64> {
    let mut arg_bits = Vec::with_capacity(argv.len());
    for word in argv {
        match alloc_string_bits(py, word) {
            Ok(bits) => arg_bits.push(bits),
            Err(bits) => {
                for owned in arg_bits {
                    dec_ref_bits(py, owned);
                }
                return Err(bits);
            }
        }
    }
    let out = tk_call_dispatch(py, handle, &arg_bits);
    for owned in arg_bits {
        dec_ref_bits(py, owned);
    }
    out
}

fn release_result_bits(py: &PyToken<'_>, result_bits: u64) {
    if !obj_from_bits(result_bits).is_none() {
        dec_ref_bits(py, result_bits);
    }
}

fn invoke_trace_callbacks(
    py: &PyToken<'_>,
    handle: i64,
    variable_name: &str,
    index: Option<&str>,
    callbacks: &[(String, String)],
) -> Result<(), u64> {
    let index_text = index.unwrap_or("");
    for (callback_name, op_name) in callbacks {
        let mut argv = trace_callback_command_words(callback_name.as_str());
        argv.push(variable_name.to_string());
        argv.push(index_text.to_string());
        argv.push(op_name.clone());
        let out_bits = call_tk_command_from_strings(py, handle, &argv)?;
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
    }
    clear_last_error(handle);
    Ok(())
}

fn trace_callback_command_words(callback_name: &str) -> Vec<String> {
    let parsed = parse_tcl_script_commands(callback_name);
    if parsed.len() == 1 && !parsed[0].is_empty() {
        return parsed.into_iter().next().unwrap_or_default();
    }
    vec![callback_name.to_string()]
}

fn handle_set_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "set expects 1 or 2 arguments",
        ));
    }
    let var_name = get_string_arg(py, handle, args[1], "set variable name")?;
    let (trace_var_name, trace_index) = split_array_variable_reference(&var_name);
    let (result_bits, trace_callbacks) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        if args.len() == 2 {
            let Some(bits) = app.variables.get(&var_name).copied() else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("can't read \"{var_name}\": no such variable"),
                ));
            };
            inc_ref_bits(py, bits);
            let callbacks = collect_trace_callbacks_for_operation(
                app,
                &var_name,
                "read",
                trace_index.as_deref(),
            );
            app.last_error = None;
            (bits, callbacks)
        } else {
            let value_bits = args[2];
            inc_ref_bits(py, value_bits);
            if let Some(old_bits) = app.variables.insert(var_name.clone(), value_bits) {
                dec_ref_bits(py, old_bits);
            }
            bump_variable_versions_for_reference(app, &var_name);
            let callbacks = collect_trace_callbacks_for_operation(
                app,
                &var_name,
                "write",
                trace_index.as_deref(),
            );
            app.last_error = None;
            inc_ref_bits(py, value_bits);
            (value_bits, callbacks)
        }
    };
    if !trace_callbacks.is_empty()
        && let Err(bits) = invoke_trace_callbacks(
            py,
            handle,
            &trace_var_name,
            trace_index.as_deref(),
            &trace_callbacks,
        )
    {
        if !obj_from_bits(result_bits).is_none() {
            dec_ref_bits(py, result_bits);
        }
        return Err(bits);
    }
    Ok(result_bits)
}

fn handle_unset_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "unset expects exactly 1 argument",
        ));
    }
    let var_name = get_string_arg(py, handle, args[1], "unset variable name")?;
    let (trace_var_name, trace_index) = split_array_variable_reference(&var_name);
    let trace_callbacks = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let had_value = if let Some(old_bits) = app.variables.remove(&var_name) {
            dec_ref_bits(py, old_bits);
            true
        } else {
            false
        };
        if had_value {
            bump_variable_versions_for_reference(app, &var_name);
        }
        let callbacks =
            collect_trace_callbacks_for_operation(app, &var_name, "unset", trace_index.as_deref());
        app.last_error = None;
        callbacks
    };
    if !trace_callbacks.is_empty()
        && let Err(bits) = invoke_trace_callbacks(
            py,
            handle,
            &trace_var_name,
            trace_index.as_deref(),
            &trace_callbacks,
        )
    {
        return Err(bits);
    }
    Ok(MoltObject::none().bits())
}

fn handle_expr_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "expr expects an expression argument",
        ));
    }
    if args.len() == 2 {
        let obj = obj_from_bits(args[1]);
        if let Some(i) = to_i64(obj) {
            clear_last_error(handle);
            return Ok(MoltObject::from_int(i).bits());
        }
        if let Some(f) = to_f64(obj) {
            clear_last_error(handle);
            return Ok(MoltObject::from_float(f).bits());
        }
    }
    let mut parts = Vec::with_capacity(args.len() - 1);
    for &bits in &args[1..] {
        let text = get_string_arg(py, handle, bits, "expr argument")?;
        parts.push(text);
    }
    let expression = parts.join(" ");
    let Some(parsed) = parse_expr_literal(&expression) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("invalid expression \"{expression}\""),
        ));
    };
    clear_last_error(handle);
    Ok(match parsed {
        TkExprLiteral::Int(i) => MoltObject::from_int(i).bits(),
        TkExprLiteral::Float(f) => MoltObject::from_float(f).bits(),
    })
}

fn handle_loadtk_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 1 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "loadtk expects no arguments",
        ));
    }
    clear_last_error(handle);
    Ok(MoltObject::none().bits())
}

fn handle_after_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "after expects at least one argument",
        ));
    }

    if let Some(delay_ms) = to_i64(obj_from_bits(args[1])) {
        if delay_ms < 0 {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                "after delay must be non-negative",
            ));
        }
        if args.len() == 2 {
            let mut remaining = u64::try_from(delay_ms).unwrap_or(u64::MAX);
            while remaining > 0 {
                let _ = pump_tcl_events(py, handle, 0)?;
                let _ = dispatch_next_pending_event(py, handle)?;
                std::thread::sleep(Duration::from_millis(1));
                remaining = remaining.saturating_sub(1);
            }
            clear_last_error(handle);
            return Ok(MoltObject::none().bits());
        }
        let mut command_words = Vec::with_capacity(args.len().saturating_sub(2));
        for &bits in &args[2..] {
            command_words.push(get_text_arg(py, handle, bits, "after script part")?);
        }
        if command_words.is_empty() {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                "after delay command form expects delay and command",
            ));
        }
        let command_name = command_words.join(" ");
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let token = next_after_token(&mut app.next_after_id);
        register_after_command_token(app, &token, &command_name, "timer");
        schedule_after_timer_token(app, &token, delay_ms);
        app.event_queue.push_back(TkEvent::Script {
            token: token.clone(),
            commands: vec![command_words],
        });
        app.last_error = None;
        return alloc_string_bits(py, &token);
    }

    let subcommand = get_string_arg(py, handle, args[1], "after subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "idle" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after idle expects a command name",
                ));
            }
            let mut command_words = Vec::with_capacity(args.len().saturating_sub(2));
            for &bits in &args[2..] {
                command_words.push(get_text_arg(py, handle, bits, "after idle script part")?);
            }
            if command_words.is_empty() {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after idle expects a command name",
                ));
            }
            let command_name = command_words.join(" ");
            let token = next_after_token(&mut app.next_after_id);
            register_after_command_token(app, &token, &command_name, "idle");
            app.event_queue.push_back(TkEvent::Script {
                token: token.clone(),
                commands: vec![command_words],
            });
            app.last_error = None;
            alloc_string_bits(py, &token)
        }
        "cancel" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after cancel expects a token or command name",
                ));
            }
            let key = get_string_arg(py, handle, args[2], "after cancel token")?;
            let mut tokens = HashSet::new();
            if app.after_command_tokens.contains_key(&key) {
                tokens.insert(key.clone());
            } else {
                tokens.extend(tokens_for_after_command(app, &key));
                if tokens.is_empty() && key.starts_with("after#") {
                    tokens.insert(key.clone());
                }
            }
            cleanup_after_tokens(py, app, &tokens);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_after_info_all(py, app);
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after info expects optional token argument",
                ));
            }
            let token = get_string_arg(py, handle, args[2], "after info token")?;
            app.last_error = None;
            alloc_after_info_token(py, app, token.as_str())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad after option \"{subcommand}\": must be cancel, idle, or info"),
        )),
    }
}

fn default_bindtags_for_target(app: &TkAppState, target_name: &str) -> Vec<String> {
    if target_name == "." {
        return vec![".".to_string(), "Tk".to_string(), "all".to_string()];
    }
    if target_name == "all" {
        return vec!["all".to_string()];
    }
    if let Some(widget) = app.widgets.get(target_name) {
        return vec![
            target_name.to_string(),
            tk_widget_class_name(&widget.widget_command),
            ".".to_string(),
            "all".to_string(),
        ];
    }
    vec![target_name.to_string()]
}

fn handle_bind_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 || args.len() > 4 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "bind expects target, optional sequence, optional script",
        ));
    }
    let target_name = get_string_arg(py, handle, args[1], "bind target")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;

    if args.len() == 2 {
        let mut sequences: Vec<String> = app
            .bind_scripts
            .get(&target_name)
            .map(|scripts| scripts.keys().cloned().collect())
            .unwrap_or_default();
        sequences.sort_unstable();
        app.last_error = None;
        return alloc_tuple_from_strings(py, sequences.as_slice(), "failed to allocate bind tuple");
    }

    let sequence = get_string_arg(py, handle, args[2], "bind sequence")?;
    if args.len() == 3 {
        let script = app
            .bind_scripts
            .get(&target_name)
            .and_then(|scripts| scripts.get(&sequence))
            .cloned()
            .unwrap_or_default();
        app.last_error = None;
        return alloc_string_bits(py, &script);
    }

    let script = get_string_arg(py, handle, args[3], "bind script")?;
    let scripts = app.bind_scripts.entry(target_name).or_default();
    if script.is_empty() {
        scripts.remove(&sequence);
    } else if script.starts_with('+') {
        let merged = if let Some(previous) = scripts.get(&sequence) {
            if previous.trim().is_empty() {
                script
            } else {
                format!("{previous}\n{script}")
            }
        } else {
            script
        };
        scripts.insert(sequence, merged);
    } else {
        scripts.insert(sequence, script);
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn handle_bindtags_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "bindtags expects target and optional tag list",
        ));
    }
    let target_name = get_string_arg(py, handle, args[1], "bindtags target")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if args.len() == 2 {
        let tags = app
            .bindtags
            .get(&target_name)
            .cloned()
            .unwrap_or_else(|| default_bindtags_for_target(app, &target_name));
        app.last_error = None;
        return alloc_tuple_from_strings(py, tags.as_slice(), "failed to allocate bindtags tuple");
    }

    let tag_values = if let Some(raw) = decode_value_list(obj_from_bits(args[2])) {
        let mut tags = Vec::with_capacity(raw.len());
        for tag_bits in raw {
            tags.push(get_string_arg(py, handle, tag_bits, "bindtags tag")?);
        }
        tags
    } else {
        vec![get_string_arg(py, handle, args[2], "bindtags tag list")?]
    };
    app.bindtags.insert(target_name, tag_values);
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn parse_event_generate_options(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
    start_index: usize,
) -> Result<HashMap<String, String>, u64> {
    let mut options = HashMap::new();
    if start_index >= args.len() {
        return Ok(options);
    }
    let tail_len = args.len() - start_index;
    if !tail_len.is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "event generate option list must contain key/value pairs",
        ));
    }
    let mut index = start_index;
    while index < args.len() {
        let name = get_string_arg(py, handle, args[index], "event option name")?;
        let value = get_text_arg(py, handle, args[index + 1], "event option value")?;
        options.insert(name.to_ascii_lowercase(), value);
        index += 2;
    }
    Ok(options)
}

fn event_generate_type_name(sequence: &str) -> String {
    if sequence.starts_with("<<") && sequence.ends_with(">>") && sequence.len() >= 4 {
        return "VirtualEvent".to_string();
    }
    if sequence.starts_with('<') && sequence.ends_with('>') && sequence.len() >= 2 {
        let inner = &sequence[1..sequence.len() - 1];
        if !inner.is_empty() {
            return inner.to_string();
        }
    }
    sequence.to_string()
}

fn event_generate_placeholder_value(
    placeholder: &str,
    target_path: &str,
    sequence: &str,
    options: &HashMap<String, String>,
) -> Option<String> {
    let fallback_xy = options
        .get("-x")
        .cloned()
        .or_else(|| options.get("-rootx").cloned())
        .unwrap_or_else(|| "0".to_string());
    let fallback_yy = options
        .get("-y")
        .cloned()
        .or_else(|| options.get("-rooty").cloned())
        .unwrap_or_else(|| "0".to_string());
    let value = match placeholder {
        "%#" => options
            .get("-serial")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%b" => options
            .get("-button")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%f" => options
            .get("-focus")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%h" => options
            .get("-height")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%k" => options
            .get("-keycode")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%s" => options
            .get("-state")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%t" => options
            .get("-time")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%w" => options
            .get("-width")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%x" => options
            .get("-x")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%y" => options
            .get("-y")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%A" => options
            .get("-char")
            .cloned()
            .or_else(|| options.get("-data").cloned())
            .unwrap_or_default(),
        "%E" => options
            .get("-sendevent")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%K" => options.get("-keysym").cloned().unwrap_or_default(),
        "%N" => options
            .get("-keysym_num")
            .cloned()
            .or_else(|| options.get("-keycode").cloned())
            .unwrap_or_else(|| "0".to_string()),
        "%W" => target_path.to_string(),
        "%T" => event_generate_type_name(sequence),
        "%X" => options.get("-rootx").cloned().unwrap_or(fallback_xy),
        "%Y" => options.get("-rooty").cloned().unwrap_or(fallback_yy),
        "%D" => options
            .get("-delta")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        _ => return None,
    };
    Some(value)
}

fn parse_bind_script_commands(script: &str) -> Vec<Vec<String>> {
    let trimmed = script.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let extracted = if trimmed.starts_with("if ") {
        if let Some(open_idx) = trimmed.find('[') {
            if let Some(close_rel) = trimmed[open_idx + 1..].find(']') {
                trimmed[open_idx + 1..open_idx + 1 + close_rel].trim()
            } else {
                trimmed
            }
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let command = extracted.trim_start_matches('+').trim();
    if command.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with("if ") {
        return parse_tcl_script_commands(command)
            .into_iter()
            .next()
            .map(|words| vec![words])
            .unwrap_or_default();
    }
    parse_tcl_script_commands(command)
}

const TK_EVENT_SUBST_FIELD_COUNT: usize = 19;

fn flatten_event_subst_arg(mut value_bits: u64) -> u64 {
    for _ in 0..8 {
        let Some(values) = decode_value_list(obj_from_bits(value_bits)) else {
            break;
        };
        if values.len() != 1 {
            break;
        }
        value_bits = values[0];
    }
    value_bits
}

fn parse_event_subst_i64(value_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(value_bits);
    if let Some(value) = to_i64(obj) {
        return Some(value);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return text.trim().parse::<i64>().ok();
    }
    if let Some(value) = to_f64(obj)
        && value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        return Some(value as i64);
    }
    None
}

fn normalize_event_subst_int_field(value_bits: u64) -> u64 {
    parse_event_subst_i64(value_bits)
        .map(MoltObject::from_int)
        .map(MoltObject::bits)
        .unwrap_or(value_bits)
}

fn normalize_event_subst_bool_field(value_bits: u64) -> u64 {
    let obj = obj_from_bits(value_bits);
    let parsed = if obj.is_bool() {
        obj.as_bool()
    } else if let Some(value) = to_i64(obj) {
        Some(value != 0)
    } else if let Some(text) = string_obj_to_owned(obj) {
        parse_bool_text(&text)
    } else if let Some(value) = to_f64(obj) {
        Some(value != 0.0)
    } else {
        None
    };
    parsed
        .map(MoltObject::from_bool)
        .map(MoltObject::bits)
        .unwrap_or_else(|| MoltObject::none().bits())
}

fn event_subst_value_is_empty(value_bits: u64) -> bool {
    let obj = obj_from_bits(value_bits);
    if obj.is_none() {
        return true;
    }
    string_obj_to_owned(obj).is_some_and(|value| value.is_empty())
}

fn normalize_event_subst_delta_field(value_bits: u64) -> u64 {
    if let Some(value) = parse_event_subst_i64(value_bits) {
        return MoltObject::from_int(value).bits();
    }
    if event_subst_value_is_empty(value_bits) {
        return MoltObject::from_int(0).bits();
    }
    value_bits
}

fn bind_script_line_invokes_command(line: &str, command_name: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    let normalized = trimmed.trim_start_matches('+').trim_start();
    if normalized.starts_with(command_name)
        && normalized[command_name.len()..]
            .chars()
            .next()
            .map_or(true, char::is_whitespace)
    {
        return true;
    }

    let wrapped_prefix = format!("[{command_name} ");
    let wrapped_exact = format!("[{command_name}]");
    if normalized.starts_with("if ")
        && (normalized.contains(&wrapped_prefix) || normalized.contains(&wrapped_exact))
    {
        return true;
    }

    parse_bind_script_commands(normalized)
        .into_iter()
        .any(|words| {
            let Some(first) = words.first() else {
                return false;
            };
            let first = first.trim_start_matches('+');
            if first == command_name {
                return true;
            }
            first == "if"
                && words.iter().any(|word| {
                    word.contains(wrapped_prefix.as_str()) || word.contains(wrapped_exact.as_str())
                })
        })
}

fn remove_bind_script_command_invocations(script: &str, command_name: &str) -> String {
    if script.is_empty() || command_name.is_empty() {
        return script.to_string();
    }
    let mut out = String::with_capacity(script.len());
    for segment in script.split_inclusive('\n') {
        let (line, ending) = match segment.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (segment, ""),
        };
        let parse_line = line.strip_suffix('\r').unwrap_or(line);
        if bind_script_line_invokes_command(parse_line, command_name) {
            continue;
        }
        out.push_str(line);
        out.push_str(ending);
    }
    if out.trim().is_empty() {
        return String::new();
    }
    out
}

fn event_generate_binding_sequences(app: &TkAppState, sequence: &str) -> Vec<String> {
    let mut sequences = vec![sequence.to_string()];
    if !(sequence.starts_with("<<") && sequence.ends_with(">>")) {
        for (virtual_name, physical_sequences) in &app.virtual_events {
            if physical_sequences.iter().any(|name| name == sequence)
                && !sequences.iter().any(|name| name == virtual_name)
            {
                sequences.push(virtual_name.clone());
            }
        }
    }
    sequences
}

fn build_event_generate_commands(
    app: &TkAppState,
    target_path: &str,
    sequence: &str,
    binding_sequences: &[String],
    options: &HashMap<String, String>,
) -> Vec<Vec<String>> {
    let tags = app
        .bindtags
        .get(target_path)
        .cloned()
        .unwrap_or_else(|| default_bindtags_for_target(app, target_path));

    let mut out = Vec::new();
    for tag in tags {
        let Some(bindings) = app.bind_scripts.get(&tag) else {
            continue;
        };
        for binding_sequence in binding_sequences {
            let Some(script) = bindings.get(binding_sequence) else {
                continue;
            };
            for mut words in parse_bind_script_commands(script) {
                if words.is_empty() {
                    continue;
                }
                for word in &mut words {
                    if let Some(substituted) =
                        event_generate_placeholder_value(word, target_path, sequence, options)
                    {
                        *word = substituted;
                    }
                }
                out.push(words);
            }
        }
    }
    out
}

fn treeview_event_target_item(
    treeview: &TkTreeviewState,
    options: &HashMap<String, String>,
) -> Option<String> {
    if let Some(item) = options
        .get("-item")
        .or_else(|| options.get("-iid"))
        .filter(|candidate| !candidate.is_empty())
        && treeview.items.contains_key(item.as_str())
    {
        return Some(item.clone());
    }
    if let Some(focus) = treeview
        .focus
        .as_deref()
        .filter(|candidate| treeview.items.contains_key(*candidate))
    {
        return Some(focus.to_string());
    }
    treeview
        .selection
        .iter()
        .find(|candidate| treeview.items.contains_key(candidate.as_str()))
        .cloned()
}

fn build_treeview_tag_event_commands(
    app: &TkAppState,
    target_path: &str,
    sequence: &str,
    binding_sequences: &[String],
    options: &HashMap<String, String>,
) -> Vec<Vec<String>> {
    let Some(treeview) = app
        .widgets
        .get(target_path)
        .and_then(|widget| widget.treeview.as_ref())
    else {
        return Vec::new();
    };
    let Some(item_id) = treeview_event_target_item(treeview, options) else {
        return Vec::new();
    };
    let Some(item) = treeview.items.get(&item_id) else {
        return Vec::new();
    };
    let item_tags = parse_treeview_tags(item);
    if item_tags.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for tag_name in item_tags {
        let Some(tag_state) = treeview.tags.get(&tag_name) else {
            continue;
        };
        for binding_sequence in binding_sequences {
            let Some(script) = tag_state.bindings.get(binding_sequence) else {
                continue;
            };
            for mut words in parse_bind_script_commands(script) {
                if words.is_empty() {
                    continue;
                }
                for word in &mut words {
                    if let Some(substituted) =
                        event_generate_placeholder_value(word, target_path, sequence, options)
                    {
                        *word = substituted;
                    }
                }
                out.push(words);
            }
        }
    }
    out
}

fn handle_event_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "event requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "event subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "add" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event add expects virtual event and sequences",
                ));
            }
            let virtual_name = get_string_arg(py, handle, args[2], "virtual event name")?;
            let sequences = app.virtual_events.entry(virtual_name).or_default();
            for &sequence_bits in &args[3..] {
                let sequence = get_string_arg(py, handle, sequence_bits, "event sequence")?;
                if !sequences.iter().any(|existing| existing == &sequence) {
                    sequences.push(sequence);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "delete" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event delete expects virtual event name",
                ));
            }
            let virtual_name = get_string_arg(py, handle, args[2], "virtual event name")?;
            if args.len() == 3 {
                app.virtual_events.remove(&virtual_name);
            } else if let Some(sequences) = app.virtual_events.get_mut(&virtual_name) {
                for &sequence_bits in &args[3..] {
                    let sequence = get_string_arg(py, handle, sequence_bits, "event sequence")?;
                    sequences.retain(|existing| existing != &sequence);
                }
                if sequences.is_empty() {
                    app.virtual_events.remove(&virtual_name);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "generate" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event generate expects widget path and sequence",
                ));
            }
            let target_path = get_string_arg(py, handle, args[2], "event target widget")?;
            let sequence = get_string_arg(py, handle, args[3], "event sequence")?;
            let options = parse_event_generate_options(py, handle, args, 4)?;
            let binding_sequences = event_generate_binding_sequences(app, &sequence);
            let mut command_lines = build_event_generate_commands(
                app,
                &target_path,
                &sequence,
                &binding_sequences,
                &options,
            );
            let mut tree_tag_command_lines = build_treeview_tag_event_commands(
                app,
                &target_path,
                &sequence,
                &binding_sequences,
                &options,
            );
            command_lines.append(&mut tree_tag_command_lines);
            app.last_error = None;
            drop(registry);

            for words in command_lines {
                let mut argv = Vec::with_capacity(words.len());
                for word in &words {
                    match alloc_string_bits(py, word) {
                        Ok(bits) => argv.push(bits),
                        Err(bits) => {
                            for owned in argv {
                                dec_ref_bits(py, owned);
                            }
                            return Err(bits);
                        }
                    }
                }
                let dispatch_out = tk_call_dispatch(py, handle, &argv);
                for owned in argv {
                    dec_ref_bits(py, owned);
                }
                let out_bits = dispatch_out?;
                let should_break = string_obj_to_owned(obj_from_bits(out_bits))
                    .is_some_and(|value| value == "break");
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(py, out_bits);
                }
                if should_break {
                    break;
                }
            }
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() == 2 {
                let mut names: Vec<String> = app.virtual_events.keys().cloned().collect();
                names.sort_unstable();
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    names.as_slice(),
                    "failed to allocate event info tuple",
                );
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event info expects optional virtual event name",
                ));
            }
            let virtual_name = get_string_arg(py, handle, args[2], "virtual event name")?;
            let sequences = app
                .virtual_events
                .get(&virtual_name)
                .cloned()
                .unwrap_or_default();
            app.last_error = None;
            alloc_tuple_from_strings(py, sequences.as_slice(), "failed to allocate event tuple")
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad event option \"{subcommand}\": must be add, delete, generate, or info"),
        )),
    }
}

fn handle_update_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() == 1 {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    if args.len() == 2 {
        let mode = get_string_arg(py, handle, args[1], "update mode")?;
        if mode == "idletasks" {
            clear_last_error(handle);
            return Ok(MoltObject::none().bits());
        }
    }
    Err(raise_tcl_for_handle(
        py,
        handle,
        "update expects optional idletasks argument",
    ))
}

fn wait_for_tk_condition<F>(py: &PyToken<'_>, handle: i64, mut done: F) -> Result<(), u64>
where
    F: FnMut(&TkAppState) -> bool,
{
    loop {
        let is_done = {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            done(app)
        };
        if is_done {
            clear_last_error(handle);
            return Ok(());
        }
        if pump_tcl_events(py, handle, 0)? {
            continue;
        }
        let progressed = dispatch_next_pending_event(py, handle)?;
        if progressed {
            continue;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn tkwait_window_exists_in_app(app: &TkAppState, target: &str) -> bool {
    if target == "." {
        return true;
    }
    app.widgets.contains_key(target)
}

fn tkwait_window_exists(registry: &TkRegistry, handle: i64, target: &str) -> bool {
    if target == "." {
        return registry.apps.contains_key(&handle);
    }
    registry
        .apps
        .get(&handle)
        .is_some_and(|app| tkwait_window_exists_in_app(app, target))
}

fn tkwait_visibility_reached_in_app(app: &TkAppState, target: &str) -> bool {
    if target == "." {
        return app.wm.state != "withdrawn" && app.wm.state != "iconic";
    }
    app.widgets
        .get(target)
        .is_some_and(|widget| widget.manager.is_some())
}

fn handle_tkwait_variable_target(
    py: &PyToken<'_>,
    handle: i64,
    variable_name: &str,
) -> Result<u64, u64> {
    let start_version = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        variable_version(app, variable_name)
    };
    wait_for_tk_condition(py, handle, |app| {
        variable_version(app, variable_name) != start_version
    })?;
    Ok(MoltObject::none().bits())
}

fn handle_tkwait_window_target(py: &PyToken<'_>, handle: i64, target: &str) -> Result<u64, u64> {
    let start_exists = {
        let registry = tk_registry().lock().unwrap();
        tkwait_window_exists(&registry, handle, target)
    };
    if !start_exists {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    wait_for_tk_condition(py, handle, |app| !tkwait_window_exists_in_app(app, target))?;
    Ok(MoltObject::none().bits())
}

fn handle_tkwait_visibility_target(
    py: &PyToken<'_>,
    handle: i64,
    target: &str,
) -> Result<u64, u64> {
    if target != "." {
        let exists_now = {
            let registry = tk_registry().lock().unwrap();
            tkwait_window_exists(&registry, handle, target)
        };
        if !exists_now {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                format!("bad window path name \"{target}\""),
            ));
        }
    }
    wait_for_tk_condition(py, handle, |app| {
        tkwait_visibility_reached_in_app(app, target)
    })?;
    Ok(MoltObject::none().bits())
}

fn handle_tkwait_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tkwait expects kind and target",
        ));
    }
    let kind = get_string_arg(py, handle, args[1], "tkwait kind")?;
    let target = get_string_arg(py, handle, args[2], "tkwait target")?;
    match kind.as_str() {
        "variable" => handle_tkwait_variable_target(py, handle, &target),
        "window" => handle_tkwait_window_target(py, handle, &target),
        "visibility" => handle_tkwait_visibility_target(py, handle, &target),
        _ => Err(raise_tcl_for_handle(
            py,
            handle,
            format!("bad tkwait kind \"{kind}\": must be variable, window, or visibility"),
        )),
    }
}

fn alloc_trace_info(
    py: &PyToken<'_>,
    registrations: Option<&Vec<TkTraceRegistration>>,
) -> Result<u64, u64> {
    let mut info_rows = Vec::new();
    if let Some(registrations) = registrations {
        let mut ordered: Vec<&TkTraceRegistration> = registrations.iter().collect();
        ordered.sort_by_key(|registration| registration.order);
        for registration in ordered {
            let mode_bits = alloc_string_bits(py, registration.mode_name.as_str())?;
            let callback_bits = alloc_string_bits(py, registration.callback_name.as_str())?;
            let pair = [mode_bits, callback_bits];
            let row_bits =
                match alloc_tuple_bits(py, &pair, "failed to allocate trace info row tuple") {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(py, mode_bits);
                        dec_ref_bits(py, callback_bits);
                        for owned_bits in info_rows {
                            dec_ref_bits(py, owned_bits);
                        }
                        return Err(bits);
                    }
                };
            dec_ref_bits(py, mode_bits);
            dec_ref_bits(py, callback_bits);
            info_rows.push(row_bits);
        }
    }
    let out = alloc_tuple_bits(py, info_rows.as_slice(), "failed to allocate trace info");
    for bits in info_rows {
        dec_ref_bits(py, bits);
    }
    out
}

fn handle_trace_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "trace requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "trace subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "add" => {
            if args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "trace add expects variable name, mode, and callback",
                ));
            }
            let subject = get_string_arg(py, handle, args[2], "trace subject")?;
            if subject != "variable" && subject != "array" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad trace subject \"{subject}\": must be variable or array"),
                ));
            }
            let variable_name = get_string_arg(py, handle, args[3], "trace variable name")?;
            let mode_name_raw = get_string_arg(py, handle, args[4], "trace mode")?;
            let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
                Ok(value) => value,
                Err(message) => {
                    return Err(app_tcl_error_locked(py, app, message));
                }
            };
            let callback_name = get_string_arg(py, handle, args[5], "trace callback")?;
            let registrations = app.traces.entry(variable_name).or_default();
            app.next_trace_order = app.next_trace_order.saturating_add(1);
            if app.next_trace_order == 0 {
                app.next_trace_order = 1;
            }
            registrations.push(TkTraceRegistration {
                mode_name,
                callback_name,
                order: app.next_trace_order,
            });
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "remove" => {
            if args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "trace remove expects variable name, mode, and callback",
                ));
            }
            let subject = get_string_arg(py, handle, args[2], "trace subject")?;
            if subject != "variable" && subject != "array" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad trace subject \"{subject}\": must be variable or array"),
                ));
            }
            let variable_name = get_string_arg(py, handle, args[3], "trace variable name")?;
            let mode_name_raw = get_string_arg(py, handle, args[4], "trace mode")?;
            let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
                Ok(value) => value,
                Err(message) => {
                    return Err(app_tcl_error_locked(py, app, message));
                }
            };
            let callback_name = get_string_arg(py, handle, args[5], "trace callback")?;
            remove_trace_registration(py, app, &variable_name, &mode_name, &callback_name);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "trace info expects variable name",
                ));
            }
            let subject = get_string_arg(py, handle, args[2], "trace subject")?;
            if subject != "variable" && subject != "array" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad trace subject \"{subject}\": must be variable or array"),
                ));
            }
            let variable_name = get_string_arg(py, handle, args[3], "trace variable name")?;
            app.last_error = None;
            alloc_trace_info(py, app.traces.get(&variable_name))
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad trace subcommand \"{subcommand}\": must be add, remove, or info"),
        )),
    }
}

fn handle_focus_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match args.len() {
        1 => {
            let value = app.focus_widget.clone().unwrap_or_default();
            app.last_error = None;
            alloc_string_bits(py, &value)
        }
        2 => {
            let target = get_string_arg(py, handle, args[1], "focus target")?;
            app.focus_widget = if target.is_empty() {
                None
            } else {
                Some(target)
            };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        3 => {
            let op = get_string_arg(py, handle, args[1], "focus option")?;
            let target = get_string_arg(py, handle, args[2], "focus target")?;
            match op.as_str() {
                "-force" => {
                    app.focus_widget = if target.is_empty() {
                        None
                    } else {
                        Some(target)
                    };
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "-lastfor" => {
                    if app.focus_widget.is_none() {
                        app.focus_widget = if target.is_empty() {
                            None
                        } else {
                            Some(target.clone())
                        };
                    }
                    let value = app.focus_widget.clone().unwrap_or_default();
                    app.last_error = None;
                    alloc_string_bits(py, &value)
                }
                "-displayof" => {
                    let value = app.focus_widget.clone().unwrap_or(target);
                    app.last_error = None;
                    alloc_string_bits(py, &value)
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad focus option \"{op}\": must be -displayof, -force, or -lastfor"),
                )),
            }
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            "focus expects no args, a target, or -force/-lastfor target",
        )),
    }
}

fn handle_focus_direction_command(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
    label: &str,
) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} expects a widget target"),
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "focus widget")?;
    clear_last_error(handle);
    alloc_string_bits(py, &widget_path)
}

fn handle_grab_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "grab requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "grab subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "set" => {
            if args.len() == 3 {
                let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
                app.grab_widget = Some(widget_path);
                app.grab_is_global = false;
                app.last_error = None;
                return Ok(MoltObject::none().bits());
            }
            if args.len() == 4 {
                let scope = get_string_arg(py, handle, args[2], "grab scope")?;
                if scope != "-global" {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "grab set scope must be -global",
                    ));
                }
                let widget_path = get_string_arg(py, handle, args[3], "grab widget")?;
                app.grab_widget = Some(widget_path);
                app.grab_is_global = true;
                app.last_error = None;
                return Ok(MoltObject::none().bits());
            }
            Err(app_tcl_error_locked(
                py,
                app,
                "grab set expects widget or -global widget",
            ))
        }
        "release" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab release expects a widget",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
            if app.grab_widget.as_deref() == Some(widget_path.as_str()) {
                app.grab_widget = None;
                app.grab_is_global = false;
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "current" => {
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab current expects no extra arguments",
                ));
            }
            let widget_path = app.grab_widget.clone().unwrap_or_default();
            app.last_error = None;
            alloc_string_bits(py, &widget_path)
        }
        "status" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab status expects a widget",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
            let status = if app.grab_widget.as_deref() == Some(widget_path.as_str()) {
                if app.grab_is_global {
                    "global"
                } else {
                    "local"
                }
            } else {
                ""
            };
            app.last_error = None;
            alloc_string_bits(py, status)
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad grab option \"{subcommand}\": must be current, release, set, or status"),
        )),
    }
}

fn handle_clipboard_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "clipboard requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "clipboard subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "clear" => {
            app.clipboard_text.clear();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "append" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "clipboard append expects a string payload",
                ));
            }
            let mut payload = String::new();
            let mut idx = 2;
            while idx < args.len() {
                let token = get_string_arg(py, handle, args[idx], "clipboard token")?;
                if token == "--" && idx + 1 < args.len() {
                    payload = get_string_arg(py, handle, args[idx + 1], "clipboard payload")?;
                    break;
                }
                payload = token;
                idx += 1;
            }
            app.clipboard_text.push_str(&payload);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            app.last_error = None;
            alloc_string_bits(py, &app.clipboard_text)
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad clipboard option \"{subcommand}\": must be append, clear, or get"),
        )),
    }
}

fn handle_selection_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "selection requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "selection subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "clear" => {
            app.selection_text.clear();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            let value = if app.selection_text.is_empty() {
                app.clipboard_text.clone()
            } else {
                app.selection_text.clone()
            };
            app.last_error = None;
            alloc_string_bits(py, &value)
        }
        "own" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_string_bits(py, app.selection_owner.as_deref().unwrap_or(""));
            }
            let mut owner: Option<String> = None;
            for &bits in &args[2..] {
                let token = get_string_arg(py, handle, bits, "selection own argument")?;
                if token.starts_with('-') {
                    continue;
                }
                owner = Some(token);
            }
            app.selection_owner = owner;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "handle" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad selection option \"{subcommand}\": must be clear, get, handle, or own"),
        )),
    }
}

fn widget_layout_options_mut<'a>(
    widget: &'a mut TkWidgetState,
    manager: &str,
) -> &'a mut HashMap<String, u64> {
    match manager {
        "pack" => &mut widget.pack_options,
        "grid" => &mut widget.grid_options,
        "place" => &mut widget.place_options,
        _ => &mut widget.pack_options,
    }
}

fn widget_layout_options<'a>(widget: &'a TkWidgetState, manager: &str) -> &'a HashMap<String, u64> {
    match manager {
        "pack" => &widget.pack_options,
        "grid" => &widget.grid_options,
        "place" => &widget.place_options,
        _ => &widget.pack_options,
    }
}

fn handle_geometry_command(
    py: &PyToken<'_>,
    handle: i64,
    manager: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{manager} requires a subcommand"),
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "geometry subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "configure" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} configure expects a widget path"),
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "geometry widget path")?;
            if args.len() == 3 {
                let Some(widget) = app.widgets.get(&widget_path) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("bad window path name \"{widget_path}\""),
                    ));
                };
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    widget_layout_options(widget, manager),
                    "failed to allocate geometry option tuple",
                );
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "geometry option name")?;
                let Some(widget) = app.widgets.get(&widget_path) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("bad window path name \"{widget_path}\""),
                    ));
                };
                app.last_error = None;
                return option_map_query_or_empty(
                    py,
                    widget_layout_options(widget, manager),
                    &option_name,
                );
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "geometry options")?;
            {
                let Some(widget) = app.widgets.get_mut(&widget_path) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("bad window path name \"{widget_path}\""),
                    ));
                };
                let options = widget_layout_options_mut(widget, manager);
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, options, option_name, value_bits);
                }
                widget.manager = Some(manager.to_string());
            }
            ensure_layout_membership(app, manager, &widget_path);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "forget" | "remove" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} {subcommand} expects a widget path"),
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "geometry widget path")?;
            let Some(widget) = app.widgets.get_mut(&widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            if widget.manager.as_deref() == Some(manager) {
                widget.manager = None;
            }
            remove_widget_from_layout_lists(app, &widget_path);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} info expects a widget path"),
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "geometry widget path")?;
            let Some(widget) = app.widgets.get(&widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            app.last_error = None;
            option_map_to_tuple(
                py,
                widget_layout_options(widget, manager),
                "failed to allocate geometry info tuple",
            )
        }
        "propagate" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} propagate expects widget and optional flag"),
                ));
            }
            let container = get_string_arg(py, handle, args[2], "geometry container path")?;
            let propagate_map = if manager == "grid" {
                &mut app.grid_propagate
            } else {
                &mut app.pack_propagate
            };
            if args.len() == 3 {
                let current = propagate_map.get(&container).copied().unwrap_or(true);
                app.last_error = None;
                return Ok(MoltObject::from_bool(current).bits());
            }
            let enabled = parse_bool_arg(py, handle, args[3], "geometry propagate flag")?;
            propagate_map.insert(container, enabled);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "slaves" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} slaves expects a container path"),
                ));
            }
            let container = get_string_arg(py, handle, args[2], "geometry container path")?;
            let items = if container == "." {
                if manager == "pack" {
                    app.pack_slaves.clone()
                } else if manager == "grid" {
                    app.grid_slaves.clone()
                } else {
                    app.place_slaves.clone()
                }
            } else {
                Vec::new()
            };
            app.last_error = None;
            alloc_tuple_from_strings(py, items.as_slice(), "failed to allocate geometry slaves")
        }
        "bbox" if manager == "grid" => {
            if args.len() < 3 || args.len() > 7 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid bbox expects container and optional index bounds",
                ));
            }
            let bbox = vec![
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
            ];
            app.last_error = None;
            alloc_tuple_from_strings(py, &bbox, "failed to allocate grid bbox tuple")
        }
        "location" if manager == "grid" => {
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid location expects container path and x/y coordinates",
                ));
            }
            app.last_error = None;
            alloc_int_tuple2_bits(py, 0, 0, "failed to allocate grid location tuple")
        }
        "size" if manager == "grid" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid size expects a container path",
                ));
            }
            app.last_error = None;
            alloc_int_tuple2_bits(
                py,
                0,
                app.grid_slaves.len() as i64,
                "failed to allocate grid size tuple",
            )
        }
        "columnconfigure" | "rowconfigure" if manager == "grid" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid row/columnconfigure expects container and index",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grid container path")?;
            let index = get_string_arg(py, handle, args[3], "grid index")?;
            let Some(widget) = app.widgets.get_mut(&widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            let configs = if subcommand == "columnconfigure" {
                widget.grid_columnconfigure.entry(index).or_default()
            } else {
                widget.grid_rowconfigure.entry(index).or_default()
            };
            if args.len() == 4 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    configs,
                    "failed to allocate grid row/columnconfigure tuple",
                );
            }
            if args.len() == 5 {
                let option_name = parse_widget_option_name_arg(
                    py,
                    handle,
                    args[4],
                    "grid row/columnconfigure option",
                )?;
                app.last_error = None;
                return option_map_query_or_empty(py, configs, &option_name);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 4, "grid row/columnconfigure options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, configs, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad {manager} option \"{subcommand}\""),
        )),
    }
}

fn handle_raise_or_lower_command(
    py: &PyToken<'_>,
    handle: i64,
    command: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{command} expects widget and optional sibling"),
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "widget path")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get(&widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    let manager = widget.manager.clone();
    let order_list = match manager.as_deref() {
        Some("pack") => &mut app.pack_slaves,
        Some("grid") => &mut app.grid_slaves,
        Some("place") => &mut app.place_slaves,
        _ => {
            app.last_error = None;
            return Ok(MoltObject::none().bits());
        }
    };
    if let Some(idx) = order_list.iter().position(|name| name == &widget_path) {
        order_list.remove(idx);
    }
    if command == "raise" {
        if args.len() == 3 {
            let sibling = get_string_arg(py, handle, args[2], "sibling widget path")?;
            if let Some(idx) = order_list.iter().position(|name| name == &sibling) {
                order_list.insert(idx + 1, widget_path);
            } else {
                order_list.push(widget_path);
            }
        } else {
            order_list.push(widget_path);
        }
    } else if args.len() == 3 {
        let sibling = get_string_arg(py, handle, args[2], "sibling widget path")?;
        if let Some(idx) = order_list.iter().position(|name| name == &sibling) {
            order_list.insert(idx, widget_path);
        } else {
            order_list.insert(0, widget_path);
        }
    } else {
        order_list.insert(0, widget_path);
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn handle_wm_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "wm expects operation and toplevel path",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "wm subcommand")?;
    let toplevel = get_string_arg(py, handle, args[2], "wm toplevel path")?;
    if toplevel != "." {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        if !app.widgets.contains_key(&toplevel) {
            return Err(app_tcl_error_locked(
                py,
                app,
                format!("bad window path name \"{toplevel}\""),
            ));
        }
    }
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let wm = &mut app.wm;
    match subcommand.as_str() {
        "title" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.title);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm title expects optional title value",
                ));
            }
            wm.title = get_string_arg(py, handle, args[3], "wm title")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "geometry" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.geometry);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm geometry expects optional geometry spec",
                ));
            }
            wm.geometry = get_string_arg(py, handle, args[3], "wm geometry spec")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "state" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.state);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm state expects optional state value",
                ));
            }
            wm.state = get_string_arg(py, handle, args[3], "wm state")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "attributes" => {
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(py, &wm.attributes, "failed to allocate wm attributes");
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "wm attribute name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &wm.attributes, &option_name);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "wm attributes")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut wm.attributes, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "aspect" => {
            if args.len() == 3 {
                app.last_error = None;
                if let Some((min_num, min_den, max_num, max_den)) = wm.aspect {
                    return alloc_tuple_from_strings(
                        py,
                        &[
                            min_num.to_string(),
                            min_den.to_string(),
                            max_num.to_string(),
                            max_den.to_string(),
                        ],
                        "failed to allocate wm aspect tuple",
                    );
                }
                return alloc_empty_string_bits(py);
            }
            if args.len() == 4 {
                let value = get_string_arg(py, handle, args[3], "wm aspect value")?;
                if value.is_empty() {
                    wm.aspect = None;
                    app.last_error = None;
                    return Ok(MoltObject::none().bits());
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm aspect expects 4 integer arguments or empty string",
                ));
            }
            if args.len() != 7 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm aspect expects 4 integer arguments",
                ));
            }
            wm.aspect = Some((
                parse_i64_arg(py, handle, args[3], "wm aspect minNumerator")?,
                parse_i64_arg(py, handle, args[4], "wm aspect minDenominator")?,
                parse_i64_arg(py, handle, args[5], "wm aspect maxNumerator")?,
                parse_i64_arg(py, handle, args[6], "wm aspect maxDenominator")?,
            ));
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "client" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.client);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm client expects optional name",
                ));
            }
            wm.client = get_string_arg(py, handle, args[3], "wm client name")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "colormapwindows" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    wm.colormapwindows.as_slice(),
                    "failed to allocate wm colormapwindows tuple",
                );
            }
            wm.colormapwindows.clear();
            for &bits in &args[3..] {
                wm.colormapwindows.push(get_string_arg(
                    py,
                    handle,
                    bits,
                    "wm colormap window path",
                )?);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "command" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    wm.command.as_slice(),
                    "failed to allocate wm command tuple",
                );
            }
            wm.command.clear();
            for &bits in &args[3..] {
                wm.command
                    .push(get_string_arg(py, handle, bits, "wm command argument")?);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "focusmodel" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.focusmodel);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm focusmodel expects optional model",
                ));
            }
            wm.focusmodel = get_string_arg(py, handle, args[3], "wm focusmodel")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "forget" | "manage" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "frame" => {
            app.last_error = None;
            alloc_string_bits(py, &wm.frame)
        }
        "grid" => {
            if args.len() == 3 {
                app.last_error = None;
                if let Some((base_width, base_height, width_inc, height_inc)) = wm.grid {
                    return alloc_tuple_from_strings(
                        py,
                        &[
                            base_width.to_string(),
                            base_height.to_string(),
                            width_inc.to_string(),
                            height_inc.to_string(),
                        ],
                        "failed to allocate wm grid tuple",
                    );
                }
                return alloc_empty_string_bits(py);
            }
            if args.len() == 4 {
                let value = get_string_arg(py, handle, args[3], "wm grid value")?;
                if value.is_empty() {
                    wm.grid = None;
                    app.last_error = None;
                    return Ok(MoltObject::none().bits());
                }
            }
            if args.len() != 7 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm grid expects 4 integer arguments",
                ));
            }
            wm.grid = Some((
                parse_i64_arg(py, handle, args[3], "wm grid baseWidth")?,
                parse_i64_arg(py, handle, args[4], "wm grid baseHeight")?,
                parse_i64_arg(py, handle, args[5], "wm grid widthInc")?,
                parse_i64_arg(py, handle, args[6], "wm grid heightInc")?,
            ));
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "group" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, wm.group.as_deref().unwrap_or(""));
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm group expects optional path",
                ));
            }
            let value = get_string_arg(py, handle, args[3], "wm group path")?;
            wm.group = if value.is_empty() { None } else { Some(value) };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconbitmap" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.iconbitmap);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconbitmap expects optional bitmap path",
                ));
            }
            wm.iconbitmap = get_string_arg(py, handle, args[3], "wm iconbitmap path")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconmask" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.iconmask);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconmask expects optional mask path",
                ));
            }
            wm.iconmask = get_string_arg(py, handle, args[3], "wm iconmask path")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconphoto" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconposition" => {
            if args.len() == 3 {
                app.last_error = None;
                if let Some((x, y)) = wm.iconposition {
                    return alloc_int_tuple2_bits(
                        py,
                        x,
                        y,
                        "failed to allocate wm iconposition tuple",
                    );
                }
                return alloc_empty_string_bits(py);
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconposition expects x and y",
                ));
            }
            wm.iconposition = Some((
                parse_i64_arg(py, handle, args[3], "wm iconposition x")?,
                parse_i64_arg(py, handle, args[4], "wm iconposition y")?,
            ));
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconwindow" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, wm.iconwindow.as_deref().unwrap_or(""));
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconwindow expects optional widget path",
                ));
            }
            let value = get_string_arg(py, handle, args[3], "wm iconwindow path")?;
            wm.iconwindow = if value.is_empty() { None } else { Some(value) };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "resizable" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_int_tuple2_bits(
                    py,
                    i64::from(wm.resizable_width),
                    i64::from(wm.resizable_height),
                    "failed to allocate wm resizable tuple",
                );
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm resizable expects width and height",
                ));
            }
            wm.resizable_width = parse_bool_arg(py, handle, args[3], "wm resizable width")?;
            wm.resizable_height = parse_bool_arg(py, handle, args[4], "wm resizable height")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "protocol" => {
            if args.len() == 3 {
                let mut names: Vec<String> = wm.protocols.keys().cloned().collect();
                names.sort_unstable();
                let mut flat = Vec::with_capacity(names.len() * 2);
                for name in names {
                    let Some(cmd) = wm.protocols.get(&name) else {
                        continue;
                    };
                    flat.push(name);
                    flat.push(cmd.clone());
                }
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    flat.as_slice(),
                    "failed to allocate wm protocol tuple",
                );
            }
            if args.len() == 4 {
                let protocol_name = get_string_arg(py, handle, args[3], "wm protocol name")?;
                let command_name = wm
                    .protocols
                    .get(&protocol_name)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return alloc_string_bits(py, &command_name);
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm protocol expects name and optional command",
                ));
            }
            let protocol_name = get_string_arg(py, handle, args[3], "wm protocol name")?;
            let command_name = get_string_arg(py, handle, args[4], "wm protocol callback")?;
            if command_name.is_empty() {
                wm.protocols.remove(&protocol_name);
            } else {
                wm.protocols.insert(protocol_name, command_name);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconify" => {
            wm.state = "iconic".to_string();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "deiconify" => {
            wm.state = "normal".to_string();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "withdraw" => {
            wm.state = "withdrawn".to_string();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "minsize" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_int_tuple2_bits(
                    py,
                    wm.minsize.0,
                    wm.minsize.1,
                    "failed to allocate wm minsize tuple",
                );
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm minsize expects width and height",
                ));
            }
            wm.minsize.0 = parse_i64_arg(py, handle, args[3], "wm minsize width")?;
            wm.minsize.1 = parse_i64_arg(py, handle, args[4], "wm minsize height")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "maxsize" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_int_tuple2_bits(
                    py,
                    wm.maxsize.0,
                    wm.maxsize.1,
                    "failed to allocate wm maxsize tuple",
                );
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm maxsize expects width and height",
                ));
            }
            wm.maxsize.0 = parse_i64_arg(py, handle, args[3], "wm maxsize width")?;
            wm.maxsize.1 = parse_i64_arg(py, handle, args[4], "wm maxsize height")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "overrideredirect" => {
            if args.len() == 3 {
                app.last_error = None;
                return Ok(MoltObject::from_bool(wm.overrideredirect).bits());
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm overrideredirect expects optional boolean",
                ));
            }
            wm.overrideredirect = parse_bool_arg(py, handle, args[3], "wm overrideredirect flag")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "transient" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, wm.transient.as_deref().unwrap_or(""));
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm transient expects optional master path",
                ));
            }
            let master_path = get_string_arg(py, handle, args[3], "wm transient master")?;
            wm.transient = if master_path.is_empty() {
                None
            } else {
                Some(master_path)
            };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconname" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.iconname);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconname expects optional string",
                ));
            }
            wm.iconname = get_string_arg(py, handle, args[3], "wm iconname")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "positionfrom" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.positionfrom);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm positionfrom expects optional source",
                ));
            }
            wm.positionfrom = get_string_arg(py, handle, args[3], "wm positionfrom source")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "sizefrom" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.sizefrom);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm sizefrom expects optional source",
                ));
            }
            wm.sizefrom = get_string_arg(py, handle, args[3], "wm sizefrom source")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            if args.len() == 3 {
                app.last_error = None;
                alloc_empty_string_bits(py)
            } else {
                app.last_error = None;
                Ok(MoltObject::none().bits())
            }
        }
    }
}

fn handle_winfo_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "winfo requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "winfo subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "children" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo children expects a widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let children: Vec<String> = if path == "." {
                let mut names: Vec<String> = app.widgets.keys().cloned().collect();
                names.sort_unstable();
                names
            } else {
                Vec::new()
            };
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                children.as_slice(),
                "failed to allocate children",
            );
        }
        "exists" | "ismapped" | "viewable" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo exists/ismapped/viewable expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let exists = path == "." || app.widgets.contains_key(&path);
            let value = if subcommand == "exists" {
                exists
            } else if path == "." {
                true
            } else {
                app.widgets
                    .get(&path)
                    .is_some_and(|widget| widget.manager.is_some())
            };
            app.last_error = None;
            return Ok(MoltObject::from_bool(value).bits());
        }
        "manager" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo manager expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let value = if path == "." {
                "wm".to_string()
            } else {
                app.widgets
                    .get(&path)
                    .and_then(|widget| widget.manager.clone())
                    .unwrap_or_default()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "class" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo class expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let class_name = if path == "." {
                "Tk".to_string()
            } else if let Some(widget) = app.widgets.get(&path) {
                tk_widget_class_name(&widget.widget_command)
            } else {
                String::new()
            };
            app.last_error = None;
            return alloc_string_bits(py, &class_name);
        }
        "name" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo name expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let name = if path == "." {
                "tk".to_string()
            } else {
                path.trim_start_matches('.')
                    .trim_start_matches('!')
                    .to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &name);
        }
        "parent" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo parent expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let parent = if path == "." {
                String::new()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &parent);
        }
        "toplevel" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo toplevel expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, ".");
        }
        "id" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo id expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let id = if path == "." {
                1
            } else {
                (path
                    .bytes()
                    .fold(17_u64, |acc, b| acc.wrapping_mul(33).wrapping_add(b as u64))
                    % 1_000_000) as i64
                    + 2
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(id).bits());
        }
        "width" | "reqwidth" | "height" | "reqheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo width/height/reqwidth/reqheight expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let value = if path == "." {
                if subcommand.ends_with("width") {
                    200
                } else {
                    160
                }
            } else if let Some(widget) = app.widgets.get(&path) {
                if subcommand.ends_with("width") {
                    widget_option_i64_default(&widget.options, "-width", 200)
                } else {
                    widget_option_i64_default(&widget.options, "-height", 160)
                }
            } else if subcommand.ends_with("width") {
                0
            } else {
                0
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(value).bits());
        }
        "x" | "y" | "rootx" | "rooty" | "pointerx" | "pointery" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo coordinate query expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(0).bits());
        }
        "screenwidth" => {
            app.last_error = None;
            return Ok(MoltObject::from_int(1024).bits());
        }
        "screenheight" => {
            app.last_error = None;
            return Ok(MoltObject::from_int(768).bits());
        }
        "pointerxy" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pointerxy expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_int_tuple2_bits(py, 0, 0, "failed to allocate pointerxy tuple");
        }
        "rgb" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo rgb expects widget path and color",
                ));
            }
            let color = get_string_arg(py, handle, args[3], "winfo color")?;
            let (r, g, b) = parse_winfo_rgb_components(&color);
            let elems = vec![
                MoltObject::from_int(r).bits(),
                MoltObject::from_int(g).bits(),
                MoltObject::from_int(b).bits(),
            ];
            app.last_error = None;
            return alloc_tuple_bits(py, elems.as_slice(), "failed to allocate winfo rgb tuple");
        }
        "atom" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo atom expects atom name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "atom name")?;
            let id = if let Some(id) = app.atoms_by_name.get(&name).copied() {
                id
            } else {
                app.next_atom_id = app.next_atom_id.saturating_add(1);
                let id = app.next_atom_id;
                app.atoms_by_name.insert(name.clone(), id);
                app.atoms_by_id.insert(id, name);
                id
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(id).bits());
        }
        "atomname" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo atomname expects atom id",
                ));
            }
            let atom_id = parse_i64_arg(py, handle, args[2], "atom id")?;
            let name = app.atoms_by_id.get(&atom_id).cloned().unwrap_or_default();
            app.last_error = None;
            return alloc_string_bits(py, &name);
        }
        "containing" => {
            if args.len() != 4 && args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo containing expects root coordinates with optional -displayof",
                ));
            }
            let value = if let Some(first) = app.widgets.keys().next() {
                first.clone()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "cells" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo cells expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(256).bits());
        }
        "colormapfull" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo colormapfull expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_bool(false).bits());
        }
        "depth" | "screendepth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo depth/screendepth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(24).bits());
        }
        "fpixels" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo fpixels expects widget path and distance",
                ));
            }
            let distance = get_text_arg(py, handle, args[3], "winfo fpixels distance")?;
            let value = distance.trim().parse::<f64>().unwrap_or(0.0);
            app.last_error = None;
            return Ok(MoltObject::from_float(value).bits());
        }
        "pixels" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pixels expects widget path and distance",
                ));
            }
            let distance = get_text_arg(py, handle, args[3], "winfo pixels distance")?;
            let value = distance
                .trim()
                .parse::<f64>()
                .map(|v| v.round() as i64)
                .unwrap_or(0);
            app.last_error = None;
            return Ok(MoltObject::from_int(value).bits());
        }
        "geometry" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo geometry expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let (width, height) = if path == "." {
                (200, 160)
            } else if let Some(widget) = app.widgets.get(&path) {
                (
                    widget_option_i64_default(&widget.options, "-width", 200),
                    widget_option_i64_default(&widget.options, "-height", 160),
                )
            } else {
                (0, 0)
            };
            app.last_error = None;
            return alloc_string_bits(py, &format!("{width}x{height}+0+0"));
        }
        "interps" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo interps expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                &[String::from("molt")],
                "failed to allocate winfo interps",
            );
        }
        "pathname" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pathname expects window id",
                ));
            }
            let window_id = parse_i64_arg(py, handle, args[2], "winfo window id")?;
            let value = if window_id <= 1 {
                ".".to_string()
            } else if let Some(path) = app.widgets.keys().next() {
                path.clone()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "screen" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screen expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, ":0.0");
        }
        "screencells" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screencells expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(16_777_216).bits());
        }
        "screenmmheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screenmmheight expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(270).bits());
        }
        "screenmmwidth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screenmmwidth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(340).bits());
        }
        "screenvisual" | "visual" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo visual/screenvisual expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "truecolor");
        }
        "server" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo server expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "MoltTk");
        }
        "visualid" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo visualid expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "0x00000021");
        }
        "vrootheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootheight expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(768).bits());
        }
        "vrootwidth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootwidth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(1024).bits());
        }
        "vrootx" | "vrooty" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootx/vrooty expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(0).bits());
        }
        _ => {}
    }
    app.last_error = None;
    alloc_empty_string_bits(py)
}

fn handle_image_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "image expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "image subcommand")?;
    match subcommand.as_str() {
        "create" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "image create expects an image type",
                ));
            }
            let kind = get_string_arg(py, handle, args[2], "image type")?;
            let explicit_name = if args.len() >= 4 {
                let candidate = get_string_arg(py, handle, args[3], "image name")?;
                (!candidate.starts_with('-')).then_some(candidate)
            } else {
                None
            };
            let option_start = if explicit_name.is_some() { 4 } else { 3 };
            if !(args.len() - option_start).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "image create expects key/value options",
                ));
            }
            let mut option_names = Vec::with_capacity((args.len() - option_start) / 2);
            for idx in (option_start..args.len()).step_by(2) {
                option_names.push(get_string_arg(py, handle, args[idx], "image option name")?);
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let name = if let Some(name) = explicit_name {
                name
            } else {
                let mut id = app.images.len() as i64 + 1;
                let mut generated = format!("pyimage{id}");
                while app.images.contains_key(&generated) {
                    id += 1;
                    generated = format!("pyimage{id}");
                }
                generated
            };
            if let Some(existing) = app.images.get_mut(&name) {
                clear_value_map_refs(py, &mut existing.options);
            }
            let mut options = HashMap::new();
            for (idx, option_name) in option_names.into_iter().enumerate() {
                let value_bits = args[option_start + idx * 2 + 1];
                inc_ref_bits(py, value_bits);
                if let Some(old_bits) = options.insert(option_name, value_bits) {
                    dec_ref_bits(py, old_bits);
                }
            }
            app.images
                .insert(name.clone(), TkImageState { kind, options });
            app.last_error = None;
            alloc_string_bits(py, &name)
        }
        "delete" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            for &bits in &args[2..] {
                let name = get_string_arg(py, handle, bits, "image name")?;
                if let Some(mut image) = app.images.remove(&name) {
                    clear_value_map_refs(py, &mut image.options);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "names" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let mut names: Vec<String> = app.images.keys().cloned().collect();
            names.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, names.as_slice(), "failed to allocate image names tuple")
        }
        "types" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let mut kinds: Vec<String> = app
                .images
                .values()
                .map(|image| image.kind.clone())
                .collect();
            kinds.sort_unstable();
            kinds.dedup();
            app.last_error = None;
            alloc_tuple_from_strings(py, kinds.as_slice(), "failed to allocate image types tuple")
        }
        "width" | "height" | "type" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("image {subcommand} expects an image name"),
                ));
            }
            let name = get_string_arg(py, handle, args[2], "image name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(image) = app.images.get(&name) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("image \"{name}\" does not exist"),
                ));
            };
            app.last_error = None;
            match subcommand.as_str() {
                "width" => Ok(MoltObject::from_int(widget_option_i64_default(
                    &image.options,
                    "-width",
                    0,
                ))
                .bits()),
                "height" => Ok(MoltObject::from_int(widget_option_i64_default(
                    &image.options,
                    "-height",
                    0,
                ))
                .bits()),
                _ => alloc_string_bits(py, &image.kind),
            }
        }
        _ => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
    }
}

fn handle_image_instance_command(
    py: &PyToken<'_>,
    handle: i64,
    image_name: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "image command expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "image command subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(image) = app.images.get_mut(image_name) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("image \"{image_name}\" does not exist"),
        ));
    };
    match subcommand.as_str() {
        "configure" => {
            if args.len() == 2 {
                app.last_error = None;
                return option_map_to_tuple(py, &image.options, "failed to allocate image config");
            }
            if args.len() == 3 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[2], "image option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &image.options, &option_name);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 2, "image configure options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut image.options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "image cget expects exactly one option",
                ));
            }
            let option_name =
                parse_widget_option_name_arg(py, handle, args[2], "image option name")?;
            app.last_error = None;
            option_map_query_or_empty(py, &image.options, &option_name)
        }
        "blank" | "copy" | "put" | "write" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "type" => {
            app.last_error = None;
            alloc_string_bits(py, &image.kind)
        }
        "width" => {
            app.last_error = None;
            Ok(MoltObject::from_int(widget_option_i64_default(&image.options, "-width", 0)).bits())
        }
        "height" => {
            app.last_error = None;
            Ok(
                MoltObject::from_int(widget_option_i64_default(&image.options, "-height", 0))
                    .bits(),
            )
        }
        "get" => {
            app.last_error = None;
            alloc_tuple_from_strings(
                py,
                &[String::from("0"), String::from("0"), String::from("0")],
                "failed to allocate image pixel tuple",
            )
        }
        "transparency" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "image transparency op")?;
                if op == "get" {
                    app.last_error = None;
                    return Ok(MoltObject::from_bool(false).bits());
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("unknown image subcommand \"{subcommand}\" for image \"{image_name}\""),
        )),
    }
}

fn handle_font_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "font expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "font subcommand")?;
    match subcommand.as_str() {
        "create" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font create expects a font name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "font name")?;
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font create expects key/value options",
                ));
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "font create options")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            if let Some(existing) = app.fonts.get_mut(&name) {
                clear_value_map_refs(py, &mut existing.options);
            }
            let mut state = TkFontState::default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut state.options, option_name, value_bits);
            }
            app.fonts.insert(name.clone(), state);
            app.last_error = None;
            alloc_string_bits(py, &name)
        }
        "delete" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            for &bits in &args[2..] {
                let name = get_string_arg(py, handle, bits, "font name")?;
                if let Some(mut font) = app.fonts.remove(&name) {
                    clear_value_map_refs(py, &mut font.options);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "names" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let mut names: Vec<String> = app.fonts.keys().cloned().collect();
            names.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, names.as_slice(), "failed to allocate font names tuple")
        }
        "families" => {
            let families = [
                String::from("TkDefaultFont"),
                String::from("TkTextFont"),
                String::from("TkFixedFont"),
            ];
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            alloc_tuple_from_strings(py, &families, "failed to allocate font families tuple")
        }
        "measure" => {
            if args.len() < 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font measure expects name and text",
                ));
            }
            let text = get_text_arg(py, handle, args[args.len() - 1], "font measure text")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            Ok(MoltObject::from_int((text.chars().count() as i64) * 8).bits())
        }
        "configure" | "actual" | "metrics" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font configure/actual/metrics expects a font name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "font name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let font = app.fonts.entry(name).or_default();
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &font.options,
                    "failed to allocate font option tuple",
                );
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "font option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &font.options, &option_name);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "font options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut font.options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
    }
}

fn handle_tix_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(py, handle, "tix expects a subcommand"));
    }
    let subcommand = get_string_arg(py, handle, args[1], "tix subcommand")?;
    match subcommand.as_str() {
        "addbitmapdir" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tix addbitmapdir expects a directory",
                ));
            }
            let _directory = get_string_arg(py, handle, args[2], "bitmap directory")?;
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tix cget expects one option",
                ));
            }
            let option_name = parse_widget_option_name_arg(py, handle, args[2], "tix option")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            option_map_query_or_empty(py, &app.tix_options, &option_name)
        }
        "configure" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            if args.len() == 2 {
                app.last_error = None;
                return option_map_to_tuple(py, &app.tix_options, "failed to allocate tix options");
            }
            if args.len() == 3 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[2], "tix option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &app.tix_options, &option_name);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 2, "tix options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut app.tix_options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "filedialog" => {
            clear_last_error(handle);
            alloc_empty_string_bits(py)
        }
        "getbitmap" | "getimage" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("tix {subcommand} expects a name"),
                ));
            }
            let name = get_string_arg(py, handle, args[2], "tix image name")?;
            clear_last_error(handle);
            alloc_string_bits(py, &name)
        }
        "option" => {
            if args.len() != 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tix option expects `get <name>`",
                ));
            }
            let op = get_string_arg(py, handle, args[2], "tix option operation")?;
            if op != "get" {
                clear_last_error(handle);
                return alloc_empty_string_bits(py);
            }
            let name = get_string_arg(py, handle, args[3], "tix option name")?;
            let option_name = if name.starts_with('-') {
                name
            } else {
                format!("-{name}")
            };
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            option_map_query_or_empty(py, &app.tix_options, &option_name)
        }
        "resetoptions" => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
        _ => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
    }
}

fn handle_tix_form_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tixForm expects a widget path or subcommand",
        ));
    }
    let first = get_string_arg(py, handle, args[1], "tixForm argument")?;
    let (subcommand, widget_path, option_start) = match first.as_str() {
        "check" | "forget" | "grid" | "info" | "slaves" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("tixForm {first} expects a widget path"),
                ));
            }
            (
                first.clone(),
                get_string_arg(py, handle, args[2], "tixForm widget path")?,
                3,
            )
        }
        _ => (
            "configure".to_string(),
            get_string_arg(py, handle, args[1], "tixForm widget path")?,
            2,
        ),
    };
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(&widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    match subcommand.as_str() {
        "configure" => {
            if (args.len() - option_start).is_multiple_of(2) {
                let option_pairs = parse_widget_option_pairs(
                    py,
                    handle,
                    args,
                    option_start,
                    "tixForm configure options",
                )?;
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, &mut widget.place_options, option_name, value_bits);
                }
            } else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "tixForm configure expects key/value options",
                ));
            }
            widget.manager = Some("place".to_string());
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "check" | "forget" => {
            if subcommand == "forget" {
                widget.manager = None;
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "grid" => {
            if args.len() == option_start {
                app.last_error = None;
                alloc_int_tuple2_bits(py, 0, 0, "failed to allocate tixForm grid tuple")
            } else {
                app.last_error = None;
                Ok(MoltObject::none().bits())
            }
        }
        "info" => {
            if args.len() == option_start {
                app.last_error = None;
                option_map_to_tuple(py, &widget.place_options, "failed to allocate tixForm info")
            } else if args.len() == option_start + 1 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[option_start], "tixForm option")?;
                app.last_error = None;
                option_map_query_or_empty(py, &widget.place_options, &option_name)
            } else {
                Err(app_tcl_error_locked(
                    py,
                    app,
                    "tixForm info expects an optional option name",
                ))
            }
        }
        "slaves" => {
            let mut slaves: Vec<String> = app
                .widgets
                .iter()
                .filter(|(_, child)| child.manager.as_deref() == Some("place"))
                .map(|(path, _)| path.clone())
                .collect();
            slaves.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, slaves.as_slice(), "failed to allocate tixForm slaves")
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad tixForm option \"{subcommand}\""),
        )),
    }
}

fn handle_tix_set_silent_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tixSetSilent expects widget path and value",
        ));
    }
    let _widget_path = get_string_arg(py, handle, args[1], "tixSetSilent widget path")?;
    let _value = get_text_arg(py, handle, args[2], "tixSetSilent value")?;
    clear_last_error(handle);
    Ok(MoltObject::none().bits())
}

fn handle_option_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "option expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "option subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "add" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "option add expects pattern and value",
                ));
            }
            let pattern = get_string_arg(py, handle, args[2], "option pattern")?;
            value_map_set_bits(py, &mut app.option_db, pattern, args[3]);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "clear" => {
            clear_value_map_refs(py, &mut app.option_db);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "option get expects name and class",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "option name")?;
            let class_name = get_string_arg(py, handle, args[3], "option class")?;
            if let Some(bits) = app.option_db.get(&name).copied() {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(bits);
            }
            if let Some(bits) = app.option_db.get(&class_name).copied() {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(bits);
            }
            app.last_error = None;
            alloc_empty_string_bits(py)
        }
        "readfile" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad option option \"{subcommand}\": must be add, clear, get, or readfile"),
        )),
    }
}

fn handle_send_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "send expects an interpreter and script",
        ));
    }
    clear_last_error(handle);
    alloc_empty_string_bits(py)
}

fn handle_tk_global_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.is_empty() {
        return Err(raise_tcl_for_handle(py, handle, "empty tk global command"));
    }
    let command = get_string_arg(py, handle, args[0], "tk global command")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match command.as_str() {
        "tk_strictMotif" => {
            if args.len() == 1 {
                app.last_error = None;
                return Ok(MoltObject::from_bool(app.strict_motif).bits());
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "tk_strictMotif expects optional boolean",
                ));
            }
            app.strict_motif = parse_bool_arg(py, handle, args[1], "tk_strictMotif value")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "tk_bisque" | "tk_setPalette" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("unknown tk command \"{command}\""),
        )),
    }
}

fn command_is_image_instance(py: &PyToken<'_>, handle: i64, command: &str) -> Result<bool, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    Ok(app.images.contains_key(command))
}

fn handle_rename_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "rename expects exactly old/new command names",
        ));
    }
    let old_name = get_string_arg(py, handle, args[1], "rename old command name")?;
    let new_name = get_string_arg(py, handle, args[2], "rename new command name")?;

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(callback_bits) = app.callbacks.remove(&old_name) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("invalid command name \"{old_name}\""),
        ));
    };
    if new_name.is_empty() {
        dec_ref_bits(py, callback_bits);
        app.last_error = None;
        return Ok(MoltObject::none().bits());
    }
    if let Some(old_bits) = app.callbacks.insert(new_name, callback_bits) {
        dec_ref_bits(py, old_bits);
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn handle_widget_create_command(
    py: &PyToken<'_>,
    handle: i64,
    widget_command: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget creation requires a widget path",
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "widget path")?;
    if !widget_path.starts_with('.') {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget path must start with '.'",
        ));
    }
    if !(args.len() - 2).is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget configure options must be key/value pairs",
        ));
    }
    let mut option_names = Vec::with_capacity((args.len() - 2) / 2);
    for idx in (2..args.len()).step_by(2) {
        option_names.push(get_string_arg(py, handle, args[idx], "widget option name")?);
    }
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if let Some(old_widget) = app.widgets.remove(&widget_path) {
        clear_widget_refs(py, old_widget);
    }
    let mut options = HashMap::new();
    for (idx, option_name) in option_names.into_iter().enumerate() {
        let value_bits = args[3 + idx * 2];
        inc_ref_bits(py, value_bits);
        if let Some(old_bits) = options.insert(option_name, value_bits) {
            dec_ref_bits(py, old_bits);
        }
    }
    app.widgets.insert(
        widget_path.clone(),
        TkWidgetState {
            widget_command: widget_command.to_string(),
            options,
            treeview: (widget_command == "ttk::treeview").then(TkTreeviewState::default),
            ..TkWidgetState::default()
        },
    );
    app.last_error = None;
    drop(registry);
    alloc_string_bits(py, &widget_path)
}

fn is_widget_constructor_command(command: &str) -> bool {
    matches!(
        command,
        "button"
            | "canvas"
            | "checkbutton"
            | "entry"
            | "frame"
            | "label"
            | "labelframe"
            | "listbox"
            | "menu"
            | "menubutton"
            | "message"
            | "panedwindow"
            | "radiobutton"
            | "scale"
            | "scrollbar"
            | "spinbox"
            | "text"
            | "toplevel"
            | "ttk::widget"
            | "ttk::button"
            | "ttk::checkbutton"
            | "ttk::combobox"
            | "ttk::entry"
            | "ttk::frame"
            | "ttk::label"
            | "ttk::labelframe"
            | "ttk::menubutton"
            | "ttk::notebook"
            | "ttk::panedwindow"
            | "ttk::progressbar"
            | "ttk::radiobutton"
            | "ttk::scale"
            | "ttk::scrollbar"
            | "ttk::separator"
            | "ttk::sizegrip"
            | "ttk::spinbox"
            | "ttk::treeview"
            | "tixBalloon"
            | "tixButtonBox"
            | "tixCObjView"
            | "tixCheckList"
            | "tixComboBox"
            | "tixControl"
            | "tixDialogShell"
            | "tixDirList"
            | "tixDirSelectBox"
            | "tixDirSelectDialog"
            | "tixDirTree"
            | "tixExFileSelectBox"
            | "tixExFileSelectDialog"
            | "tixFileEntry"
            | "tixFileSelectBox"
            | "tixFileSelectDialog"
            | "tixForm"
            | "tixGrid"
            | "tixHList"
            | "tixItemizedWidget"
            | "tixLabelEntry"
            | "tixLabelFrame"
            | "tixListNoteBook"
            | "tixMainWindow"
            | "tixMeter"
            | "tixNoteBook"
            | "tixNoteBookFrame"
            | "tixOptionMenu"
            | "tixPanedWindow"
            | "tixPopupMenu"
            | "tixResizeHandle"
            | "tixScrolledGrid"
            | "tixScrolledHList"
            | "tixScrolledListBox"
            | "tixScrolledTList"
            | "tixScrolledText"
            | "tixScrolledWindow"
            | "tixSelect"
            | "tixShell"
            | "tixStdButtonBox"
            | "tixTList"
            | "tixTree"
    )
}

fn handle_ttk_style_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "ttk::style requires a subcommand",
        ));
    }
    let style_subcommand = get_string_arg(py, handle, args[1], "ttk::style subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let style_state = &mut app.ttk_style;

    match style_subcommand.as_str() {
        "configure" | "map" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style configure/map expects a style name",
                ));
            }
            let style_name = get_string_arg(py, handle, args[2], "ttk style name")?;
            let style_options = if style_subcommand == "configure" {
                style_state.configure.entry(style_name).or_default()
            } else {
                style_state.style_map.entry(style_name).or_default()
            };
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    style_options,
                    "failed to allocate ttk style option tuple",
                );
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "ttk style option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, style_options, &option_name);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "ttk::style options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, style_options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "lookup" => {
            if args.len() < 4 || args.len() > 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style lookup expects style, option, optional state, optional default",
                ));
            }
            let style_name = get_string_arg(py, handle, args[2], "ttk style name")?;
            let option_name =
                parse_widget_option_name_arg(py, handle, args[3], "ttk style option name")?;
            if let Some(value_bits) = style_state
                .style_map
                .get(&style_name)
                .and_then(|options| options.get(&option_name).copied())
                .or_else(|| {
                    style_state
                        .configure
                        .get(&style_name)
                        .and_then(|options| options.get(&option_name).copied())
                })
            {
                inc_ref_bits(py, value_bits);
                app.last_error = None;
                return Ok(value_bits);
            }
            if args.len() >= 6 {
                let default_bits = args[5];
                inc_ref_bits(py, default_bits);
                app.last_error = None;
                return Ok(default_bits);
            }
            app.last_error = None;
            alloc_string_bits(py, "")
        }
        "layout" => {
            if args.len() < 3 || args.len() > 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style layout expects style and optional layout spec",
                ));
            }
            let style_name = get_string_arg(py, handle, args[2], "ttk style name")?;
            if args.len() == 3 {
                if let Some(layout_bits) = style_state.layouts.get(&style_name).copied() {
                    inc_ref_bits(py, layout_bits);
                    app.last_error = None;
                    return Ok(layout_bits);
                }
                app.last_error = None;
                return alloc_string_bits(py, "");
            }
            let layout_bits = args[3];
            inc_ref_bits(py, layout_bits);
            if let Some(old_bits) = style_state.layouts.insert(style_name, layout_bits) {
                dec_ref_bits(py, old_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "element" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style element requires an operation",
                ));
            }
            let element_subcommand = get_string_arg(py, handle, args[2], "ttk style element op")?;
            match element_subcommand.as_str() {
                "create" => {
                    if args.len() < 5 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style element create expects element and type",
                        ));
                    }
                    let element_name =
                        get_string_arg(py, handle, args[3], "ttk style element name")?;
                    style_state.elements.insert(element_name.clone());
                    let mut option_names = Vec::new();
                    let mut idx = 5;
                    while idx < args.len() {
                        let Some(name) = string_obj_to_owned(obj_from_bits(args[idx])) else {
                            idx += 1;
                            continue;
                        };
                        if !name.starts_with('-') {
                            idx += 1;
                            continue;
                        }
                        option_names.push(name);
                        idx += 2;
                    }
                    style_state
                        .element_options
                        .insert(element_name, option_names);
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "names" => {
                    if args.len() != 3 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style element names expects no extra arguments",
                        ));
                    }
                    app.last_error = None;
                    set_to_sorted_tuple(
                        py,
                        &style_state.elements,
                        "failed to allocate ttk style element tuple",
                    )
                }
                "options" => {
                    if args.len() != 4 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style element options expects an element name",
                        ));
                    }
                    let element_name =
                        get_string_arg(py, handle, args[3], "ttk style element name")?;
                    let option_names = style_state
                        .element_options
                        .get(&element_name)
                        .cloned()
                        .unwrap_or_default();
                    app.last_error = None;
                    alloc_tuple_from_strings(
                        py,
                        option_names.as_slice(),
                        "failed to allocate ttk style element option tuple",
                    )
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!(
                        "bad ttk::style element option \"{element_subcommand}\": must be create, names, or options"
                    ),
                )),
            }
        }
        "theme" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style theme requires an operation",
                ));
            }
            let theme_subcommand = get_string_arg(py, handle, args[2], "ttk style theme op")?;
            match theme_subcommand.as_str() {
                "create" => {
                    if args.len() < 4 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme create expects a theme name",
                        ));
                    }
                    let theme_name = get_string_arg(py, handle, args[3], "ttk theme name")?;
                    style_state.themes.insert(theme_name.clone());
                    if style_state.current_theme.is_none() {
                        style_state.current_theme = Some(theme_name);
                    }
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "settings" => {
                    if args.len() != 5 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme settings expects theme and settings",
                        ));
                    }
                    let theme_name = get_string_arg(py, handle, args[3], "ttk theme name")?;
                    style_state.themes.insert(theme_name);
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "names" => {
                    if args.len() != 3 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme names expects no extra arguments",
                        ));
                    }
                    app.last_error = None;
                    set_to_sorted_tuple(
                        py,
                        &style_state.themes,
                        "failed to allocate ttk style theme tuple",
                    )
                }
                "use" => {
                    if args.len() == 3 {
                        app.last_error = None;
                        return if let Some(current) = style_state.current_theme.as_deref() {
                            alloc_string_bits(py, current)
                        } else {
                            alloc_string_bits(py, "")
                        };
                    }
                    if args.len() != 4 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme use expects optional theme name",
                        ));
                    }
                    let theme_name = get_string_arg(py, handle, args[3], "ttk theme name")?;
                    style_state.themes.insert(theme_name.clone());
                    style_state.current_theme = Some(theme_name);
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!(
                        "bad ttk::style theme option \"{theme_subcommand}\": must be create, names, settings, or use"
                    ),
                )),
            }
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!(
                "bad ttk::style option \"{style_subcommand}\": must be configure, element, layout, lookup, map, or theme"
            ),
        )),
    }
}

fn handle_ttk_notebook_enable_traversal(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "ttk::notebook::enableTraversal expects a notebook widget path",
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "notebook widget path")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get(&widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    if widget.widget_command != "ttk::notebook" {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("widget \"{widget_path}\" is not a ttk::notebook"),
        ));
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn handle_treeview_widget_path_command(
    py: &PyToken<'_>,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    let Some(treeview) = widget.treeview.as_mut() else {
        return Ok(None);
    };
    if treeview_subcommand_is_noop_generic_fallback(subcommand) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            unknown_widget_subcommand_message(widget_path, subcommand),
        ));
    }

    match subcommand {
        "bbox" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "bbox expects item and optional column",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item")?;
            if !treeview.items.contains_key(&item_id) {
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            let visible = treeview_visible_items(treeview);
            let Some(row_index) = visible.iter().position(|candidate| candidate == &item_id) else {
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            };
            let x = if args.len() == 4 {
                let column = get_string_arg(py, handle, args[3], "treeview bbox column")?;
                let Some(offset) = parse_treeview_column_offset(&column) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("invalid column index \"{column}\""),
                    ));
                };
                offset
            } else {
                0
            };
            let y = (row_index as i64) * 20;
            let bbox = vec![
                x.to_string(),
                y.to_string(),
                "120".to_string(),
                "20".to_string(),
            ];
            app.last_error = None;
            return alloc_tuple_from_strings(py, &bbox, "failed to allocate treeview bbox")
                .map(Some);
        }
        "children" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "children expects item and optional replacement children",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item")?;
            if args.len() == 3 {
                let children = if item_id.is_empty() {
                    treeview.root_children.clone()
                } else {
                    let Some(item) = treeview.items.get(&item_id) else {
                        return Err(raise_tcl_for_handle(
                            py,
                            handle,
                            format!("item \"{item_id}\" not found"),
                        ));
                    };
                    item.children.clone()
                };
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    &children,
                    "failed to allocate treeview children tuple",
                )
                .map(Some);
            }

            let replacement = parse_treeview_item_list_arg(
                py,
                handle,
                args[3],
                "treeview replacement child item",
            )?;
            let mut replacement_seen = HashSet::new();
            for child in &replacement {
                if !treeview.items.contains_key(child) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" not found"),
                    ));
                }
                if !replacement_seen.insert(child.clone()) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" appears more than once"),
                    ));
                }
                if !item_id.is_empty() && child == &item_id {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" cannot be its own child"),
                    ));
                }
                if !item_id.is_empty() && treeview_item_is_descendant_of(treeview, &item_id, child)
                {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" is an ancestor of \"{item_id}\""),
                    ));
                }
            }

            let old_children = if item_id.is_empty() {
                std::mem::take(&mut treeview.root_children)
            } else {
                let Some(parent) = treeview.items.get_mut(&item_id) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{item_id}\" not found"),
                    ));
                };
                std::mem::take(&mut parent.children)
            };
            for child in old_children {
                if let Some(item) = treeview.items.get_mut(&child) {
                    item.parent.clear();
                }
            }
            for child in &replacement {
                treeview_remove_from_parent(treeview, child);
                if let Some(item) = treeview.items.get_mut(child) {
                    item.parent = item_id.clone();
                }
            }
            if item_id.is_empty() {
                treeview.root_children = replacement;
            } else if let Some(parent) = treeview.items.get_mut(&item_id) {
                parent.children = replacement;
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "column" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "column expects a column identifier",
                ));
            }
            let column = get_string_arg(py, handle, args[2], "treeview column")?;
            let options = treeview.columns.entry(column).or_default();
            if args.len() == 4 {
                let opt = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[3],
                    "treeview column option",
                )?);
                if !option_allowed(opt.as_str(), TREEVIEW_COLUMN_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{opt}\""),
                    ));
                }
                let bits = options
                    .get(&opt)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "column configure expects key/value pairs",
                ));
            }
            for idx in (3..args.len()).step_by(2) {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview column option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_COLUMN_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                value_map_set_bits(py, options, option, args[idx + 1]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "delete" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "delete expects one or more item ids",
                ));
            }
            let mut item_ids = Vec::with_capacity(args.len() - 2);
            for &item_bits in &args[2..] {
                let item_id = get_string_arg(py, handle, item_bits, "treeview item id")?;
                if !treeview.items.contains_key(&item_id) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{item_id}\" not found"),
                    ));
                }
                item_ids.push(item_id);
            }
            for item_id in item_ids {
                treeview_remove_from_parent(treeview, &item_id);
                treeview_remove_item(py, treeview, &item_id);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "detach" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "detach expects one or more item ids",
                ));
            }
            let mut item_ids = Vec::with_capacity(args.len() - 2);
            for &item_bits in &args[2..] {
                let item_id = get_string_arg(py, handle, item_bits, "treeview item id")?;
                if !treeview.items.contains_key(&item_id) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{item_id}\" not found"),
                    ));
                }
                item_ids.push(item_id);
            }
            for item_id in item_ids {
                treeview_remove_from_parent(treeview, &item_id);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "exists" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "exists expects exactly one item id",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            app.last_error = None;
            return Ok(Some(
                MoltObject::from_bool(treeview.items.contains_key(&item_id)).bits(),
            ));
        }
        "focus" => {
            if args.len() == 2 {
                let value = treeview.focus.clone().unwrap_or_default();
                app.last_error = None;
                return alloc_string_bits(py, &value).map(Some);
            }
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "focus expects zero or one item id",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            if !item_id.is_empty() && !treeview.items.contains_key(&item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            }
            treeview.focus = if item_id.is_empty() {
                None
            } else {
                Some(item_id)
            };
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "heading" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "heading expects a column identifier",
                ));
            }
            let column = get_string_arg(py, handle, args[2], "treeview heading column")?;
            let options = treeview.headings.entry(column).or_default();
            if args.len() == 4 {
                let opt = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[3],
                    "treeview heading option",
                )?);
                if !option_allowed(opt.as_str(), TREEVIEW_HEADING_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{opt}\""),
                    ));
                }
                let bits = options
                    .get(&opt)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "heading configure expects key/value pairs",
                ));
            }
            for idx in (3..args.len()).step_by(2) {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview heading option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_HEADING_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                value_map_set_bits(py, options, option, args[idx + 1]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "identify" => {
            if args.len() != 5 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "identify expects component, x, y",
                ));
            }
            let component = get_string_arg(py, handle, args[2], "treeview identify component")?;
            let x = parse_i64_arg(py, handle, args[3], "treeview identify x")?;
            let y = parse_i64_arg(py, handle, args[4], "treeview identify y")?;
            let hit_item = treeview_hit_item_by_y(treeview, y);
            let result = match component.as_str() {
                "row" | "item" => hit_item.clone().unwrap_or_default(),
                "column" => {
                    if x < 0 {
                        String::new()
                    } else {
                        format!("#{}", x / 120)
                    }
                }
                "region" => {
                    if y < 0 {
                        "heading".to_string()
                    } else if hit_item.is_some() {
                        "cell".to_string()
                    } else {
                        String::new()
                    }
                }
                "element" => {
                    if hit_item.is_some() {
                        "text".to_string()
                    } else {
                        String::new()
                    }
                }
                _ => {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!(
                            "bad identify component \"{component}\": must be column, element, item, region, or row"
                        ),
                    ));
                }
            };
            app.last_error = None;
            return alloc_string_bits(py, &result).map(Some);
        }
        "index" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(py, handle, "index expects an item id"));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            let siblings = if item.parent.is_empty() {
                &treeview.root_children
            } else {
                let Some(parent) = treeview.items.get(&item.parent) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("parent \"{}\" not found", item.parent),
                    ));
                };
                &parent.children
            };
            let position = siblings
                .iter()
                .position(|candidate| candidate == &item_id)
                .unwrap_or(0) as i64;
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(position).bits()));
        }
        "insert" => {
            if args.len() < 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "insert expects parent and index",
                ));
            }
            let parent = get_string_arg(py, handle, args[2], "treeview parent item")?;
            let index_spec = get_string_arg(py, handle, args[3], "treeview insert index")?;
            if !parent.is_empty() && !treeview.items.contains_key(&parent) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{parent}\" not found"),
                ));
            }
            if !(args.len() - 4).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "insert options must be key/value pairs",
                ));
            }
            let mut item_id: Option<String> = None;
            let mut item_options: HashMap<String, u64> = HashMap::new();
            for idx in (4..args.len()).step_by(2) {
                let option_name = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview insert option name",
                )?);
                let value_bits = args[idx + 1];
                if option_name == "-id" {
                    item_id = Some(get_string_arg(
                        py,
                        handle,
                        value_bits,
                        "treeview inserted item id",
                    )?);
                    continue;
                }
                if !option_allowed(option_name.as_str(), TREEVIEW_ITEM_OPTIONS) {
                    clear_value_map_refs(py, &mut item_options);
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
                value_map_set_bits(py, &mut item_options, option_name, value_bits);
            }
            let resolved_item_id = if let Some(value) = item_id {
                value
            } else {
                treeview.next_auto_id = treeview.next_auto_id.saturating_add(1);
                format!("I{}", treeview.next_auto_id)
            };
            if treeview.items.contains_key(&resolved_item_id) {
                clear_value_map_refs(py, &mut item_options);
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{resolved_item_id}\" already exists"),
                ));
            }
            let sibling_len = if parent.is_empty() {
                treeview.root_children.len()
            } else {
                treeview
                    .items
                    .get(&parent)
                    .map(|item| item.children.len())
                    .unwrap_or(0)
            };
            let Some(index) = parse_treeview_index_strict(&index_spec, sibling_len) else {
                clear_value_map_refs(py, &mut item_options);
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("treeview index \"{index_spec}\" must be an integer or end"),
                ));
            };
            treeview_insert_into_parent(treeview, &parent, index, resolved_item_id.clone());
            treeview.items.insert(
                resolved_item_id.clone(),
                TkTreeviewItem {
                    parent,
                    children: Vec::new(),
                    options: item_options,
                    values: HashMap::new(),
                },
            );
            app.last_error = None;
            return alloc_string_bits(py, &resolved_item_id).map(Some);
        }
        "item" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(py, handle, "item expects an item id"));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get_mut(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            if args.len() == 3 {
                let mut keys: Vec<String> = item.options.keys().cloned().collect();
                keys.sort_unstable();
                let mut tuple_elems = Vec::with_capacity(keys.len() * 2);
                for key in keys {
                    let key_bits = alloc_string_bits(py, &key)?;
                    tuple_elems.push(key_bits);
                    if let Some(bits) = item.options.get(&key).copied() {
                        tuple_elems.push(bits);
                    } else {
                        tuple_elems.push(MoltObject::none().bits());
                    }
                }
                let out = alloc_tuple_bits(
                    py,
                    tuple_elems.as_slice(),
                    "failed to allocate treeview item tuple",
                );
                for bits in tuple_elems {
                    dec_ref_bits(py, bits);
                }
                app.last_error = None;
                return out.map(Some);
            }
            if args.len() == 4 {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[3],
                    "treeview item option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_ITEM_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                let bits = item
                    .options
                    .get(&option)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "item configure expects key/value pairs",
                ));
            }
            for idx in (3..args.len()).step_by(2) {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview item option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_ITEM_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                value_map_set_bits(py, &mut item.options, option, args[idx + 1]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "move" => {
            if args.len() != 5 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "move expects item, parent, index",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let parent = get_string_arg(py, handle, args[3], "treeview parent item")?;
            let index_spec = get_string_arg(py, handle, args[4], "treeview index")?;
            if !treeview.items.contains_key(&item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            }
            if !parent.is_empty() && !treeview.items.contains_key(&parent) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{parent}\" not found"),
                ));
            }
            if !parent.is_empty() && parent == item_id {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" cannot be moved under itself"),
                ));
            }
            if !parent.is_empty() && treeview_item_is_descendant_of(treeview, &parent, &item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" cannot be moved under its descendant \"{parent}\""),
                ));
            }
            treeview_remove_from_parent(treeview, &item_id);
            let sibling_len = if parent.is_empty() {
                treeview.root_children.len()
            } else {
                treeview
                    .items
                    .get(&parent)
                    .map(|item| item.children.len())
                    .unwrap_or(0)
            };
            let Some(index) = parse_treeview_index_strict(&index_spec, sibling_len) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("treeview index \"{index_spec}\" must be an integer or end"),
                ));
            };
            if let Some(item) = treeview.items.get_mut(&item_id) {
                item.parent = parent.clone();
            }
            treeview_insert_into_parent(treeview, &parent, index, item_id);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "next" | "prev" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("{subcommand} expects an item id"),
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            let siblings = if item.parent.is_empty() {
                &treeview.root_children
            } else if let Some(parent) = treeview.items.get(&item.parent) {
                &parent.children
            } else {
                &treeview.root_children
            };
            let mut result = String::new();
            if let Some(position) = siblings.iter().position(|candidate| candidate == &item_id) {
                let neighbor = if subcommand == "next" {
                    siblings.get(position + 1)
                } else if position > 0 {
                    siblings.get(position - 1)
                } else {
                    None
                };
                if let Some(item) = neighbor {
                    result = item.clone();
                }
            }
            app.last_error = None;
            return alloc_string_bits(py, &result).map(Some);
        }
        "parent" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "parent expects an item id",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            app.last_error = None;
            return alloc_string_bits(py, &item.parent).map(Some);
        }
        "see" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(py, handle, "see expects an item id"));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            if !treeview.items.contains_key(&item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "selection" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    &treeview.selection,
                    "failed to allocate treeview selection tuple",
                )
                .map(Some);
            }
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "selection expects operation and optional item ids",
                ));
            }
            let op = get_string_arg(py, handle, args[2], "treeview selection operation")?;
            let mut items = Vec::new();
            if args.len() > 3 {
                items.reserve(args.len() - 3);
                for &item_bits in &args[3..] {
                    items.push(get_string_arg(
                        py,
                        handle,
                        item_bits,
                        "treeview selection item",
                    )?);
                }
            }
            if let Some(missing_item) = first_missing_treeview_item(treeview, &items) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{missing_item}\" not found"),
                ));
            }
            match op.as_str() {
                "set" => {
                    treeview.selection.clear();
                    let mut selected: HashSet<String> = HashSet::with_capacity(items.len());
                    for item in items {
                        if selected.insert(item.clone()) {
                            treeview.selection.push(item);
                        }
                    }
                }
                "add" => {
                    let mut selected: HashSet<String> =
                        treeview.selection.iter().cloned().collect();
                    for item in items {
                        if selected.insert(item.clone()) {
                            treeview.selection.push(item);
                        }
                    }
                }
                "remove" => {
                    if !items.is_empty() {
                        let remove_set: HashSet<String> = items.into_iter().collect();
                        treeview
                            .selection
                            .retain(|selected| !remove_set.contains(selected));
                    }
                }
                "toggle" => {
                    let mut selected: HashSet<String> =
                        treeview.selection.iter().cloned().collect();
                    let mut remove_set: HashSet<String> = HashSet::new();
                    let mut add_items: Vec<String> = Vec::new();
                    for item in items {
                        if selected.remove(&item) {
                            remove_set.insert(item);
                        } else {
                            selected.insert(item.clone());
                            add_items.push(item);
                        }
                    }
                    if !remove_set.is_empty() {
                        treeview
                            .selection
                            .retain(|selected| !remove_set.contains(selected));
                    }
                    treeview.selection.extend(add_items);
                }
                _ => {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!(
                            "bad selection operation \"{op}\": must be add, remove, set, or toggle"
                        ),
                    ));
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "set" => {
            if args.len() < 3 || args.len() > 5 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "set expects item, optional column, and optional value",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get_mut(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            if args.len() == 3 {
                app.last_error = None;
                return treeview_set_pairs_to_tuple(py, item).map(Some);
            }
            let column = get_string_arg(py, handle, args[3], "treeview column")?;
            if args.len() == 4 {
                let bits = item
                    .values
                    .get(&column)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            value_map_set_bits(py, &mut item.values, column, args[4]);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "tag" => {
            if args.len() < 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tag expects operation and tagname",
                ));
            }
            let tag_op = get_string_arg(py, handle, args[2], "treeview tag operation")?;
            let tagname = get_string_arg(py, handle, args[3], "treeview tag name")?;
            match tag_op.as_str() {
                "bind" => {
                    let tag_state = treeview.tags.entry(tagname).or_default();
                    if args.len() == 4 {
                        let mut sequences: Vec<String> =
                            tag_state.bindings.keys().cloned().collect();
                        sequences.sort_unstable();
                        let sequence_list = sequences.join(" ");
                        app.last_error = None;
                        return alloc_string_bits(py, &sequence_list).map(Some);
                    }
                    if args.len() == 5 {
                        let sequence =
                            get_string_arg(py, handle, args[4], "treeview tag bind sequence")?;
                        let script = tag_state
                            .bindings
                            .get(&sequence)
                            .cloned()
                            .unwrap_or_default();
                        app.last_error = None;
                        return alloc_string_bits(py, &script).map(Some);
                    }
                    if args.len() == 6 {
                        let sequence =
                            get_string_arg(py, handle, args[4], "treeview tag bind sequence")?;
                        let mut script =
                            get_string_arg(py, handle, args[5], "treeview tag bind script")?;
                        if script.starts_with('+') {
                            script = if let Some(previous) = tag_state.bindings.get(&sequence) {
                                if previous.trim().is_empty() {
                                    script
                                } else {
                                    format!("{previous}\n{script}")
                                }
                            } else {
                                script
                            };
                        }
                        if script.is_empty() {
                            tag_state.bindings.remove(&sequence);
                        } else {
                            tag_state.bindings.insert(sequence, script);
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    if args.len() == 7 {
                        let sequence =
                            get_string_arg(py, handle, args[4], "treeview tag bind sequence")?;
                        let command_name =
                            get_string_arg(py, handle, args[6], "treeview tag bind callback id")?;
                        if let Some(existing_script) = tag_state.bindings.get(&sequence).cloned() {
                            let replacement = remove_bind_script_command_invocations(
                                &existing_script,
                                &command_name,
                            );
                            if replacement.is_empty() {
                                tag_state.bindings.remove(&sequence);
                            } else {
                                tag_state.bindings.insert(sequence, replacement);
                            }
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        "tag bind expects tagname, optional sequence, optional script",
                    ));
                }
                "configure" => {
                    let tag_state = treeview.tags.entry(tagname).or_default();
                    if args.len() == 4 {
                        app.last_error = None;
                        return option_map_to_tuple(
                            py,
                            &tag_state.options,
                            "failed to allocate treeview tag option tuple",
                        )
                        .map(Some);
                    }
                    if args.len() == 5 {
                        let option = parse_widget_option_name_arg(
                            py,
                            handle,
                            args[4],
                            "treeview tag configure option",
                        )?;
                        if !option_allowed(option.as_str(), TREEVIEW_TAG_OPTIONS) {
                            return Err(raise_tcl_for_handle(
                                py,
                                handle,
                                format!("unknown option \"{option}\""),
                            ));
                        }
                        let bits = tag_state
                            .options
                            .get(&option)
                            .copied()
                            .unwrap_or_else(|| MoltObject::none().bits());
                        if bits != MoltObject::none().bits() {
                            inc_ref_bits(py, bits);
                            app.last_error = None;
                            return Ok(Some(bits));
                        }
                        app.last_error = None;
                        return alloc_string_bits(py, "").map(Some);
                    }
                    if !(args.len() - 4).is_multiple_of(2) {
                        return Err(raise_tcl_for_handle(
                            py,
                            handle,
                            "tag configure expects key/value pairs",
                        ));
                    }
                    for idx in (4..args.len()).step_by(2) {
                        let option = parse_widget_option_name_arg(
                            py,
                            handle,
                            args[idx],
                            "treeview tag option",
                        )?;
                        if !option_allowed(option.as_str(), TREEVIEW_TAG_OPTIONS) {
                            return Err(raise_tcl_for_handle(
                                py,
                                handle,
                                format!("unknown option \"{option}\""),
                            ));
                        }
                        value_map_set_bits(py, &mut tag_state.options, option, args[idx + 1]);
                    }
                    app.last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "has" => {
                    if args.len() == 4 {
                        let mut item_ids: Vec<String> = treeview
                            .items
                            .iter()
                            .filter_map(|(item_id, item)| {
                                parse_treeview_tags(item)
                                    .iter()
                                    .any(|tag| tag == &tagname)
                                    .then_some(item_id.clone())
                            })
                            .collect();
                        item_ids.sort_unstable();
                        app.last_error = None;
                        return alloc_tuple_from_strings(
                            py,
                            &item_ids,
                            "failed to allocate treeview tag has tuple",
                        )
                        .map(Some);
                    }
                    if args.len() == 5 {
                        let item_id = get_string_arg(py, handle, args[4], "treeview tag has item")?;
                        let has_tag = treeview.items.get(&item_id).is_some_and(|item| {
                            parse_treeview_tags(item).iter().any(|tag| tag == &tagname)
                        });
                        app.last_error = None;
                        return Ok(Some(MoltObject::from_bool(has_tag).bits()));
                    }
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        "tag has expects tagname and optional item",
                    ));
                }
                _ => {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!(
                            "bad treeview tag operation \"{tag_op}\": must be bind, configure, or has"
                        ),
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(None)
}

fn handle_ttk_widget_path_command(
    py: &PyToken<'_>,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    if !widget.widget_command.starts_with("ttk::") {
        return Ok(None);
    }
    let widget_command = widget.widget_command.as_str();
    let is_ttk_entry = widget_command == "ttk::entry";
    let is_ttk_combobox = widget_command == "ttk::combobox";
    let is_ttk_spinbox = widget_command == "ttk::spinbox";
    let is_ttk_progressbar = widget_command == "ttk::progressbar";
    let is_ttk_notebook = widget_command == "ttk::notebook";
    let is_ttk_panedwindow = widget_command == "ttk::panedwindow";
    let supports_ttk_invoke = matches!(
        widget_command,
        "ttk::button" | "ttk::checkbutton" | "ttk::radiobutton" | "ttk::menubutton"
    );
    let supports_ttk_current = is_ttk_combobox || is_ttk_spinbox;
    let supports_ttk_set = is_ttk_combobox || is_ttk_spinbox;
    let supports_ttk_bbox = is_ttk_entry || is_ttk_combobox || is_ttk_spinbox;
    let supports_ttk_validate = is_ttk_entry || is_ttk_combobox || is_ttk_spinbox;

    match subcommand {
        "state" => {
            if args.len() == 2 {
                app.last_error = None;
                return set_to_sorted_tuple(
                    py,
                    &widget.ttk_state,
                    "failed to allocate ttk state tuple",
                )
                .map(Some);
            }
            for &state_bits in &args[2..] {
                let state_spec = get_string_arg(py, handle, state_bits, "ttk state spec")?;
                if state_spec.is_empty() {
                    continue;
                }
                if let Some(removed) = state_spec.strip_prefix('!') {
                    if !removed.is_empty() {
                        widget.ttk_state.remove(removed);
                    }
                    continue;
                }
                widget.ttk_state.insert(state_spec);
            }
            app.last_error = None;
            return set_to_sorted_tuple(
                py,
                &widget.ttk_state,
                "failed to allocate ttk state tuple",
            )
            .map(Some);
        }
        "instate" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "instate expects at least one state specifier",
                ));
            }
            let mut matches_all = true;
            for &state_bits in &args[2..] {
                let state_spec = get_string_arg(py, handle, state_bits, "ttk state spec")?;
                if state_spec.is_empty() {
                    continue;
                }
                let (negated, state_name) = if let Some(raw) = state_spec.strip_prefix('!') {
                    (true, raw)
                } else {
                    (false, state_spec.as_str())
                };
                let has_state = widget.ttk_state.contains(state_name);
                if (negated && has_state) || (!negated && !has_state) {
                    matches_all = false;
                    break;
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::from_bool(matches_all).bits()));
        }
        "identify" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "identify expects x and y coordinates",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "element").map(Some);
        }
        "invoke" => {
            if !supports_ttk_invoke {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "invoke expects no extra arguments",
                ));
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "current" => {
            if !supports_ttk_current {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() == 2 {
                if let Some(bits) = widget.ttk_values.get("-current").copied() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(-1).bits()));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "current expects optional index argument",
                ));
            }
            let Some(index) = to_i64(obj_from_bits(args[2])) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "combobox index must be an integer",
                ));
            };
            value_map_set_bits(
                py,
                &mut widget.ttk_values,
                "-current".to_string(),
                MoltObject::from_int(index).bits(),
            );
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "set" => {
            if !supports_ttk_set {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "set expects a value argument",
                ));
            }
            value_map_set_bits(py, &mut widget.ttk_values, "-value".to_string(), args[2]);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "bbox" => {
            if !supports_ttk_bbox {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "bbox expects an index argument",
                ));
            }
            let bbox = vec![
                "1".to_string(),
                "2".to_string(),
                "3".to_string(),
                "4".to_string(),
            ];
            app.last_error = None;
            return alloc_tuple_from_strings(py, &bbox, "failed to allocate ttk bbox tuple")
                .map(Some);
        }
        "validate" => {
            if !supports_ttk_validate {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "validate expects no extra arguments",
                ));
            }
            app.last_error = None;
            return Ok(Some(MoltObject::from_bool(true).bits()));
        }
        "get" => {
            if is_ttk_entry {
                return Ok(None);
            }
            if !supports_ttk_set {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "get expects no extra arguments",
                ));
            }
            if let Some(bits) = widget.ttk_values.get("-value").copied() {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(Some(bits));
            }
            app.last_error = None;
            return alloc_string_bits(py, "").map(Some);
        }
        "start" => {
            if !is_ttk_progressbar {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 && args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "start expects optional interval argument",
                ));
            }
            widget.ttk_state.insert("running".to_string());
            if args.len() == 3 {
                value_map_set_bits(py, &mut widget.ttk_values, "-interval".to_string(), args[2]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "step" => {
            if !is_ttk_progressbar {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 && args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "step expects optional amount argument",
                ));
            }
            let current = widget
                .ttk_values
                .get("-value")
                .and_then(|bits| {
                    to_f64(obj_from_bits(*bits))
                        .or_else(|| to_i64(obj_from_bits(*bits)).map(|v| v as f64))
                })
                .unwrap_or(0.0);
            let amount = if args.len() == 3 {
                let amount_obj = obj_from_bits(args[2]);
                if let Some(value) = to_f64(amount_obj) {
                    value
                } else if let Some(value) = to_i64(amount_obj) {
                    value as f64
                } else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "progressbar step amount must be numeric",
                    ));
                }
            } else {
                1.0
            };
            value_map_set_bits(
                py,
                &mut widget.ttk_values,
                "-value".to_string(),
                MoltObject::from_float(current + amount).bits(),
            );
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "stop" => {
            if !is_ttk_progressbar {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "stop expects no extra arguments",
                ));
            }
            widget.ttk_state.remove("running");
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        _ => {}
    }

    match subcommand {
        "add" => {
            if !is_ttk_notebook && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "add expects a child widget path",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "ttk child path")?;
            if !widget.ttk_items.iter().any(|existing| existing == &child) {
                widget.ttk_items.push(child.clone());
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "ttk item options")?;
            let allowed_options = if is_ttk_notebook {
                TTK_NOTEBOOK_TAB_OPTIONS
            } else {
                TTK_PANEDWINDOW_PANE_OPTIONS
            };
            for (option_name, _) in &option_pairs {
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
            }
            let item_options = widget.ttk_item_options.entry(child.clone()).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, item_options, option_name, value_bits);
            }
            if is_ttk_notebook
                && !widget.ttk_values.contains_key("-selected")
                && let Ok(child_bits) = alloc_string_bits(py, &child)
            {
                value_map_set_bits(
                    py,
                    &mut widget.ttk_values,
                    "-selected".to_string(),
                    child_bits,
                );
                dec_ref_bits(py, child_bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "insert" => {
            if !is_ttk_notebook && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "insert expects index and child widget path",
                ));
            }
            let index_spec = get_string_arg(py, handle, args[2], "ttk insert index")?;
            let child = get_string_arg(py, handle, args[3], "ttk child path")?;
            widget.ttk_items.retain(|existing| existing != &child);
            let Some(index) = parse_ttk_insert_index_strict(&index_spec, widget.ttk_items.len())
            else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("ttk insert index \"{index_spec}\" must be an integer or end"),
                ));
            };
            widget.ttk_items.insert(index, child.clone());
            let option_pairs = parse_widget_option_pairs(py, handle, args, 4, "ttk item options")?;
            let allowed_options = if is_ttk_notebook {
                TTK_NOTEBOOK_TAB_OPTIONS
            } else {
                TTK_PANEDWINDOW_PANE_OPTIONS
            };
            for (option_name, _) in &option_pairs {
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
            }
            let item_options = widget.ttk_item_options.entry(child.clone()).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, item_options, option_name, value_bits);
            }
            if is_ttk_notebook
                && !widget.ttk_values.contains_key("-selected")
                && let Ok(child_bits) = alloc_string_bits(py, &child)
            {
                value_map_set_bits(
                    py,
                    &mut widget.ttk_values,
                    "-selected".to_string(),
                    child_bits,
                );
                dec_ref_bits(py, child_bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "forget" | "hide" => {
            if subcommand == "hide" && !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if subcommand == "forget" && !is_ttk_notebook && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} expects a child widget path"),
                ));
            }
            let child = get_string_arg(py, handle, args[2], "ttk child path")?;
            if !widget.ttk_items.iter().any(|existing| existing == &child) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} \"{child}\" is not managed by {widget_path}"),
                ));
            }
            widget.ttk_items.retain(|existing| existing != &child);
            if subcommand == "forget"
                && let Some(mut old_options) = widget.ttk_item_options.remove(&child)
            {
                clear_value_map_refs(py, &mut old_options);
            }
            if is_ttk_notebook {
                let selected_child = widget
                    .ttk_values
                    .get("-selected")
                    .and_then(|bits| string_obj_to_owned(obj_from_bits(*bits)));
                if selected_child.as_deref() == Some(child.as_str()) {
                    if let Some(next_selected) = widget.ttk_items.first()
                        && let Ok(bits) = alloc_string_bits(py, next_selected)
                    {
                        value_map_set_bits(
                            py,
                            &mut widget.ttk_values,
                            "-selected".to_string(),
                            bits,
                        );
                        dec_ref_bits(py, bits);
                    } else if let Some(old_bits) = widget.ttk_values.remove("-selected") {
                        dec_ref_bits(py, old_bits);
                    }
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "index" => {
            if !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "index expects a tab identifier",
                ));
            }
            let target = get_string_arg(py, handle, args[2], "ttk tab identifier")?;
            let idx =
                if let Some(position) = widget.ttk_items.iter().position(|item| item == &target) {
                    position as i64
                } else {
                    match parse_notebook_index_strict(&target, widget.ttk_items.len()) {
                        Ok(value) => value,
                        Err(message) => return Err(app_tcl_error_locked(py, app, message)),
                    }
                };
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(idx).bits()));
        }
        "select" => {
            if !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() == 2 {
                if let Some(bits) = widget.ttk_values.get("-selected").copied() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                if let Some(first_child) = widget.ttk_items.first() {
                    app.last_error = None;
                    return alloc_string_bits(py, first_child).map(Some);
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "select expects optional tab identifier",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "ttk tab identifier")?;
            if !widget.ttk_items.iter().any(|existing| existing == &child) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("tab \"{child}\" is not managed by {widget_path}"),
                ));
            }
            if let Ok(bits) = alloc_string_bits(py, &child) {
                value_map_set_bits(py, &mut widget.ttk_values, "-selected".to_string(), bits);
                dec_ref_bits(py, bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "tab" | "pane" => {
            if subcommand == "tab" && !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if subcommand == "pane" && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} expects an item identifier"),
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "ttk item id")?;
            if !widget.ttk_items.iter().any(|existing| existing == &item_id) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} \"{item_id}\" is not managed by {widget_path}"),
                ));
            }
            let allowed_options = if subcommand == "tab" {
                TTK_NOTEBOOK_TAB_OPTIONS
            } else {
                TTK_PANEDWINDOW_PANE_OPTIONS
            };
            let item_options = widget.ttk_item_options.entry(item_id).or_default();
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    item_options,
                    "failed to allocate ttk item option tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "ttk option name")?;
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
                app.last_error = None;
                return option_map_query_or_empty(py, item_options, &option_name).map(Some);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "ttk item options")?;
            for (option_name, _) in &option_pairs {
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
            }
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, item_options, option_name, value_bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "tabs" => {
            if !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "tabs expects no extra arguments",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                widget.ttk_items.as_slice(),
                "failed to allocate ttk tabs tuple",
            )
            .map(Some);
        }
        "panes" => {
            if !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panes expects no extra arguments",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                widget.ttk_items.as_slice(),
                "failed to allocate ttk panes tuple",
            )
            .map(Some);
        }
        "sashpos" => {
            if !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 && args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "sashpos expects index and optional position",
                ));
            }
            let Some(index) = to_i64(obj_from_bits(args[2])) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "sash index must be an integer",
                ));
            };
            if args.len() == 3 {
                let current = widget.ttk_sash_positions.get(&index).copied().unwrap_or(0);
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(current).bits()));
            }
            let Some(position) = to_i64(obj_from_bits(args[3])) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "sash position must be an integer",
                ));
            };
            widget.ttk_sash_positions.insert(index, position);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        _ => {}
    }

    Ok(None)
}

fn alloc_empty_string_bits(py: &PyToken<'_>) -> Result<u64, u64> {
    alloc_string_bits(py, "")
}

fn alloc_empty_tuple_bits(py: &PyToken<'_>) -> Result<u64, u64> {
    alloc_tuple_from_strings(py, &[], "failed to allocate empty tkinter tuple")
}

fn alloc_widget_bbox_bits(py: &PyToken<'_>) -> Result<u64, u64> {
    let values = [
        String::from("0"),
        String::from("0"),
        String::from("0"),
        String::from("0"),
    ];
    alloc_tuple_from_strings(py, &values, "failed to allocate tkinter bbox tuple")
}

fn alloc_widget_coord_bits(py: &PyToken<'_>) -> Result<u64, u64> {
    let values = [String::from("0"), String::from("0")];
    alloc_tuple_from_strings(py, &values, "failed to allocate tkinter coord tuple")
}

fn alloc_widget_view_bits(py: &PyToken<'_>) -> Result<u64, u64> {
    let values = [String::from("0.0"), String::from("1.0")];
    alloc_tuple_from_strings(
        py,
        &values,
        "failed to allocate tkinter view fraction tuple",
    )
}

fn unknown_widget_subcommand_message(widget_path: &str, subcommand: &str) -> String {
    format!("unknown subcommand \"{subcommand}\" for widget \"{widget_path}\"")
}

fn evaluate_index_compare(left: usize, op: &str, right: usize) -> Result<bool, String> {
    match op {
        "<" => Ok(left < right),
        "<=" => Ok(left <= right),
        "==" => Ok(left == right),
        ">=" => Ok(left >= right),
        ">" => Ok(left > right),
        "!=" => Ok(left != right),
        _ => Err(format!(
            "bad comparison operator \"{op}\": must be <, <=, ==, >=, >, or !="
        )),
    }
}

fn clamp_text_widget_indices(widget: &mut TkWidgetState) {
    let max_index = text_char_count(&widget.text_value);
    widget.insert_cursor = widget.insert_cursor.min(max_index);
    for index in widget.text_marks.values_mut() {
        *index = (*index).min(max_index);
    }
}

fn listbox_shift_item_options_for_insert(
    widget: &mut TkWidgetState,
    insert_index: usize,
    inserted_count: usize,
) {
    if inserted_count == 0 || widget.list_item_options.is_empty() {
        return;
    }
    let mut shifted = HashMap::with_capacity(widget.list_item_options.len());
    for (index, options) in widget.list_item_options.drain() {
        let target = if index >= insert_index {
            index.saturating_add(inserted_count)
        } else {
            index
        };
        shifted.insert(target, options);
    }
    widget.list_item_options = shifted;
    if let Some(active_index) = widget.list_active_index
        && active_index >= insert_index
    {
        widget.list_active_index = Some(active_index.saturating_add(inserted_count));
    }
}

fn listbox_reindex_item_options_after_delete(
    py: &PyToken<'_>,
    widget: &mut TkWidgetState,
    first: usize,
    end: usize,
) {
    if first > end {
        return;
    }
    let removed_count = end - first + 1;
    if widget.list_item_options.is_empty() {
        if let Some(active_index) = widget.list_active_index {
            widget.list_active_index = if active_index < first {
                Some(active_index)
            } else if active_index > end {
                Some(active_index - removed_count)
            } else {
                None
            };
        }
        return;
    }
    let mut shifted = HashMap::with_capacity(widget.list_item_options.len());
    for (index, mut options) in widget.list_item_options.drain() {
        if index < first {
            shifted.insert(index, options);
            continue;
        }
        if index > end {
            shifted.insert(index - removed_count, options);
            continue;
        }
        clear_value_map_refs(py, &mut options);
    }
    widget.list_item_options = shifted;
    if let Some(active_index) = widget.list_active_index {
        widget.list_active_index = if active_index < first {
            Some(active_index)
        } else if active_index > end {
            Some(active_index - removed_count)
        } else {
            None
        };
    }
}

fn ensure_text_tag_order_entry(widget: &mut TkWidgetState, tag_name: &str) {
    if !widget
        .text_tag_order
        .iter()
        .any(|existing| existing == tag_name)
    {
        widget.text_tag_order.push(tag_name.to_string());
    }
}

fn normalize_text_tag_ranges(ranges: &mut Vec<(usize, usize)>) {
    ranges.retain(|(start, end)| end > start);
    ranges.sort_unstable_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    if ranges.is_empty() {
        return;
    }
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.iter().copied() {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            if end > last.1 {
                last.1 = end;
            }
            continue;
        }
        merged.push((start, end));
    }
    *ranges = merged;
}

fn handle_generic_widget_path_command(
    py: &PyToken<'_>,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };

    match subcommand {
        "create" => {
            widget.next_item_id += 1;
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(widget.next_item_id).bits()));
        }
        "add" => {
            if widget.widget_command == "menu" {
                if args.len() < 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu add expects item type and optional key/value pairs",
                    ));
                }
                let item_type =
                    get_string_arg(py, handle, args[2], "menu item type")?.to_ascii_lowercase();
                if !menu_item_type_supported(&item_type) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!(
                            "bad menu entry type \"{item_type}\": must be cascade, checkbutton, command, radiobutton, or separator"
                        ),
                    ));
                }
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 3, "menu add options")?;
                let mut entry = TkMenuEntryState {
                    item_type,
                    ..TkMenuEntryState::default()
                };
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, &mut entry.options, option_name, value_bits);
                }
                widget.menu_entries.push(entry);
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            if widget.widget_command == "panedwindow" {
                if args.len() < 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow add expects child path and optional key/value pairs",
                    ));
                }
                let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
                if !widget
                    .pane_children
                    .iter()
                    .any(|existing| existing == &child)
                {
                    widget.pane_children.push(child.clone());
                }
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 3, "panedwindow pane options")?;
                let pane_options = widget.pane_child_options.entry(child).or_default();
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, pane_options, option_name, value_bits);
                }
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
        }
        "insert" => {
            if widget.widget_command == "listbox" {
                if args.len() < 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox insert expects index and one or more elements",
                    ));
                }
                let Some(mut insert_index) =
                    parse_listbox_index_bits(args[2], widget.list_items.len(), true)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox insert index must be an integer or end",
                    ));
                };
                let original_insert_index = insert_index;
                let inserted_count = args.len().saturating_sub(3);
                for value_bits in &args[3..] {
                    inc_ref_bits(py, *value_bits);
                    widget.list_items.insert(insert_index, *value_bits);
                    insert_index += 1;
                }
                if inserted_count > 0 && !widget.list_selection.is_empty() {
                    let mut shifted = HashSet::with_capacity(widget.list_selection.len());
                    for index in widget.list_selection.drain() {
                        if index >= original_insert_index {
                            shifted.insert(index + inserted_count);
                        } else {
                            shifted.insert(index);
                        }
                    }
                    widget.list_selection = shifted;
                }
                listbox_shift_item_options_for_insert(
                    widget,
                    original_insert_index,
                    inserted_count,
                );
            } else if widget.widget_command == "menu" {
                if args.len() < 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu insert expects index, item type, and optional key/value pairs",
                    ));
                }
                let Some(index) = parse_menu_insert_index_bits(args[2], widget.menu_entries.len())
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu insert index must be an integer or end",
                    ));
                };
                let item_type =
                    get_string_arg(py, handle, args[3], "menu item type")?.to_ascii_lowercase();
                if !menu_item_type_supported(&item_type) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!(
                            "bad menu entry type \"{item_type}\": must be cascade, checkbutton, command, radiobutton, or separator"
                        ),
                    ));
                }
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 4, "menu insert options")?;
                let mut entry = TkMenuEntryState {
                    item_type,
                    ..TkMenuEntryState::default()
                };
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, &mut entry.options, option_name, value_bits);
                }
                widget.menu_entries.insert(index, entry);
                if let Some(active_index) = widget.menu_active_index
                    && active_index >= index
                {
                    widget.menu_active_index = Some(active_index + 1);
                }
            } else if widget.widget_command == "panedwindow" {
                if args.len() < 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow insert expects index, child path, and optional key/value pairs",
                    ));
                }
                let Some(index) =
                    parse_simple_end_or_int_index_bits(args[2], widget.pane_children.len())
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow insert index must be an integer or end",
                    ));
                };
                let child = get_string_arg(py, handle, args[3], "panedwindow child path")?;
                widget.pane_children.retain(|existing| existing != &child);
                let insert_index = index.min(widget.pane_children.len());
                widget.pane_children.insert(insert_index, child.clone());
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 4, "panedwindow pane options")?;
                let pane_options = widget.pane_child_options.entry(child).or_default();
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, pane_options, option_name, value_bits);
                }
            } else if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox")
                && args.len() > 3
            {
                let insert_index = if widget.widget_command == "text" {
                    let Some(index) = parse_text_index_bits(args[2], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text insert index must be an integer, end, or line.column",
                        ));
                    };
                    index
                } else {
                    let Some(index) = parse_entry_like_index_bits(
                        args[2],
                        text_char_count(&widget.text_value),
                        widget.insert_cursor,
                        widget.selection_anchor,
                    ) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "entry/spinbox insert index must be an integer, end, insert, or anchor",
                        ));
                    };
                    index
                };
                let value = get_text_arg(py, handle, args[3], "widget insert value")?;
                let byte_index = char_index_to_byte_index(&widget.text_value, insert_index);
                widget.text_value.insert_str(byte_index, &value);
                widget.insert_cursor = insert_index.saturating_add(text_char_count(&value));
                clamp_text_widget_indices(widget);
                if widget.widget_command == "text" {
                    widget.text_edit_modified = true;
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "delete" => {
            if widget.widget_command == "listbox" {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox delete expects first index and optional last index",
                    ));
                }
                let Some(first) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox delete first index must be integer or end",
                    ));
                };
                let last = if args.len() == 4 {
                    let Some(last) =
                        parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                    else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "listbox delete last index must be integer or end",
                        ));
                    };
                    last
                } else {
                    first
                };
                if !widget.list_items.is_empty() && first < widget.list_items.len() {
                    let end = last.min(widget.list_items.len() - 1);
                    if end >= first {
                        let removed_count = end - first + 1;
                        for bits in widget.list_items.drain(first..=end) {
                            dec_ref_bits(py, bits);
                        }
                        if !widget.list_selection.is_empty() {
                            let mut shifted = HashSet::with_capacity(widget.list_selection.len());
                            for index in widget.list_selection.drain() {
                                if index < first {
                                    shifted.insert(index);
                                } else if index > end {
                                    shifted.insert(index - removed_count);
                                }
                            }
                            widget.list_selection = shifted;
                        }
                        listbox_reindex_item_options_after_delete(py, widget, first, end);
                    }
                }
            } else if widget.widget_command == "menu" {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu delete expects first index and optional last index",
                    ));
                }
                let Some(first) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu delete first index must resolve to an existing entry",
                    ));
                };
                let last = if args.len() == 4 {
                    let Some(last) = parse_menu_existing_index_bits(
                        args[3],
                        widget.menu_entries.len(),
                        widget.menu_active_index,
                    ) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "menu delete last index must resolve to an existing entry",
                        ));
                    };
                    last
                } else {
                    first
                };
                let end = last.max(first);
                if end < widget.menu_entries.len() {
                    let removed_count = end - first + 1;
                    for mut entry in widget.menu_entries.drain(first..=end) {
                        clear_value_map_refs(py, &mut entry.options);
                    }
                    if let Some(active_index) = widget.menu_active_index {
                        widget.menu_active_index = if active_index < first {
                            Some(active_index)
                        } else if active_index > end {
                            Some(active_index - removed_count)
                        } else {
                            None
                        };
                    }
                }
            } else if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "delete expects first index and optional last index",
                    ));
                }
                let start = if widget.widget_command == "text" {
                    let Some(start) = parse_text_index_bits(args[2], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text delete first index must be an integer, end, or line.column",
                        ));
                    };
                    start
                } else {
                    let Some(start) = parse_entry_like_index_bits(
                        args[2],
                        text_char_count(&widget.text_value),
                        widget.insert_cursor,
                        widget.selection_anchor,
                    ) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "entry/spinbox delete first index must be an integer, end, insert, or anchor",
                        ));
                    };
                    start
                };
                let end = if args.len() == 4 {
                    if widget.widget_command == "text" {
                        let Some(end) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text delete last index must be an integer, end, or line.column",
                            ));
                        };
                        end
                    } else {
                        let Some(end) = parse_entry_like_index_bits(
                            args[3],
                            text_char_count(&widget.text_value),
                            widget.insert_cursor,
                            widget.selection_anchor,
                        ) else {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "entry/spinbox delete last index must be an integer, end, insert, or anchor",
                            ));
                        };
                        end
                    }
                } else {
                    (start + 1).min(text_char_count(&widget.text_value))
                };
                if end > start {
                    let start_byte = char_index_to_byte_index(&widget.text_value, start);
                    let end_byte = char_index_to_byte_index(&widget.text_value, end);
                    widget.text_value.replace_range(start_byte..end_byte, "");
                }
                widget.insert_cursor = start;
                widget.selection_range = None;
                clamp_text_widget_indices(widget);
                if widget.widget_command == "text" {
                    widget.text_edit_modified = true;
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "get" => {
            if widget.widget_command == "listbox" {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox get expects first index and optional last index",
                    ));
                }
                let Some(first) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox get first index must be integer or end",
                    ));
                };
                if args.len() == 4 {
                    let Some(last) =
                        parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                    else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "listbox get last index must be integer or end",
                        ));
                    };
                    if widget.list_items.is_empty() || first >= widget.list_items.len() {
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    let end = last.min(widget.list_items.len() - 1);
                    if end < first {
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    let range = widget.list_items[first..=end].to_vec();
                    app.last_error = None;
                    return alloc_tuple_bits(
                        py,
                        range.as_slice(),
                        "failed to allocate listbox get range tuple",
                    )
                    .map(Some);
                }
                if let Some(bits) = widget.list_items.get(first).copied() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
            } else if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
                if widget.widget_command == "text" && (args.len() == 3 || args.len() == 4) {
                    let Some(start) = parse_text_index_bits(args[2], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text get start index must be an integer, end, or line.column",
                        ));
                    };
                    let end = if args.len() == 4 {
                        let Some(end) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text get end index must be an integer, end, or line.column",
                            ));
                        };
                        end
                    } else {
                        text_char_count(&widget.text_value)
                    };
                    if end <= start {
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    let start_byte = char_index_to_byte_index(&widget.text_value, start);
                    let end_byte = char_index_to_byte_index(&widget.text_value, end);
                    let slice = widget.text_value[start_byte..end_byte].to_string();
                    app.last_error = None;
                    return alloc_string_bits(py, &slice).map(Some);
                }
                let text = widget.text_value.clone();
                app.last_error = None;
                return alloc_string_bits(py, &text).map(Some);
            }
            app.last_error = None;
            return alloc_empty_string_bits(py).map(Some);
        }
        "size" | "count" => {
            let value = if widget.widget_command == "listbox" {
                widget.list_items.len() as i64
            } else {
                0
            };
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(value).bits()));
        }
        "forget" => {
            if widget.widget_command == "panedwindow" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow forget expects exactly one child path",
                    ));
                }
                let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
                widget.pane_children.retain(|existing| existing != &child);
                if let Some(mut options) = widget.pane_child_options.remove(&child) {
                    clear_value_map_refs(py, &mut options);
                }
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
        }
        "replace" => {
            if widget.widget_command == "text" {
                if args.len() < 5 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text replace expects index1, index2, and replacement text",
                    ));
                }
                let Some(start) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text replace start index must be an integer, end, or line.column",
                    ));
                };
                let Some(end) = parse_text_index_bits(args[3], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text replace end index must be an integer, end, or line.column",
                    ));
                };
                let replacement = get_text_arg(py, handle, args[4], "text replace chars")?;
                let replace_start = start.min(end);
                let replace_end = start.max(end);
                let start_byte = char_index_to_byte_index(&widget.text_value, replace_start);
                let end_byte = char_index_to_byte_index(&widget.text_value, replace_end);
                widget
                    .text_value
                    .replace_range(start_byte..end_byte, replacement.as_str());
                widget.insert_cursor = replace_start.saturating_add(text_char_count(&replacement));
                widget.selection_range = None;
                widget.text_edit_modified = true;
                clamp_text_widget_indices(widget);
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
        }
        "edit" => {
            if widget.widget_command == "text" {
                if args.len() < 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text edit expects a subcommand",
                    ));
                }
                let op = get_string_arg(py, handle, args[2], "text edit subcommand")?;
                match op.as_str() {
                    "modified" => {
                        if args.len() == 3 {
                            app.last_error = None;
                            return Ok(Some(
                                MoltObject::from_bool(widget.text_edit_modified).bits(),
                            ));
                        }
                        if args.len() != 4 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text edit modified expects optional boolean argument",
                            ));
                        }
                        let value = parse_bool_arg(py, handle, args[3], "text edit modified flag")?;
                        widget.text_edit_modified = value;
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "reset" => {
                        if args.len() != 3 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text edit reset expects no additional arguments",
                            ));
                        }
                        widget.text_edit_modified = false;
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "separator" | "undo" | "redo" => {
                        if args.len() != 3 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                format!("text edit {op} expects no additional arguments"),
                            ));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(widget_path, &format!("edit {op}")),
                        ));
                    }
                }
            }
        }
        "dump" => {
            if widget.widget_command == "text" {
                let mut idx = 2usize;
                let mut callback_words: Option<Vec<String>> = None;
                let mut include_text = true;
                while idx < args.len() {
                    let token = get_string_arg(py, handle, args[idx], "text dump option")?;
                    if !token.starts_with('-') {
                        break;
                    }
                    match token.as_str() {
                        "-all" | "-text" | "-mark" | "-tag" | "-image" | "-window" => {}
                        "-command" => {
                            idx += 1;
                            if idx >= args.len() {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "text dump -command expects a command name",
                                ));
                            }
                            let command_name =
                                get_string_arg(py, handle, args[idx], "text dump command name")?;
                            callback_words = Some(parse_command_words(&command_name));
                        }
                        "-elide" => {
                            include_text = true;
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                format!("bad text dump option \"{token}\""),
                            ));
                        }
                    }
                    idx += 1;
                }
                if idx >= args.len() {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text dump expects index1 and optional index2",
                    ));
                }
                let Some(start) = parse_text_index_bits(args[idx], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text dump start index must be an integer, end, or line.column",
                    ));
                };
                idx += 1;
                let end = if idx < args.len() {
                    let Some(end) = parse_text_index_bits(args[idx], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text dump end index must be an integer, end, or line.column",
                        ));
                    };
                    idx += 1;
                    end
                } else {
                    text_char_count(&widget.text_value)
                };
                if idx != args.len() {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text dump received unexpected extra arguments",
                    ));
                }
                let dump_start = start.min(end);
                let dump_end = start.max(end);
                let start_byte = char_index_to_byte_index(&widget.text_value, dump_start);
                let end_byte = char_index_to_byte_index(&widget.text_value, dump_end);
                let slice = widget.text_value[start_byte..end_byte].to_string();
                let mut flat_words: Vec<String> = Vec::new();
                if include_text && !slice.is_empty() {
                    flat_words.push("text".to_string());
                    flat_words.push(slice.clone());
                    flat_words.push(format!("1.{dump_start}"));
                }
                let callback_invocations: Vec<Vec<String>> =
                    if let Some(command_words) = callback_words {
                        let mut calls = Vec::new();
                        for triple in flat_words.chunks_exact(3) {
                            let mut words = command_words.clone();
                            words.push(triple[0].clone());
                            words.push(triple[1].clone());
                            words.push(triple[2].clone());
                            calls.push(words);
                        }
                        calls
                    } else {
                        Vec::new()
                    };
                app.last_error = None;
                if !callback_invocations.is_empty() {
                    drop(registry);
                    run_tk_word_commands(py, handle, &callback_invocations)?;
                    return Ok(Some(MoltObject::none().bits()));
                }
                return alloc_tuple_from_strings(
                    py,
                    flat_words.as_slice(),
                    "failed to allocate text dump tuple",
                )
                .map(Some);
            }
        }
        "subwidget" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "subwidget expects exactly one child name",
                ));
            }
            let child_name = get_string_arg(py, handle, args[2], "subwidget child name")?;
            let child_path = format!("{widget_path}.{child_name}");
            app.last_error = None;
            return alloc_string_bits(py, &child_path).map(Some);
        }
        _ => {}
    }

    match subcommand {
        "bbox" | "coords" => {
            app.last_error = None;
            alloc_widget_bbox_bits(py).map(Some)
        }
        "index" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "index expects exactly one index argument",
                ));
            }
            if widget.widget_command == "listbox" {
                let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox index must be an integer or end",
                    ));
                };
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            if matches!(widget.widget_command.as_str(), "entry" | "spinbox") {
                let Some(index) = parse_entry_like_index_bits(
                    args[2],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "entry/spinbox index must be an integer, end, insert, or anchor",
                    ));
                };
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            if widget.widget_command == "text" {
                let Some(index) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text index must be an integer, end, or line.column",
                    ));
                };
                app.last_error = None;
                return alloc_string_bits(py, &format!("1.{index}")).map(Some);
            }
            if widget.widget_command == "menu" {
                let maybe_index = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                );
                app.last_error = None;
                if let Some(index) = maybe_index {
                    return Ok(Some(MoltObject::from_int(index as i64).bits()));
                }
                return Ok(Some(MoltObject::none().bits()));
            }
            if widget.widget_command == "panedwindow" {
                let token = get_string_arg(py, handle, args[2], "panedwindow index")?;
                if let Some(position) = widget.pane_children.iter().position(|item| item == &token)
                {
                    app.last_error = None;
                    return Ok(Some(MoltObject::from_int(position as i64).bits()));
                }
                if let Some(index) =
                    parse_simple_end_or_int_index(token.as_str(), widget.pane_children.len())
                {
                    app.last_error = None;
                    return Ok(Some(MoltObject::from_int(index as i64).bits()));
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad panedwindow index \"{token}\""),
                ));
            }
            app.last_error = None;
            Ok(Some(MoltObject::from_int(0).bits()))
        }
        "nearest" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "nearest expects exactly one coordinate argument",
                ));
            }
            if widget.widget_command == "listbox" {
                let y = parse_i64_arg(py, handle, args[2], "listbox nearest coordinate")?;
                let index = clamp_index_i64(y, widget.list_items.len().saturating_sub(1));
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::from_int(0).bits()))
        }
        "compare" => {
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "compare expects index1, operator, and index2",
                ));
            }
            let op = get_string_arg(py, handle, args[3], "compare operator")?;
            let (left, right) = if widget.widget_command == "text" {
                let Some(left) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text compare index1 must be an integer, end, or line.column",
                    ));
                };
                let Some(right) = parse_text_index_bits(args[4], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text compare index2 must be an integer, end, or line.column",
                    ));
                };
                (left, right)
            } else if widget.widget_command == "listbox" {
                let Some(left) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox compare index1 must be an integer or end",
                    ));
                };
                let Some(right) = parse_listbox_index_bits(args[4], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox compare index2 must be an integer or end",
                    ));
                };
                (left, right)
            } else {
                let Some(left) = parse_entry_like_index_bits(
                    args[2],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "compare index1 must be an integer, end, insert, or anchor",
                    ));
                };
                let Some(right) = parse_entry_like_index_bits(
                    args[4],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "compare index2 must be an integer, end, insert, or anchor",
                    ));
                };
                (left, right)
            };
            let result = evaluate_index_compare(left, &op, right)
                .map_err(|message| app_tcl_error_locked(py, app, message))?;
            app.last_error = None;
            Ok(Some(MoltObject::from_bool(result).bits()))
        }
        "curselection" => {
            if widget.widget_command == "listbox" {
                let mut indices: Vec<String> = widget
                    .list_selection
                    .iter()
                    .copied()
                    .filter(|idx| *idx < widget.list_items.len())
                    .map(|idx| idx.to_string())
                    .collect();
                indices.sort_unstable_by_key(|value| value.parse::<usize>().unwrap_or(0));
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    indices.as_slice(),
                    "failed to allocate listbox curselection tuple",
                )
                .map(Some);
            }
            app.last_error = None;
            alloc_empty_tuple_bits(py).map(Some)
        }
        "find" | "tabs" | "panes" => {
            if subcommand == "panes" && widget.widget_command == "panedwindow" {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    widget.pane_children.as_slice(),
                    "failed to allocate panedwindow panes tuple",
                )
                .map(Some);
            }
            app.last_error = None;
            alloc_empty_tuple_bits(py).map(Some)
        }
        "subwidgets" => {
            let mut names = Vec::new();
            let prefix = format!("{widget_path}.");
            for path in app.widgets.keys() {
                if let Some(name) = path.strip_prefix(&prefix) {
                    names.push(name.to_string());
                }
            }
            names.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, names.as_slice(), "failed to allocate subwidgets tuple")
                .map(Some)
        }
        "identify" => {
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "type" => {
            if widget.widget_command == "menu" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu type expects exactly one index argument",
                    ));
                }
                let Some(index) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    app.last_error = None;
                    return alloc_empty_string_bits(py).map(Some);
                };
                if let Some(entry) = widget.menu_entries.get(index) {
                    app.last_error = None;
                    return alloc_string_bits(py, &entry.item_type).map(Some);
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "itemcget" => {
            if widget.widget_command == "listbox" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox itemcget expects index and option",
                    ));
                }
                let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox itemcget index must be an integer or end",
                    ));
                };
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "listbox item option name")?;
                if let Some(bits) = widget
                    .list_item_options
                    .get(&index)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "entrycget" => {
            if widget.widget_command == "menu" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu entrycget expects index and option",
                    ));
                }
                let Some(index) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu entrycget index must resolve to an existing entry",
                    ));
                };
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "menu entry option name")?;
                if let Some(bits) = widget
                    .menu_entries
                    .get(index)
                    .and_then(|entry| entry.options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "panecget" => {
            if widget.widget_command == "panedwindow" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow panecget expects child and option",
                    ));
                }
                let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "pane option name")?;
                if let Some(bits) = widget
                    .pane_child_options
                    .get(&child)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "image_cget" | "window_cget" => {
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "bind" => {
            if args.len() < 3 || args.len() > 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "bind expects target, optional sequence, optional script",
                ));
            }
            let target_name = get_string_arg(py, handle, args[2], "bind target")?;
            let bindings = widget.tag_bindings.entry(target_name).or_default();
            if args.len() == 3 {
                let mut sequences: Vec<String> = bindings.keys().cloned().collect();
                sequences.sort_unstable();
                app.last_error = None;
                return alloc_string_bits(py, &sequences.join(" ")).map(Some);
            }
            let sequence = get_string_arg(py, handle, args[3], "bind sequence")?;
            if args.len() == 4 {
                let script = bindings.get(&sequence).cloned().unwrap_or_default();
                app.last_error = None;
                return alloc_string_bits(py, &script).map(Some);
            }
            let mut script = get_string_arg(py, handle, args[4], "bind script")?;
            if args.len() == 6 {
                let command_name = get_string_arg(py, handle, args[5], "bind callback id")?;
                if script.is_empty() {
                    script = bindings.get(&sequence).cloned().unwrap_or_default();
                }
                script = remove_bind_script_command_invocations(&script, &command_name);
            } else if script.starts_with('+') {
                script = if let Some(previous) = bindings.get(&sequence) {
                    if previous.trim().is_empty() {
                        script
                    } else {
                        format!("{previous}\n{script}")
                    }
                } else {
                    script
                };
            }
            if script.is_empty() {
                bindings.remove(&sequence);
            } else {
                bindings.insert(sequence, script);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "xview" | "yview" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_widget_view_bits(py).map(Some);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "xposition" | "yposition" => {
            if widget.widget_command != "menu" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} expects exactly one index argument"),
                ));
            }
            let Some(index) = parse_menu_existing_index_bits(
                args[2],
                widget.menu_entries.len(),
                widget.menu_active_index,
            ) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} index must resolve to an existing menu entry"),
                ));
            };
            let value = if subcommand == "xposition" {
                (index as i64) * 20
            } else {
                (index as i64) * 18
            };
            app.last_error = None;
            Ok(Some(MoltObject::from_int(value).bits()))
        }
        "selection" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "selection subcommand")?;
                if widget.widget_command == "listbox" {
                    match op.as_str() {
                        "anchor" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection anchor expects one index argument",
                                ));
                            }
                            let Some(index) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection anchor index must be an integer or end",
                                ));
                            };
                            widget.selection_anchor = Some(index);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "set" => {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection set expects first and optional last index",
                                ));
                            }
                            let Some(first) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection set first index must be an integer or end",
                                ));
                            };
                            let last = if args.len() == 5 {
                                let Some(last) = parse_listbox_index_bits(
                                    args[4],
                                    widget.list_items.len(),
                                    false,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection set last index must be an integer or end",
                                    ));
                                };
                                last
                            } else {
                                first
                            };
                            if !widget.list_items.is_empty() {
                                let end = last.min(widget.list_items.len() - 1);
                                if end >= first {
                                    for idx in first..=end {
                                        widget.list_selection.insert(idx);
                                    }
                                }
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "clear" => {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection clear expects first and optional last index",
                                ));
                            }
                            let Some(first) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection clear first index must be an integer or end",
                                ));
                            };
                            let last = if args.len() == 5 {
                                let Some(last) = parse_listbox_index_bits(
                                    args[4],
                                    widget.list_items.len(),
                                    false,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection clear last index must be an integer or end",
                                    ));
                                };
                                last
                            } else {
                                first
                            };
                            let end = last.max(first);
                            widget
                                .list_selection
                                .retain(|index| *index < first || *index > end);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "includes" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection includes expects one index argument",
                                ));
                            }
                            let Some(index) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection includes index must be an integer or end",
                                ));
                            };
                            app.last_error = None;
                            return Ok(Some(
                                MoltObject::from_bool(widget.list_selection.contains(&index))
                                    .bits(),
                            ));
                        }
                        "present" => {
                            app.last_error = None;
                            return Ok(Some(
                                MoltObject::from_bool(!widget.list_selection.is_empty()).bits(),
                            ));
                        }
                        "get" => {
                            let mut selected: Vec<usize> =
                                widget.list_selection.iter().copied().collect();
                            selected.sort_unstable();
                            if let Some(index) = selected
                                .into_iter()
                                .find(|idx| *idx < widget.list_items.len())
                                && let Some(bits) = widget.list_items.get(index).copied()
                            {
                                inc_ref_bits(py, bits);
                                app.last_error = None;
                                return Ok(Some(bits));
                            }
                            app.last_error = None;
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                unknown_widget_subcommand_message(
                                    widget_path,
                                    &format!("selection {op}"),
                                ),
                            ));
                        }
                    }
                }
                if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
                    match op.as_str() {
                        "clear" => {
                            widget.selection_range = None;
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "present" => {
                            let present = widget
                                .selection_range
                                .is_some_and(|(start, end)| end > start);
                            app.last_error = None;
                            return Ok(Some(MoltObject::from_bool(present).bits()));
                        }
                        "get" => {
                            if let Some((start, end)) = widget.selection_range
                                && end > start
                            {
                                let start_byte =
                                    char_index_to_byte_index(&widget.text_value, start);
                                let end_byte = char_index_to_byte_index(&widget.text_value, end);
                                let slice = widget.text_value[start_byte..end_byte].to_string();
                                app.last_error = None;
                                return alloc_string_bits(py, &slice).map(Some);
                            }
                            app.last_error = None;
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        "from" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection from expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection from index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection from index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            widget.selection_anchor = Some(index);
                            widget.selection_range = Some((index, index));
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "to" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection to expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection to index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection to index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            let anchor = widget.selection_anchor.unwrap_or(0);
                            let (start, end) = if index >= anchor {
                                (anchor, index)
                            } else {
                                (index, anchor)
                            };
                            widget.selection_range = Some((start, end));
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "range" => {
                            if args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection range expects start and end indices",
                                ));
                            }
                            let start = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range start index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range start index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            let end = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[4], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range end index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[4],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range end index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            widget.selection_range = Some((start.min(end), start.max(end)));
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "includes" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection includes expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection includes index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection includes index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            let includes = widget
                                .selection_range
                                .is_some_and(|(start, end)| index >= start && index < end);
                            app.last_error = None;
                            return Ok(Some(MoltObject::from_bool(includes).bits()));
                        }
                        "adjust" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection adjust expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection adjust index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection adjust index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            if let Some((start, end)) = widget.selection_range {
                                let dist_start = start.abs_diff(index);
                                let dist_end = end.abs_diff(index);
                                widget.selection_range = if dist_start <= dist_end {
                                    Some((index.min(end), index.max(end)))
                                } else {
                                    Some((start.min(index), start.max(index)))
                                };
                            } else {
                                widget.selection_anchor = Some(index);
                                widget.selection_range = Some((index, index));
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "element" => {
                            app.last_error = None;
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                unknown_widget_subcommand_message(
                                    widget_path,
                                    &format!("selection {op}"),
                                ),
                            ));
                        }
                    }
                }
                match op.as_str() {
                    "includes" | "present" => {
                        app.last_error = None;
                        return Ok(Some(MoltObject::from_bool(false).bits()));
                    }
                    "get" => {
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(
                                widget_path,
                                &format!("selection {op}"),
                            ),
                        ));
                    }
                }
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "mark" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "mark subcommand")?;
                if widget.widget_command == "text" {
                    match op.as_str() {
                        "names" => {
                            widget
                                .text_marks
                                .entry("insert".to_string())
                                .or_insert(widget.insert_cursor);
                            widget
                                .text_marks
                                .entry("current".to_string())
                                .or_insert(widget.insert_cursor);
                            let mut names: Vec<String> =
                                widget.text_marks.keys().cloned().collect();
                            names.sort_unstable();
                            app.last_error = None;
                            return alloc_tuple_from_strings(
                                py,
                                names.as_slice(),
                                "failed to allocate text mark names tuple",
                            )
                            .map(Some);
                        }
                        "next" | "previous" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark next/previous expects one index or mark name",
                                ));
                            }
                            widget
                                .text_marks
                                .entry("insert".to_string())
                                .or_insert(widget.insert_cursor);
                            widget
                                .text_marks
                                .entry("current".to_string())
                                .or_insert(widget.insert_cursor);
                            let token = get_string_arg(py, handle, args[3], "mark index or name")?;
                            let mut ordered_marks: Vec<(usize, String)> = widget
                                .text_marks
                                .iter()
                                .map(|(name, index)| (*index, name.clone()))
                                .collect();
                            ordered_marks.sort_unstable_by(|left, right| {
                                left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1))
                            });
                            let selected = if let Some(index) =
                                widget.text_marks.get(&token).copied()
                            {
                                if op == "next" {
                                    ordered_marks
                                        .into_iter()
                                        .find_map(|(mark_index, mark_name)| {
                                            ((mark_index, mark_name.as_str())
                                                > (index, token.as_str()))
                                                .then_some(mark_name)
                                        })
                                } else {
                                    ordered_marks.into_iter().rev().find_map(
                                        |(mark_index, mark_name)| {
                                            ((mark_index, mark_name.as_str())
                                                < (index, token.as_str()))
                                                .then_some(mark_name)
                                        },
                                    )
                                }
                            } else {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "mark next/previous index must be an integer, end, line.column, or mark name",
                                    ));
                                };
                                if op == "next" {
                                    ordered_marks
                                        .into_iter()
                                        .find_map(|(mark_index, mark_name)| {
                                            (mark_index >= index).then_some(mark_name)
                                        })
                                } else {
                                    ordered_marks.into_iter().rev().find_map(
                                        |(mark_index, mark_name)| {
                                            (mark_index <= index).then_some(mark_name)
                                        },
                                    )
                                }
                            };
                            app.last_error = None;
                            if let Some(mark_name) = selected {
                                return alloc_string_bits(py, &mark_name).map(Some);
                            }
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        "set" => {
                            if args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark set expects mark name and index",
                                ));
                            }
                            let mark_name = get_string_arg(py, handle, args[3], "mark name")?;
                            let Some(index) = parse_text_index_bits(args[4], &widget.text_value)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark set index must be an integer, end, or line.column",
                                ));
                            };
                            if mark_name == "insert" {
                                widget.insert_cursor = index;
                            }
                            widget.text_marks.insert(mark_name, index);
                            clamp_text_widget_indices(widget);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "unset" => {
                            for &mark_bits in &args[3..] {
                                let mark_name = get_string_arg(py, handle, mark_bits, "mark name")?;
                                widget.text_marks.remove(&mark_name);
                                widget.text_mark_gravity.remove(&mark_name);
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "gravity" => {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark gravity expects mark name and optional direction",
                                ));
                            }
                            let mark_name = get_string_arg(py, handle, args[3], "mark name")?;
                            if args.len() == 4 {
                                let gravity = widget
                                    .text_mark_gravity
                                    .get(&mark_name)
                                    .cloned()
                                    .unwrap_or_else(|| "right".to_string());
                                app.last_error = None;
                                return alloc_string_bits(py, &gravity).map(Some);
                            }
                            let gravity =
                                get_string_arg(py, handle, args[4], "mark gravity direction")?;
                            let normalized = gravity.to_ascii_lowercase();
                            if normalized != "left" && normalized != "right" {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark gravity must be left or right",
                                ));
                            }
                            widget.text_mark_gravity.insert(mark_name, normalized);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                unknown_widget_subcommand_message(
                                    widget_path,
                                    &format!("mark {op}"),
                                ),
                            ));
                        }
                    }
                }
                match op.as_str() {
                    "names" => {
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "next" | "previous" => {
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(widget_path, &format!("mark {op}")),
                        ));
                    }
                }
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "tag" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "tag subcommand")?;
                match op.as_str() {
                    "bind" => {
                        if args.len() < 4 || args.len() > 7 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "tag bind expects tagname, optional sequence, optional script",
                            ));
                        }
                        let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                        let bindings = widget.tag_bindings.entry(tag_name.clone()).or_default();
                        if args.len() == 4 {
                            let mut sequences: Vec<String> = bindings.keys().cloned().collect();
                            sequences.sort_unstable();
                            app.last_error = None;
                            return alloc_string_bits(py, &sequences.join(" ")).map(Some);
                        }
                        let sequence = get_string_arg(py, handle, args[4], "tag bind sequence")?;
                        if args.len() == 5 {
                            let script = bindings.get(&sequence).cloned().unwrap_or_default();
                            app.last_error = None;
                            return alloc_string_bits(py, &script).map(Some);
                        }
                        let mut script = get_string_arg(py, handle, args[5], "tag bind script")?;
                        if args.len() == 7 {
                            let command_name =
                                get_string_arg(py, handle, args[6], "tag bind callback id")?;
                            if script.is_empty() {
                                script = bindings.get(&sequence).cloned().unwrap_or_default();
                            }
                            script = remove_bind_script_command_invocations(&script, &command_name);
                        } else if script.starts_with('+') {
                            script = if let Some(previous) = bindings.get(&sequence) {
                                if previous.trim().is_empty() {
                                    script
                                } else {
                                    format!("{previous}\n{script}")
                                }
                            } else {
                                script
                            };
                        }
                        if script.is_empty() {
                            bindings.remove(&sequence);
                        } else {
                            bindings.insert(sequence, script);
                            if widget.widget_command == "text" {
                                ensure_text_tag_order_entry(widget, &tag_name);
                            }
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "names" => {
                        if widget.widget_command == "text" {
                            let mut names: HashSet<String> =
                                widget.text_tag_ranges.keys().cloned().collect();
                            names.extend(widget.tag_bindings.keys().cloned());
                            names.extend(widget.text_tag_options.keys().cloned());
                            if args.len() == 5 {
                                let Some(index) =
                                    parse_text_index_bits(args[4], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag names index must be an integer, end, or line.column",
                                    ));
                                };
                                names.retain(|tag_name| {
                                    widget.text_tag_ranges.get(tag_name).is_some_and(|ranges| {
                                        ranges
                                            .iter()
                                            .any(|(start, end)| index >= *start && index < *end)
                                    })
                                });
                            }
                            let mut ordered: Vec<String> = Vec::new();
                            for tag_name in &widget.text_tag_order {
                                if names.remove(tag_name) {
                                    ordered.push(tag_name.clone());
                                }
                            }
                            let mut leftovers: Vec<String> = names.into_iter().collect();
                            leftovers.sort_unstable();
                            ordered.extend(leftovers);
                            app.last_error = None;
                            return alloc_tuple_from_strings(
                                py,
                                ordered.as_slice(),
                                "failed to allocate text tag names tuple",
                            )
                            .map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "ranges" => {
                        if widget.widget_command == "text" {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag ranges expects a tag name",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let mut values: Vec<String> = Vec::new();
                            if let Some(ranges) = widget.text_tag_ranges.get(&tag_name) {
                                for (start, end) in ranges {
                                    values.push(format!("1.{start}"));
                                    values.push(format!("1.{end}"));
                                }
                            }
                            app.last_error = None;
                            return alloc_tuple_from_strings(
                                py,
                                values.as_slice(),
                                "failed to allocate text tag ranges tuple",
                            )
                            .map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "nextrange" | "prevrange" => {
                        if widget.widget_command == "text" {
                            if args.len() != 5 && args.len() != 6 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag nextrange/prevrange expects tagname, start, and optional stop",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let Some(start_index) =
                                parse_text_index_bits(args[4], &widget.text_value)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag range start index must be an integer, end, or line.column",
                                ));
                            };
                            let stop_index = if args.len() == 6 {
                                let Some(stop) = parse_text_index_bits(args[5], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag range stop index must be an integer, end, or line.column",
                                    ));
                                };
                                Some(stop)
                            } else {
                                None
                            };
                            let mut ranges = widget
                                .text_tag_ranges
                                .get(&tag_name)
                                .cloned()
                                .unwrap_or_default();
                            ranges.sort_unstable_by_key(|(start, _end)| *start);
                            let selected = if op == "nextrange" {
                                ranges.into_iter().find(|(start, end)| {
                                    *end > start_index
                                        && stop_index.is_none_or(|stop| *start < stop)
                                })
                            } else {
                                ranges.into_iter().rev().find(|(start, _end)| {
                                    *start < start_index
                                        && stop_index.is_none_or(|stop| *start >= stop)
                                })
                            };
                            if let Some((start, end)) = selected {
                                app.last_error = None;
                                return alloc_tuple_from_strings(
                                    py,
                                    &[format!("1.{start}"), format!("1.{end}")],
                                    "failed to allocate text tag range tuple",
                                )
                                .map(Some);
                            }
                            app.last_error = None;
                            return alloc_empty_tuple_bits(py).map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "cget" => {
                        if widget.widget_command == "text" {
                            if args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag cget expects tagname and option",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let option_name =
                                parse_widget_option_name_arg(py, handle, args[4], "tag option")?;
                            let value = widget
                                .text_tag_options
                                .get(&tag_name)
                                .and_then(|options| options.get(&option_name))
                                .cloned()
                                .unwrap_or_default();
                            app.last_error = None;
                            return alloc_string_bits(py, &value).map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    "add" => {
                        if widget.widget_command == "text" {
                            if args.len() < 6 || (args.len() - 4) % 2 != 0 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag add expects tagname plus one or more start/end index pairs",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let ranges =
                                widget.text_tag_ranges.entry(tag_name.clone()).or_default();
                            let mut idx = 4;
                            while idx + 1 < args.len() {
                                let Some(start) =
                                    parse_text_index_bits(args[idx], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag add start index must be an integer, end, or line.column",
                                    ));
                                };
                                let Some(end) =
                                    parse_text_index_bits(args[idx + 1], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag add end index must be an integer, end, or line.column",
                                    ));
                                };
                                if end > start {
                                    ranges.push((start, end));
                                } else if start > end {
                                    ranges.push((end, start));
                                }
                                idx += 2;
                            }
                            normalize_text_tag_ranges(ranges);
                            ensure_text_tag_order_entry(widget, &tag_name);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "remove" => {
                        if widget.widget_command == "text" {
                            if args.len() != 5 && args.len() != 6 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag remove expects tagname, start, and optional end index",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let Some(start) = parse_text_index_bits(args[4], &widget.text_value)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag remove start index must be an integer, end, or line.column",
                                ));
                            };
                            let end = if args.len() == 6 {
                                let Some(end) = parse_text_index_bits(args[5], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag remove end index must be an integer, end, or line.column",
                                    ));
                                };
                                end
                            } else {
                                (start + 1).min(text_char_count(&widget.text_value))
                            };
                            let (remove_start, remove_end) = (start.min(end), start.max(end));
                            if let Some(ranges) = widget.text_tag_ranges.get_mut(&tag_name) {
                                let mut updated: Vec<(usize, usize)> =
                                    Vec::with_capacity(ranges.len().saturating_mul(2));
                                for (range_start, range_end) in ranges.iter().copied() {
                                    if range_end <= remove_start || range_start >= remove_end {
                                        updated.push((range_start, range_end));
                                        continue;
                                    }
                                    if range_start < remove_start {
                                        updated.push((range_start, remove_start));
                                    }
                                    if range_end > remove_end {
                                        updated.push((remove_end, range_end));
                                    }
                                }
                                *ranges = updated;
                                normalize_text_tag_ranges(ranges);
                                if ranges.is_empty() {
                                    widget.text_tag_ranges.remove(&tag_name);
                                }
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "delete" => {
                        if widget.widget_command == "text" {
                            for &tag_bits in &args[3..] {
                                let tag_name = get_string_arg(py, handle, tag_bits, "tag name")?;
                                widget.text_tag_ranges.remove(&tag_name);
                                widget.tag_bindings.remove(&tag_name);
                                widget.text_tag_options.remove(&tag_name);
                                widget.text_tag_order.retain(|name| name != &tag_name);
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "configure" => {
                        if widget.widget_command == "text" {
                            if args.len() < 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag configure expects tagname",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            if args.len() == 4 {
                                let options = widget.text_tag_options.get(&tag_name);
                                let mut out =
                                    Vec::with_capacity(options.map_or(0, HashMap::len) * 2);
                                let mut ordered: Vec<(&String, &String)> =
                                    options.map(|map| map.iter().collect()).unwrap_or_default();
                                ordered.sort_unstable_by(|left, right| left.0.cmp(right.0));
                                for (key, value) in ordered {
                                    out.push(key.clone());
                                    out.push(value.clone());
                                }
                                app.last_error = None;
                                return alloc_tuple_from_strings(
                                    py,
                                    out.as_slice(),
                                    "failed to allocate text tag configure tuple",
                                )
                                .map(Some);
                            }
                            if args.len() == 5 {
                                let option_name = parse_widget_option_name_arg(
                                    py,
                                    handle,
                                    args[4],
                                    "tag option",
                                )?;
                                let value = widget
                                    .text_tag_options
                                    .get(&tag_name)
                                    .and_then(|options| options.get(&option_name))
                                    .cloned()
                                    .unwrap_or_default();
                                app.last_error = None;
                                return alloc_string_bits(py, &value).map(Some);
                            }
                            if !(args.len() - 4).is_multiple_of(2) {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag configure expects key/value pairs",
                                ));
                            }
                            let options =
                                widget.text_tag_options.entry(tag_name.clone()).or_default();
                            let mut idx = 4;
                            while idx + 1 < args.len() {
                                let option_name = parse_widget_option_name_arg(
                                    py,
                                    handle,
                                    args[idx],
                                    "tag option",
                                )?;
                                let option_value =
                                    get_string_arg(py, handle, args[idx + 1], "tag value")?;
                                options.insert(option_name, option_value);
                                idx += 2;
                            }
                            ensure_text_tag_order_entry(widget, &tag_name);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "lower" | "raise" => {
                        if widget.widget_command == "text" {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag lower/raise expects tagname and optional reference tag",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            ensure_text_tag_order_entry(widget, &tag_name);
                            widget.text_tag_order.retain(|name| name != &tag_name);
                            if args.len() == 4 {
                                if op == "lower" {
                                    widget.text_tag_order.insert(0, tag_name);
                                } else {
                                    widget.text_tag_order.push(tag_name);
                                }
                                app.last_error = None;
                                return Ok(Some(MoltObject::none().bits()));
                            }
                            let reference_name =
                                get_string_arg(py, handle, args[4], "reference tag name")?;
                            let reference_index = widget
                                .text_tag_order
                                .iter()
                                .position(|name| name == &reference_name);
                            match reference_index {
                                Some(index) => {
                                    let insert_index = if op == "lower" {
                                        index
                                    } else {
                                        index.saturating_add(1)
                                    }
                                    .min(widget.text_tag_order.len());
                                    widget.text_tag_order.insert(insert_index, tag_name);
                                }
                                None => {
                                    if op == "lower" {
                                        widget.text_tag_order.insert(0, tag_name);
                                    } else {
                                        widget.text_tag_order.push(tag_name);
                                    }
                                }
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(widget_path, &format!("tag {op}")),
                        ));
                    }
                }
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "proxy" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "proxy subcommand")?;
                if op == "coord" {
                    app.last_error = None;
                    return alloc_widget_coord_bits(py).map(Some);
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, &format!("proxy {op}")),
                ));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "sash" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "sash subcommand")?;
                if op == "coord" {
                    app.last_error = None;
                    return alloc_widget_coord_bits(py).map(Some);
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, &format!("sash {op}")),
                ));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "icursor" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "icursor expects exactly one index argument",
                ));
            }
            let index = if widget.widget_command == "text" {
                let Some(index) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text icursor index must be an integer, end, or line.column",
                    ));
                };
                index
            } else if matches!(widget.widget_command.as_str(), "entry" | "spinbox") {
                let Some(index) = parse_entry_like_index_bits(
                    args[2],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "entry/spinbox icursor index must be an integer, end, insert, or anchor",
                    ));
                };
                index
            } else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "icursor"),
                ));
            };
            widget.insert_cursor = index;
            clamp_text_widget_indices(widget);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "itemconfigure" => {
            if widget.widget_command != "listbox" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "itemconfigure"),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "listbox itemconfigure expects index and optional key/value options",
                ));
            }
            let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
            else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "listbox itemconfigure index must be an integer or end",
                ));
            };
            if widget.list_items.is_empty() || index >= widget.list_items.len() {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("listbox item \"{index}\" is out of range"),
                ));
            }
            if args.len() == 3 {
                let options = widget
                    .list_item_options
                    .get(&index)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate listbox itemconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "listbox item option")?;
                if let Some(bits) = widget
                    .list_item_options
                    .get(&index)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "listbox item options")?;
            let options = widget.list_item_options.entry(index).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "entryconfigure" => {
            if widget.widget_command != "menu" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "entryconfigure"),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu entryconfigure expects index and optional key/value options",
                ));
            }
            let Some(index) = parse_menu_existing_index_bits(
                args[2],
                widget.menu_entries.len(),
                widget.menu_active_index,
            ) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu entryconfigure index must resolve to an existing entry",
                ));
            };
            if args.len() == 3 {
                let options = widget
                    .menu_entries
                    .get(index)
                    .map(|entry| entry.options.clone())
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate menu entryconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "menu entry option")?;
                if let Some(bits) = widget
                    .menu_entries
                    .get(index)
                    .and_then(|entry| entry.options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "menu entry options")?;
            let Some(entry) = widget.menu_entries.get_mut(index) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu entryconfigure target does not exist",
                ));
            };
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut entry.options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "paneconfigure" => {
            if widget.widget_command != "panedwindow" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "paneconfigure"),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow paneconfigure expects child and optional key/value options",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
            if !widget
                .pane_children
                .iter()
                .any(|existing| existing == &child)
            {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("unknown pane \"{child}\""),
                ));
            }
            if args.len() == 3 {
                let options = widget
                    .pane_child_options
                    .get(&child)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate panedwindow paneconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "pane option name")?;
                if let Some(bits) = widget
                    .pane_child_options
                    .get(&child)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "panedwindow pane options")?;
            let options = widget.pane_child_options.entry(child).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "activate" => {
            if widget.widget_command == "listbox" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox activate expects exactly one index argument",
                    ));
                }
                let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox activate index must be an integer or end",
                    ));
                };
                widget.list_active_index = Some(index);
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            if widget.widget_command == "menu" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu activate expects exactly one index argument",
                    ));
                }
                widget.menu_active_index = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                );
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "post" => {
            if widget.widget_command == "menu" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu post expects x and y coordinates",
                    ));
                }
                let x = parse_i64_arg(py, handle, args[2], "menu post x")?;
                let y = parse_i64_arg(py, handle, args[3], "menu post y")?;
                widget.menu_posted_at = Some((x, y));
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "unpost" => {
            if widget.widget_command == "menu" {
                if args.len() != 2 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu unpost expects no additional arguments",
                    ));
                }
                widget.menu_posted_at = None;
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "tk_popup" => {
            if widget.widget_command != "menu" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "tk_popup"),
                ));
            }
            if args.len() != 4 && args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu tk_popup expects x, y, and optional entry index",
                ));
            }
            let x = parse_i64_arg(py, handle, args[2], "menu popup x")?;
            let y = parse_i64_arg(py, handle, args[3], "menu popup y")?;
            widget.menu_posted_at = Some((x, y));
            if args.len() == 5 {
                widget.menu_active_index = parse_menu_existing_index_bits(
                    args[4],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                );
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "invoke" => {
            let mut invoke_words: Option<Vec<String>> = None;
            if widget.widget_command == "menu" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu invoke expects exactly one entry index",
                    ));
                }
                let Some(index) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu invoke index must resolve to an existing entry",
                    ));
                };
                if let Some(command_bits) = widget
                    .menu_entries
                    .get(index)
                    .and_then(|entry| entry.options.get("-command"))
                    .copied()
                {
                    let command = get_string_arg(py, handle, command_bits, "menu command")?;
                    if !command.trim().is_empty() {
                        invoke_words = Some(parse_command_words(&command));
                    }
                }
                widget.menu_active_index = Some(index);
            } else if matches!(
                widget.widget_command.as_str(),
                "button" | "checkbutton" | "radiobutton" | "menubutton"
            ) {
                if args.len() != 2 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "invoke expects no additional arguments",
                    ));
                }
                if let Some(command_bits) = widget.options.get("-command").copied() {
                    let command =
                        get_string_arg(py, handle, command_bits, "widget invoke command")?;
                    if !command.trim().is_empty() {
                        invoke_words = Some(parse_command_words(&command));
                    }
                }
            } else if widget.widget_command == "spinbox" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "spinbox invoke expects exactly one element name",
                    ));
                }
            } else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "invoke"),
                ));
            }
            app.last_error = None;
            if let Some(words) = invoke_words {
                drop(registry);
                return call_tk_command_from_strings(py, handle, &words).map(Some);
            }
            Ok(Some(MoltObject::none().bits()))
        }
        "add" | "addtag" | "dtag" | "scan" | "image_configure" | "window_configure" | "see" => {
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            unknown_widget_subcommand_message(widget_path, subcommand),
        )),
    }
}

fn handle_widget_path_command(
    py: &PyToken<'_>,
    handle: i64,
    widget_path: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget path command requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "widget subcommand")?;
    if let Some(bits) =
        handle_treeview_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    if let Some(bits) = handle_ttk_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    match subcommand.as_str() {
        "configure" => {
            if args.len() == 2 {
                clear_last_error(handle);
                return Ok(MoltObject::none().bits());
            }
            if !(args.len() - 2).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "configure expects key/value pairs",
                ));
            }
            let mut option_names = Vec::with_capacity((args.len() - 2) / 2);
            for idx in (2..args.len()).step_by(2) {
                option_names.push(get_string_arg(py, handle, args[idx], "widget option name")?);
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.get_mut(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            for (idx, option_name) in option_names.into_iter().enumerate() {
                let value_bits = args[3 + idx * 2];
                inc_ref_bits(py, value_bits);
                if let Some(old_bits) = widget.options.insert(option_name, value_bits) {
                    dec_ref_bits(py, old_bits);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "cget expects exactly one option name",
                ));
            }
            let option_name = get_string_arg(py, handle, args[2], "widget option name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.get(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            let Some(value_bits) = widget.options.get(&option_name).copied() else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("unknown option \"{option_name}\""),
                ));
            };
            inc_ref_bits(py, value_bits);
            app.last_error = None;
            Ok(value_bits)
        }
        "destroy" => {
            if args.len() != 2 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "destroy expects no additional arguments",
                ));
            }
            if widget_path == "." {
                let mut registry = tk_registry().lock().unwrap();
                let Some(mut app) = registry.apps.remove(&handle) else {
                    return Err(raise_invalid_handle_error(py));
                };
                drop_app_state_refs(py, &mut app);
                return Ok(MoltObject::none().bits());
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.remove(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            clear_widget_refs(py, widget);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            if let Some(bits) =
                handle_generic_widget_path_command(py, handle, widget_path, &subcommand, args)?
            {
                return Ok(bits);
            }
            Err(raise_tcl_for_handle(
                py,
                handle,
                unknown_widget_subcommand_message(widget_path, &subcommand),
            ))
        }
    }
}

fn handle_eval_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "eval expects a script argument",
        ));
    }
    let mut script_parts = Vec::with_capacity(args.len() - 1);
    for &bits in &args[1..] {
        script_parts.push(get_string_arg(py, handle, bits, "eval script segment")?);
    }
    let script = script_parts.join(" ");
    let commands = parse_tcl_script_commands(&script);
    if commands.is_empty() {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    let mut last_out = MoltObject::none().bits();
    for words in commands {
        let out = call_tk_command_from_strings(py, handle, &words)?;
        if !obj_from_bits(last_out).is_none() {
            dec_ref_bits(py, last_out);
        }
        last_out = out;
    }
    Ok(last_out)
}

fn handle_source_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "source expects exactly one filename argument",
        ));
    }
    let filename = get_string_arg(py, handle, args[1], "source filename")?;
    let script = std::fs::read_to_string(&filename).map_err(|err| {
        raise_tcl_for_handle(py, handle, format!("could not read source file: {err}"))
    })?;
    let commands = parse_tcl_script_commands(&script);
    if commands.is_empty() {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    let mut last_out = MoltObject::none().bits();
    for words in commands {
        let out = call_tk_command_from_strings(py, handle, &words)?;
        if !obj_from_bits(last_out).is_none() {
            dec_ref_bits(py, last_out);
        }
        last_out = out;
    }
    Ok(last_out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn run_tcl_rename_and_sync_callbacks(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 3 {
        return run_tcl_command(py, handle, args);
    }
    let old_name = get_string_arg(py, handle, args[1], "rename old command name")?;
    let new_name = get_string_arg(py, handle, args[2], "rename new command name")?;
    let out = run_tcl_command(py, handle, args)?;

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(callback_bits) = app.callbacks.remove(&old_name) else {
        app.last_error = None;
        return Ok(out);
    };
    let was_one_shot = app.one_shot_callbacks.remove(&old_name);
    if new_name.is_empty() {
        dec_ref_bits(py, callback_bits);
        app.last_error = None;
        return Ok(out);
    }
    if let Some(old_bits) = app.callbacks.insert(new_name.clone(), callback_bits) {
        dec_ref_bits(py, old_bits);
    }
    if was_one_shot {
        app.one_shot_callbacks.insert(new_name);
    } else {
        app.one_shot_callbacks.remove(&new_name);
    }
    app.last_error = None;
    Ok(out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn run_tcl_after_and_sync_callbacks(
    py: &PyToken<'_>,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    let out = run_tcl_command(py, handle, args)?;
    if args.len() != 3 {
        return Ok(out);
    }
    let Some(subcommand) = string_obj_to_owned(obj_from_bits(args[1])) else {
        return Ok(out);
    };
    if subcommand != "cancel" {
        return Ok(out);
    }
    let Some(key) = string_obj_to_owned(obj_from_bits(args[2])) else {
        return Ok(out);
    };

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let mut tokens = HashSet::new();
    if app.after_command_tokens.contains_key(&key) {
        tokens.insert(key.clone());
    } else {
        tokens.extend(tokens_for_after_command(app, &key));
        if tokens.is_empty() && key.starts_with("after#") {
            tokens.insert(key);
        }
    }
    cleanup_after_tokens(py, app, &tokens);
    app.last_error = None;
    Ok(out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn native_loadtk_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 1 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "loadtk expects no arguments",
        ));
    }
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if app.tk_loaded {
        app.last_error = None;
        return Ok(MoltObject::none().bits());
    }
    let Some(interp) = app.interpreter.as_ref() else {
        return Err(app_tcl_error_locked(
            py,
            app,
            "tk runtime interpreter is unavailable",
        ));
    };
    match interp.eval(("package", "require", "Tk")) {
        Ok(_) => {
            app.tk_loaded = true;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        Err(err) => Err(app_tcl_error_locked(
            py,
            app,
            format!("failed to load Tk package: {err}"),
        )),
    }
}

fn handle_tk_popup_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 4 && args.len() != 5 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tk_popup expects menu path, x, y, and optional entry index",
        ));
    }
    let menu_path = get_string_arg(py, handle, args[1], "tk_popup menu path")?;
    let x = parse_i64_arg(py, handle, args[2], "tk_popup x")?;
    let y = parse_i64_arg(py, handle, args[3], "tk_popup y")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(&menu_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{menu_path}\""),
        ));
    };
    if widget.widget_command != "menu" {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("widget \"{menu_path}\" is not a menu"),
        ));
    }
    widget.menu_posted_at = Some((x, y));
    if args.len() == 5 {
        widget.menu_active_index = parse_menu_existing_index_bits(
            args[4],
            widget.menu_entries.len(),
            widget.menu_active_index,
        );
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn tk_call_dispatch(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.is_empty() {
        return Err(raise_tcl_for_handle(py, handle, "empty tkinter command"));
    }
    let command = get_string_arg(py, handle, args[0], "command name")?;
    if let Some(callback_bits) = lookup_bound_callback(py, handle, &command)? {
        let out_bits = invoke_callback(py, callback_bits, &args[1..]);
        dec_ref_bits(py, callback_bits);
        if exception_pending(py) {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(py, out_bits);
            }
            set_last_error(handle, "bound tkinter command raised an exception");
            return Err(MoltObject::none().bits());
        }
        clear_last_error(handle);
        return Ok(out_bits);
    }
    if let Some(out_bits) = invoke_filehandler_command(py, handle, &command)? {
        return Ok(out_bits);
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    {
        if command == "rename" {
            return run_tcl_rename_and_sync_callbacks(py, handle, args);
        }
        if command == "after" {
            return run_tcl_after_and_sync_callbacks(py, handle, args);
        }
        if command == "loadtk" {
            return native_loadtk_command(py, handle, args);
        }
        return run_tcl_command(py, handle, args);
    }

    #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
    {
        match command.as_str() {
            "tk_messageBox" | "tk_getOpenFile" | "tk_getSaveFile" | "tk_chooseDirectory"
            | "tk_chooseColor" => handle_headless_commondialog_command(py, handle, args),
            "tk_popup" => handle_tk_popup_command(py, handle, args),
            "tk_dialog" => handle_headless_tk_dialog_command(py, handle, args),
            "set" => handle_set_command(py, handle, args),
            "unset" => handle_unset_command(py, handle, args),
            "loadtk" => handle_loadtk_command(py, handle, args),
            "after" => handle_after_command(py, handle, args),
            "update" => handle_update_command(py, handle, args),
            "tkwait" => handle_tkwait_command(py, handle, args),
            "trace" => handle_trace_command(py, handle, args),
            "rename" => handle_rename_command(py, handle, args),
            "bind" => handle_bind_command(py, handle, args),
            "bindtags" => handle_bindtags_command(py, handle, args),
            "event" => handle_event_command(py, handle, args),
            "option" => handle_option_command(py, handle, args),
            "send" => handle_send_command(py, handle, args),
            "focus" => handle_focus_command(py, handle, args),
            "tk_focusNext" => handle_focus_direction_command(py, handle, args, "tk_focusNext"),
            "tk_focusPrev" => handle_focus_direction_command(py, handle, args, "tk_focusPrev"),
            "tk_strictMotif" | "tk_bisque" | "tk_setPalette" => {
                handle_tk_global_command(py, handle, args)
            }
            "tk_focusFollowsMouse" => {
                if args.len() != 1 {
                    Err(raise_tcl_for_handle(
                        py,
                        handle,
                        "tk_focusFollowsMouse expects no arguments",
                    ))
                } else {
                    clear_last_error(handle);
                    Ok(MoltObject::none().bits())
                }
            }
            "grab" => handle_grab_command(py, handle, args),
            "clipboard" => handle_clipboard_command(py, handle, args),
            "selection" => handle_selection_command(py, handle, args),
            "bell" => {
                clear_last_error(handle);
                Ok(MoltObject::none().bits())
            }
            "wm" => handle_wm_command(py, handle, args),
            "winfo" => handle_winfo_command(py, handle, args),
            "image" => handle_image_command(py, handle, args),
            "font" => handle_font_command(py, handle, args),
            "tix" => handle_tix_command(py, handle, args),
            "tixForm" => handle_tix_form_command(py, handle, args),
            "tixSetSilent" => handle_tix_set_silent_command(py, handle, args),
            "pack" => handle_geometry_command(py, handle, "pack", args),
            "grid" => handle_geometry_command(py, handle, "grid", args),
            "place" => handle_geometry_command(py, handle, "place", args),
            "raise" | "lower" => handle_raise_or_lower_command(py, handle, &command, args),
            "eval" => handle_eval_command(py, handle, args),
            "source" => handle_source_command(py, handle, args),
            "expr" => handle_expr_command(py, handle, args),
            "ttk::style" => handle_ttk_style_command(py, handle, args),
            "ttk::notebook::enableTraversal" => {
                handle_ttk_notebook_enable_traversal(py, handle, args)
            }
            _ => {
                if command.starts_with('.') {
                    return handle_widget_path_command(py, handle, &command, args);
                }
                if command_is_image_instance(py, handle, &command)? {
                    return handle_image_instance_command(py, handle, &command, args);
                }
                if args.len() >= 2
                    && is_widget_constructor_command(command.as_str())
                    && let Some(path) = string_obj_to_owned(obj_from_bits(args[1]))
                    && path.starts_with('.')
                {
                    return handle_widget_create_command(py, handle, &command, args);
                }
                Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("unknown tkinter command \"{command}\""),
                ))
            }
        }
    }
}

fn parse_do_one_event_flags(py: &PyToken<'_>, handle: i64, flags_bits: u64) -> Result<i32, u64> {
    let flags_obj = obj_from_bits(flags_bits);
    if flags_obj.is_none() {
        return Ok(0);
    }
    let Some(raw_flags) = to_i64(flags_obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "dooneevent flags must be an integer",
        ));
    };
    let Ok(flags) = i32::try_from(raw_flags) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "dooneevent flags are out of range",
        ));
    };
    Ok(flags)
}

fn event_token(event: &TkEvent) -> &str {
    match event {
        TkEvent::Callback { token } => token.as_str(),
        TkEvent::Script { token, .. } => token.as_str(),
    }
}

fn event_is_idle(app: &TkAppState, token: &str) -> bool {
    app.after_command_kinds
        .get(token)
        .is_some_and(|kind| kind == "idle")
}

fn event_is_due(app: &TkAppState, token: &str) -> bool {
    app.after_due_at_ms
        .get(token)
        .is_none_or(|due_at| *due_at <= app.after_clock_ms)
}

fn pop_next_ready_event(app: &mut TkAppState) -> Option<TkEvent> {
    app.after_clock_ms = app.after_clock_ms.saturating_add(1);
    let mut ready_idle_index: Option<usize> = None;
    let mut ready_non_idle_index: Option<usize> = None;

    for idx in 0..app.event_queue.len() {
        let Some(event) = app.event_queue.get(idx) else {
            continue;
        };
        let token = event_token(event);
        if !event_is_due(app, token) {
            continue;
        }
        if event_is_idle(app, token) {
            if ready_idle_index.is_none() {
                ready_idle_index = Some(idx);
            }
        } else {
            ready_non_idle_index = Some(idx);
            break;
        }
    }

    if let Some(idx) = ready_non_idle_index.or(ready_idle_index) {
        return app.event_queue.remove(idx);
    }
    None
}

fn app_has_pending_after_work(app: &TkAppState) -> bool {
    !app.event_queue.is_empty() || !app.after_due_at_ms.is_empty()
}

fn dispatch_next_pending_event(py: &PyToken<'_>, handle: i64) -> Result<bool, u64> {
    let ready_filehandler_commands = next_ready_filehandler_commands(py, handle)?;
    for command_name in ready_filehandler_commands {
        if let Some(out_bits) = invoke_filehandler_command(py, handle, &command_name)? {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(py, out_bits);
            }
            clear_last_error(handle);
            return Ok(true);
        }
    }

    let event = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        pop_next_ready_event(app)
    };
    let Some(event) = event else {
        return Ok(false);
    };
    run_event_callback(py, handle, event)?;
    Ok(true)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_available() -> u64 {
    crate::with_gil_entry!(_py, {
        let gate = tk_gate_state(_py, TkOperation::AvailabilityProbe);
        let available = !gate.wasm_unsupported && !gate.backend_unimplemented;
        MoltObject::from_bool(available).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_app_new(_options_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        let use_tk = option_use_tk(_py, _options_bits);
        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        let use_tk = true;
        if let Err(bits) = require_tk_app_new(_py, use_tk) {
            return bits;
        }
        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        let app_state = {
            match build_native_tk_app(_py, use_tk) {
                Ok(app) => app,
                Err(bits) => return bits,
            }
        };
        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        let app_state = TkAppState::default();
        let mut registry = tk_registry().lock().unwrap();
        let mut handle = registry.next_handle;
        while handle <= 0 || registry.apps.contains_key(&handle) {
            handle = if handle == i64::MAX { 1 } else { handle + 1 };
        }
        registry.next_handle = if handle == i64::MAX { 1 } else { handle + 1 };
        registry.apps.insert(handle, app_state);
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_quit(app_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Quit) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Some(app) = registry.apps.get_mut(&handle) else {
            return raise_invalid_handle_error(_py);
        };
        app.quit_requested = true;
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_mainloop(app_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Mainloop) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        loop {
            let should_exit = {
                let mut registry = tk_registry().lock().unwrap();
                let Some(app) = registry.apps.get_mut(&handle) else {
                    return MoltObject::none().bits();
                };
                app.quit_requested
            };
            if should_exit {
                let mut registry = tk_registry().lock().unwrap();
                if let Some(app) = registry.apps.get_mut(&handle) {
                    app.quit_requested = false;
                    app.last_error = None;
                }
                return MoltObject::none().bits();
            }
            let pumped = match pump_tcl_events(_py, handle, 0) {
                Ok(pumped) => pumped,
                Err(bits) => return bits,
            };
            if pumped {
                continue;
            }
            let processed = match dispatch_next_pending_event(_py, handle) {
                Ok(processed) => processed,
                Err(bits) => return bits,
            };
            if processed {
                continue;
            }
            let has_pending = {
                let mut registry = tk_registry().lock().unwrap();
                let Some(app) = registry.apps.get_mut(&handle) else {
                    return MoltObject::none().bits();
                };
                app_has_pending_after_work(app)
            };
            if has_pending {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            clear_last_error(handle);
            return MoltObject::none().bits();
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_do_one_event(app_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::DoOneEvent) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let flags = match parse_do_one_event_flags(_py, handle, flags_bits) {
            Ok(flags) => flags,
            Err(bits) => return bits,
        };
        let pumped = match pump_tcl_events(_py, handle, flags) {
            Ok(pumped) => pumped,
            Err(bits) => return bits,
        };
        if pumped {
            clear_last_error(handle);
            return MoltObject::from_bool(true).bits();
        }
        let processed = match dispatch_next_pending_event(_py, handle) {
            Ok(processed) => processed,
            Err(bits) => return bits,
        };
        if processed {
            clear_last_error(handle);
            return MoltObject::from_bool(true).bits();
        }
        let dont_wait = (flags & TK_DONT_WAIT_FLAG) != 0;
        if !dont_wait {
            loop {
                let has_pending = {
                    let mut registry = tk_registry().lock().unwrap();
                    let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                        return raise_invalid_handle_error(_py);
                    };
                    app_has_pending_after_work(app)
                };
                if !has_pending {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
                let progressed = match dispatch_next_pending_event(_py, handle) {
                    Ok(progressed) => progressed,
                    Err(bits) => return bits,
                };
                if progressed {
                    clear_last_error(handle);
                    return MoltObject::from_bool(true).bits();
                }
            }
        }
        clear_last_error(handle);
        MoltObject::from_bool(false).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after(app_bits: u64, delay_ms_bits: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::After) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(delay_ms) = to_i64(obj_from_bits(delay_ms_bits)) else {
            return raise_tcl_for_handle(_py, handle, "after delay must be an integer");
        };
        if delay_ms < 0 {
            return raise_tcl_for_handle(_py, handle, "after delay must be non-negative");
        }
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let token = next_after_token(&mut app.next_after_id);
        let callback_name = after_callback_name_from_token(&token);

        inc_ref_bits(_py, callback_bits);
        if let Some(old_bits) = app.callbacks.insert(callback_name.clone(), callback_bits) {
            dec_ref_bits(_py, old_bits);
        }
        app.one_shot_callbacks.insert(callback_name.clone());

        if let Err(err) = register_tcl_callback_proc(app, &callback_name) {
            app.one_shot_callbacks.remove(&callback_name);
            if let Some(bits) = app.callbacks.remove(&callback_name) {
                dec_ref_bits(_py, bits);
            }
            return app_tcl_error_locked(
                _py,
                app,
                format!("failed to register tkinter callback command \"{callback_name}\": {err}"),
            );
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        {
            let Some(interp) = app.interpreter.as_ref() else {
                unregister_tcl_callback_proc(app, &callback_name);
                app.one_shot_callbacks.remove(&callback_name);
                if let Some(bits) = app.callbacks.remove(&callback_name) {
                    dec_ref_bits(_py, bits);
                }
                return app_tcl_error_locked(_py, app, "tk runtime interpreter is unavailable");
            };
            let after_token = match interp.eval(("after", delay_ms, callback_name.clone())) {
                Ok(value) => value.to_string(),
                Err(err) => {
                    unregister_tcl_callback_proc(app, &callback_name);
                    app.one_shot_callbacks.remove(&callback_name);
                    if let Some(bits) = app.callbacks.remove(&callback_name) {
                        dec_ref_bits(_py, bits);
                    }
                    return app_tcl_error_locked(_py, app, format!("tk command failed: {err}"));
                }
            };
            register_after_command_token(app, &after_token, &callback_name, "timer");
            app.last_error = None;
            drop(registry);
            return match alloc_string_bits(_py, &after_token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        {
            register_after_command_token(app, &token, &callback_name, "timer");
            schedule_after_timer_token(app, &token, delay_ms);
            app.event_queue.push_back(TkEvent::Callback {
                token: token.clone(),
            });
            app.last_error = None;
            drop(registry);
            match alloc_string_bits(_py, &token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after_idle(app_bits: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::AfterIdle) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let token = next_after_token(&mut app.next_after_id);
        let callback_name = after_callback_name_from_token(&token);

        inc_ref_bits(_py, callback_bits);
        if let Some(old_bits) = app.callbacks.insert(callback_name.clone(), callback_bits) {
            dec_ref_bits(_py, old_bits);
        }
        app.one_shot_callbacks.insert(callback_name.clone());

        if let Err(err) = register_tcl_callback_proc(app, &callback_name) {
            app.one_shot_callbacks.remove(&callback_name);
            if let Some(bits) = app.callbacks.remove(&callback_name) {
                dec_ref_bits(_py, bits);
            }
            return app_tcl_error_locked(
                _py,
                app,
                format!("failed to register tkinter callback command \"{callback_name}\": {err}"),
            );
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        {
            let Some(interp) = app.interpreter.as_ref() else {
                unregister_tcl_callback_proc(app, &callback_name);
                app.one_shot_callbacks.remove(&callback_name);
                if let Some(bits) = app.callbacks.remove(&callback_name) {
                    dec_ref_bits(_py, bits);
                }
                return app_tcl_error_locked(_py, app, "tk runtime interpreter is unavailable");
            };
            let after_token = match interp.eval(("after", "idle", callback_name.clone())) {
                Ok(value) => value.to_string(),
                Err(err) => {
                    unregister_tcl_callback_proc(app, &callback_name);
                    app.one_shot_callbacks.remove(&callback_name);
                    if let Some(bits) = app.callbacks.remove(&callback_name) {
                        dec_ref_bits(_py, bits);
                    }
                    return app_tcl_error_locked(_py, app, format!("tk command failed: {err}"));
                }
            };
            register_after_command_token(app, &after_token, &callback_name, "idle");
            app.last_error = None;
            drop(registry);
            return match alloc_string_bits(_py, &after_token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        {
            register_after_command_token(app, &token, &callback_name, "idle");
            app.event_queue.push_back(TkEvent::Callback {
                token: token.clone(),
            });
            app.last_error = None;
            drop(registry);
            match alloc_string_bits(_py, &token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after_cancel(app_bits: u64, identifier_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::AfterCancel) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let identifier_obj = obj_from_bits(identifier_bits);
        if !is_truthy(_py, identifier_obj) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "id must be a valid identifier returned from after or after_idle",
            );
        }
        let key = match get_text_arg(_py, handle, identifier_bits, "after cancel token") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let mut tokens = HashSet::new();
        if app.after_command_tokens.contains_key(&key) {
            tokens.insert(key.clone());
        } else {
            tokens.extend(tokens_for_after_command(app, &key));
            if tokens.is_empty() && key.starts_with("after#") {
                tokens.insert(key);
            }
        }
        cleanup_after_tokens(_py, app, &tokens);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after_info(app_bits: u64, identifier_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::AfterInfo) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if obj_from_bits(identifier_bits).is_none() {
            app.last_error = None;
            return match alloc_after_info_all(_py, app) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        let token = match get_text_arg(_py, handle, identifier_bits, "after info token") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        app.last_error = None;
        match alloc_after_info_token(_py, app, token.as_str()) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_call(app_bits: u64, argv_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }
        let Some(args) = decode_value_list(obj_from_bits(argv_bits)) else {
            return raise_tcl_for_handle(_py, handle, "tk call argv must be a list or tuple");
        };
        match tk_call_dispatch(_py, handle, &args) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_add(
    app_bits: u64,
    variable_name_bits: u64,
    mode_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mode_name_raw = match get_string_arg(_py, handle, mode_bits, "trace mode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
            Ok(value) => value,
            Err(message) => return raise_tcl_for_handle(_py, handle, message),
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception::<u64>(_py, "TypeError", "trace callback must be callable");
        }

        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "trace_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter trace callback command",
            ) {
                return bits;
            }
            let registrations = app.traces.entry(variable_name).or_default();
            app.next_trace_order = app.next_trace_order.saturating_add(1);
            if app.next_trace_order == 0 {
                app.next_trace_order = 1;
            }
            registrations.push(TkTraceRegistration {
                mode_name,
                callback_name: command_name.clone(),
                order: app.next_trace_order,
            });
            app.last_error = None;
            command_name
        };

        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_remove(
    app_bits: u64,
    variable_name_bits: u64,
    mode_bits: u64,
    cbname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mode_name_raw = match get_string_arg(_py, handle, mode_bits, "trace mode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
            Ok(value) => value,
            Err(message) => return raise_tcl_for_handle(_py, handle, message),
        };
        let callback_name = match get_string_arg(_py, handle, cbname_bits, "trace callback") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        remove_trace_registration(_py, app, &variable_name, &mode_name, &callback_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_info(app_bits: u64, variable_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        app.last_error = None;
        match alloc_trace_info(_py, app.traces.get(&variable_name)) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_clear(app_bits: u64, variable_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        clear_trace_registrations_for_variable(_py, app, &variable_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_tkwait_variable(app_bits: u64, variable_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name = match get_string_arg(_py, handle, variable_name_bits, "tkwait target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match handle_tkwait_variable_target(_py, handle, &variable_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_tkwait_window(app_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target = match get_string_arg(_py, handle, target_bits, "tkwait target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match handle_tkwait_window_target(_py, handle, &target) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_tkwait_visibility(app_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target = match get_string_arg(_py, handle, target_bits, "tkwait target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match handle_tkwait_visibility_target(_py, handle, &target) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_callback_register(
    app_bits: u64,
    target_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
    add_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target_name = match get_string_arg(_py, handle, target_bits, "bind target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception::<u64>(_py, "TypeError", "bind callback must be callable");
        }
        let add_prefix = match normalize_bind_add_prefix(_py, add_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let merged_script = if add_prefix.is_empty() {
            bind_script
        } else {
            format!("{add_prefix}{bind_script}")
        };
        let set_bind_argv = vec!["bind".to_string(), target_name, sequence, merged_script];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_callback_unregister(
    app_bits: u64,
    target_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target_name = match get_string_arg(_py, handle, target_bits, "bind target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = match get_string_arg(_py, handle, command_name_bits, "bind callback id")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let get_bind_argv = vec!["bind".to_string(), target_name.clone(), sequence.clone()];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec!["bind".to_string(), target_name, sequence, replacement];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_widget_bind_callback_register(
    app_bits: u64,
    widget_path_bits: u64,
    bind_target_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
    add_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bind_target = match get_string_arg(_py, handle, bind_target_bits, "widget bind target")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "widget bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception::<u64>(_py, "TypeError", "tag_bind callback must be callable");
        }
        let add_prefix = match normalize_bind_add_prefix(_py, add_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "widget_bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter widget bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let merged_script = if add_prefix.is_empty() {
            bind_script
        } else {
            format!("{add_prefix}{bind_script}")
        };
        let set_bind_argv = vec![
            widget_path,
            "bind".to_string(),
            bind_target,
            sequence,
            merged_script,
        ];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_widget_bind_callback_unregister(
    app_bits: u64,
    widget_path_bits: u64,
    bind_target_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bind_target = match get_string_arg(_py, handle, bind_target_bits, "widget bind target")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "widget bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name =
            match get_string_arg(_py, handle, command_name_bits, "widget bind callback id") {
                Ok(value) => value,
                Err(bits) => return bits,
            };

        let get_bind_argv = vec![
            widget_path.clone(),
            "bind".to_string(),
            bind_target.clone(),
            sequence.clone(),
        ];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec![
            widget_path,
            "bind".to_string(),
            bind_target,
            sequence,
            replacement,
        ];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_text_tag_bind_callback_register(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
    add_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "text widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "text tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "text tag bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception::<u64>(_py, "TypeError", "tag_bind callback must be callable");
        }
        let add_prefix = match normalize_bind_add_prefix(_py, add_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "text_tag_bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter text tag bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let merged_script = if add_prefix.is_empty() {
            bind_script
        } else {
            format!("{add_prefix}{bind_script}")
        };
        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            merged_script,
        ];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_text_tag_bind_callback_unregister(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "text widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "text tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "text tag bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name =
            match get_string_arg(_py, handle, command_name_bits, "text tag bind callback id") {
                Ok(value) => value,
                Err(bits) => return bits,
            };

        let get_bind_argv = vec![
            widget_path.clone(),
            "tag".to_string(),
            "bind".to_string(),
            tagname.clone(),
            sequence.clone(),
        ];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            replacement,
        ];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_treeview_tag_bind_callback_register(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path =
            match get_string_arg(_py, handle, widget_path_bits, "treeview widget path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "treeview tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence =
            match get_string_arg(_py, handle, sequence_bits, "treeview tag bind sequence") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if !callback_is_callable(callback_bits) {
            return raise_exception::<u64>(_py, "TypeError", "tag_bind callback must be callable");
        }

        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "treeview_tag_bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter treeview tag bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            bind_script,
        ];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_treeview_tag_bind_callback_unregister(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path =
            match get_string_arg(_py, handle, widget_path_bits, "treeview widget path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "treeview tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence =
            match get_string_arg(_py, handle, sequence_bits, "treeview tag bind sequence") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let command_name = match get_string_arg(
            _py,
            handle,
            command_name_bits,
            "treeview tag bind callback id",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let get_bind_argv = vec![
            widget_path.clone(),
            "tag".to_string(),
            "bind".to_string(),
            tagname.clone(),
            sequence.clone(),
        ];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            replacement,
        ];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_command(app_bits: u64, name_bits: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::BindCommand) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_tcl_for_handle(_py, handle, "bind command name must be str");
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Err(err) = register_tcl_callback_proc(app, &name) {
            return app_tcl_error_locked(
                _py,
                app,
                format!("failed to register tkinter command \"{name}\": {err}"),
            );
        }
        inc_ref_bits(_py, callback_bits);
        if let Some(old_bits) = app.callbacks.insert(name.clone(), callback_bits) {
            dec_ref_bits(_py, old_bits);
        }
        app.one_shot_callbacks.remove(&name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_unbind_command(app_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::UnbindCommand) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_tcl_for_handle(_py, handle, "unbind command name must be str");
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Some(callback_bits) = app.callbacks.remove(&name) {
            app.one_shot_callbacks.remove(&name);
            unregister_tcl_callback_proc(app, &name);
            dec_ref_bits(_py, callback_bits);
            app.last_error = None;
            return MoltObject::none().bits();
        }
        if let Some(filehandler) = app.filehandler_commands.get(&name).copied() {
            if let Err(bits) = clear_filehandler_registration_locked(_py, app, filehandler.fd) {
                return bits;
            }
            app.last_error = None;
            return MoltObject::none().bits();
        }
        app_tcl_error_locked(_py, app, format!("invalid command name \"{name}\""))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_filehandler_create(
    app_bits: u64,
    fd_bits: u64,
    mask_bits: u64,
    callback_bits: u64,
    file_obj_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::FileHandlerCreate) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_tcl_for_handle(_py, handle, "file descriptor must be an integer");
        };
        if fd < 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("file descriptor cannot be a negative integer ({fd})"),
            );
        }
        let Some(mask) = to_i64(obj_from_bits(mask_bits)) else {
            return raise_tcl_for_handle(_py, handle, "filehandler mask must be an integer");
        };
        let callable_check = crate::molt_is_callable(callback_bits);
        if to_i64(obj_from_bits(callable_check)) != Some(1) {
            return raise_exception::<u64>(_py, "TypeError", "bad argument list");
        }

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Err(bits) = clear_filehandler_registration_locked(_py, app, fd) {
            return bits;
        }

        if mask == 0 {
            app.last_error = None;
            return MoltObject::none().bits();
        }

        let mut registration = TkFileHandlerRegistration {
            callback_bits,
            file_obj_bits,
            commands: HashMap::new(),
        };
        inc_ref_bits(_py, callback_bits);
        inc_ref_bits(_py, file_obj_bits);

        for (event_mask, event_name) in [
            (TK_FILE_EVENT_READABLE, "readable"),
            (TK_FILE_EVENT_WRITABLE, "writable"),
            (TK_FILE_EVENT_EXCEPTION, "exception"),
        ] {
            if (mask & event_mask) == 0 {
                #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
                if let Err(bits) = app_interp_eval_list(
                    _py,
                    app,
                    vec![
                        "fileevent".to_string(),
                        fd.to_string(),
                        event_name.to_string(),
                        String::new(),
                    ],
                ) {
                    rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                    return bits;
                }
                continue;
            }

            let command_name = filehandler_command_name(fd, event_name);
            if app.callbacks.contains_key(&command_name) {
                rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                return app_tcl_error_locked(
                    _py,
                    app,
                    format!("filehandler command name collision for \"{command_name}\""),
                );
            }
            #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
            if let Err(err) = register_tcl_callback_proc(app, &command_name) {
                rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                return app_tcl_error_locked(
                    _py,
                    app,
                    format!(
                        "failed to register tkinter filehandler command \"{command_name}\": {err}"
                    ),
                );
            }
            #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
            if let Err(bits) = app_interp_eval_list(
                _py,
                app,
                vec![
                    "fileevent".to_string(),
                    fd.to_string(),
                    event_name.to_string(),
                    command_name.clone(),
                ],
            ) {
                unregister_tcl_callback_proc(app, &command_name);
                rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                return bits;
            }
            app.filehandler_commands.insert(
                command_name.clone(),
                TkFileHandlerCommand {
                    fd,
                    mask: event_mask,
                },
            );
            registration.commands.insert(event_mask, command_name);
        }

        if registration.commands.is_empty() {
            rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
            app.last_error = None;
            return MoltObject::none().bits();
        }
        app.filehandlers.insert(fd, registration);
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_filehandler_delete(app_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::FileHandlerDelete) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_tcl_for_handle(_py, handle, "file descriptor must be an integer");
        };
        if fd < 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("file descriptor cannot be a negative integer ({fd})"),
            );
        }
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Err(bits) = clear_filehandler_registration_locked(_py, app, fd) {
            return bits;
        }
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_destroy_widget(app_bits: u64, widget_path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::DestroyWidget) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(widget_path) = string_obj_to_owned(obj_from_bits(widget_path_bits)) else {
            return raise_tcl_for_handle(_py, handle, "widget path must be str");
        };
        let mut registry = tk_registry().lock().unwrap();
        if widget_path == "." {
            let Some(mut app) = registry.apps.remove(&handle) else {
                return raise_invalid_handle_error(_py);
            };
            drop_app_state_refs(_py, &mut app);
            return MoltObject::none().bits();
        }
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        {
            let Some(interp) = app.interpreter.as_ref() else {
                return app_tcl_error_locked(_py, app, "tk runtime interpreter is unavailable");
            };
            if let Err(err) = interp.eval(("destroy", widget_path.clone())) {
                return app_tcl_error_locked(_py, app, format!("tk command failed: {err}"));
            }
            if let Some(widget) = app.widgets.remove(&widget_path) {
                clear_widget_refs(_py, widget);
            }
            app.last_error = None;
            return MoltObject::none().bits();
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        {
            let Some(widget) = app.widgets.remove(&widget_path) else {
                return app_tcl_error_locked(
                    _py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                );
            };
            clear_widget_refs(_py, widget);
            app.last_error = None;
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_last_error(app_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::LastError) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Some(message) = app.last_error.as_deref() {
            return match alloc_string_bits(_py, message) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_getboolean(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        if obj.is_bool() {
            return MoltObject::from_bool(obj.as_bool().unwrap_or(false)).bits();
        }
        if let Some(value) = to_i64(obj) {
            return MoltObject::from_bool(value != 0).bits();
        }
        if let Some(value) = to_f64(obj) {
            return MoltObject::from_bool(value != 0.0).bits();
        }
        if let Some(text) = string_obj_to_owned(obj) {
            if let Some(parsed) = parse_bool_text(&text) {
                return MoltObject::from_bool(parsed).bits();
            }
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("invalid boolean value \"{text}\""),
            );
        }
        MoltObject::from_bool(is_truthy(_py, obj)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_getdouble(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        if let Some(value) = to_f64(obj) {
            return MoltObject::from_float(value).bits();
        }
        if let Some(text) = string_obj_to_owned(obj)
            && let Ok(value) = text.trim().parse::<f64>()
        {
            return MoltObject::from_float(value).bits();
        }
        raise_exception::<u64>(
            _py,
            "ValueError",
            &format!(
                "invalid floating-point value \"{}\"",
                string_obj_to_owned(obj).unwrap_or_else(|| "?".to_string())
            ),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_splitlist(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        if let Some(items) = decode_value_list(obj) {
            return match alloc_tuple_bits(
                _py,
                items.as_slice(),
                "failed to allocate splitlist tuple",
            ) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        if let Some(text) = string_obj_to_owned(obj) {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return match alloc_tuple_from_strings(
                    _py,
                    &[],
                    "failed to allocate splitlist empty tuple",
                ) {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
            }
            let mut words = Vec::new();
            for command in parse_tcl_script_commands(trimmed) {
                words.extend(command);
            }
            return match alloc_tuple_from_strings(
                _py,
                words.as_slice(),
                "failed to allocate splitlist tuple",
            ) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        match alloc_tuple_bits(_py, &[value_bits], "failed to allocate splitlist tuple") {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_subst_parse(_widget_path_bits: u64, event_args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(raw_args) = decode_value_list(obj_from_bits(event_args_bits)) else {
            return MoltObject::none().bits();
        };
        let args: Vec<u64> = raw_args.into_iter().map(flatten_event_subst_arg).collect();
        if args.len() != TK_EVENT_SUBST_FIELD_COUNT {
            return MoltObject::none().bits();
        }

        let payload = [
            normalize_event_subst_int_field(args[0]),
            normalize_event_subst_int_field(args[1]),
            normalize_event_subst_bool_field(args[2]),
            normalize_event_subst_int_field(args[3]),
            normalize_event_subst_int_field(args[4]),
            normalize_event_subst_int_field(args[5]),
            normalize_event_subst_int_field(args[6]),
            normalize_event_subst_int_field(args[7]),
            normalize_event_subst_int_field(args[8]),
            normalize_event_subst_int_field(args[9]),
            args[10],
            normalize_event_subst_bool_field(args[11]),
            args[12],
            normalize_event_subst_int_field(args[13]),
            args[14],
            args[15],
            normalize_event_subst_int_field(args[16]),
            normalize_event_subst_int_field(args[17]),
            normalize_event_subst_delta_field(args[18]),
        ];

        match alloc_tuple_bits(
            _py,
            &payload,
            "failed to allocate tkinter event substitution tuple",
        ) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_script_remove_command(
    script_bits: u64,
    command_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(script) = string_obj_to_owned(obj_from_bits(script_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "bind script must be str");
        };
        let Some(command_name) = string_obj_to_owned(obj_from_bits(command_name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "bind command name must be str");
        };
        let replacement = remove_bind_script_command_invocations(&script, &command_name);
        match alloc_string_bits(_py, &replacement) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_errorinfo_append(app_bits: u64, message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let message = match get_string_arg(_py, handle, message_bits, "errorinfo message") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let current = app
            .variables
            .get("errorInfo")
            .copied()
            .and_then(|bits| string_obj_to_owned(obj_from_bits(bits)))
            .unwrap_or_default();
        let merged = if current.is_empty() {
            message
        } else if message.starts_with('\n') {
            format!("{current}{message}")
        } else {
            format!("{current}\n{message}")
        };
        let merged_bits = match alloc_string_bits(_py, &merged) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        if let Some(old_bits) = app.variables.insert("errorInfo".to_string(), merged_bits) {
            dec_ref_bits(_py, old_bits);
        }
        bump_variable_versions_for_reference(app, "errorInfo");
        app.last_error = None;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_dialog_show(
    app_bits: u64,
    master_path_bits: u64,
    title_bits: u64,
    text_bits: u64,
    bitmap_bits: u64,
    default_index_bits: u64,
    strings_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::DialogShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }

        let _master_path = match get_string_arg(_py, handle, master_path_bits, "dialog master path")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _title = match get_string_arg_allow_none(_py, handle, title_bits, "dialog title") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _text = match get_string_arg_allow_none(_py, handle, text_bits, "dialog text") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _bitmap = match get_string_arg_allow_none(_py, handle, bitmap_bits, "dialog bitmap") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(default_index) = to_i64(obj_from_bits(default_index_bits)) else {
            return raise_tcl_for_handle(_py, handle, "dialog default index must be an integer");
        };
        let Some(raw_strings) = decode_value_list(obj_from_bits(strings_bits)) else {
            return raise_tcl_for_handle(_py, handle, "dialog button strings must be a list/tuple");
        };
        let mut button_labels = Vec::with_capacity(raw_strings.len());
        for item_bits in raw_strings {
            let label = match get_string_arg(_py, handle, item_bits, "dialog button label") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            button_labels.push(label);
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        {
            let mut command = vec![
                "tk_dialog".to_string(),
                _master_path,
                _title,
                _text,
                _bitmap,
                default_index.to_string(),
            ];
            command.extend(button_labels);
            return match tk_dispatch_string_command(_py, handle, &command) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        {
            let selected = if button_labels.is_empty() {
                0_i64
            } else {
                let mut index = default_index;
                if index < 0 {
                    index = 0;
                }
                let max = (button_labels.len() - 1) as i64;
                if index > max {
                    index = max;
                }
                index
            };
            clear_last_error(handle);
            MoltObject::from_int(selected).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_commondialog_show(
    app_bits: u64,
    master_path_bits: u64,
    command_bits: u64,
    options_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::CommonDialogShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }

        let _master_path = match get_string_arg_allow_none(
            _py,
            handle,
            master_path_bits,
            "commondialog master path",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command = match get_string_arg(_py, handle, command_bits, "commondialog command") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let options = match parse_commondialog_options(_py, handle, options_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        if !commondialog_is_supported_command(command.as_str()) {
            return raise_unsupported_commondialog_command(_py, handle, command.as_str());
        }

        match dispatch_commondialog_via_tk_call(_py, handle, &_master_path, &command, &options) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_messagebox_show(
    app_bits: u64,
    master_path_bits: u64,
    options_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::MessageBoxShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }
        let master_path = match get_string_arg_allow_none(
            _py,
            handle,
            master_path_bits,
            "messagebox master path",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let options = match parse_commondialog_options(_py, handle, options_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match dispatch_commondialog_via_tk_call(
            _py,
            handle,
            &master_path,
            "tk_messageBox",
            &options,
        ) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_filedialog_show(
    app_bits: u64,
    master_path_bits: u64,
    command_bits: u64,
    options_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::FileDialogShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }
        let master_path = match get_string_arg_allow_none(
            _py,
            handle,
            master_path_bits,
            "filedialog master path",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command = match get_string_arg(_py, handle, command_bits, "filedialog command") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !filedialog_is_supported_command(command.as_str()) {
            return raise_unsupported_filedialog_command(_py, handle, command.as_str());
        }
        let options = match parse_commondialog_options(_py, handle, options_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match dispatch_commondialog_via_tk_call(_py, handle, &master_path, &command, &options) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_simpledialog_query(
    app_bits: u64,
    parent_path_bits: u64,
    title_bits: u64,
    prompt_bits: u64,
    initial_value_bits: u64,
    query_kind_bits: u64,
    min_value_bits: u64,
    max_value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::SimpleDialogQuery) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }

        let _parent_path =
            match get_string_arg(_py, handle, parent_path_bits, "simpledialog parent path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let _title = match get_string_arg_allow_none(_py, handle, title_bits, "simpledialog title")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _prompt = match get_string_arg(_py, handle, prompt_bits, "simpledialog prompt") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let initial_text = match get_string_arg_allow_none(
            _py,
            handle,
            initial_value_bits,
            "simpledialog initial value",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let query_kind =
            match get_string_arg(_py, handle, query_kind_bits, "simpledialog query kind") {
                Ok(value) => value,
                Err(bits) => return bits,
            };

        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        {
            let (int_min, int_max, float_min, float_max) = match query_kind.as_str() {
                "string" => (None, None, None, None),
                "int" => {
                    let min = match parse_optional_i64_arg(
                        _py,
                        handle,
                        min_value_bits,
                        "simpledialog minvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    let max = match parse_optional_i64_arg(
                        _py,
                        handle,
                        max_value_bits,
                        "simpledialog maxvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    (min, max, None, None)
                }
                "float" => {
                    let min = match parse_optional_f64_arg(
                        _py,
                        handle,
                        min_value_bits,
                        "simpledialog minvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    let max = match parse_optional_f64_arg(
                        _py,
                        handle,
                        max_value_bits,
                        "simpledialog maxvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    (None, None, min, max)
                }
                _ => {
                    return raise_tcl_for_handle(
                        _py,
                        handle,
                        "simpledialog query kind must be one of: 'string', 'int', 'float'",
                    );
                }
            };

            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };

            if !app.tk_loaded {
                if let Err(bits) = app_interp_eval_list(
                    _py,
                    app,
                    vec![
                        "package".to_string(),
                        "require".to_string(),
                        "Tk".to_string(),
                    ],
                ) {
                    return bits;
                }
                app.tk_loaded = true;
            }

            let dialog_token = next_after_token(&mut app.next_after_id).replace('#', "_");
            let dialog_path = format!(".__molt_simpledialog_{handle}_{dialog_token}");
            let body_path = format!("{dialog_path}.body");
            let prompt_widget = format!("{body_path}.prompt");
            let entry_widget = format!("{body_path}.entry");
            let button_row = format!("{dialog_path}.buttons");
            let ok_button = format!("{button_row}.ok");
            let cancel_button = format!("{button_row}.cancel");
            let state_var = format!("::__molt_simpledialog_state_{handle}_{dialog_token}");
            let ok_script = format!("set {state_var} ok");
            let cancel_script = format!("set {state_var} cancel");

            let mut created_dialog = false;

            let run_setup = |app: &mut TkAppState, words: Vec<String>| -> Result<TclObj, u64> {
                app_interp_eval_list(_py, app, words)
            };

            let setup_result = (|| -> Result<(), u64> {
                run_setup(app, vec!["toplevel".to_string(), dialog_path.clone()])?;
                created_dialog = true;
                if !_title.is_empty() {
                    run_setup(
                        app,
                        vec![
                            "wm".to_string(),
                            "title".to_string(),
                            dialog_path.clone(),
                            _title.clone(),
                        ],
                    )?;
                }
                if !_parent_path.is_empty() {
                    run_setup(
                        app,
                        vec![
                            "wm".to_string(),
                            "transient".to_string(),
                            dialog_path.clone(),
                            _parent_path.clone(),
                        ],
                    )?;
                }
                run_setup(
                    app,
                    vec![
                        "wm".to_string(),
                        "resizable".to_string(),
                        dialog_path.clone(),
                        "0".to_string(),
                        "0".to_string(),
                    ],
                )?;
                run_setup(app, vec!["frame".to_string(), body_path.clone()])?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        body_path.clone(),
                        "-padx".to_string(),
                        "8".to_string(),
                        "-pady".to_string(),
                        "8".to_string(),
                        "-fill".to_string(),
                        "x".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "label".to_string(),
                        prompt_widget.clone(),
                        "-text".to_string(),
                        _prompt.clone(),
                        "-anchor".to_string(),
                        "w".to_string(),
                        "-justify".to_string(),
                        "left".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        prompt_widget.clone(),
                        "-fill".to_string(),
                        "x".to_string(),
                    ],
                )?;
                run_setup(app, vec!["entry".to_string(), entry_widget.clone()])?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        entry_widget.clone(),
                        "-fill".to_string(),
                        "x".to_string(),
                        "-pady".to_string(),
                        "6".to_string(),
                    ],
                )?;
                run_setup(app, vec!["frame".to_string(), button_row.clone()])?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        button_row.clone(),
                        "-padx".to_string(),
                        "8".to_string(),
                        "-pady".to_string(),
                        "8".to_string(),
                        "-fill".to_string(),
                        "x".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "button".to_string(),
                        ok_button.clone(),
                        "-text".to_string(),
                        "OK".to_string(),
                        "-command".to_string(),
                        ok_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "button".to_string(),
                        cancel_button.clone(),
                        "-text".to_string(),
                        "Cancel".to_string(),
                        "-command".to_string(),
                        cancel_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        ok_button.clone(),
                        "-side".to_string(),
                        "left".to_string(),
                        "-padx".to_string(),
                        "6".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        cancel_button.clone(),
                        "-side".to_string(),
                        "left".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "wm".to_string(),
                        "protocol".to_string(),
                        dialog_path.clone(),
                        "WM_DELETE_WINDOW".to_string(),
                        cancel_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "bind".to_string(),
                        entry_widget.clone(),
                        "<Return>".to_string(),
                        ok_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "bind".to_string(),
                        entry_widget.clone(),
                        "<Escape>".to_string(),
                        cancel_script.clone(),
                    ],
                )?;
                if !initial_text.is_empty() {
                    run_setup(
                        app,
                        vec![
                            entry_widget.clone(),
                            "insert".to_string(),
                            "0".to_string(),
                            initial_text.clone(),
                        ],
                    )?;
                }
                run_setup(app, vec!["focus".to_string(), entry_widget.clone()])?;
                run_setup(
                    app,
                    vec!["grab".to_string(), "set".to_string(), dialog_path.clone()],
                )?;
                run_setup(
                    app,
                    vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                )?;
                Ok(())
            })();

            if let Err(bits) = setup_result {
                if created_dialog {
                    cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                }
                return bits;
            }

            let result_bits = loop {
                if let Err(bits) =
                    app_interp_eval_list(_py, app, vec!["vwait".to_string(), state_var.clone()])
                {
                    cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                    return bits;
                }
                let state = match app_interp_eval_list(
                    _py,
                    app,
                    vec!["set".to_string(), state_var.clone()],
                ) {
                    Ok(value) => value.to_string(),
                    Err(bits) => {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return bits;
                    }
                };
                if state == "cancel" {
                    break MoltObject::none().bits();
                }
                if state != "ok" {
                    if let Err(bits) = app_interp_eval_list(
                        _py,
                        app,
                        vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                    ) {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return bits;
                    }
                    continue;
                }

                let value_text = match app_interp_eval_list(
                    _py,
                    app,
                    vec![entry_widget.clone(), "get".to_string()],
                ) {
                    Ok(value) => value.to_string(),
                    Err(bits) => {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return bits;
                    }
                };

                match query_kind.as_str() {
                    "string" => match alloc_string_bits(_py, &value_text) {
                        Ok(bits) => break bits,
                        Err(bits) => {
                            cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                            return bits;
                        }
                    },
                    "int" => {
                        let Some(value) = parse_simpledialog_i64(&value_text) else {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        };
                        if int_min.is_some_and(|bound| value < bound)
                            || int_max.is_some_and(|bound| value > bound)
                        {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        }
                        break MoltObject::from_int(value).bits();
                    }
                    "float" => {
                        let Some(value) = parse_simpledialog_f64(&value_text) else {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        };
                        if float_min.is_some_and(|bound| value < bound)
                            || float_max.is_some_and(|bound| value > bound)
                        {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        }
                        break MoltObject::from_float(value).bits();
                    }
                    _ => {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return raise_tcl_for_handle(
                            _py,
                            handle,
                            "simpledialog query kind must be one of: 'string', 'int', 'float'",
                        );
                    }
                }
            };

            cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
            app.last_error = None;
            return result_bits;
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        match query_kind.as_str() {
            "string" => {
                clear_last_error(handle);
                match alloc_string_bits(_py, &initial_text) {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                }
            }
            "int" => {
                let value = match parse_simpledialog_i64(&initial_text) {
                    Some(parsed) => parsed,
                    None => {
                        clear_last_error(handle);
                        return MoltObject::none().bits();
                    }
                };
                let min = match parse_optional_i64_arg(
                    _py,
                    handle,
                    min_value_bits,
                    "simpledialog minvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let max = match parse_optional_i64_arg(
                    _py,
                    handle,
                    max_value_bits,
                    "simpledialog maxvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if min.is_some_and(|bound| value < bound) || max.is_some_and(|bound| value > bound)
                {
                    clear_last_error(handle);
                    return MoltObject::none().bits();
                }
                clear_last_error(handle);
                MoltObject::from_int(value).bits()
            }
            "float" => {
                let value = match parse_simpledialog_f64(&initial_text) {
                    Some(parsed) => parsed,
                    None => {
                        clear_last_error(handle);
                        return MoltObject::none().bits();
                    }
                };
                let min = match parse_optional_f64_arg(
                    _py,
                    handle,
                    min_value_bits,
                    "simpledialog minvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let max = match parse_optional_f64_arg(
                    _py,
                    handle,
                    max_value_bits,
                    "simpledialog maxvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if min.is_some_and(|bound| value < bound) || max.is_some_and(|bound| value > bound)
                {
                    clear_last_error(handle);
                    return MoltObject::none().bits();
                }
                clear_last_error(handle);
                MoltObject::from_float(value).bits()
            }
            _ => raise_tcl_for_handle(
                _py,
                handle,
                "simpledialog query kind must be one of: 'string', 'int', 'float'",
            ),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_message_single_capability_stays_stable() {
        let state = TkGateState {
            missing_gui_window: true,
            ..TkGateState::default()
        };
        assert_eq!(
            format_permission_error_message(&state),
            "missing gui.window capability"
        );
    }

    #[test]
    fn permission_message_multi_capability_stays_ordered() {
        let state = TkGateState {
            missing_gui_window: true,
            missing_process_spawn: true,
            ..TkGateState::default()
        };
        assert_eq!(
            format_permission_error_message(&state),
            "missing capabilities: gui.window, process.spawn"
        );
    }

    #[test]
    fn unavailable_message_native_blockers_exclude_backend_not_implemented() {
        let state = TkGateState {
            wasm_unsupported: false,
            backend_unimplemented: false,
            missing_gui_window: true,
            missing_process_spawn: true,
            ..TkGateState::default()
        };
        assert_eq!(
            format_tk_unavailable_message(TkOperation::AppNew, &state),
            "tkinter runtime support is not implemented yet (molt_tk_app_new) [blockers: capability.gui.window, capability.process.spawn]"
        );
    }

    #[test]
    fn unavailable_message_includes_platform_preflight_blockers() {
        let state = TkGateState {
            missing_linux_display: true,
            missing_macos_main_thread: true,
            ..TkGateState::default()
        };
        assert_eq!(
            format_tk_unavailable_message(TkOperation::AppNew, &state),
            "tkinter runtime support is not implemented yet (molt_tk_app_new) [blockers: platform.linux.display, platform.macos.main_thread]"
        );
    }

    #[test]
    fn platform_preflight_blockers_helper_matches_state() {
        let mut state = TkGateState::default();
        assert!(!has_platform_preflight_blockers(&state));
        state.missing_linux_display = true;
        assert!(has_platform_preflight_blockers(&state));
        state.missing_linux_display = false;
        state.missing_macos_main_thread = true;
        assert!(has_platform_preflight_blockers(&state));
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    #[test]
    fn tcl_find_executable_arg_prefers_non_empty_path() {
        let arg = tcl_find_executable_arg();
        assert!(!arg.as_bytes().is_empty());
    }

    #[test]
    fn tcl_script_parser_handles_quotes_braces_and_commands() {
        assert_eq!(
            parse_tcl_script_commands("  set   answer   42  "),
            vec![vec![
                "set".to_string(),
                "answer".to_string(),
                "42".to_string()
            ]]
        );
        assert_eq!(
            parse_tcl_script_commands("set a {x y}; set b \"quoted value\""),
            vec![
                vec!["set".to_string(), "a".to_string(), "x y".to_string()],
                vec![
                    "set".to_string(),
                    "b".to_string(),
                    "quoted value".to_string()
                ],
            ]
        );
        assert!(parse_tcl_script_commands(" \t\n ").is_empty());
    }

    #[test]
    fn trace_callback_words_parses_command_prefix_words() {
        assert_eq!(
            trace_callback_command_words("::__molt_trace_cb arg1 arg2"),
            vec![
                "::__molt_trace_cb".to_string(),
                "arg1".to_string(),
                "arg2".to_string(),
            ]
        );
    }

    #[test]
    fn trace_callback_words_preserves_single_braced_word() {
        assert_eq!(
            trace_callback_command_words("{::__molt trace cb}"),
            vec!["::__molt trace cb".to_string()]
        );
    }

    #[test]
    fn bind_script_parser_extracts_if_wrapper_command_words() {
        let script = "if {\"[::__molt_cb %# %x %y]\" == \"break\"} break";
        assert_eq!(
            parse_bind_script_commands(script),
            vec![vec![
                "::__molt_cb".to_string(),
                "%#".to_string(),
                "%x".to_string(),
                "%y".to_string(),
            ]]
        );
    }

    #[test]
    fn bind_script_remove_command_drops_matching_if_wrapper_lines() {
        let script = concat!(
            "if {\"[::__molt_keep %#]\" == \"break\"} break\n",
            "if {\"[::__molt_drop %#]\" == \"break\"} break\n",
        );
        assert_eq!(
            remove_bind_script_command_invocations(script, "::__molt_drop"),
            "if {\"[::__molt_keep %#]\" == \"break\"} break\n"
        );
    }

    #[test]
    fn bind_script_remove_command_preserves_non_matching_commands() {
        let script = "+::__molt_drop %x %y\n+::__molt_keep %x %y";
        assert_eq!(
            remove_bind_script_command_invocations(script, "::__molt_drop"),
            "+::__molt_keep %x %y"
        );
    }

    #[test]
    fn bind_script_remove_command_handles_plus_prefixed_if_wrapper() {
        let script = concat!(
            "+if {\"[::__molt_drop %#]\" == \"break\"} break\n",
            "+if {\"[::__molt_keep %#]\" == \"break\"} break\n",
        );
        assert_eq!(
            remove_bind_script_command_invocations(script, "::__molt_drop"),
            "+if {\"[::__molt_keep %#]\" == \"break\"} break\n"
        );
    }

    #[test]
    fn expr_literal_parsing_handles_int_and_float() {
        assert_eq!(parse_expr_literal("123"), Some(TkExprLiteral::Int(123)));
        assert_eq!(parse_expr_literal("3.5"), Some(TkExprLiteral::Float(3.5)));
        assert_eq!(parse_expr_literal("x + 1"), None);
    }

    #[test]
    fn after_token_generation_is_deterministic() {
        let mut next_after_id = 0;
        assert_eq!(next_after_token(&mut next_after_id), "after#1");
        assert_eq!(next_after_token(&mut next_after_id), "after#2");
        assert_eq!(next_after_token(&mut next_after_id), "after#3");
    }

    #[test]
    fn after_callback_name_derivation_is_deterministic() {
        assert_eq!(
            after_callback_name_from_token("after#7"),
            "::__molt_after_callback_7"
        );
        assert_eq!(
            after_callback_name_from_token("custom_token"),
            "::__molt_after_callback_custom_token"
        );
    }

    #[test]
    fn after_helpers_remove_expected_tokens() {
        let mut app = TkAppState::default();
        register_after_command_token(&mut app, "after#1", "cmd_one", "timer");
        register_after_command_token(&mut app, "after#2", "cmd_one", "idle");
        register_after_command_token(&mut app, "after#3", "cmd_two", "timer");
        schedule_after_timer_token(&mut app, "after#1", 5);

        app.event_queue.push_back(TkEvent::Callback {
            token: "after#1".to_string(),
        });
        app.event_queue.push_back(TkEvent::Script {
            token: "after#2".to_string(),
            commands: vec![vec![
                "set".to_string(),
                "value".to_string(),
                "1".to_string(),
            ]],
        });
        app.event_queue.push_back(TkEvent::Callback {
            token: "after#3".to_string(),
        });

        assert_eq!(
            lookup_after_command_for_token(&app, "after#1").as_deref(),
            Some("cmd_one")
        );
        let tokens = tokens_for_after_command(&app, "cmd_one");
        assert_eq!(tokens.len(), 2);
        remove_after_events_for_tokens(&mut app, &tokens);

        assert_eq!(app.event_queue.len(), 1);
        let Some(TkEvent::Callback { token }) = app.event_queue.front() else {
            panic!("expected callback event");
        };
        assert_eq!(token, "after#3");

        unregister_after_command_token(&mut app, "after#3");
        assert!(lookup_after_kind_for_token(&app, "after#3").is_none());
        assert!(!app.after_due_at_ms.contains_key("after#3"));
    }

    #[test]
    fn after_scheduler_waits_for_due_tokens_and_prefers_non_idle() {
        let mut app = TkAppState::default();
        register_after_command_token(&mut app, "after#timer", "timer_cmd", "timer");
        register_after_command_token(&mut app, "after#idle", "idle_cmd", "idle");
        schedule_after_timer_token(&mut app, "after#timer", 3);

        app.event_queue.push_back(TkEvent::Script {
            token: "after#idle".to_string(),
            commands: vec![vec!["set".to_string(), "idle".to_string(), "1".to_string()]],
        });
        app.event_queue.push_back(TkEvent::Script {
            token: "after#timer".to_string(),
            commands: vec![vec![
                "set".to_string(),
                "timer".to_string(),
                "1".to_string(),
            ]],
        });

        let first = pop_next_ready_event(&mut app).expect("first event");
        assert!(matches!(first, TkEvent::Script { token, .. } if token == "after#idle"));

        let second = pop_next_ready_event(&mut app);
        assert!(second.is_none());

        let third = pop_next_ready_event(&mut app).expect("third event");
        assert!(matches!(third, TkEvent::Script { token, .. } if token == "after#timer"));
    }

    #[test]
    fn after_info_token_sorting_prefers_newest_after_ids() {
        let mut tokens = vec![
            "after#2".to_string(),
            "after#10".to_string(),
            "after#1".to_string(),
            "custom".to_string(),
        ];
        sort_after_info_tokens(&mut tokens);
        assert_eq!(
            tokens,
            vec![
                "after#10".to_string(),
                "after#2".to_string(),
                "after#1".to_string(),
                "custom".to_string(),
            ]
        );
    }

    #[test]
    fn tkwait_window_exists_handles_root_handle_and_widget_paths() {
        let mut registry = TkRegistry::default();
        registry.apps.insert(7, TkAppState::default());
        assert!(tkwait_window_exists(&registry, 7, "."));
        assert!(!tkwait_window_exists(&registry, 8, "."));
        {
            let app = registry.apps.get_mut(&7).expect("app handle");
            app.widgets.insert(
                ".w".to_string(),
                TkWidgetState {
                    widget_command: "frame".to_string(),
                    ..TkWidgetState::default()
                },
            );
        }
        assert!(tkwait_window_exists(&registry, 7, ".w"));
        assert!(!tkwait_window_exists(&registry, 7, ".missing"));
    }

    #[test]
    fn tkwait_visibility_tracks_root_wm_state_and_widget_manager() {
        let mut app = TkAppState::default();
        assert!(tkwait_visibility_reached_in_app(&app, "."));

        app.wm.state = "withdrawn".to_string();
        assert!(!tkwait_visibility_reached_in_app(&app, "."));

        app.wm.state = "normal".to_string();
        app.widgets.insert(
            ".w".to_string(),
            TkWidgetState {
                widget_command: "frame".to_string(),
                manager: Some("pack".to_string()),
                ..TkWidgetState::default()
            },
        );
        assert!(tkwait_visibility_reached_in_app(&app, ".w"));
        assert!(!tkwait_visibility_reached_in_app(&app, ".missing"));
    }

    #[test]
    fn split_array_variable_reference_handles_array_element_names() {
        assert_eq!(
            split_array_variable_reference("name"),
            ("name".to_string(), None)
        );
        assert_eq!(
            split_array_variable_reference("arr(key)"),
            ("arr".to_string(), Some("key".to_string()))
        );
        assert_eq!(
            split_array_variable_reference("(broken)"),
            ("(broken)".to_string(), None)
        );
    }

    #[test]
    fn trace_mode_normalization_and_matching_are_stable() {
        assert_eq!(normalize_trace_mode_name("write").as_deref(), Ok("write"));
        assert_eq!(
            normalize_trace_mode_name("write read").as_deref(),
            Ok("read write")
        );
        assert_eq!(
            normalize_trace_mode_name("w, read, read, u").as_deref(),
            Ok("read write unset")
        );
        assert!(normalize_trace_mode_name("bogus").is_err());
        assert!(normalize_trace_mode_name("").is_err());

        assert!(trace_mode_matches("write", "write"));
        assert!(trace_mode_matches("read write", "read"));
        assert!(trace_mode_matches("read write", "write"));
        assert!(trace_mode_matches("unset", "unset"));
        assert!(!trace_mode_matches("read", "write"));
        assert!(!trace_mode_matches("", "read"));
    }

    #[test]
    fn trace_callbacks_preserve_registration_order() {
        let mut app = TkAppState::default();
        app.traces.insert(
            "trace_var".to_string(),
            vec![
                TkTraceRegistration {
                    mode_name: "write".to_string(),
                    callback_name: "cb_write".to_string(),
                    order: 20,
                },
                TkTraceRegistration {
                    mode_name: "write".to_string(),
                    callback_name: "cb_w".to_string(),
                    order: 10,
                },
                TkTraceRegistration {
                    mode_name: "read".to_string(),
                    callback_name: "cb_read".to_string(),
                    order: 30,
                },
            ],
        );

        let write_callbacks =
            collect_trace_callbacks_for_operation(&app, "trace_var", "write", None);
        assert_eq!(
            write_callbacks,
            vec![
                ("cb_w".to_string(), "write".to_string()),
                ("cb_write".to_string(), "write".to_string())
            ]
        );
        let read_callbacks = collect_trace_callbacks_for_operation(&app, "trace_var", "read", None);
        assert_eq!(
            read_callbacks,
            vec![("cb_read".to_string(), "read".to_string())]
        );
    }

    #[test]
    fn trace_callbacks_include_array_mode_for_element_access() {
        let mut app = TkAppState::default();
        app.traces.insert(
            "arr".to_string(),
            vec![
                TkTraceRegistration {
                    mode_name: "array".to_string(),
                    callback_name: "cb_array".to_string(),
                    order: 2,
                },
                TkTraceRegistration {
                    mode_name: "write".to_string(),
                    callback_name: "cb_write".to_string(),
                    order: 1,
                },
            ],
        );

        let callbacks =
            collect_trace_callbacks_for_operation(&app, "arr(index)", "write", Some("index"));
        assert_eq!(
            callbacks,
            vec![
                ("cb_write".to_string(), "write".to_string()),
                ("cb_array".to_string(), "array".to_string()),
            ]
        );
    }

    #[test]
    fn event_generate_binding_sequences_include_virtual_aliases() {
        let mut app = TkAppState::default();
        app.virtual_events.insert(
            "<<ProbeVirtual>>".to_string(),
            vec!["<KeyPress>".to_string(), "<Button-1>".to_string()],
        );
        let key_sequences = event_generate_binding_sequences(&app, "<KeyPress>");
        assert_eq!(
            key_sequences,
            vec!["<KeyPress>".to_string(), "<<ProbeVirtual>>".to_string()]
        );
        let virtual_sequences = event_generate_binding_sequences(&app, "<<ProbeVirtual>>");
        assert_eq!(virtual_sequences, vec!["<<ProbeVirtual>>".to_string()]);
    }

    #[test]
    fn treeview_descendant_detection_handles_parent_chains() {
        let mut treeview = TkTreeviewState::default();
        treeview.items.insert(
            "root_child".to_string(),
            TkTreeviewItem {
                parent: "".to_string(),
                ..TkTreeviewItem::default()
            },
        );
        treeview.items.insert(
            "leaf".to_string(),
            TkTreeviewItem {
                parent: "root_child".to_string(),
                ..TkTreeviewItem::default()
            },
        );
        treeview.items.insert(
            "deep_leaf".to_string(),
            TkTreeviewItem {
                parent: "leaf".to_string(),
                ..TkTreeviewItem::default()
            },
        );

        assert!(treeview_item_is_descendant_of(
            &treeview,
            "deep_leaf",
            "root_child"
        ));
        assert!(treeview_item_is_descendant_of(
            &treeview,
            "leaf",
            "root_child"
        ));
        assert!(!treeview_item_is_descendant_of(
            &treeview,
            "root_child",
            "deep_leaf"
        ));
    }

    #[test]
    fn treeview_event_target_item_priority_is_stable() {
        let mut treeview = TkTreeviewState::default();
        treeview
            .items
            .insert("i1".to_string(), TkTreeviewItem::default());
        treeview
            .items
            .insert("i2".to_string(), TkTreeviewItem::default());
        treeview
            .items
            .insert("i3".to_string(), TkTreeviewItem::default());
        treeview.focus = Some("i2".to_string());
        treeview.selection = vec!["i3".to_string()];

        let mut options = HashMap::new();
        options.insert("-item".to_string(), "i1".to_string());
        assert_eq!(
            treeview_event_target_item(&treeview, &options).as_deref(),
            Some("i1")
        );

        options.clear();
        assert_eq!(
            treeview_event_target_item(&treeview, &options).as_deref(),
            Some("i2")
        );

        treeview.focus = None;
        assert_eq!(
            treeview_event_target_item(&treeview, &options).as_deref(),
            Some("i3")
        );
    }

    #[test]
    fn treeview_visible_order_and_hit_testing_are_deterministic() {
        let mut treeview = TkTreeviewState::default();
        treeview.root_children = vec!["r1".to_string(), "r2".to_string()];
        treeview.items.insert(
            "r1".to_string(),
            TkTreeviewItem {
                parent: String::new(),
                children: vec!["c1".to_string()],
                ..TkTreeviewItem::default()
            },
        );
        treeview.items.insert(
            "c1".to_string(),
            TkTreeviewItem {
                parent: "r1".to_string(),
                ..TkTreeviewItem::default()
            },
        );
        treeview.items.insert(
            "r2".to_string(),
            TkTreeviewItem {
                parent: String::new(),
                ..TkTreeviewItem::default()
            },
        );

        assert_eq!(
            treeview_visible_items(&treeview),
            vec!["r1".to_string(), "c1".to_string(), "r2".to_string()]
        );
        assert_eq!(treeview_hit_item_by_y(&treeview, 0).as_deref(), Some("r1"));
        assert_eq!(treeview_hit_item_by_y(&treeview, 20).as_deref(), Some("c1"));
        assert_eq!(treeview_hit_item_by_y(&treeview, 40).as_deref(), Some("r2"));
        assert_eq!(treeview_hit_item_by_y(&treeview, 60).as_deref(), None);
    }

    #[test]
    fn treeview_column_offset_parser_accepts_hash_indices() {
        assert_eq!(parse_treeview_column_offset("#0"), Some(0));
        assert_eq!(parse_treeview_column_offset("#1"), Some(120));
        assert_eq!(parse_treeview_column_offset("#2"), Some(240));
        assert_eq!(parse_treeview_column_offset("#-1"), None);
        assert_eq!(parse_treeview_column_offset("1"), None);
        assert_eq!(parse_treeview_column_offset("bad"), None);
    }

    #[test]
    fn treeview_noop_generic_subcommands_are_identified() {
        assert!(treeview_subcommand_is_noop_generic_fallback(
            "itemconfigure"
        ));
        assert!(treeview_subcommand_is_noop_generic_fallback(
            "paneconfigure"
        ));
        assert!(treeview_subcommand_is_noop_generic_fallback("replace"));
        assert!(!treeview_subcommand_is_noop_generic_fallback("item"));
        assert!(!treeview_subcommand_is_noop_generic_fallback("selection"));
    }

    #[test]
    fn treeview_strict_index_parser_rejects_non_integer_tokens() {
        assert_eq!(parse_treeview_index_strict("end", 4), Some(4));
        assert_eq!(parse_treeview_index_strict("2", 4), Some(2));
        assert_eq!(parse_treeview_index_strict("-7", 4), Some(0));
        assert_eq!(parse_treeview_index_strict("oops", 4), None);
    }

    #[test]
    fn ttk_insert_index_parser_rejects_non_integer_tokens() {
        assert_eq!(parse_ttk_insert_index_strict("end", 3), Some(3));
        assert_eq!(parse_ttk_insert_index_strict("1", 3), Some(1));
        assert_eq!(parse_ttk_insert_index_strict("-2", 3), Some(0));
        assert_eq!(parse_ttk_insert_index_strict("bad", 3), None);
    }

    #[test]
    fn notebook_index_parser_enforces_bounds() {
        assert_eq!(parse_notebook_index_strict("end", 2), Ok(2));
        assert_eq!(parse_notebook_index_strict("0", 2), Ok(0));
        assert_eq!(parse_notebook_index_strict("1", 2), Ok(1));
        assert!(parse_notebook_index_strict("-1", 2).is_err());
        assert!(parse_notebook_index_strict("2", 2).is_err());
        assert!(parse_notebook_index_strict("tabx", 2).is_err());
    }

    #[test]
    fn treeview_missing_item_detection_reports_first_missing_id() {
        let mut treeview = TkTreeviewState::default();
        treeview
            .items
            .insert("i1".to_string(), TkTreeviewItem::default());
        treeview
            .items
            .insert("i2".to_string(), TkTreeviewItem::default());

        let items = vec!["i1".to_string(), "missing".to_string(), "i2".to_string()];
        assert_eq!(
            first_missing_treeview_item(&treeview, &items),
            Some("missing")
        );

        let existing = vec!["i1".to_string(), "i2".to_string()];
        assert_eq!(first_missing_treeview_item(&treeview, &existing), None);
    }

    #[test]
    fn variable_version_progress_is_monotonic() {
        let mut app = TkAppState::default();
        assert_eq!(variable_version(&app, "name"), 0);
        bump_variable_version(&mut app, "name");
        assert_eq!(variable_version(&app, "name"), 1);
        bump_variable_version(&mut app, "name");
        assert_eq!(variable_version(&app, "name"), 2);
    }

    #[test]
    fn commondialog_supported_command_allowlist_is_stable() {
        for command in [
            "tk_messageBox",
            "tk_getOpenFile",
            "tk_getSaveFile",
            "tk_chooseDirectory",
            "tk_chooseColor",
        ] {
            assert!(commondialog_is_supported_command(command));
            assert!(commondialog_supports_parent(command));
        }
        assert!(!commondialog_is_supported_command("tk_chooseFont"));
        assert!(!commondialog_supports_parent("tk_chooseFont"));
    }

    #[test]
    fn commondialog_option_allowlists_cover_core_dialog_flags() {
        assert!(
            commondialog_allowed_options("tk_messageBox")
                .iter()
                .any(|name| name.eq_ignore_ascii_case("-type"))
        );
        assert!(
            commondialog_allowed_options("tk_getOpenFile")
                .iter()
                .any(|name| name.eq_ignore_ascii_case("-multiple"))
        );
        assert!(
            commondialog_allowed_options("tk_chooseDirectory")
                .iter()
                .any(|name| name.eq_ignore_ascii_case("-mustexist"))
        );
        assert!(
            !commondialog_allowed_options("tk_chooseColor")
                .iter()
                .any(|name| name.eq_ignore_ascii_case("-multiple"))
        );
    }

    #[test]
    fn bool_text_parser_accepts_tcl_prefix_forms() {
        assert_eq!(parse_bool_text("t"), Some(true));
        assert_eq!(parse_bool_text("tr"), Some(true));
        assert_eq!(parse_bool_text("y"), Some(true));
        assert_eq!(parse_bool_text("f"), Some(false));
        assert_eq!(parse_bool_text("fa"), Some(false));
        assert_eq!(parse_bool_text("n"), Some(false));
        assert_eq!(parse_bool_text("o"), None);
        assert_eq!(parse_bool_text(""), None);
    }

    #[test]
    fn filedialog_supported_command_allowlist_is_stable() {
        assert!(filedialog_is_supported_command("tk_getOpenFile"));
        assert!(filedialog_is_supported_command("tk_getSaveFile"));
        assert!(filedialog_is_supported_command("tk_chooseDirectory"));
        assert!(!filedialog_is_supported_command("tk_chooseColor"));
        assert!(!filedialog_is_supported_command("tk_chooseFont"));
    }

    #[test]
    fn messagebox_selection_defaults_are_deterministic() {
        assert_eq!(
            resolve_messagebox_selection("ok", None).as_deref(),
            Ok("ok")
        );
        assert_eq!(
            resolve_messagebox_selection("yesnocancel", None).as_deref(),
            Ok("yes")
        );
        assert_eq!(
            resolve_messagebox_selection("yesno", Some("no")).as_deref(),
            Ok("no")
        );
        assert!(resolve_messagebox_selection("bogus", None).is_err());
        assert!(resolve_messagebox_selection("ok", Some("cancel")).is_err());
    }

    #[test]
    fn messagebox_icon_validation_is_stable() {
        for icon in ["error", "info", "question", "warning", "ERROR"] {
            assert!(messagebox_icon_is_supported(icon));
        }
        assert!(!messagebox_icon_is_supported("bogus"));
    }

    #[test]
    fn dialog_path_joining_handles_common_forms() {
        assert_eq!(join_dialog_path("", ""), "");
        assert_eq!(join_dialog_path("/tmp", "out.txt"), "/tmp/out.txt");
        assert_eq!(join_dialog_path("/tmp/", "out.txt"), "/tmp/out.txt");
        assert_eq!(
            join_dialog_path("C:\\Users\\me", "out.txt"),
            "C:\\Users\\me\\out.txt"
        );
    }

    #[test]
    fn default_extension_application_is_stable() {
        assert_eq!(apply_default_extension("", ".txt"), "");
        assert_eq!(
            apply_default_extension("/tmp/output", ".txt"),
            "/tmp/output.txt"
        );
        assert_eq!(
            apply_default_extension("/tmp/output", "log"),
            "/tmp/output.log"
        );
        assert_eq!(
            apply_default_extension("/tmp/output.txt", ".log"),
            "/tmp/output.txt"
        );
    }

    #[test]
    fn color_literal_normalization_supports_short_and_long_hex() {
        assert_eq!(normalize_color_literal("#abc").as_deref(), Some("#aabbcc"));
        assert_eq!(
            normalize_color_literal("#A1b2C3").as_deref(),
            Some("#A1b2C3")
        );
        assert_eq!(normalize_color_literal("red").as_deref(), Some("red"));
        assert!(normalize_color_literal("#zzzzzz").is_none());
    }

    #[test]
    fn dialog_selection_clamping_is_stable() {
        assert_eq!(clamp_dialog_selection(-2, 0), 0);
        assert_eq!(clamp_dialog_selection(-2, 3), 0);
        assert_eq!(clamp_dialog_selection(1, 3), 1);
        assert_eq!(clamp_dialog_selection(9, 3), 2);
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    #[test]
    fn filehandler_event_name_mapping_is_stable() {
        assert_eq!(
            filehandler_event_name(TK_FILE_EVENT_READABLE),
            Some("readable")
        );
        assert_eq!(
            filehandler_event_name(TK_FILE_EVENT_WRITABLE),
            Some("writable")
        );
        assert_eq!(
            filehandler_event_name(TK_FILE_EVENT_EXCEPTION),
            Some("exception")
        );
        assert_eq!(filehandler_event_name(0), None);
    }

    #[test]
    fn filehandler_command_name_is_deterministic() {
        assert_eq!(
            filehandler_command_name(41, "readable"),
            "::__molt_filehandler_41_readable"
        );
        assert_eq!(
            filehandler_command_name(7, "exception"),
            "::__molt_filehandler_7_exception"
        );
    }

    #[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
    #[test]
    fn filehandler_poll_event_mask_is_stable() {
        let mut registration = TkFileHandlerRegistration {
            callback_bits: 0,
            file_obj_bits: 0,
            commands: HashMap::new(),
        };
        registration
            .commands
            .insert(TK_FILE_EVENT_READABLE, "r".to_string());
        registration
            .commands
            .insert(TK_FILE_EVENT_WRITABLE, "w".to_string());
        assert_eq!(
            filehandler_poll_events(&registration),
            libc::POLLIN | libc::POLLOUT
        );
        registration
            .commands
            .insert(TK_FILE_EVENT_EXCEPTION, "x".to_string());
        assert_eq!(
            filehandler_poll_events(&registration),
            libc::POLLIN | libc::POLLOUT | libc::POLLPRI
        );
    }

    #[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "molt_tk_native")))]
    #[test]
    fn filehandler_revents_translation_is_stable() {
        assert_eq!(
            filehandler_revents_to_mask(libc::POLLIN),
            TK_FILE_EVENT_READABLE
        );
        assert_eq!(
            filehandler_revents_to_mask(libc::POLLOUT),
            TK_FILE_EVENT_WRITABLE
        );
        assert_eq!(
            filehandler_revents_to_mask(libc::POLLERR | libc::POLLNVAL),
            TK_FILE_EVENT_EXCEPTION
        );
        assert_eq!(
            filehandler_revents_to_mask(libc::POLLHUP | libc::POLLPRI),
            TK_FILE_EVENT_READABLE | TK_FILE_EVENT_EXCEPTION
        );
    }
}
