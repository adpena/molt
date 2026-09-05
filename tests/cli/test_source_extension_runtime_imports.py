from __future__ import annotations

import pytest

from molt.cli import source_extension_runtime_imports as imports


def scan(body: str, *, function: str = "module_exec") -> tuple[str, ...]:
    return imports.source_extension_runtime_python_imports(
        f"static int {function}(PyObject *module) {{ {body} }}"
    )


@pytest.mark.parametrize(
    "callee",
    [
        "PyImport_ImportModule",
        "PyImport_ImportModuleNoBlock",
        "PyImport_ImportFrozenModule",
    ],
)
def test_c_string_import_apis(callee: str) -> None:
    assert scan(f'{callee}("pkg.required");') == ("pkg.required",)
    assert scan(f'{callee}("pkg.lazy");', function="lazy_helper") == ()


@pytest.mark.parametrize(
    "callee",
    [
        "PyImport_AddModule",
        "PyImport_AddModuleRef",
        "PyImport_AddModuleObject",
        "PyImport_ExecCodeModule",
        "PyImport_ExecCodeModuleEx",
        "PyImport_ExecCodeModuleObject",
        "PyImport_ExecCodeModuleWithPathnames",
        "PyImport_GetModule",
        "PyImport_GetModuleDict",
        "PyImport_GetImporter",
        "PyImport_UnknownFutureAPI",
    ],
)
def test_c_api_namespace_is_not_an_import_semantic_role(callee: str) -> None:
    assert imports._source_extension_runtime_import_callee(callee) is None
    assert scan(f'{callee}("not_a_dependency");') == ()


@pytest.mark.parametrize(
    "callee",
    [
        "PyImport_Import",
        "PyImport_ImportModuleLevelObject",
        "PyImport_ImportFrozenModuleObject",
    ],
)
def test_object_name_apis_do_not_treat_char_pointers_as_python_names(
    callee: str,
) -> None:
    contract = imports._source_extension_runtime_import_callee(callee)
    assert contract is not None and contract.name_kind == "python_object"
    assert scan(f'{callee}("not_a_python_object");') == ()
    assert scan(f'{callee}(name); PyImport_ImportModule("known");') == ("known",)


@pytest.mark.parametrize("level", ["0", "0U", "0L", "0x0", "00", "/* level */ 0"])
def test_absolute_level_has_a_known_root(level: str) -> None:
    assert scan(
        'PyImport_ImportModuleLevel("pkg.module", f(a, b), NULL, '
        f'Py_BuildValue("(s)", "item"), {level});'
    ) == ("pkg.module",)


@pytest.mark.parametrize("level", ["1", "-1", "level", "0 + offset", "0 /* gap */ 1"])
def test_relative_or_computed_level_is_not_an_absolute_root(level: str) -> None:
    assert scan(f'PyImport_ImportModuleLevel("child", g, l, f, {level});') == ()


@pytest.mark.parametrize(
    ("literal", "expected"),
    [
        ('"pkg." "joined"', "pkg.joined"),
        ('"pkg" /* join */ ".joined"', "pkg.joined"),
        (r'"pkg\x2e" "joined"', "pkg.joined"),
        (r'"pkg\056joined"', "pkg.joined"),
        ('u8"café.module"', "café.module"),
        (r'"caf\xc3\xa9.module"', "café.module"),
        (r'"caf\u00e9.module"', "café.module"),
        ('"pkg.\\\njoined"', "pkg.joined"),
        ('"pkg.\\\r\njoined"', "pkg.joined"),
        (r'"math\0not_a_dependency"', "math"),
    ],
)
def test_literal_bytes_concatenation_and_c_string_termination(
    literal: str, expected: str
) -> None:
    assert scan(f"PyImport_ImportModule({literal});") == (expected,)


