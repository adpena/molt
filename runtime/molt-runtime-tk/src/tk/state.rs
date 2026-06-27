use super::callbacks::clear_filehandler_registration_locked;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::native::unregister_all_tcl_callback_procs;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{TclInterpreter, eval, get, new};
use crate::bridge::{
    alloc_string_result, dec_ref_bits, has_capability, inc_ref_bits, raise_exception_u64, to_i64,
};
use molt_runtime_core::prelude::{PyToken, obj_from_bits};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

pub(super) const TK_UNAVAILABLE_LABEL: &str = "tkinter runtime support is not implemented yet";
pub(super) const TK_CAPABILITY_GUI_WINDOW: &str = "gui.window";
pub(super) const TK_CAPABILITY_PROCESS_SPAWN: &str = "process.spawn";
pub(super) const TK_BLOCKER_WASM_TARGET: &str = "target.wasm32";
pub(super) const TK_BLOCKER_BACKEND_UNIMPLEMENTED: &str = "backend.not_implemented";
pub(super) const TK_BLOCKER_CAP_GUI_WINDOW: &str = "capability.gui.window";
pub(super) const TK_BLOCKER_CAP_PROCESS_SPAWN: &str = "capability.process.spawn";
pub(super) const TK_BLOCKER_PLATFORM_LINUX_DISPLAY: &str = "platform.linux.display";
pub(super) const TK_BLOCKER_PLATFORM_MACOS_MAIN_THREAD: &str = "platform.macos.main_thread";
pub(super) const TK_FILE_EVENT_READABLE: i64 = 2;
pub(super) const TK_FILE_EVENT_WRITABLE: i64 = 4;
pub(super) const TK_FILE_EVENT_EXCEPTION: i64 = 8;
pub(super) const TK_DONT_WAIT_FLAG: i32 = 2;
pub(super) const TK_BIND_SUBST_FORMAT_STR: &str =
    "%# %b %f %h %k %s %t %w %x %y %A %E %K %N %W %T %X %Y %D";

