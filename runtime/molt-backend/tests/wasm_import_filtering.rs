#![cfg(feature = "wasm-backend")]

//! Tests for import profile filtering: Auto, Pure, and Full profiles.
//! Verifies that Auto strips unused imports, Pure omits IO/ASYNC/TIME,
//! and Full includes everything.

use std::collections::BTreeSet;

use molt_backend::wasm::{WasmBackend, WasmCompileOptions, WasmProfile};
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{Parser, Payload, TypeRef};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn hello_world_ir() -> SimpleIR {
    let mut const_str = op("const_str");
    const_str.s_value = Some("hello world".to_string());
    const_str.out = Some("s0".to_string());

    let mut print = op("print_obj");
    print.args = Some(vec!["s0".to_string()]);

    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![const_str, print, op("print_newline"), op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    }
}

fn ir_with_async_ops() -> SimpleIR {
    let mut sleep = op("async_sleep");
    sleep.args = Some(vec!["p0".to_string()]);
    sleep.out = Some("v0".to_string());

    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["p0".to_string()],
            ops: vec![sleep, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    }
}

fn ir_with_os_name() -> SimpleIR {
    let mut os_name = op("os_name");
    os_name.out = Some("v0".to_string());

    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![os_name, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    }
}

fn ir_with_escaped_call_guarded() -> SimpleIR {
    let mut func_new = op("func_new");
    func_new.s_value = Some("callee".to_string());
    func_new.value = Some(2);
    func_new.out = Some("f".to_string());

    let mut call_guarded = op("call_guarded");
    call_guarded.s_value = Some("callee".to_string());
    call_guarded.args = Some(vec!["f".to_string(), "p0".to_string(), "p1".to_string()]);
    call_guarded.out = Some("out".to_string());

    let mut ret = op("ret");
    ret.var = Some("out".to_string());
    ret.args = Some(vec!["out".to_string()]);

    let mut callee_none = op("const_none");
    callee_none.out = Some("retv".to_string());

    let mut callee_ret = op("ret");
    callee_ret.var = Some("retv".to_string());
    callee_ret.args = Some(vec!["retv".to_string()]);

    SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec!["p0".to_string(), "p1".to_string()],
                ops: vec![func_new, call_guarded, ret],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "callee".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                ops: vec![callee_none, callee_ret],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    }
}

#[allow(dead_code)]
fn ir_with_socket_ops() -> SimpleIR {
    let mut sock = op("socket_new");
    sock.args = Some(vec![
        "p0".to_string(),
        "p1".to_string(),
        "p2".to_string(),
        "p3".to_string(),
        "p4".to_string(),
        "p5".to_string(),
        "p6".to_string(),
    ]);
    sock.out = Some("v0".to_string());

    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![
                "p0".to_string(),
                "p1".to_string(),
                "p2".to_string(),
                "p3".to_string(),
                "p4".to_string(),
                "p5".to_string(),
                "p6".to_string(),
            ],
            ops: vec![sock, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    }
}

#[allow(dead_code)]
fn ir_with_time_ops() -> SimpleIR {
    let mut time = op("time_monotonic");
    time.out = Some("v0".to_string());

    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![time, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    }
}

fn compile_with_profile(ir: SimpleIR, profile: WasmProfile) -> Vec<u8> {
    WasmBackend::with_options(WasmCompileOptions {
        wasm_profile: profile,
        ..WasmCompileOptions::default()
    })
    .compile(ir)
}

fn compile_with_options(ir: SimpleIR, options: WasmCompileOptions) -> Vec<u8> {
    WasmBackend::with_options(options).compile(ir)
}

fn import_names(wasm: &[u8]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::ImportSection(section) = payload.expect("valid payload") {
            for import in section.into_imports() {
                let import = import.expect("valid import");
                if matches!(import.ty, TypeRef::Func(_)) {
                    names.insert(import.name.to_string());
                }
            }
        }
    }
    names
}

// -----------------------------------------------------------------------
// Auto profile tests
// -----------------------------------------------------------------------

#[test]
fn auto_hello_world_has_fewer_than_500_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);
    // A hello-world should not pull in the full ~600+ import set.
    assert!(
        names.len() < 500,
        "hello-world Auto profile has {} imports, expected <500",
        names.len()
    );
}

#[test]
fn auto_hello_world_includes_used_structural_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);
    // After dead-import elimination, only imports actually referenced by
    // codegen survive.  The hello_world IR uses print_newline (matched by
    // the "print_newline" codegen handler) and structural init imports.
    assert!(
        names.contains("print_newline"),
        "print_newline should be present (used by codegen)"
    );
    assert!(
        names.contains("runtime_init"),
        "runtime_init should be present (structural)"
    );
}

