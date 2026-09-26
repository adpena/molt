use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[test]
fn numeric_scalar_layout_has_one_header_authority() {
    let root = repo_root();
    let source_header = std::fs::read_to_string(root.join("include/molt/Python.h"))
        .expect("read source transport header")
        .replace("\r\n", "\n");
    let linked_header =
        std::fs::read_to_string(root.join("runtime/molt-cpython-abi/include/Python.h"))
            .expect("read linked ABI header");
    let authority = std::fs::read_to_string(root.join("include/molt/shared/_numeric_scalar_abi.h"))
        .expect("read scalar layout authority");

    assert!(source_header.contains("#include \"shared/_numeric_scalar_abi.h\""));
    assert!(linked_header.contains("#include <_numeric_scalar_abi.h>"));
    assert!(!linked_header.contains("../../../include/"));
    for name in [
        "_numeric_scalar_abi.h",
        "_gil_state_abi.h",
        "_c_data_model.h",
    ] {
        assert!(root.join("include/molt/shared").join(name).is_file());
        assert!(!root.join("include/molt").join(name).exists());
    }
    for header in [&source_header, &linked_header] {
        assert!(header.contains("molt_capi_semantic_type"));
        assert!(header.contains("molt_capi_set_semantic_type"));
        for exact in [
            "PyLong_CheckExact",
            "PyFloat_CheckExact",
            "PyComplex_CheckExact",
            "PyUnicode_CheckExact",
            "PyList_CheckExact",
            "PyTuple_CheckExact",
            "PyDict_CheckExact",
            "PyBytes_CheckExact",
            "PyByteArray_CheckExact",
            "PySet_CheckExact",
            "PyFrozenSet_CheckExact",
        ] {
            assert!(
                header.contains(&format!("extern int {exact}")),
                "header retained a local exact-type classifier for {exact}"
            );
        }
    }
    assert!(!linked_header.contains("#define Py_TYPE(ob)     (((PyObject *)(ob))->ob_type)"));
    assert!(!linked_header.contains("#define Py_SET_TYPE(ob, type) (Py_TYPE(ob) = (type))"));
    for header in [&source_header, &linked_header] {
        assert!(
            header.contains("obj->ob_type != &MoltManaged_Type"),
            "Py_TYPE lost its physical fast path"
        );
        for forbidden in [
            "#define PyByteArray_CheckExact",
            "#define PySet_CheckExact",
            "#define PyFrozenSet_CheckExact",
            "static inline int PyBytes_CheckExact",
            "static inline int PySet_CheckExact",
            "static inline int PyByteArray_CheckExact",
        ] {
            assert!(
                !header.contains(forbidden),
                "header retained duplicate exact-type authority: {forbidden}"
            );
        }
    }
    assert!(linked_header.contains("#define PyTuple_SET_ITEM(op, i, v) ((void)PyTuple_SetItem"));
    assert!(!linked_header.contains("PyTuple_SET_ITEM(op, i, v) (((PyTupleObject"));
    for required in [
        "struct _object",
        "struct _longobject",
        "double ob_fval",
        "Py_complex cval",
    ] {
        assert!(authority.contains(required), "missing {required}");
    }
    for forbidden in [
        "struct _molt_pyobject",
        "The Molt handle lives in the POINTER value",
        "PyObject *ob_base;\n    double ob_fval",
        "PyObject *ob_base;\n    Py_complex cval",
    ] {
        assert!(
            !source_header.contains(forbidden),
            "legacy scalar representation remains: {forbidden}"
        );
    }
}