@pytest.mark.parametrize(
    "argument",
    [
        '"math" + offset',
        '"math" NAME_SUFFIX',
        '"math"[offset]',
        "name",
        "select_name()",
        '""',
        '"bad-name"',
        '"class"',
        'L"math"',
        'u"math"',
        'U"math"',
        '"pkg" L".wide"',
        r'"pkg\qname"',
        r'"pkg\x"',
        r'"pkg\x100"',
        r'"pkg\777"',
        r'"pkg\ud800"',
        r'"pkg\U00110000"',
        r'"pkg\xff"',
    ],
)
def test_unknown_invalid_or_non_char_literal_never_launders_a_prefix(
    argument: str,
) -> None:
    assert scan(
        f'PyImport_ImportModule({argument}); PyImport_ImportModule("known");'
    ) == ("known",)


def test_nested_and_cython_eager_scopes_do_not_widen_lazy_helpers() -> None:
    source = r"""
    static int module_exec(PyObject *m) { return 0; }
    extern "C" {
        static int lazy_helper(void) { PyImport_ImportModule("inspect"); }
        PyMODINIT_FUNC PyInit_nativepkg(void) {
            if (ready) { PyImport_ImportModule("math"); }
        }
    }
    static int __Pyx_modinit_type_import_code(void *mstate) {
        PyImport_ImportModule("numpy");
    }
    static int __Pyx_modinit_shared_function_import_code(void *mstate) {
        PyImport_ImportModule("scipy._cyutility");
    }
    static int __Pyx_DecompressString(void) {
        PyImport_ImportModuleLevel("compression.zstd", NULL, NULL, NULL, 0);
    }
    """
    assert imports.source_extension_runtime_python_imports(source) == (
        "math",
        "numpy",
        "scipy._cyutility",
    )


def test_control_calls_and_completed_declarations_cannot_author_eager_scope() -> None:
    assert (
        scan(
            """
        module_exec(module);
        { PyImport_ImportModule("after_call"); }
        if (module_exec(module)) { PyImport_ImportModule("conditional"); }
        for (int i = 0; i < 3; module_exec(module)) {
            PyImport_ImportModule("loop");
        }
        """,
            function="lazy_helper",
        )
        == ()
    )


def test_function_pointer_parameters_and_attributes_preserve_init_scope() -> None:
    source = """
    static int module_exec(PyObject *m, void (*callback)(int))
        __attribute__((used)) {
        PyImport_ImportModule("required");
    }
    """
    assert imports.source_extension_runtime_python_imports(source) == ("required",)


def test_helper_and_macro_policy_is_preserved_without_nonliteral_rejection() -> None:
    assert scan(
        """
        IMPORT_GLOBAL("pkg.exceptions", obj);
        IMPORT_NAME("pkg.names", obj);
        npy_cache_import("pkg.internal", "symbol");
        import_helper("pkg.helper");
        helper_import("pkg.other");
        important("not_an_import");
        PyImport_ImportModule(dynamic_name);
        import_helper(dynamic_name);
        """
    ) == ("pkg.exceptions", "pkg.helper", "pkg.internal", "pkg.names", "pkg.other")
    assert scan('IMPORT_GLOBAL("pkg.explicit", obj);', function="lazy_helper") == (
        "pkg.explicit",
    )


def test_comments_literals_and_duplicate_sites_do_not_create_extra_roots() -> None:
    assert scan(
        r"""
        // PyImport_ImportModule("comment");
        /* PyImport_ImportModule("block_comment"); */
        char *debug = "PyImport_ImportModule(\"debug_string\")";
        char brace = '{';
        PyImport_ImportModule("zeta");
        PyImport_ImportModule("alpha");
        PyImport_ImportModule("zeta");
        """
    ) == ("alpha", "zeta")


def test_long_comment_run_is_iterative() -> None:
    assert scan("PyImport_ImportModule(" + "/* gap */" * 1500 + '"known");') == (
        "known",
    )


def test_cpp_raw_literals_cannot_forge_calls_or_change_eager_scope() -> None:
    assert scan(
        """
        const char *text = R"tag(" } PyImport_ImportModule("fake"); { ")tag";
        PyImport_ImportModule("known");
        """
    ) == ("known",)
