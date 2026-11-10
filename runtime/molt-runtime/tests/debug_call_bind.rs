use molt_obj_model::MoltObject;
use std::process::Command;
use std::sync::Once;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/debug_call_bind"
));

#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

mod captured_runtime_children {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/captured_runtime_children.rs"
    ));
}

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_runtime_exit(code_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_exception_clear") ]
    fn molt_exception_clear(context: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_object_getattr_bytes") ]
    fn molt_object_getattr_bytes(
        context: u64,
        obj_bits: u64,
        name_ptr: *const u8,
        name_len: u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_object_call") ]
    fn molt_object_call(context: u64, callable_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_bool_builtin") ]
    fn molt_bool_builtin(context: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_func_new_builtin") ]
    fn molt_func_new_builtin(context: u64, fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_list_builtin") ]
    fn molt_list_builtin(context: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_list_append") ]
    fn molt_list_append(context: u64, list_bits: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_tuple_builtin") ]
    fn molt_tuple_builtin(context: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_tuple_from_list") ]
    fn molt_tuple_from_list(context: u64, bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_string_from_bytes") ]
    fn molt_string_from_bytes(context: u64, ptr: *const u8, len_bits: u64, out: *mut u64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_new") ]
    fn molt_callargs_new(context: u64, pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_push_pos") ]
    fn molt_callargs_push_pos(context: u64, builder_bits: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_call_bind_ic") ]
    fn molt_call_bind_ic(context: u64, site_bits: u64, call_bits: u64, builder_bits: u64) -> u64;
}

static INIT: Once = Once::new();

fn init() -> molt_runtime_core::CoreGilGuard {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });

    let guard = molt_runtime_core::CoreGilGuard::new();
    let py = unsafe { guard.__token_unchecked() };
    unsafe { molt_exception_clear(py.attached_context()) };
    guard
}

fn none() -> u64 {
    MoltObject::none().bits()
}

fn missing(py: &molt_runtime_core::PyToken) -> u64 {
    unsafe { molt_runtime::molt_missing(py.attached_context()) }
}

fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}

fn empty_list(py: &molt_runtime_core::PyToken) -> u64 {
    unsafe { molt_list_builtin(py.attached_context(), missing(py)) }
}

fn empty_tuple(py: &molt_runtime_core::PyToken) -> u64 {
    unsafe { molt_tuple_builtin(py.attached_context(), missing(py)) }
}

fn string_bits(py: &molt_runtime_core::PyToken, text: &str) -> u64 {
    let mut out = 0u64;
    let rc = unsafe {
        molt_string_from_bytes(
            py.attached_context(),
            text.as_ptr(),
            text.len() as u64,
            &mut out,
        )
    };
    assert_eq!(rc, 0, "string alloc failed for {text}");
    out
}

fn append_method_bits(py: &molt_runtime_core::PyToken, list_bits: u64) -> u64 {
    unsafe { molt_object_getattr_bytes(py.attached_context(), list_bits, b"append".as_ptr(), 6) }
}

fn function_with_required_kwonly_metadata(py: &molt_runtime_core::PyToken) -> u64 {
    let fn_ptr = molt_bool_builtin as *const () as usize as u64;
    let func_bits = unsafe { molt_func_new_builtin(py.attached_context(), fn_ptr, fn_ptr, 1) };
    let arg_names_bits = empty_tuple(py);
    let kwonly_list_bits = empty_list(py);
    let kwonly_name_bits = string_bits(py, "required");
    unsafe {
        let _ = molt_list_append(py.attached_context(), kwonly_list_bits, kwonly_name_bits);
    }
    let kwonly_names_bits =
        unsafe { molt_tuple_from_list(py.attached_context(), kwonly_list_bits) };
    let zero_bits = int(0);
    let none_bits = none();
    // Construction metadata uses the runtime initializer. Private-looking
    // public attributes must not alter the executable argument contract.
    let function_name = string_bits(py, "required_keyword_only");
    unsafe {
        molt_runtime::molt_function_init_metadata(
            py.attached_context(),
            func_bits,
            function_name,
            function_name,
            none_bits,
            arg_names_bits,
            zero_bits,
            kwonly_names_bits,
            none_bits,
            none_bits,
            none_bits,
            none_bits,
            none_bits,
            none_bits,
            none_bits,
        )
    };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), function_name) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), kwonly_names_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), kwonly_name_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), kwonly_list_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), arg_names_bits) };
    func_bits
}

fn spawn_child(test_name: &str, envs: &[(&str, &str)]) -> std::process::Output {
    let exe = std::env::current_exe().expect("current test executable");
    let mut cmd = Command::new(exe);
    cmd.arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .arg("--test-threads=1");
    cmd.env("MOLT_TRACE_CHILD", "1");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    captured_runtime_children::capture(&mut cmd, "trace-call-binding", test_name)
}

fn finish_trace_child() -> ! {
    unsafe {
        let _ = molt_runtime_exit(0);
    }
    unreachable!("molt_runtime_exit must terminate the dedicated trace child")
}

#[test]
fn trace_callargs_emits_builder_lifecycle_logs() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() == Ok("1") {
        return;
    }
    let output = spawn_child("trace_callargs_child", &[("MOLT_TRACE_CALLARGS", "1")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[molt callargs] new"));
    assert!(stderr.contains("[molt callargs] push_pos"));
    assert!(stderr.contains("[molt callargs] free"));
}

#[test]
fn trace_call_bind_ic_emits_hit_log() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() == Ok("1") {
        return;
    }
    let output = spawn_child(
        "trace_call_bind_ic_child",
        &[("MOLT_TRACE_CALL_BIND_IC", "1")],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[molt call_bind_ic] hit"));
}

#[test]
fn trace_function_bind_meta_emits_summary() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() == Ok("1") {
        return;
    }
    let output = spawn_child(
        "trace_function_bind_meta_child",
        &[("MOLT_TRACE_FUNCTION_BIND_META", "1")],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[molt bind_meta]"));
    assert!(stderr.contains("total_pos=0"));
    assert!(stderr.contains("kwonly=1"));
}

#[test]
fn trace_callargs_child() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() != Ok("1") {
        return;
    }
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let list_bits = empty_list(py);
    let append_bits = append_method_bits(py, list_bits);
    let args_bits = empty_list(py);
    unsafe {
        let _ = molt_list_append(py.attached_context(), args_bits, int(5));
        let out = molt_object_call(py.attached_context(), append_bits, args_bits, none());
        assert_eq!(out, none());
    }
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), args_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), append_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
    drop(fixture_execution);
    finish_trace_child();
}

#[test]
fn trace_call_bind_ic_child() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() != Ok("1") {
        return;
    }
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let list_bits = empty_list(py);
    let append_bits = append_method_bits(py, list_bits);
    unsafe {
        let builder_a = molt_callargs_new(py.attached_context(), 1, 0);
        let _ = molt_callargs_push_pos(py.attached_context(), builder_a, int(1));
        let _ = molt_call_bind_ic(py.attached_context(), int(1), append_bits, builder_a);

        let builder_b = molt_callargs_new(py.attached_context(), 1, 0);
        let _ = molt_callargs_push_pos(py.attached_context(), builder_b, int(2));
        let out = molt_call_bind_ic(py.attached_context(), int(1), append_bits, builder_b);
        assert_eq!(out, none());
    }
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), append_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
    drop(fixture_execution);
    finish_trace_child();
}

#[test]
fn trace_function_bind_meta_child() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() != Ok("1") {
        return;
    }
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let func_bits = function_with_required_kwonly_metadata(py);
    let args_bits = empty_tuple(py);
    let _ = unsafe { molt_object_call(py.attached_context(), func_bits, args_bits, none()) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), args_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), func_bits) };
    drop(fixture_execution);
    finish_trace_child();
}
