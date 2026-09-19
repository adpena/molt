use crate::{FunctionIR, OpIR, SimpleBackend, SimpleIR, stable_ic_site_id};
use cranelift_object::object::{Object, ObjectSymbol};
use std::collections::BTreeSet;

const BOXED_ATTR_SYMBOL: &str = "molt_get_attr_object";
const BOXED_ATTR_IC_SYMBOL: &str = "molt_get_attr_object_ic";
const RETIRED_RAW_PTR_SYMBOL: &str = "molt_get_attr_ptr";
const RETIRED_OFFSET_PROBE_SYMBOL: &str = "molt_ic_probe_fast";
const RETIRED_OFFSET_SLOW_SYMBOL: &str = "molt_getattr_ic_slow";

fn undefined_symbols(bytes: &[u8]) -> BTreeSet<String> {
    let object = cranelift_object::object::File::parse(bytes).expect("parse native object");
    object
        .symbols()
        .filter(|symbol| symbol.is_undefined())
        .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
        .collect()
}

fn attr_program(kind: &str) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: format!("native_{kind}_authority"),
            params: vec!["receiver".to_string()],
            ops: vec![
                OpIR {
                    kind: kind.to_string(),
                    args: Some(vec!["receiver".to_string()]),
                    out: Some("result".to_string()),
                    s_value: Some("attribute".to_string()),
                    source_op_idx: Some(17),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        profile: None,
    }
}

#[test]
fn native_attribute_lanes_keep_one_owned_boxed_result_protocol() {
    let direct = SimpleBackend::new().compile(attr_program("get_attr"));
    let direct_imports = undefined_symbols(&direct.bytes);
    assert!(direct_imports.contains(BOXED_ATTR_SYMBOL));
    assert!(!direct_imports.contains(BOXED_ATTR_IC_SYMBOL));

    for kind in ["get_attr_generic_ptr", "get_attr_generic_obj"] {
        let output = SimpleBackend::new().compile(attr_program(kind));
        let imports = undefined_symbols(&output.bytes);
        assert!(
            imports.contains(BOXED_ATTR_IC_SYMBOL),
            "{kind} must call the canonical boxed attribute entrypoint"
        );
        assert!(
            !imports.contains(BOXED_ATTR_SYMBOL),
            "{kind} must not retain a parallel non-site lookup"
        );
        assert!(
            !imports.contains(RETIRED_OFFSET_PROBE_SYMBOL),
            "{kind} must not reintroduce the result-unsafe offset probe"
        );
        assert!(
            !imports.contains(RETIRED_OFFSET_SLOW_SYMBOL),
            "{kind} must not reintroduce the split offset-cache slow path"
        );
        assert!(
            !imports.contains(RETIRED_RAW_PTR_SYMBOL),
            "{kind} must keep the receiver boxed across the runtime ABI"
        );
    }

    assert!(!direct_imports.contains(RETIRED_RAW_PTR_SYMBOL));
    assert!(!direct_imports.contains(RETIRED_OFFSET_PROBE_SYMBOL));
    assert!(!direct_imports.contains(RETIRED_OFFSET_SLOW_SYMBOL));
}

#[test]
fn native_generic_attribute_spellings_have_distinct_stable_sites() {
    let sites: BTreeSet<i64> = ["get_attr_generic_ptr", "get_attr_generic_obj"]
        .into_iter()
        .map(|kind| stable_ic_site_id("native_attr_authority", 17, kind))
        .collect();
    assert_eq!(sites.len(), 2);
}
