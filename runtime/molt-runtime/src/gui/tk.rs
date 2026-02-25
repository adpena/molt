#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
use crate::object::ops::format_obj_str;
use crate::{
    MoltObject, PyToken, call_callable0, dec_ref_bits, decode_value_list, exception_pending,
    has_capability, inc_ref_bits, obj_from_bits, raise_exception, string_obj_to_owned, to_f64,
    to_i64,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
use libloading::Library;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::PathBuf,
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
type TclGetStringResultFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
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
    get_string_result: TclGetStringResultFn,
    do_one_event: TclDoOneEventFn,
    split_list: TclSplitListFn,
    merge: TclMergeFn,
    free: TclFreeFn,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
fn tcl_library_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("MOLT_TCL_LIB") {
        if !path.trim().is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }
    let mut preferred_names: Vec<&'static str> = Vec::new();
    if cfg!(target_os = "macos") {
        preferred_names.extend(["libtcl8.7.dylib", "libtcl8.6.dylib", "libtcl.dylib"]);
        candidates.push(PathBuf::from(
            "/System/Library/Frameworks/Tcl.framework/Tcl",
        ));
        candidates.push(PathBuf::from(
            "/opt/homebrew/opt/tcl-tk/lib/libtcl8.7.dylib",
        ));
        candidates.push(PathBuf::from(
            "/opt/homebrew/opt/tcl-tk/lib/libtcl8.6.dylib",
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
                let api = TclApi {
                    find_executable: std::mem::transmute(load(b"Tcl_FindExecutable\0")?),
                    create_interp: std::mem::transmute(load(b"Tcl_CreateInterp\0")?),
                    delete_interp: std::mem::transmute(load(b"Tcl_DeleteInterp\0")?),
                    init: std::mem::transmute(load(b"Tcl_Init\0")?),
                    eval_ex: std::mem::transmute(load(b"Tcl_EvalEx\0")?),
                    get_string_result: std::mem::transmute(load(b"Tcl_GetStringResult\0")?),
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
        let mut rendered = Vec::with_capacity(parts.len());
        for part in parts {
            rendered.push(self.render_part(&part)?);
        }
        let script = tcl_merge_args(self.api, &rendered)?;
        let rc = unsafe {
            (self.api.eval_ex)(
                self.interp_ptr(),
                script.as_ptr() as *const c_char,
                script.len() as c_int,
                0,
            )
        };
        let result = tcl_result_string(self.api, self.interp_ptr());
        if rc != TCL_OK {
            return Err(if result.is_empty() {
                "Tcl_EvalEx failed".to_string()
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
    Call,
    BindCommand,
    FileHandlerCreate,
    FileHandlerDelete,
    DestroyWidget,
    LastError,
    DialogShow,
    CommonDialogShow,
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
            Self::Call => "molt_tk_call",
            Self::BindCommand => "molt_tk_bind_command",
            Self::FileHandlerCreate => "molt_tk_filehandler_create",
            Self::FileHandlerDelete => "molt_tk_filehandler_delete",
            Self::DestroyWidget => "molt_tk_destroy_widget",
            Self::LastError => "molt_tk_last_error",
            Self::DialogShow => "molt_tk_dialog_show",
            Self::CommonDialogShow => "molt_tk_commondialog_show",
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
                | Self::Call
                | Self::BindCommand
                | Self::FileHandlerCreate
                | Self::FileHandlerDelete
                | Self::DestroyWidget
                | Self::DialogShow
                | Self::CommonDialogShow
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
    widgets: HashMap<String, TkWidgetState>,
    last_error: Option<String>,
    next_after_id: u64,
    quit_requested: bool,
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    interpreter: Option<TclInterpreter>,
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    tk_loaded: bool,
}

struct TkWidgetState {
    widget_command: String,
    options: HashMap<String, u64>,
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

enum TkEvent {
    Callback { callback_bits: u64, _token: String },
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

fn clear_widget_refs(py: &PyToken<'_>, widget: TkWidgetState) {
    let mut options = widget.options;
    clear_value_map_refs(py, &mut options);
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
            TkEvent::Callback { callback_bits, .. } => dec_ref_bits(py, callback_bits),
        }
    }
    for widget in app.widgets.drain().map(|(_, widget)| widget) {
        clear_widget_refs(py, widget);
    }
    app.last_error = None;
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

fn clear_filehandler_registration_locked(
    py: &PyToken<'_>,
    app: &mut TkAppState,
    fd: i64,
) -> Result<(), u64> {
    let Some(registration) = app.filehandlers.remove(&fd) else {
        return Ok(());
    };
    for (mask, command_name) in &registration.commands {
        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        if let Some(event_name) = filehandler_event_name(*mask) {
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
    fd: i64,
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
                    fd.to_string(),
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

fn split_eval_script(script: &str) -> Vec<String> {
    script
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
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

fn commondialog_option_text(options: &[(String, u64)], key: &str) -> Option<String> {
    let bits = commondialog_option_value_bits(options, key)?;
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    if let Some(value) = to_i64(obj) {
        return Some(value.to_string());
    }
    if let Some(value) = to_f64(obj) {
        return Some(value.to_string());
    }
    None
}

fn commondialog_supports_parent(command: &str) -> bool {
    matches!(
        command,
        "tk_messageBox"
            | "tk_getOpenFile"
            | "tk_getSaveFile"
            | "tk_chooseDirectory"
            | "tk_chooseColor"
    )
}

fn commondialog_messagebox_fallback_choice(options: &[(String, u64)]) -> String {
    let dialog_type = commondialog_option_text(options, "-type")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ok".to_string());
    let default_choice = commondialog_option_text(options, "-default")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());

    let (allowed, fallback): (&[&str], &str) = match dialog_type.as_str() {
        "okcancel" => (&["ok", "cancel"], "cancel"),
        "yesno" => (&["yes", "no"], "no"),
        "yesnocancel" => (&["yes", "no", "cancel"], "cancel"),
        "retrycancel" => (&["retry", "cancel"], "cancel"),
        "abortretryignore" => (&["abort", "retry", "ignore"], "ignore"),
        "ok" => (&["ok"], "ok"),
        _ => (&["ok"], "ok"),
    };

    if let Some(choice) = default_choice
        && allowed
            .iter()
            .any(|candidate| *candidate == choice.as_str())
    {
        return choice;
    }
    fallback.to_string()
}

fn commondialog_fallback_result(
    py: &PyToken<'_>,
    handle: i64,
    command: &str,
    options: &[(String, u64)],
) -> u64 {
    let result = match command {
        "tk_messageBox" => commondialog_messagebox_fallback_choice(options),
        "tk_getOpenFile" | "tk_getSaveFile" | "tk_chooseDirectory" | "tk_chooseColor" => {
            String::new()
        }
        _ => {
            return raise_tcl_for_handle(
                py,
                handle,
                format!("unsupported commondialog command \"{command}\""),
            );
        }
    };
    clear_last_error(handle);
    match alloc_string_bits(py, &result) {
        Ok(bits) => bits,
        Err(bits) => bits,
    }
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

fn pop_next_event(py: &PyToken<'_>, handle: i64) -> Result<Option<TkEvent>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    Ok(app.event_queue.pop_front())
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
        TkEvent::Callback { callback_bits, .. } => {
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

fn handle_set_command(py: &PyToken<'_>, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "set expects 1 or 2 arguments",
        ));
    }
    let var_name = get_string_arg(py, handle, args[1], "set variable name")?;
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
        app.last_error = None;
        return Ok(bits);
    }
    let value_bits = args[2];
    inc_ref_bits(py, value_bits);
    if let Some(old_bits) = app.variables.insert(var_name, value_bits) {
        dec_ref_bits(py, old_bits);
    }
    app.last_error = None;
    inc_ref_bits(py, value_bits);
    Ok(value_bits)
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
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if let Some(old_bits) = app.variables.remove(&var_name) {
        dec_ref_bits(py, old_bits);
    }
    app.last_error = None;
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
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "after expects exactly one delay argument in headless mode",
        ));
    }
    let Some(delay_ms) = to_i64(obj_from_bits(args[1])) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "after delay must be an integer",
        ));
    };
    if delay_ms < 0 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "after delay must be non-negative",
        ));
    }
    clear_last_error(handle);
    Ok(MoltObject::none().bits())
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
        },
    );
    app.last_error = None;
    drop(registry);
    alloc_string_bits(py, &widget_path)
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
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let widget_kind = app
                .widgets
                .get(widget_path)
                .map(|widget| widget.widget_command.clone())
                .unwrap_or_else(|| "widget".to_string());
            Err(app_tcl_error_locked(
                py,
                app,
                format!(
                    "unsupported {widget_kind} widget subcommand \"{subcommand}\" for \"{widget_path}\""
                ),
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
    let tokens = split_eval_script(&script);
    if tokens.is_empty() {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    let mut token_bits = Vec::with_capacity(tokens.len());
    for token in tokens {
        match alloc_string_bits(py, &token) {
            Ok(bits) => token_bits.push(bits),
            Err(bits) => {
                for owned in token_bits {
                    dec_ref_bits(py, owned);
                }
                return Err(bits);
            }
        }
    }
    let out = tk_call_dispatch(py, handle, &token_bits);
    for owned in token_bits {
        dec_ref_bits(py, owned);
    }
    out
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
    let tokens = split_eval_script(&script);
    if tokens.is_empty() {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    let mut token_bits = Vec::with_capacity(tokens.len());
    for token in tokens {
        match alloc_string_bits(py, &token) {
            Ok(bits) => token_bits.push(bits),
            Err(bits) => {
                for owned in token_bits {
                    dec_ref_bits(py, owned);
                }
                return Err(bits);
            }
        }
    }
    let out = tk_call_dispatch(py, handle, &token_bits);
    for owned in token_bits {
        dec_ref_bits(py, owned);
    }
    out
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
        if command == "loadtk" {
            return native_loadtk_command(py, handle, args);
        }
        return run_tcl_command(py, handle, args);
    }

    #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
    {
        match command.as_str() {
            "set" => handle_set_command(py, handle, args),
            "unset" => handle_unset_command(py, handle, args),
            "loadtk" => handle_loadtk_command(py, handle, args),
            "after" => handle_after_command(py, handle, args),
            "rename" => handle_rename_command(py, handle, args),
            "eval" => handle_eval_command(py, handle, args),
            "source" => handle_source_command(py, handle, args),
            "expr" => handle_expr_command(py, handle, args),
            _ => {
                if command.starts_with('.') {
                    return handle_widget_path_command(py, handle, &command, args);
                }
                if args.len() >= 2
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

fn dispatch_next_pending_event(py: &PyToken<'_>, handle: i64) -> Result<bool, u64> {
    let Some(event) = pop_next_event(py, handle)? else {
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
            app.last_error = None;
            drop(registry);
            return match alloc_string_bits(_py, &after_token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "molt_tk_native")))]
        {
            app.event_queue.push_back(TkEvent::Callback {
                callback_bits,
                _token: token.clone(),
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

        let master_path = match get_string_arg(_py, handle, master_path_bits, "dialog master path")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let title = match get_string_arg_allow_none(_py, handle, title_bits, "dialog title") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let text = match get_string_arg_allow_none(_py, handle, text_bits, "dialog text") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bitmap = match get_string_arg_allow_none(_py, handle, bitmap_bits, "dialog bitmap") {
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
                master_path,
                title,
                text,
                bitmap,
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

        if !commondialog_supports_parent(command.as_str()) {
            return commondialog_fallback_result(_py, handle, command.as_str(), &options);
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
        {
            let native_ready = {
                let mut registry = tk_registry().lock().unwrap();
                let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                    return raise_invalid_handle_error(_py);
                };
                if app.tk_loaded {
                    true
                } else {
                    let Some(interp) = app.interpreter.as_ref() else {
                        return app_tcl_error_locked(
                            _py,
                            app,
                            "tk runtime interpreter is unavailable",
                        );
                    };
                    match interp.eval(("package", "require", "Tk")) {
                        Ok(_) => {
                            app.tk_loaded = true;
                            true
                        }
                        Err(_) => {
                            app.last_error = None;
                            false
                        }
                    }
                }
            };

            if native_ready {
                let inject_parent = !_master_path.is_empty()
                    && commondialog_option_value_bits(&options, "-parent").is_none();
                let mut argv =
                    Vec::with_capacity(1 + options.len() * 2 + usize::from(inject_parent) * 2);
                let mut allocated =
                    Vec::with_capacity(1 + options.len() + usize::from(inject_parent) * 2);

                let command_arg = match alloc_string_bits(_py, &command) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                };
                allocated.push(command_arg);
                argv.push(command_arg);

                if inject_parent {
                    let parent_name = match alloc_string_bits(_py, "-parent") {
                        Ok(bits) => bits,
                        Err(bits) => {
                            for bits in allocated {
                                dec_ref_bits(_py, bits);
                            }
                            return bits;
                        }
                    };
                    allocated.push(parent_name);
                    argv.push(parent_name);

                    let parent_value = match alloc_string_bits(_py, &_master_path) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            for bits in allocated {
                                dec_ref_bits(_py, bits);
                            }
                            return bits;
                        }
                    };
                    allocated.push(parent_value);
                    argv.push(parent_value);
                }

                for (name, value_bits) in &options {
                    let name_bits = match alloc_string_bits(_py, name) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            for bits in allocated {
                                dec_ref_bits(_py, bits);
                            }
                            return bits;
                        }
                    };
                    allocated.push(name_bits);
                    argv.push(name_bits);
                    argv.push(*value_bits);
                }

                let out = tk_call_dispatch(_py, handle, &argv);
                for bits in allocated {
                    dec_ref_bits(_py, bits);
                }
                return match out {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
            }
        }

        commondialog_fallback_result(_py, handle, command.as_str(), &options)
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

        let parent_path =
            match get_string_arg(_py, handle, parent_path_bits, "simpledialog parent path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let title = match get_string_arg_allow_none(_py, handle, title_bits, "simpledialog title") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let prompt = match get_string_arg(_py, handle, prompt_bits, "simpledialog prompt") {
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
                if !title.is_empty() {
                    run_setup(
                        app,
                        vec![
                            "wm".to_string(),
                            "title".to_string(),
                            dialog_path.clone(),
                            title.clone(),
                        ],
                    )?;
                }
                if !parent_path.is_empty() {
                    run_setup(
                        app,
                        vec![
                            "wm".to_string(),
                            "transient".to_string(),
                            dialog_path.clone(),
                            parent_path.clone(),
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
                        prompt.clone(),
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
    fn eval_split_is_whitespace_deterministic() {
        assert_eq!(
            split_eval_script("  set   answer   42  "),
            vec!["set", "answer", "42"]
        );
        assert!(split_eval_script(" \t\n ").is_empty());
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
}
