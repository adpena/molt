use molt_obj_model::MoltObject;
use std::process::Command;
use std::sync::Once;

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_: u64) -> u64 {
    MoltObject::none().bits()
}

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_exception_clear() -> u64;
    fn molt_object_getattr_bytes(obj_bits: u64, name_ptr: *const u8, name_len: u64) -> u64;
    fn molt_object_call(callable_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64;
    fn molt_list_builtin(val_bits: u64) -> u64;
    fn molt_list_append(list_bits: u64, val_bits: u64) -> u64;
    fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64;
    fn molt_callargs_push_pos(builder_bits: u64, val_bits: u64) -> u64;
    fn molt_call_bind_ic(site_bits: u64, call_bits: u64, builder_bits: u64) -> u64;
}

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });
    let _ = unsafe { molt_exception_clear() };
}

fn none() -> u64 {
    MoltObject::none().bits()
}

fn missing() -> u64 {
    molt_runtime::molt_missing()
}

fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}

fn empty_list() -> u64 {
    unsafe { molt_list_builtin(missing()) }
}

fn append_method_bits(list_bits: u64) -> u64 {
    unsafe { molt_object_getattr_bytes(list_bits, b"append".as_ptr(), 6) }
}

fn spawn_child(test_name: &str, envs: &[(&str, &str)]) -> std::process::Output {
    let exe = std::env::current_exe().expect("current test executable");
    let mut cmd = Command::new(exe);
    cmd.arg("--exact").arg(test_name).arg("--nocapture");
    cmd.env("MOLT_TRACE_CHILD", "1");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("spawn trace child")
}

#[test]
fn trace_callargs_emits_builder_lifecycle_logs() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() == Ok("1") {
        return;
    }
    let output = spawn_child(
        "trace_callargs_child",
        &[("MOLT_TRACE_CALLARGS", "1")],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[molt callargs] new"));
    assert!(stderr.contains("[molt callargs] push_pos"));
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
fn trace_callargs_child() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() != Ok("1") {
        return;
    }
    init();
    let list_bits = empty_list();
    let append_bits = append_method_bits(list_bits);
    let args_bits = empty_list();
    unsafe {
        let _ = molt_list_append(args_bits, int(5));
        let out = molt_object_call(append_bits, args_bits, none());
        assert_eq!(out, none());
    }
    molt_runtime::molt_dec_ref_obj(args_bits);
    molt_runtime::molt_dec_ref_obj(append_bits);
    molt_runtime::molt_dec_ref_obj(list_bits);
}

#[test]
fn trace_call_bind_ic_child() {
    if std::env::var("MOLT_TRACE_CHILD").as_deref() != Ok("1") {
        return;
    }
    init();
    let list_bits = empty_list();
    let append_bits = append_method_bits(list_bits);
    unsafe {
        let builder_a = molt_callargs_new(1, 0);
        let _ = molt_callargs_push_pos(builder_a, int(1));
        let _ = molt_call_bind_ic(int(1), append_bits, builder_a);

        let builder_b = molt_callargs_new(1, 0);
        let _ = molt_callargs_push_pos(builder_b, int(2));
        let out = molt_call_bind_ic(int(1), append_bits, builder_b);
        assert_eq!(out, none());
    }
    molt_runtime::molt_dec_ref_obj(append_bits);
    molt_runtime::molt_dec_ref_obj(list_bits);
}