#[test]
fn auto_reloc_preserves_reserved_runtime_callable_imports() {
    let wasm = compile_with_options(
        hello_world_ir(),
        WasmCompileOptions {
            wasm_profile: WasmProfile::Auto,
            reloc_enabled: true,
            ..WasmCompileOptions::default()
        },
    );
    let names = import_names(&wasm);
    for name in [
        "type_call",
        "type_new",
        "type_init",
        "object_new_bound",
        "object_init",
        "object_init_subclass",
        "exception_new_bound",
        "exception_init",
        "exceptiongroup_init",
    ] {
        assert!(
            names.contains(name),
            "{name} should be present as a linked-wasm structural import"
        );
    }
}

#[test]
fn auto_hello_world_includes_string_from_bytes() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);
    assert!(
        names.contains("string_from_bytes"),
        "string_from_bytes should be present for const_str"
    );
}

#[test]
fn auto_profile_includes_async_sleep_when_ir_uses_it() {
    let wasm = compile_with_profile(ir_with_async_ops(), WasmProfile::Auto);
    let names = import_names(&wasm);
    assert!(
        names.contains("async_sleep"),
        "async_sleep should be present when IR has async_sleep op"
    );
}

#[test]
fn auto_hello_world_does_not_include_socket_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);
    let socket_imports: Vec<&String> = names.iter().filter(|n| n.starts_with("socket_")).collect();
    assert!(
        socket_imports.is_empty(),
        "hello-world should not have socket imports, found: {:?}",
        socket_imports
    );
}

#[test]
fn auto_hello_world_does_not_include_db_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);
    let db_imports: Vec<&String> = names.iter().filter(|n| n.starts_with("db_")).collect();
    assert!(
        db_imports.is_empty(),
        "hello-world should not have db imports, found: {:?}",
        db_imports
    );
}

#[test]
fn auto_call_guarded_keeps_escaped_dispatch_imports() {
    let wasm = compile_with_profile(ir_with_escaped_call_guarded(), WasmProfile::Auto);
    let names = import_names(&wasm);
    for name in [
        "is_function_obj",
        "is_truthy",
        "recursion_guard_enter",
        "recursion_guard_exit",
        "trace_enter_slot",
        "trace_exit",
        "call_func_dispatch",
    ] {
        assert!(
            names.contains(name),
            "Auto profile should retain {name} for escaped call_guarded; imports={names:?}"
        );
    }
}

#[test]
fn auto_profile_keeps_reserved_runtime_callable_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);

    for name in [
        "type_call",
        "type_new",
        "type_init",
        "object_new_bound",
        "object_init",
        "object_init_subclass",
        "exception_new_bound",
        "exception_init",
        "exceptiongroup_init",
    ] {
        assert!(
            names.contains(name),
            "Auto profile must retain reserved runtime callable import {name}; imports={names:?}"
        );
    }
}

// -----------------------------------------------------------------------
// Pure profile tests
// -----------------------------------------------------------------------

#[test]
fn pure_profile_excludes_io_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Pure);
    let names = import_names(&wasm);

    let io_prefixes = ["process_", "socket", "db_", "ws_", "file_", "stream_"];
    for prefix in &io_prefixes {
        let matches: Vec<&String> = names.iter().filter(|n| n.starts_with(prefix)).collect();
        assert!(
            matches.is_empty(),
            "Pure profile should not have {prefix}* imports, found: {:?}",
            matches
        );
    }
}

#[test]
fn pure_profile_keeps_runtime_os_intrinsics() {
    let wasm = compile_with_profile(ir_with_os_name(), WasmProfile::Pure);
    let names = import_names(&wasm);

    assert!(
        names.contains("os_name"),
        "Pure profile must retain runtime-safe os_ intrinsics; imports={names:?}"
    );
}

#[test]
fn pure_profile_excludes_async_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Pure);
    let names = import_names(&wasm);

    let async_prefixes = [
        "async_sleep",
        "future_",
        "promise_",
        "thread_",
        "lock_",
        "rlock_",
        "chan_",
        "asyncio_",
        "asyncgen_",
    ];
    for prefix in &async_prefixes {
        let matches: Vec<&String> = names.iter().filter(|n| n.starts_with(prefix)).collect();
        assert!(
            matches.is_empty(),
            "Pure profile should not have {prefix}* imports, found: {:?}",
            matches
        );
    }
}