#[test]
fn module_and_cfunction_headers_have_one_linked_authority() {
    let root = repo_root();
    let source = std::fs::read_to_string(root.join("include/molt/Python.h"))
        .expect("read source transport header");
    let linked = std::fs::read_to_string(root.join("runtime/molt-cpython-abi/include/Python.h"))
        .expect("read linked ABI header");
    let types = std::fs::read_to_string(root.join("include/molt/shared/_cfunction_abi.h"))
        .expect("read C-callable layout authority");
    let module = std::fs::read_to_string(root.join("include/molt/shared/_module_definition_abi.h"))
        .expect("read module layout authority");
    let exports =
        std::fs::read_to_string(root.join("include/molt/shared/_module_callable_exports.h"))
            .expect("read module/callable export authority");

    for (name, source_include, linked_include) in [
        (
            "_cfunction_abi.h",
            "\"shared/_cfunction_abi.h\"",
            "<_cfunction_abi.h>",
        ),
        (
            "_module_definition_abi.h",
            "\"shared/_module_definition_abi.h\"",
            "<_module_definition_abi.h>",
        ),
        (
            "_module_callable_exports.h",
            "\"shared/_module_callable_exports.h\"",
            "<_module_callable_exports.h>",
        ),
    ] {
        assert!(source.contains(source_include), "source lacks {name}");
        assert!(
            linked.contains(linked_include),
            "linked header lacks {name}"
        );
    }
    for required in [
        "METH_FASTCALL",
        "METH_METHOD",
        "PyCFunctionObject",
        "PyCMethodObject",
    ] {
        assert!(
            types.contains(required),
            "missing callable authority: {required}"
        );
    }
    assert!(module.contains("PyModuleDef_HEAD_INIT"));
    for required in [
        "PyModule_Create2",
        "PyModuleDef_Init",
        "PyModule_FromDefAndSpec2",
        "PyModule_ExecDef",
        "PyModule_AddFunctions",
        "PyModule_GetDef",
        "PyModule_CheckExact",
        "PyCFunction_NewEx",
        "PyCMethod_New",
        "PyCFunction_GetFunction",
    ] {
        assert!(
            exports.contains(required),
            "missing ABI declaration: {required}"
        );
    }
    for line in source.lines().map(str::trim_start) {
        if line.starts_with("static inline ") {
            assert!(
                !line.contains("PyModule_")
                    && !line.contains("PyState_")
                    && !line.contains("PyCFunction_")
                    && !line.contains("PyCMethod_"),
                "source retained a competing module/callable implementation: {line}"
            );
        }
    }
}

#[test]
fn external_c_api_data_uses_one_shared_linkage_policy() {
    let root = repo_root();
    let source = std::fs::read_to_string(root.join("include/molt/Python.h"))
        .expect("read source transport header");
    let linked = std::fs::read_to_string(root.join("runtime/molt-cpython-abi/include/Python.h"))
        .expect("read linked ABI header");
    let exports =
        std::fs::read_to_string(root.join("include/molt/shared/_module_callable_exports.h"))
            .expect("read shared module export header");
    let policy = std::fs::read_to_string(root.join("include/molt/shared/_c_api_linkage.h"))
        .expect("read shared C-API data linkage policy");

    assert!(source.contains("#include \"shared/_c_api_linkage.h\""));
    assert!(linked.contains("#include <_c_api_linkage.h>"));
    assert!(exports.contains("#include \"_c_api_linkage.h\""));
    assert!(
        policy.contains("#ifndef PyAPI_DATA"),
        "caller override was lost"
    );
    assert!(policy.contains(
        "defined(_WIN32) && defined(MOLT_CPYTHON_ABI_SHARED) && MOLT_CPYTHON_ABI_SHARED"
    ));
    assert!(policy.contains("#define PyAPI_DATA(RTYPE) extern __declspec(dllimport) RTYPE"));
    assert!(policy.contains("#define PyAPI_DATA(RTYPE) extern RTYPE"));
    for (header, declarations) in [
        (
            &source,
            &[
                "PyAPI_DATA(PyTypeObject) MoltManaged_Type;",
                "PyAPI_DATA(PyLongObject) _Py_TrueStruct;",
            ][..],
        ),
        (
            &linked,
            &[
                "PyAPI_DATA(PyObject) Py_None;",
                "PyAPI_DATA(PyObject) PyExc_ValueError;",
                "PyAPI_DATA(const unsigned long) Py_Version;",
                "PyAPI_DATA(int) Py_OptimizeFlag;",
            ][..],
        ),
        (
            &exports,
            &[
                "PyAPI_DATA(PyTypeObject) PyModule_Type;",
                "PyAPI_DATA(PyTypeObject) PyModuleDef_Type;",
                "PyAPI_DATA(PyTypeObject) PyCFunction_Type;",
                "PyAPI_DATA(PyTypeObject) PyCMethod_Type;",
            ][..],
        ),
    ] {
        for declaration in declarations {
            assert!(
                header.contains(*declaration),
                "missing shared DATA declaration: {declaration}"
            );
        }
        for line in header.lines().map(str::trim) {
            assert!(
                !(line.starts_with("extern ") && line.ends_with(';') && !line.contains('(')),
                "external DATA bypasses shared policy: {line}"
            );
        }
    }
    assert!(source.contains("static int Py_OptimizeFlag = 0;"));
}

