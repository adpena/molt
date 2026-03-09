use molt_lang_backend::rust::RustBackend;
use molt_lang_backend::{FunctionIR, OpIR, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        value: None,
        f_value: None,
        s_value: None,
        bytes: None,
        var: None,
        args: None,
        out: None,
        fast_int: None,
        task_kind: None,
        container_type: None,
        stack_eligible: None,
        fast_float: None,
        raw_int: None,
        type_hint: None,
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
    assert!(source.contains("molt_get_item(&point_obj, &MoltValue::Str(\"__slot_8\".to_string()))"));
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
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("preview rust backend should lower module attr IR");

    assert!(!source.contains("MOLT_STUB: module_set_attr"));
    assert!(!source.contains("MOLT_STUB: module_get_attr"));
    assert!(source
        .contains("molt_set_attr_name(&mut module_obj, attr_name.clone(), point_cls.clone());"));
    assert!(source.contains("molt_get_attr_name(&module_obj, &attr_name)"));
}