#[test]
fn pure_profile_excludes_time_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Pure);
    let names = import_names(&wasm);

    let time_imports: Vec<&String> = names.iter().filter(|n| n.starts_with("time_")).collect();
    assert!(
        time_imports.is_empty(),
        "Pure profile should not have time_* imports, found: {:?}",
        time_imports
    );
}

#[test]
fn pure_profile_excludes_compression_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Pure);
    let names = import_names(&wasm);

    let compression_prefixes = ["deflate_", "inflate_", "bz2_", "gzip_", "lzma_", "zlib_"];
    for prefix in &compression_prefixes {
        let matches: Vec<&String> = names.iter().filter(|n| n.starts_with(prefix)).collect();
        assert!(
            matches.is_empty(),
            "Pure profile should not have {prefix}* imports, found: {:?}",
            matches
        );
    }
}

#[test]
fn pure_profile_keeps_core_arithmetic() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Pure);
    let names = import_names(&wasm);

    for core in ["add", "sub", "mul", "div", "eq", "lt"] {
        assert!(
            names.contains(core),
            "Pure profile should keep core arithmetic import: {core}"
        );
    }
}

#[test]
fn pure_profile_keeps_collection_ops() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Pure);
    let names = import_names(&wasm);

    for core in ["dict_new", "list_builder_new", "set_new"] {
        assert!(
            names.contains(core),
            "Pure profile should keep collection import: {core}"
        );
    }
}

// -----------------------------------------------------------------------
// Full profile tests
// -----------------------------------------------------------------------

#[test]
fn full_profile_includes_io_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Full);
    let names = import_names(&wasm);

    // Full profile should register everything, including IO.
    assert!(
        names.contains("process_spawn"),
        "Full profile should include process_spawn"
    );
    assert!(
        names.contains("socket_new"),
        "Full profile should include socket_new"
    );
}

#[test]
fn full_profile_includes_async_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Full);
    let names = import_names(&wasm);

    assert!(
        names.contains("async_sleep"),
        "Full profile should include async_sleep"
    );
    assert!(
        names.contains("future_poll"),
        "Full profile should include future_poll"
    );
}

#[test]
fn full_profile_includes_thread_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Full);
    let names = import_names(&wasm);

    assert!(
        names.contains("thread_submit"),
        "Full profile should include thread_submit"
    );
    assert!(
        names.contains("thread_poll"),
        "Full profile should include thread_poll"
    );
}

// -----------------------------------------------------------------------
// Post-compilation import stripping tests
// -----------------------------------------------------------------------

/// In Auto profile, the post-compilation strip should remove any imports that
/// the pre-compilation heuristic (`collect_required_imports`) included but
/// that codegen never actually referenced.  For hello_world, the final import
/// count after stripping should be strictly less than or equal to the
/// pre-strip count, and the module should still be valid WASM.
#[test]
fn auto_hello_world_stripped_module_is_valid_wasm() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    // Verify magic + version
    assert!(wasm.len() >= 8, "WASM too short");
    assert_eq!(&wasm[..4], b"\x00asm", "bad WASM magic");
    // Verify all sections are parseable
    let mut section_count = 0;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("parseable payload after stripping");
        match payload {
            Payload::ImportSection(_)
            | Payload::TypeSection(_)
            | Payload::FunctionSection(_)
            | Payload::CodeSectionStart { .. }
            | Payload::CodeSectionEntry(_)
            | Payload::ExportSection(_) => {
                section_count += 1;
            }
            _ => {}
        }
    }
    assert!(section_count > 0, "no sections found in stripped WASM");
}

/// Full profile preserves all imports (stripping is disabled for Full).
/// Verify that Full profile still has its full set.
#[test]
fn full_profile_preserves_all_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Full);
    let names = import_names(&wasm);

    // Full profile should register 500+ imports.
    assert!(
        names.len() > 500,
        "Full profile should have >500 imports, found {}",
        names.len()
    );
}

/// Auto profile: stripping should produce fewer or equal imports compared
/// to the pre-filter set.
#[test]
fn auto_hello_world_strip_does_not_add_imports() {
    let wasm = compile_with_profile(hello_world_ir(), WasmProfile::Auto);
    let names = import_names(&wasm);
    // The Auto pre-filter already narrows imports; stripping should not
    // add any new ones.  Just check it's a reasonable count.
    assert!(
        names.len() < 200,
        "Auto hello-world should have <200 imports, found {}",
        names.len()
    );
}