#[test]
fn overlay_probe_rejects_raw_pointer_as_pyobject_contract() {
    let probe = std::fs::read_to_string(
        repo_root().join("runtime/molt-cpython-abi-test-support/l7_overlay_probe.c"),
    )
    .expect("read overlay probe");
    let raw_symbol = ["molt_l7_overlay_", "raw_"].concat();
    assert!(!probe.contains(&raw_symbol));
    assert!(!probe.contains("(PyObject *)bits"));
    let integer_tests = std::fs::read_to_string(
        repo_root().join("runtime/molt-cpython-abi/tests/test_l7_integer_authority.rs"),
    )
    .expect("read integer authority tests");
    assert!(
        !integer_tests.contains(&raw_symbol),
        "legacy raw-pointer probe caller remains"
    );
    for required in [
        "Py_TYPE(left) != &PyLong_Type",
        "PyLong_CheckExact(left)",
        "PyNumber_Add(left, right)",
        "PyFloat_CheckExact(value)",
        "PyComplex_CheckExact(complex_value)",
        "Py_DECREF(sum)",
        "value->ob_refcnt += 1",
        "remaining = --value->ob_refcnt",
        "_Py_Dealloc(value)",
        "PyTuple_SET_ITEM(tuple, 0, value)",
        "PyTuple_GET_ITEM(tuple, 0) == value",
    ] {
        assert!(probe.contains(required), "probe missing {required}");
    }
}

#[test]
fn scalar_results_and_lifetime_use_single_provenance_authorities() {
    let root = repo_root();
    let numbers = std::fs::read_to_string(root.join("runtime/molt-cpython-abi/src/api/numbers.rs"))
        .expect("read numeric ABI authority");
    assert!(
        !numbers.contains("GLOBAL_BRIDGE.owned_handle_to_pyobj"),
        "a public numeric result bypasses the concrete scalar carrier"
    );
    assert!(!numbers.contains("PyObject_Free(op.cast())"));
    let protocol =
        std::fs::read_to_string(root.join("runtime/molt-cpython-abi/src/api/abstract_number.rs"))
            .expect("read numeric protocol authority");
    assert!(!protocol.contains("left.bits() == right.bits()"));
    assert!(protocol.contains("a == b"));
    let bridge = std::fs::read_to_string(root.join("runtime/molt-cpython-abi/src/bridge.rs"))
        .expect("read bridge lifetime authority");
    for required in [
        "transition_runtime_owner_add",
        "transition_runtime_owner_release",
        "has_direct_c_refs",
        "retire_runtime_object_deferred",
        "gc_ref_adjustment",
    ] {
        assert!(
            bridge.contains(required),
            "missing lifecycle authority: {required}"
        );
    }
    assert!(bridge.contains("try_mark_abi_view"));
}