#[cfg(target_arch = "wasm32")]
pub(super) const TK_BACKEND_IMPLEMENTED: bool = false;
#[cfg(not(target_arch = "wasm32"))]
pub(super) const TK_BACKEND_IMPLEMENTED: bool = true;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tk_runtime_available() -> bool {
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

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_runtime_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE
        .get_or_init(|| std::panic::catch_unwind(|| TclInterpreter::new().is_ok()).unwrap_or(false))
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn tcl_runtime_available() -> bool {
    true
}

#[cfg(target_arch = "wasm32")]
pub(super) fn tcl_runtime_available() -> bool {
    false
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn tk_runtime_available() -> bool {
    // Headless phase-0 path remains available without compiling Tcl bindings.
    true
}

#[cfg(target_arch = "wasm32")]
pub(super) fn tk_runtime_available() -> bool {
    false
}

#[derive(Clone, Copy)]
pub(super) enum TkOperation {
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
pub(super) struct TkGateState {
    pub(super) wasm_unsupported: bool,
    pub(super) backend_unimplemented: bool,
    pub(super) missing_gui_window: bool,
    pub(super) missing_process_spawn: bool,
    pub(super) missing_linux_display: bool,
    pub(super) missing_macos_main_thread: bool,
}

#[derive(Default)]
pub(super) struct TkRegistry {
    pub(super) next_handle: i64,
    pub(super) apps: HashMap<i64, TkAppState>,
}

#[derive(Default)]
pub(super) struct TkAppState {
    pub(super) callbacks: HashMap<String, u64>,
    pub(super) one_shot_callbacks: HashSet<String>,
    pub(super) filehandlers: HashMap<i64, TkFileHandlerRegistration>,
    pub(super) filehandler_commands: HashMap<String, TkFileHandlerCommand>,
    pub(super) event_queue: VecDeque<TkEvent>,
    pub(super) variables: HashMap<String, u64>,
    pub(super) variable_versions: HashMap<String, u64>,
    pub(super) next_variable_version: u64,
    pub(super) widgets: HashMap<String, TkWidgetState>,
    pub(super) images: HashMap<String, TkImageState>,
    pub(super) fonts: HashMap<String, TkFontState>,
    pub(super) tix_options: HashMap<String, u64>,
    pub(super) option_db: HashMap<String, u64>,
    pub(super) strict_motif: bool,
    pub(super) ttk_style: TkTtkStyleState,
    pub(super) bind_scripts: HashMap<String, HashMap<String, String>>,
    pub(super) bindtags: HashMap<String, Vec<String>>,
    pub(super) virtual_events: HashMap<String, Vec<String>>,
    pub(super) traces: HashMap<String, Vec<TkTraceRegistration>>,
    pub(super) next_trace_order: u64,
    pub(super) pack_slaves: Vec<String>,
    pub(super) grid_slaves: Vec<String>,
    pub(super) place_slaves: Vec<String>,
    pub(super) pack_propagate: HashMap<String, bool>,
    pub(super) grid_propagate: HashMap<String, bool>,
    pub(super) focus_widget: Option<String>,
    pub(super) grab_widget: Option<String>,
    pub(super) grab_is_global: bool,
    pub(super) clipboard_text: String,
    pub(super) selection_text: String,
    pub(super) selection_owner: Option<String>,
    pub(super) after_command_tokens: HashMap<String, String>,
    pub(super) after_command_kinds: HashMap<String, String>,
    pub(super) after_due_at_ms: HashMap<String, u64>,
    pub(super) after_clock_ms: u64,
    pub(super) wm: TkWmState,
    pub(super) atoms_by_name: HashMap<String, i64>,
    pub(super) atoms_by_id: HashMap<i64, String>,
    pub(super) next_atom_id: i64,
    pub(super) last_error: Option<String>,
    pub(super) next_after_id: u64,
    pub(super) next_callback_command_id: u64,
    pub(super) quit_requested: bool,
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
    pub(super) interpreter: Option<TclInterpreter>,
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
    pub(super) tk_loaded: bool,
}

#[derive(Default)]
pub(super) struct TkImageState {
    pub(super) kind: String,
    pub(super) options: HashMap<String, u64>,
}

#[derive(Default)]
pub(super) struct TkFontState {
    pub(super) options: HashMap<String, u64>,
}

#[derive(Default)]
pub(super) struct TkMenuEntryState {
    pub(super) item_type: String,
    pub(super) options: HashMap<String, u64>,
}

#[derive(Default)]
pub(super) struct TkWidgetState {
    pub(super) widget_command: String,
    pub(super) options: HashMap<String, u64>,
    pub(super) wm: Option<TkWmState>,
    pub(super) treeview: Option<TkTreeviewState>,
    pub(super) ttk_state: HashSet<String>,
    pub(super) ttk_values: HashMap<String, u64>,
    pub(super) ttk_items: Vec<String>,
    pub(super) ttk_item_options: HashMap<String, HashMap<String, u64>>,
    pub(super) ttk_sash_positions: HashMap<i64, i64>,
    pub(super) manager: Option<String>,
    pub(super) pack_options: HashMap<String, u64>,
    pub(super) grid_options: HashMap<String, u64>,
    pub(super) place_options: HashMap<String, u64>,
    pub(super) grid_columnconfigure: HashMap<String, HashMap<String, u64>>,
    pub(super) grid_rowconfigure: HashMap<String, HashMap<String, u64>>,
    pub(super) tag_bindings: HashMap<String, HashMap<String, String>>,
    pub(super) text_tag_ranges: HashMap<String, Vec<(usize, usize)>>,
    pub(super) text_tag_options: HashMap<String, HashMap<String, String>>,
    pub(super) text_tag_order: Vec<String>,
    pub(super) text_marks: HashMap<String, usize>,
    pub(super) text_mark_gravity: HashMap<String, String>,
    pub(super) text_value: String,
    pub(super) list_items: Vec<u64>,
    pub(super) list_item_options: HashMap<usize, HashMap<String, u64>>,
    pub(super) list_selection: HashSet<usize>,
    pub(super) list_active_index: Option<usize>,
    pub(super) menu_entries: Vec<TkMenuEntryState>,
    pub(super) menu_active_index: Option<usize>,
    pub(super) menu_posted_at: Option<(i64, i64)>,
    pub(super) pane_children: Vec<String>,
    pub(super) pane_child_options: HashMap<String, HashMap<String, u64>>,
    pub(super) selection_anchor: Option<usize>,
    pub(super) selection_range: Option<(usize, usize)>,
    pub(super) insert_cursor: usize,
    pub(super) text_edit_modified: bool,
    pub(super) next_item_id: i64,
}

pub(super) struct TkFileHandlerRegistration {
    pub(super) callback_bits: u64,
    pub(super) file_obj_bits: u64,
    pub(super) commands: HashMap<i64, String>,
}

#[derive(Clone, Copy)]
pub(super) struct TkFileHandlerCommand {
    pub(super) fd: i64,
    pub(super) mask: i64,
}

#[derive(Clone)]
pub(super) struct TkTraceRegistration {
    pub(super) mode_name: String,
    pub(super) callback_name: String,
    pub(super) order: u64,
}

#[derive(Default)]
pub(super) struct TkTtkStyleState {
    pub(super) configure: HashMap<String, HashMap<String, u64>>,
    pub(super) style_map: HashMap<String, HashMap<String, u64>>,
    pub(super) layouts: HashMap<String, u64>,
    pub(super) elements: HashSet<String>,
    pub(super) element_options: HashMap<String, Vec<String>>,
    pub(super) themes: HashSet<String>,
    pub(super) current_theme: Option<String>,
}

pub(super) struct TkWmState {
    pub(super) title: String,
    pub(super) geometry: String,
    pub(super) state: String,
    pub(super) attributes: HashMap<String, u64>,
    pub(super) aspect: Option<(i64, i64, i64, i64)>,
    pub(super) client: String,
    pub(super) colormapwindows: Vec<String>,
    pub(super) command: Vec<String>,
    pub(super) focusmodel: String,
    pub(super) frame: String,
    pub(super) grid: Option<(i64, i64, i64, i64)>,
    pub(super) group: Option<String>,
    pub(super) iconbitmap: String,
    pub(super) iconmask: String,
    pub(super) resizable_width: bool,
    pub(super) resizable_height: bool,
    pub(super) minsize: (i64, i64),
    pub(super) maxsize: (i64, i64),
    pub(super) overrideredirect: bool,
    pub(super) transient: Option<String>,
    pub(super) iconname: String,
    pub(super) iconposition: Option<(i64, i64)>,
    pub(super) iconwindow: Option<String>,
    pub(super) positionfrom: String,
    pub(super) sizefrom: String,
    pub(super) protocols: HashMap<String, String>,
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
pub(super) struct TkTreeviewState {
    pub(super) items: HashMap<String, TkTreeviewItem>,
    pub(super) root_children: Vec<String>,
    pub(super) selection: Vec<String>,
    pub(super) focus: Option<String>,
    pub(super) columns: HashMap<String, HashMap<String, u64>>,
    pub(super) headings: HashMap<String, HashMap<String, u64>>,
    pub(super) tags: HashMap<String, TkTreeTagState>,
    pub(super) next_auto_id: u64,
}

#[derive(Default)]
pub(super) struct TkTreeviewItem {
    pub(super) parent: String,
    pub(super) children: Vec<String>,
    pub(super) options: HashMap<String, u64>,
    pub(super) values: HashMap<String, u64>,
}

#[derive(Default)]
pub(super) struct TkTreeTagState {
    pub(super) options: HashMap<String, u64>,
    pub(super) bindings: HashMap<String, String>,
}

pub(super) enum TkEvent {
    Callback {
        token: String,
    },
    Script {
        token: String,
        commands: Vec<Vec<String>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TkExprLiteral {
    Int(i64),
    Float(f64),
}

pub(super) fn has_gui_window_capability(py: &PyToken) -> bool {
    has_capability(py, TK_CAPABILITY_GUI_WINDOW) || has_capability(py, "gui")
}

pub(super) fn has_process_spawn_capability(py: &PyToken) -> bool {
    has_capability(py, TK_CAPABILITY_PROCESS_SPAWN) || has_capability(py, "process")
}

#[cfg(target_os = "linux")]
pub(super) fn env_var_non_empty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.to_string_lossy().trim().is_empty())
}

#[cfg(target_os = "linux")]
pub(super) fn linux_display_available() -> bool {
    env_var_non_empty("DISPLAY") || env_var_non_empty("WAYLAND_DISPLAY")
}

#[cfg(not(target_os = "linux"))]
pub(super) fn linux_display_available() -> bool {
    true
}

#[cfg(target_os = "macos")]
pub(super) fn macos_on_main_thread() -> bool {
    unsafe { libc::pthread_main_np() == 1 }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn macos_on_main_thread() -> bool {
    true
}

pub(super) fn tk_gate_state(py: &PyToken, op: TkOperation) -> TkGateState {
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

pub(super) fn tk_registry() -> &'static Mutex<TkRegistry> {
    static REGISTRY: OnceLock<Mutex<TkRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(TkRegistry {
            next_handle: 1,
            apps: HashMap::new(),
        })
    })
}

pub(super) fn tk_blockers(state: &TkGateState) -> Vec<&'static str> {
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

pub(super) fn has_platform_preflight_blockers(state: &TkGateState) -> bool {
    state.missing_linux_display || state.missing_macos_main_thread
}

pub(super) fn format_tk_unavailable_message(op: TkOperation, state: &TkGateState) -> String {
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

pub(super) fn format_permission_error_message(state: &TkGateState) -> String {
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

pub(super) fn raise_tk_gate_error(py: &PyToken, op: TkOperation, state: &TkGateState) -> u64 {
    if state.wasm_unsupported {
        return raise_exception_u64(
            py,
            "NotImplementedError",
            &format_tk_unavailable_message(op, state),
        );
    }
    if state.backend_unimplemented {
        return raise_exception_u64(
            py,
            "RuntimeError",
            &format_tk_unavailable_message(op, state),
        );
    }
    if state.missing_gui_window || state.missing_process_spawn {
        return raise_exception_u64(
            py,
            "PermissionError",
            &format_permission_error_message(state),
        );
    }
    if has_platform_preflight_blockers(state) {
        return raise_exception_u64(
            py,
            "RuntimeError",
            &format_tk_unavailable_message(op, state),
        );
    }
    raise_exception_u64(
        py,
        "RuntimeError",
        &format!("internal tkinter gate error ({})", op.symbol()),
    )
}

pub(super) fn require_tk_operation(py: &PyToken, op: TkOperation) -> Result<(), u64> {
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

pub(super) fn require_tk_app_new(py: &PyToken, _use_tk: bool) -> Result<(), u64> {
    let state = tk_gate_state(py, TkOperation::AppNew);
    if state.wasm_unsupported
        || state.backend_unimplemented
        || state.missing_gui_window
        || state.missing_process_spawn
        || has_platform_preflight_blockers(&state)
    {
        return Err(raise_tk_gate_error(py, TkOperation::AppNew, &state));
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
    if _use_tk && !tk_runtime_available() {
        let unavailable = TkGateState {
            backend_unimplemented: true,
            ..TkGateState::default()
        };
        return Err(raise_tk_gate_error(py, TkOperation::AppNew, &unavailable));
    }
    Ok(())
}

pub(super) fn raise_tcl_error(py: &PyToken, message: &str) -> u64 {
    raise_exception_u64(py, "RuntimeError", &format!("TclError: {message}"))
}

pub(super) fn alloc_string_bits(py: &PyToken, value: &str) -> Result<u64, u64> {
    let _ = py;
    alloc_string_result(value, "failed to allocate tkinter string")
}

pub(super) fn app_tcl_error_locked(
    py: &PyToken,
    app: &mut TkAppState,
    message: impl Into<String>,
) -> u64 {
    let message = message.into();
    app.last_error = Some(message.clone());
    raise_tcl_error(py, &message)
}

pub(super) fn raise_invalid_handle_error(py: &PyToken) -> u64 {
    raise_exception_u64(py, "ValueError", "invalid tkinter app handle")
}

pub(super) fn parse_app_handle(py: &PyToken, app_bits: u64) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(app_bits)) else {
        return Err(raise_invalid_handle_error(py));
    };
    if handle <= 0 {
        return Err(raise_invalid_handle_error(py));
    }
    Ok(handle)
}

pub(super) fn app_mut_from_registry<'a>(
    py: &PyToken,
    registry: &'a mut TkRegistry,
    handle: i64,
) -> Result<&'a mut TkAppState, u64> {
    registry
        .apps
        .get_mut(&handle)
        .ok_or_else(|| raise_invalid_handle_error(py))
}

pub(super) fn clear_value_map_refs(py: &PyToken, values: &mut HashMap<String, u64>) {
    for bits in values.drain().map(|(_, bits)| bits) {
        dec_ref_bits(py, bits);
    }
}

pub(super) fn clear_nested_value_map_refs(
    py: &PyToken,
    values: &mut HashMap<String, HashMap<String, u64>>,
) {
    for mut nested in values.drain().map(|(_, nested)| nested) {
        clear_value_map_refs(py, &mut nested);
    }
}

pub(super) fn value_map_set_bits(
    py: &PyToken,
    values: &mut HashMap<String, u64>,
    key: String,
    bits: u64,
) {
    inc_ref_bits(py, bits);
    if let Some(old_bits) = values.insert(key, bits) {
        dec_ref_bits(py, old_bits);
    }
}

pub(super) fn clear_treeview_refs(py: &PyToken, treeview: &mut TkTreeviewState) {
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

pub(super) fn clear_widget_refs(py: &PyToken, widget: TkWidgetState) {
    let mut options = widget.options;
    clear_value_map_refs(py, &mut options);
    if let Some(mut wm) = widget.wm {
        clear_wm_refs(py, &mut wm);
    }
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

pub(super) fn clear_ttk_style_refs(py: &PyToken, style: &mut TkTtkStyleState) {
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

pub(super) fn clear_wm_refs(py: &PyToken, wm: &mut TkWmState) {
    clear_value_map_refs(py, &mut wm.attributes);
}

pub(super) fn wm_state_for_path<'a>(app: &'a TkAppState, toplevel: &str) -> Option<&'a TkWmState> {
    if toplevel == "." {
        return Some(&app.wm);
    }
    app.widgets
        .get(toplevel)
        .and_then(|widget| widget.wm.as_ref())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn wm_state_for_path_mut<'a>(
    app: &'a mut TkAppState,
    toplevel: &str,
) -> Option<&'a mut TkWmState> {
    if toplevel == "." {
        return Some(&mut app.wm);
    }
    app.widgets
        .get_mut(toplevel)
        .and_then(|widget| widget.wm.as_mut())
}

pub(super) fn drop_app_state_refs(py: &PyToken, app: &mut TkAppState) {
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
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
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
    {
        app.interpreter = None;
        app.tk_loaded = false;
    }
}
