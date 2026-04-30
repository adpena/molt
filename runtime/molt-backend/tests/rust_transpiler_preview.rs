#![cfg(feature = "rust-backend")]

use molt_backend::rust::RustBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

#[test]
fn rust_backend_lowers_class_slots_without_stub_placeholders() {
    let mut class_new = op("class_new");
    class_new.out = Some("point_cls".to_string());

    let mut alloc_obj = op("alloc_class_static");
    alloc_obj.args = Some(vec!["point_cls".to_string()]);
    alloc_obj.value = Some(24);
    alloc_obj.out = Some("point_obj".to_string());

    let mut set_class = op("object_set_class");
    set_class.args = Some(vec!["point_obj".to_string(), "point_cls".to_string()]);

    let mut none0 = op("const_none");
    none0.out = Some("slot0_init".to_string());

    let mut store0_init = op("store_init");
    store0_init.args = Some(vec!["point_obj".to_string(), "slot0_init".to_string()]);
    store0_init.value = Some(0);

    let mut none1 = op("const_none");
    none1.out = Some("slot1_init".to_string());

    let mut store1_init = op("store_init");
    store1_init.args = Some(vec!["point_obj".to_string(), "slot1_init".to_string()]);
    store1_init.value = Some(8);

    let mut init_name = op("const_str");
    init_name.s_value = Some("__init__".to_string());
    init_name.out = Some("init_name".to_string());

    let mut get_init = op("get_attr_name");
    get_init.args = Some(vec!["point_cls".to_string(), "init_name".to_string()]);
    get_init.out = Some("init_func".to_string());

    let mut bind_init = op("bound_method_new");
    bind_init.args = Some(vec!["init_func".to_string(), "point_obj".to_string()]);
    bind_init.out = Some("bound_init".to_string());

    let mut call_args = op("callargs_new");
    call_args.out = Some("call_args".to_string());

    let mut call_init = op("call_bind");
    call_init.args = Some(vec!["bound_init".to_string(), "call_args".to_string()]);
    call_init.out = Some("init_result".to_string());

    let mut x = op("const");
    x.value = Some(10);
    x.out = Some("x".to_string());

    let mut store0 = op("store");
    store0.args = Some(vec!["point_obj".to_string(), "x".to_string()]);
    store0.value = Some(0);

    let mut y = op("const");
    y.value = Some(32);
    y.out = Some("y".to_string());

    let mut store1 = op("store");
    store1.args = Some(vec!["point_obj".to_string(), "y".to_string()]);
    store1.value = Some(8);

    let mut load0 = op("load");
    load0.args = Some(vec!["point_obj".to_string()]);
    load0.value = Some(0);
    load0.out = Some("loaded_x".to_string());

    let mut load1 = op("load");
    load1.args = Some(vec!["point_obj".to_string()]);
    load1.value = Some(8);
    load1.out = Some("loaded_y".to_string());

    let mut add = op("add");
    add.args = Some(vec!["loaded_x".to_string(), "loaded_y".to_string()]);
    add.out = Some("sum".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["sum".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_class_slots".to_string(),
            params: Vec::new(),
            ops: vec![
                class_new,
                alloc_obj,
                set_class,
                none0,
                store0_init,
                none1,
                store1_init,
                init_name,
                get_init,
                bind_init,
                call_args,
                call_init,
                x,
                store0,
                y,
                store1,
                load0,
                load1,
                add,
                ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("preview rust backend should lower class slot IR");

    assert!(!source.contains("MOLT_STUB: alloc_class_static"));
    assert!(!source.contains("MOLT_STUB: bound_method_new"));
    assert!(!source.contains("_.clone()"));
    assert!(source.contains(
        "molt_set_item(&mut point_obj, MoltValue::Str(\"__slot_0\".to_string()), x.clone());"
    ));
    assert!(
        source.contains("molt_get_item(&point_obj, &MoltValue::Str(\"__slot_8\".to_string()))")
    );
}

#[test]
fn rust_backend_lowers_module_attr_round_trip() {
    let mut module_new = op("module_new");
    module_new.out = Some("module_obj".to_string());

    let mut attr_name = op("const_str");
    attr_name.s_value = Some("Point".to_string());
    attr_name.out = Some("attr_name".to_string());

    let mut class_new = op("class_new");
    class_new.out = Some("point_cls".to_string());

    let mut module_set = op("module_set_attr");
    module_set.args = Some(vec![
        "module_obj".to_string(),
        "attr_name".to_string(),
        "point_cls".to_string(),
    ]);

    let mut module_get = op("module_get_attr");
    module_get.args = Some(vec!["module_obj".to_string(), "attr_name".to_string()]);
    module_get.out = Some("loaded_cls".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["loaded_cls".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_module_attrs".to_string(),
            params: Vec::new(),
            ops: vec![
                module_new, attr_name, class_new, module_set, module_get, ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("preview rust backend should lower module attr IR");

    assert!(!source.contains("MOLT_STUB: module_set_attr"));
    assert!(!source.contains("MOLT_STUB: module_get_attr"));
    assert!(
        source
            .contains("molt_set_attr_name(&mut module_obj, attr_name.clone(), point_cls.clone());")
    );
    assert!(source.contains("molt_get_attr_name(&module_obj, &attr_name)"));
}

#[test]
fn rust_backend_stamps_target_python_version_state() {
    let mut major_raw = op("const");
    major_raw.value = Some(3);
    major_raw.out = Some("major_raw".to_string());

    let mut major = op("box");
    major.args = Some(vec!["major_raw".to_string()]);
    major.out = Some("major".to_string());

    let mut minor_raw = op("const");
    minor_raw.value = Some(13);
    minor_raw.out = Some("minor_raw".to_string());

    let mut minor = op("box");
    minor.args = Some(vec!["minor_raw".to_string()]);
    minor.out = Some("minor".to_string());

    let mut micro_raw = op("const");
    micro_raw.value = Some(0);
    micro_raw.out = Some("micro_raw".to_string());

    let mut micro = op("box");
    micro.args = Some(vec!["micro_raw".to_string()]);
    micro.out = Some("micro".to_string());

    let mut releaselevel = op("const_str");
    releaselevel.s_value = Some("final".to_string());
    releaselevel.out = Some("releaselevel".to_string());

    let mut serial_raw = op("const");
    serial_raw.value = Some(0);
    serial_raw.out = Some("serial_raw".to_string());

    let mut serial = op("box");
    serial.args = Some(vec!["serial_raw".to_string()]);
    serial.out = Some("serial".to_string());

    let mut version = op("const_str");
    version.s_value = Some("3.13.0 (molt)".to_string());
    version.out = Some("version".to_string());

    let mut set_version = op("call");
    set_version.s_value = Some("molt_sys_set_version_info".to_string());
    set_version.args = Some(vec![
        "major".to_string(),
        "minor".to_string(),
        "micro".to_string(),
        "releaselevel".to_string(),
        "serial".to_string(),
        "version".to_string(),
    ]);
    set_version.out = Some("set_result".to_string());

    let mut get_version_info = op("call");
    get_version_info.s_value = Some("molt_sys_version_info".to_string());
    get_version_info.out = Some("version_info".to_string());

    let mut get_version = op("call");
    get_version.s_value = Some("molt_sys_version".to_string());
    get_version.out = Some("version_text".to_string());

    let mut get_hexversion = op("call");
    get_hexversion.s_value = Some("molt_sys_hexversion".to_string());
    get_hexversion.out = Some("hexversion".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["version_text".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_target_python_probe".to_string(),
            params: Vec::new(),
            ops: vec![
                major_raw,
                major,
                minor_raw,
                minor,
                micro_raw,
                micro,
                releaselevel,
                serial_raw,
                serial,
                version,
                set_version,
                get_version_info,
                get_version,
                get_hexversion,
                ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("preview rust backend should lower sys version intrinsics");

    assert!(!source.contains(
        "fn molt_sys_set_version_info(_args: &mut Vec<MoltValue>) -> MoltValue { MoltValue::None }"
    ));
    assert!(!source.contains("MOLT_STUB: box"));
    assert!(source.contains("struct MoltSysVersionInfo"));
    assert!(
        source.contains("fn molt_sys_set_version_info(args: &mut Vec<MoltValue>) -> MoltValue")
    );
    assert!(source.contains("fn molt_sys_version_info(_args: &mut Vec<MoltValue>) -> MoltValue"));
    assert!(source.contains("fn molt_sys_version(_args: &mut Vec<MoltValue>) -> MoltValue"));
    assert!(source.contains("fn molt_sys_hexversion(_args: &mut Vec<MoltValue>) -> MoltValue"));
    assert!(source.contains("MoltValue::Str(state.version.clone())"));
    assert!(source.contains("MoltValue::List(vec!["));
}

#[test]
fn rust_backend_imports_sys_version_metadata_without_stub() {
    let mut major_raw = op("const");
    major_raw.value = Some(3);
    major_raw.out = Some("major_raw".to_string());

    let mut major = op("box");
    major.args = Some(vec!["major_raw".to_string()]);
    major.out = Some("major".to_string());

    let mut minor_raw = op("const");
    minor_raw.value = Some(14);
    minor_raw.out = Some("minor_raw".to_string());

    let mut minor = op("box");
    minor.args = Some(vec!["minor_raw".to_string()]);
    minor.out = Some("minor".to_string());

    let mut micro_raw = op("const");
    micro_raw.value = Some(0);
    micro_raw.out = Some("micro_raw".to_string());

    let mut micro = op("box");
    micro.args = Some(vec!["micro_raw".to_string()]);
    micro.out = Some("micro".to_string());

    let mut releaselevel = op("const_str");
    releaselevel.s_value = Some("final".to_string());
    releaselevel.out = Some("releaselevel".to_string());

    let mut serial_raw = op("const");
    serial_raw.value = Some(0);
    serial_raw.out = Some("serial_raw".to_string());

    let mut serial = op("box");
    serial.args = Some(vec!["serial_raw".to_string()]);
    serial.out = Some("serial".to_string());

    let mut version = op("const_str");
    version.s_value = Some("3.14.0 (molt)".to_string());
    version.out = Some("version".to_string());

    let mut set_version = op("call");
    set_version.s_value = Some("molt_sys_set_version_info".to_string());
    set_version.args = Some(vec![
        "major".to_string(),
        "minor".to_string(),
        "micro".to_string(),
        "releaselevel".to_string(),
        "serial".to_string(),
        "version".to_string(),
    ]);
    set_version.out = Some("set_result".to_string());

    let mut sys_name = op("const_str");
    sys_name.s_value = Some("sys".to_string());
    sys_name.out = Some("sys_name".to_string());

    let mut import_sys = op("module_import");
    import_sys.args = Some(vec!["sys_name".to_string()]);
    import_sys.out = Some("sys_module".to_string());

    let mut version_info_name = op("const_str");
    version_info_name.s_value = Some("version_info".to_string());
    version_info_name.out = Some("version_info_name".to_string());

    let mut version_info = op("module_get_attr");
    version_info.args = Some(vec![
        "sys_module".to_string(),
        "version_info_name".to_string(),
    ]);
    version_info.out = Some("version_info".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["version_info".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_target_python_sys_import_probe".to_string(),
            params: Vec::new(),
            ops: vec![
                major_raw,
                major,
                minor_raw,
                minor,
                micro_raw,
                micro,
                releaselevel,
                serial_raw,
                serial,
                version,
                set_version,
                sys_name,
                import_sys,
                version_info_name,
                version_info,
                ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("preview rust backend should materialize sys metadata");

    assert!(!source.contains("MOLT_STUB: module_import"));
    assert!(source.contains("fn molt_import_module(name: &MoltValue) -> MoltValue"));
    assert!(source.contains("\"version_info\".to_string()"));
    assert!(source.contains("\"hexversion\".to_string()"));
    assert!(source.contains("molt_import_module(&sys_name)"));
}

#[test]
fn rust_backend_lowers_class_merge_layout_with_real_helper_and_result() {
    let mut class_new = op("class_new");
    class_new.out = Some("point_cls".to_string());

    let mut field_name = op("const_str");
    field_name.s_value = Some("x".to_string());
    field_name.out = Some("field_name".to_string());

    let mut field_offset = op("const");
    field_offset.value = Some(0);
    field_offset.out = Some("field_offset".to_string());

    let mut offsets = op("dict_new");
    offsets.args = Some(vec!["field_name".to_string(), "field_offset".to_string()]);
    offsets.out = Some("offsets".to_string());

    let mut layout_size = op("const");
    layout_size.value = Some(8);
    layout_size.out = Some("layout_size".to_string());

    let mut merge_layout = op("class_merge_layout");
    merge_layout.args = Some(vec![
        "point_cls".to_string(),
        "offsets".to_string(),
        "layout_size".to_string(),
    ]);
    merge_layout.out = Some("merge_result".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["merge_result".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_class_merge_layout".to_string(),
            params: Vec::new(),
            ops: vec![
                class_new,
                field_name,
                field_offset,
                offsets,
                layout_size,
                merge_layout,
                ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("preview rust backend should lower class layout merge IR");

    assert!(source.contains("fn molt_class_merge_layout("));
    assert!(source.contains(
        "let mut merge_result: MoltValue = molt_class_merge_layout(&mut point_cls, offsets.clone(), layout_size.clone());"
    ));
}
