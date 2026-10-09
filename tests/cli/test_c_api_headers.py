from __future__ import annotations

from pathlib import Path
import shutil
import sys
import tomllib

import pytest

from molt.c_api_headers import CAPIHeaderClosureError, c_api_header_closure
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.source_extension_toolchain import (
    _source_extension_include_dirs_for_abi_tier,
    _source_extension_python_header_for_abi_tier,
)
from tests.cli.process_guard import run_cli_test_process


@pytest.mark.parametrize("transport", ["source-linked", "source-host", "abi-linked"])
def test_descriptor_headers_share_layout_and_compiled_constructors(
    tmp_path: Path, transport: str
) -> None:
    """Prove descriptor/method carriers and every transport's linked owner.

    Independent owners check exact argument and return identities. A local
    method factory, identity wrapper or builtin-name lookup cannot satisfy this
    transport contract; runtime binding semantics are tested in readiness.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for C-API transport execution")
    root = Path(__file__).resolve().parents[2]
    probe = tmp_path / "probe.c"
    header = "Python.h" if transport == "abi-linked" else "molt/Python.h"
    probe.write_text(
        f"#include <{header}>\n"
        + """
#define REQUIRE(expr) do { if (!(expr)) return __LINE__; } while (0)
_Static_assert(offsetof(PyMethodDescrObject, d_common) == 0, "descriptor header");
_Static_assert(offsetof(PyMethodDescrObject, d_method) == sizeof(PyDescrObject), "definition edge");
_Static_assert(offsetof(PyMethodDescrObject, vectorcall) == sizeof(PyDescrObject) + sizeof(void *), "vectorcall edge");
_Static_assert(sizeof(PyMethodDescrObject) == sizeof(PyDescrObject) + 2 * sizeof(void *), "method layout");
_Static_assert(sizeof(PyMemberDescrObject) == sizeof(PyDescrObject) + sizeof(void *), "member layout");
_Static_assert(sizeof(PyGetSetDescrObject) == sizeof(PyDescrObject) + sizeof(void *), "getset layout");
_Static_assert(offsetof(PyMethodObject, vectorcall) == sizeof(PyObject) + 3 * sizeof(void *), "bound method vector offset");
_Static_assert(sizeof(PyMethodObject) == sizeof(PyObject) + 4 * sizeof(void *), "bound method layout");
int probe(void **token, void **types) {
    PyTypeObject *owner = token[0];
    PyMethodDef *method = token[1];
    PyMemberDef *member = token[2];
    PyGetSetDef *getset = token[3];
    struct wrapperbase *base = token[4];
    PyObject *value = token[5], *result = token[6];
    REQUIRE(&PyMethodDescr_Type == types[0]);
    REQUIRE(&PyClassMethodDescr_Type == types[1]);
    REQUIRE(&PyMemberDescr_Type == types[2]);
    REQUIRE(&PyGetSetDescr_Type == types[3]);
    REQUIRE(&PyWrapperDescr_Type == types[4]);
    REQUIRE(&_PyMethodWrapper_Type == types[5]);
    REQUIRE(&PyClassMethod_Type == types[6]);
    REQUIRE(&PyStaticMethod_Type == types[7]);
    REQUIRE(&PyMethod_Type == types[8]);
    REQUIRE(PyMethod_New(value, result) == result);
    REQUIRE(PyMethod_Check(result) == 29);
    REQUIRE(PyMethod_GET_FUNCTION(result) == value);
    REQUIRE(PyMethod_GET_SELF(result) == result);
    REQUIRE(PyDescr_NewMethod(owner, method) == result);
    REQUIRE(PyDescr_NewClassMethod(owner, method) == result);
    REQUIRE(PyDescr_NewMember(owner, member) == result);
    REQUIRE(PyDescr_NewGetSet(owner, getset) == result);
    REQUIRE(PyDescr_NewWrapper(owner, base, value) == result);
    REQUIRE(PyWrapper_New(result, value) == result);
    REQUIRE(PyClassMethod_New(value) == result);
    REQUIRE(PyStaticMethod_New(value) == result);
    REQUIRE(PyDescr_NAME(result) == value);
    REQUIRE(PyDescr_IsData(result) == 19);
    REQUIRE(PyMember_GetOne((const char *)value, member) == result);
    REQUIRE(PyMember_SetOne((char *)value, member, result) == 23);
    return 0;
}
""",
        encoding="utf-8",
    )
    owner = tmp_path / "owner.c"
    owner.write_text(
        """#include <stdint.h>
#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#else
#define EXPORT __attribute__((visibility("default")))
#endif
EXPORT intptr_t PyMethodDescr_Type[64], PyClassMethodDescr_Type[64];
EXPORT intptr_t PyMemberDescr_Type[64], PyGetSetDescr_Type[64];
EXPORT intptr_t PyWrapperDescr_Type[64], _PyMethodWrapper_Type[64];
EXPORT intptr_t PyClassMethod_Type[64], PyStaticMethod_Type[64], PyMethod_Type[64];
static intptr_t storage[7][64];
static unsigned int calls;
#define MATCH(index, expr) do { if (!(expr)) return 0; calls |= 1U << (index); } while (0)
EXPORT void *PyDescr_NewMethod(void *type, void *method) { MATCH(0, type == storage[0] && method == storage[1]); return storage[6]; }
EXPORT void *PyDescr_NewClassMethod(void *type, void *method) { MATCH(1, type == storage[0] && method == storage[1]); return storage[6]; }
EXPORT void *PyDescr_NewMember(void *type, void *member) { MATCH(2, type == storage[0] && member == storage[2]); return storage[6]; }
EXPORT void *PyDescr_NewGetSet(void *type, void *getset) { MATCH(3, type == storage[0] && getset == storage[3]); return storage[6]; }
EXPORT void *PyDescr_NewWrapper(void *type, void *base, void *target) { MATCH(4, type == storage[0] && base == storage[4] && target == storage[5]); return storage[6]; }
EXPORT void *PyWrapper_New(void *descr, void *value) { MATCH(5, descr == storage[6] && value == storage[5]); return storage[6]; }
EXPORT void *PyClassMethod_New(void *value) { MATCH(6, value == storage[5]); return storage[6]; }
EXPORT void *PyStaticMethod_New(void *value) { MATCH(7, value == storage[5]); return storage[6]; }
EXPORT void *PyDescr_NAME(void *descr) { MATCH(8, descr == storage[6]); return storage[5]; }
EXPORT int PyDescr_IsData(void *descr) { MATCH(9, descr == storage[6]); return 19; }
EXPORT void *PyMember_GetOne(void *value, void *member) { MATCH(10, value == storage[5] && member == storage[2]); return storage[6]; }
EXPORT int PyMember_SetOne(void *value, void *member, void *result) { MATCH(11, value == storage[5] && member == storage[2] && result == storage[6]); return 23; }
EXPORT void *PyMethod_New(void *func, void *self) { MATCH(12, func == storage[5] && self == storage[6]); return storage[6]; }
EXPORT int PyMethod_Check(void *value) { MATCH(13, value == storage[6]); return 29; }
EXPORT void *PyMethod_GET_FUNCTION(void *value) { MATCH(14, value == storage[6]); return storage[5]; }
EXPORT void *PyMethod_GET_SELF(void *value) { MATCH(15, value == storage[6]); return storage[6]; }
extern int probe(void **, void **);

int main(void) {
    void *token[] = {storage[0], storage[1], storage[2], storage[3], storage[4], storage[5], storage[6]};
    void *types[] = {PyMethodDescr_Type, PyClassMethodDescr_Type, PyMemberDescr_Type, PyGetSetDescr_Type, PyWrapperDescr_Type, _PyMethodWrapper_Type, PyClassMethod_Type, PyStaticMethod_Type, PyMethod_Type};
    int result = probe(token, types);
    if (result) return result;
    return calls != ((1U << 16) - 1);
}
""",
        encoding="utf-8",
    )
    includes = _source_extension_include_dirs_for_abi_tier(
        molt_root=root,
        abi_tier="cpython-abi" if transport == "abi-linked" else "source-compat",
    )
    output = tmp_path / ("probe.exe" if sys.platform == "win32" else "probe")
    command = [clang, "-O2", "-std=c11", *(f"-I{path}" for path in includes)]
    if transport == "source-host":
        command.append("-DMOLT_EXTENSION_HOST_ABI")
        if sys.platform == "darwin":
            command.append("-Wl,-export_dynamic")
        elif sys.platform != "win32":
            command.extend(["-Wl,--export-dynamic", "-ldl"])
    result = run_cli_test_process(
        [*command, str(probe), str(owner), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    result = run_cli_test_process(
        [str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("transport", ["source-linked", "source-host", "abi-linked"])
def test_ownership_headers_forward_to_compiled_owners(
    tmp_path: Path, transport: str
) -> None:
    """Preserve linked ownership/allocation and the private CHeap boundary."""
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for C-API transport execution")
    root = Path(__file__).resolve().parents[2]
    probe = tmp_path / "probe.c"
    header = "Python.h" if transport == "abi-linked" else "molt/Python.h"
    probe.write_text(
        f"#include <{header}>\n"
        + """
#define REQUIRE(expr) do { if (!(expr)) return __LINE__; } while (0)
extern void set_private(void *);
int probe(PyObject *value, PyVarObject *variable, PyTypeObject *type) {
    PyObject *obj = value;
    PyObject *(*initialize)(PyObject *, PyTypeObject *) = PyObject_Init;
    PyVarObject *(*initialize_var)(PyVarObject *, PyTypeObject *, Py_ssize_t) = PyObject_InitVar;
    PyObject *(*generic_alloc)(PyTypeObject *, Py_ssize_t) = PyType_GenericAlloc;
    PyObject *(*generic_new)(PyTypeObject *, PyObject *, PyObject *) = PyType_GenericNew;
    REQUIRE(PyUnstable_Object_IsUniquelyReferenced(obj) == 11);
    REQUIRE(PyUnstable_Object_IsUniqueReferencedTemporary(obj) == 12);
    REQUIRE(PyUnstable_SetImmortal(obj) == 13);
    REQUIRE(PyUnstable_Object_EnableDeferredRefcount(obj) == 14);
    Py_SET_REFCNT(obj, 17);
    REQUIRE(Py_REFCNT(obj) == 41); /* the linked owner records the write */
    REQUIRE(initialize(obj, type) == obj);
    REQUIRE(PyObject_INIT(obj, type) == obj);
    REQUIRE(initialize_var(variable, type, 23) == variable);
    REQUIRE(PyObject_INIT_VAR(variable, type, 23) == variable);
    REQUIRE(_PyObject_New(type) == obj);
    REQUIRE(PyObject_New(PyObject, type) == obj);
    REQUIRE(_PyObject_NewVar(type, 23) == variable);
    REQUIRE(PyObject_NewVar(PyVarObject, type, 23) == variable);
    REQUIRE(generic_alloc(type, 29) == obj);
    REQUIRE(generic_new(type, obj, (PyObject *)variable) == obj);
    REQUIRE(PyObject_LengthHint(obj, 31) == 37);
    REQUIRE(PySequence_Check(obj) == 19);
    REQUIRE(PySequence_Fast(obj, "required") == obj);
    REQUIRE(PySequence_List(obj) == obj);
    REQUIRE(PySequence_Tuple(obj) == obj);
    REQUIRE(PySequence_Fast_GET_SIZE(obj) == 23);
    REQUIRE(PySequence_Fast_GET_ITEM(obj, 29) == obj);
    REQUIRE(PySequence_Fast_ITEMS(obj)[0] == obj);
#ifdef MOLT_C_API_PYTHON_H
    _MoltCHeapObject private_storage = {_MOLT_C_HEAP_MAGIC, 1, 0, NULL, NULL};
    PyObject *private_obj = (PyObject *)&private_storage;
    set_private(private_obj);
    REQUIRE(PyUnstable_Object_IsUniquelyReferenced(private_obj) == 1);
    REQUIRE(PyUnstable_Object_IsUniqueReferencedTemporary(private_obj) == 0);
    REQUIRE(PyUnstable_SetImmortal(private_obj) == 0);
    REQUIRE(PyUnstable_Object_EnableDeferredRefcount(private_obj) == 0);
    REQUIRE(private_storage.refcnt == 1);
    Py_SET_REFCNT(private_obj, 2);
    REQUIRE(private_storage.refcnt == 2);
    REQUIRE(PyUnstable_Object_IsUniquelyReferenced(private_obj) == 0);
    REQUIRE(PyUnstable_Object_IsUniqueReferencedTemporary(private_obj) == 0);
    REQUIRE(PyUnstable_SetImmortal(private_obj) == 0);
    Py_SET_REFCNT(private_obj, _MOLT_C_HEAP_REFCNT_IMMORTAL);
    Py_SET_REFCNT(private_obj, 1);
    REQUIRE(private_storage.refcnt == _MOLT_C_HEAP_REFCNT_IMMORTAL);
    REQUIRE(PyUnstable_SetImmortal(private_obj) == 0);
    set_private(NULL);
#endif
    return 0;
}
""",
        encoding="utf-8",
    )
    owner = tmp_path / "owner.c"
    scalar_header = (
        "_numeric_scalar_abi.h"
        if transport == "abi-linked"
        else "molt/shared/_numeric_scalar_abi.h"
    )
    owner.write_text(
        """#include <stdint.h>
typedef intptr_t Py_ssize_t;
typedef uint32_t digit;
typedef struct { double real; double imag; } Py_complex;
"""
        + f"#include <{scalar_header}>\n"
        + """
#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#else
#define EXPORT __attribute__((visibility("default")))
#endif
static PyObject storage = {41, 0};
static PyVarObject variable = {43, 0, 47};
static PyObject type_token = {53, 0};
static uintptr_t private_address;
static int calls[19], bad;
EXPORT int PyUnstable_Object_IsUniquelyReferenced(PyObject *obj) {
    ++calls[0]; bad |= obj != &storage; return 11;
}
EXPORT int PyUnstable_Object_IsUniqueReferencedTemporary(PyObject *obj) {
    ++calls[1]; bad |= obj != &storage; return 12;
}
EXPORT int PyUnstable_SetImmortal(PyObject *obj) {
    ++calls[2]; bad |= obj != &storage; return 13;
}
EXPORT void molt_capi_set_refcnt(PyObject *obj, Py_ssize_t refs) {
    ++calls[3]; bad |= obj != &storage || refs != 17;
}
EXPORT int PyUnstable_Object_EnableDeferredRefcount(PyObject *obj) {
    ++calls[4]; bad |= obj != &storage; return 14;
}
EXPORT PyObject *PyObject_Init(PyObject *obj, PyTypeObject *type) {
    ++calls[5]; bad |= obj != &storage || type != (PyTypeObject *)&type_token;
    return &storage;
}
EXPORT PyVarObject *PyObject_InitVar(PyVarObject *obj, PyTypeObject *type, Py_ssize_t size) {
    ++calls[6]; bad |= obj != &variable || type != (PyTypeObject *)&type_token || size != 23;
    return &variable;
}
EXPORT PyObject *_PyObject_New(PyTypeObject *type) {
    ++calls[7]; bad |= type != (PyTypeObject *)&type_token; return &storage;
}
EXPORT PyVarObject *_PyObject_NewVar(PyTypeObject *type, Py_ssize_t size) {
    ++calls[8]; bad |= type != (PyTypeObject *)&type_token || size != 23;
    return &variable;
}
EXPORT PyObject *PyType_GenericAlloc(PyTypeObject *type, Py_ssize_t size) {
    ++calls[9]; bad |= type != (PyTypeObject *)&type_token || size != 29; return &storage;
}
EXPORT PyObject *PyType_GenericNew(PyTypeObject *type, PyObject *args, PyObject *kwargs) {
    ++calls[10]; bad |= type != (PyTypeObject *)&type_token
        || args != &storage || kwargs != (PyObject *)&variable;
    return &storage;
}
EXPORT int PySequence_Check(PyObject *obj) {
    ++calls[11]; bad |= obj != &storage; return 19;
}
EXPORT PyObject *PySequence_Fast(PyObject *obj, const char *message) {
    ++calls[12]; bad |= obj != &storage || message[0] != 'r'; return &storage;
}
EXPORT PyObject *PySequence_List(PyObject *obj) {
    ++calls[13]; bad |= obj != &storage; return &storage;
}
EXPORT PyObject *PySequence_Tuple(PyObject *obj) {
    ++calls[14]; bad |= obj != &storage; return &storage;
}
EXPORT Py_ssize_t PySequence_Fast_GET_SIZE(PyObject *obj) {
    ++calls[15]; bad |= obj != &storage; return 23;
}
EXPORT PyObject *PySequence_Fast_GET_ITEM(PyObject *obj, Py_ssize_t index) {
    ++calls[16]; bad |= obj != &storage || index != 29; return &storage;
}
EXPORT PyObject **PySequence_Fast_ITEMS(PyObject *obj) {
    static PyObject *items[] = { &storage };
    ++calls[17]; bad |= obj != &storage; return items;
}
EXPORT Py_ssize_t PyObject_LengthHint(PyObject *obj, Py_ssize_t defaultvalue) {
    ++calls[18]; bad |= obj != &storage || defaultvalue != 31; return 37;
}
EXPORT int32_t molt_c_heap_contains(uintptr_t address) {
    return address != 0 && address == private_address;
}
void set_private(void *obj) { private_address = (uintptr_t)obj; }
extern int probe(PyObject *, PyVarObject *, PyTypeObject *);
int main(void) {
    int result = probe(&storage, &variable, (PyTypeObject *)&type_token);
    if (result) return result;
    if (bad || storage.ob_refcnt != 41 || variable.ob_size != 47) return 100;
    for (int i = 0; i < 19; ++i) {
        int expected = i >= 5 && i <= 8 ? 2 : 1;
        if (calls[i] != expected) return 101 + i;
    }
    return 0;
}
""",
        encoding="utf-8",
    )
    includes = _source_extension_include_dirs_for_abi_tier(
        molt_root=root,
        abi_tier="cpython-abi" if transport == "abi-linked" else "source-compat",
    )
    output = tmp_path / ("probe.exe" if sys.platform == "win32" else "probe")
    command = [clang, "-O2", "-std=c11", *(f"-I{path}" for path in includes)]
    if transport == "source-host":
        command.append("-DMOLT_EXTENSION_HOST_ABI")
        if sys.platform == "darwin":
            command.append("-Wl,-export_dynamic")
        elif sys.platform != "win32":
            command.extend(["-Wl,--export-dynamic", "-ldl"])
    result = run_cli_test_process(
        [*command, str(probe), str(owner), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    result = run_cli_test_process(
        [str(output)], capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("transport", ["source-linked", "source-host", "abi-linked"])
def test_borrowed_namespace_getters_use_linked_owners(
    tmp_path: Path, transport: str
) -> None:
    """Compile/link each header transport against independent namespace owners.

    A copied inline getter would require unrelated runtime imports or alter the
    owner's identity/reference count. The host transport must resolve the same
    exported symbols rather than require static C-API linkage.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for C-API transport execution")
    root = Path(__file__).resolve().parents[2]
    probe = tmp_path / "probe.c"
    header = "Python.h" if transport == "abi-linked" else "molt/Python.h"
    probe.write_text(
        f"#include <{header}>\n"
        + """
int probe(void *modules, void *builtins, void *value) {
    for (int i = 0; i < 2; ++i) {
        if (PyImport_GetModuleDict() != modules) return 1;
        if (PyEval_GetBuiltins() != builtins) return 2;
        if (PySys_GetObject("value") != value) return 3;
    }
    return 0;
}
""",
        encoding="utf-8",
    )
    owner = tmp_path / "owner.c"
    owner.write_text(
        """#include <stdint.h>
#include <string.h>
#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#else
#define EXPORT __attribute__((visibility("default")))
#endif
static intptr_t modules[16] = {41}, builtins[16] = {42}, value[16] = {43};
static int calls[3];
EXPORT void *PyImport_GetModuleDict(void) { ++calls[0]; return modules; }
EXPORT void *PyEval_GetBuiltins(void) { ++calls[1]; return builtins; }
EXPORT void *PySys_GetObject(const char *name) {
    ++calls[2]; return strcmp(name, "value") == 0 ? value : 0;
}
extern int probe(void *, void *, void *);
int main(void) {
    int result = probe(modules, builtins, value);
    if (result) return result;
    if (calls[0] != 2 || calls[1] != 2 || calls[2] != 2) return 4;
    return modules[0] != 41 || builtins[0] != 42 || value[0] != 43;
}
""",
        encoding="utf-8",
    )
    includes = _source_extension_include_dirs_for_abi_tier(
        molt_root=root,
        abi_tier="cpython-abi" if transport == "abi-linked" else "source-compat",
    )
    output = tmp_path / ("probe.exe" if sys.platform == "win32" else "probe")
    command = [clang, "-O2", "-std=c11", *(f"-I{path}" for path in includes)]
    if transport == "source-host":
        command.append("-DMOLT_EXTENSION_HOST_ABI")
        if sys.platform == "darwin":
            command.append("-Wl,-export_dynamic")
        elif sys.platform != "win32":
            command.extend(["-Wl,--export-dynamic", "-ldl"])
    result = run_cli_test_process(
        [*command, str(probe), str(owner), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    result = run_cli_test_process(
        [str(output)], capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("transport", ["source-linked", "source-host", "abi-linked"])
def test_exception_and_attribute_headers_use_compiled_owners(
    tmp_path: Path, transport: str
) -> None:
    """Exercise header routing against independently linked owner functions.

    This tests the C transport, not runtime exception/descriptor semantics. The
    owners distinguish pending/handled state and normal/generic attribute paths;
    header-local normalization, swallowed failures, or handle conversions cannot
    satisfy their identity, argument, and call coverage checks.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for C-API transport execution")
    root = Path(__file__).resolve().parents[2]
    stable_abi = tomllib.loads(
        (root / "config/cpython_stable_abi_3_12.toml").read_text(encoding="utf-8")
    )
    aliases = {"PyExc_EnvironmentError", "PyExc_IOError", "PyExc_WindowsError"}
    symbols = sorted(
        name
        for name in stable_abi["data"]
        if name.startswith("PyExc_") and name not in aliases
    )
    symbol_checks = "\n".join(
        f"    REQUIRE({name} == symbols[{index}]);"
        for index, name in enumerate(symbols)
    )
    symbol_definitions = "\n".join(
        f"EXPORT intptr_t {name}[16] = {{{100 + index}}};"
        for index, name in enumerate(symbols)
    )
    symbol_values = ", ".join(symbols)
    probe = tmp_path / "probe.c"
    header = "Python.h" if transport == "abi-linked" else "molt/Python.h"
    probe.write_text(
        f"#include <{header}>\n"
        + """
#define REQUIRE(expr) do { if (!(expr)) return __LINE__; } while (0)
int probe(void **tokens, void **symbols) {
    PyObject *type = tokens[0], *pending = tokens[1], *handled = tokens[2];
    PyObject *normal = tokens[3], *generic = tokens[4], *dict = tokens[5];
    PyObject *name = tokens[6], *obj = tokens[7];
    PyObject *t = NULL, *v = NULL, *tb = NULL;
    PyErr_Clear();
    REQUIRE(PyErr_Occurred() == NULL);
    PyErr_SetString(type, "message");
    REQUIRE(PyErr_Occurred() == type);
    /* EXCEPTION_SYMBOL_CHECKS */
    REQUIRE(PyExc_EnvironmentError == PyExc_OSError && PyExc_IOError == PyExc_OSError);
#if defined(_WIN32) || defined(MS_WINDOWS)
    REQUIRE(PyExc_WindowsError == PyExc_OSError);
#endif
    REQUIRE(PyErr_GetRaisedException() == pending);
    PyErr_SetRaisedException(pending);
    PyErr_SetNone(type);
    PyErr_SetObject(type, pending);
    PyErr_Fetch(&t, &v, &tb);
    REQUIRE(t == type && v == pending && tb == dict);
    REQUIRE(PyErr_Occurred() == NULL);
    PyErr_Restore(t, v, tb);
    REQUIRE(PyErr_GetRaisedException() == pending);
    REQUIRE(PyErr_Occurred() == NULL);
    PyErr_SetRaisedException(pending);
    PyErr_SetHandledException(handled);
    REQUIRE(PyErr_GetHandledException() == handled);
    PyErr_GetExcInfo(&t, &v, &tb);
    REQUIRE(t == type && v == handled && tb == dict);
    REQUIRE(PyErr_GetRaisedException() == pending);
    PyErr_SetRaisedException(pending);
    PyErr_SetExcInfo(type, handled, dict);
    REQUIRE(PyErr_GetRaisedException() == pending);
    REQUIRE(PyErr_GetHandledException() == handled);
    PyErr_SetRaisedException(pending);
    PyErr_SetRaisedException(NULL);
    REQUIRE(PyErr_Occurred() == NULL);
    t = v = tb = NULL;
    PyErr_NormalizeException(&t, &v, &tb);
    REQUIRE(t == type && v == pending && tb == dict);
    REQUIRE(PyErr_GivenExceptionMatches(pending, type) == 17);
    REQUIRE(PyErr_ExceptionMatches(type) == 19);
    REQUIRE(PyErr_NoMemory() == NULL);
    REQUIRE(PyErr_BadArgument() == 0);
    PyErr_BadInternalCall();
    PyErr_Print();
    PyErr_PrintEx(1);
    REQUIRE(PyErr_WarnEx(type, "message", 7) == -1);
    REQUIRE(PyErr_WarnFormat(type, 9, "%s", "message") == -1);
    REQUIRE(PyObject_GetAttr(obj, name) == normal);
    REQUIRE(PyObject_GetAttrString(obj, "name") == normal);
    REQUIRE(PyObject_SetAttr(obj, name, normal) == -21);
    REQUIRE(PyObject_SetAttrString(obj, "name", normal) == -22);
    REQUIRE(PyObject_DelAttr(obj, name) == -21);
    REQUIRE(PyObject_DelAttrString(obj, "name") == -22);
    REQUIRE(PyObject_HasAttr(obj, name) == 1);
    REQUIRE(PyObject_HasAttrString(obj, "name") == 1);
    REQUIRE(PyObject_HasAttrWithError(obj, name) == -1);
    REQUIRE(PyObject_HasAttrStringWithError(obj, "name") == -1);
    REQUIRE(PyObject_GetOptionalAttr(obj, name, &v) == 1 && v == normal);
    REQUIRE(PyObject_GetOptionalAttrString(obj, "name", &v) == 1 && v == normal);
    REQUIRE(PyObject_GenericGetAttr(obj, name) == generic);
    REQUIRE(PyObject_GenericSetAttr(obj, name, generic) == -23);
    REQUIRE(_PyObject_GenericGetAttrWithDict(obj, name, dict, 1) == dict);
    REQUIRE(PyObject_GenericGetDict(obj, pending) == dict);
    REQUIRE(PyObject_GenericSetDict(obj, dict, pending) == -24);
    return 0;
}
""".replace("/* EXCEPTION_SYMBOL_CHECKS */", symbol_checks),
        encoding="utf-8",
    )
    owner = tmp_path / "owner.c"
    owner.write_text(
        """#include <stdint.h>
#include <stdarg.h>
#include <string.h>
#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#else
#define EXPORT __attribute__((visibility("default")))
#endif
/* EXCEPTION_SYMBOL_DEFINITIONS */
static intptr_t storage[8][16] = {{41}, {42}, {43}, {44}, {45}, {46}, {47}, {48}};
static void *tokens[] = {storage[0], storage[1], storage[2], storage[3],
                        storage[4], storage[5], storage[6], storage[7]};
static void *raised, *handled;
static uint64_t seen;
static int bad, deleted;
#define T tokens[0]
#define P tokens[1]
#define H tokens[2]
#define N tokens[3]
#define G tokens[4]
#define D tokens[5]
#define A tokens[6]
#define O tokens[7]
#define HIT(n) (seen |= UINT64_C(1) << (n))
#define CHECK(expr) (bad |= !(expr))
#define ATTR(obj, name) CHECK((obj) == O && (name) == A)
#define ATTRSTR(obj, name) CHECK((obj) == O && strcmp(name, "name") == 0)
EXPORT void *PyErr_Occurred(void) { HIT(0); return raised ? T : 0; }
EXPORT void PyErr_Clear(void) { HIT(1); raised = 0; }
EXPORT void PyErr_SetString(void *t, const char *m) {
    HIT(2); CHECK(t == T && strcmp(m, "message") == 0); raised = P;
}
EXPORT void PyErr_SetObject(void *t, void *v) { HIT(3); CHECK(t == T && v == P); raised = v; }
EXPORT void PyErr_SetNone(void *t) { HIT(4); CHECK(t == T); raised = P; }
EXPORT void *PyErr_NoMemory(void) { HIT(5); return 0; }
EXPORT void PyErr_Fetch(void **t, void **v, void **tb) {
    HIT(6); *t = T; *v = raised; *tb = D; raised = 0;
}
EXPORT void PyErr_Restore(void *t, void *v, void *tb) {
    HIT(7); CHECK(t == T && v == P && tb == D); raised = v;
}
EXPORT void PyErr_NormalizeException(void **t, void **v, void **tb) {
    HIT(8); CHECK(!*t && !*v && !*tb); *t = T; *v = P; *tb = D;
}
EXPORT void *PyErr_GetRaisedException(void) { HIT(9); void *v = raised; raised = 0; return v; }
EXPORT void PyErr_SetRaisedException(void *v) { HIT(10); CHECK(!v || v == P); raised = v; }
EXPORT void *PyErr_GetHandledException(void) { HIT(11); return handled; }
EXPORT void PyErr_SetHandledException(void *v) { HIT(12); CHECK(v == H); handled = v; }
EXPORT void PyErr_GetExcInfo(void **t, void **v, void **tb) {
    HIT(13); *t = T; *v = handled; *tb = D;
}
EXPORT void PyErr_SetExcInfo(void *t, void *v, void *tb) {
    HIT(14); CHECK(t == T && v == H && tb == D); handled = v;
}
EXPORT int PyErr_GivenExceptionMatches(void *v, void *t) { HIT(15); CHECK(v == P && t == T); return 17; }
EXPORT int PyErr_ExceptionMatches(void *t) { HIT(16); CHECK(t == T); return 19; }
EXPORT int PyErr_BadArgument(void) { HIT(17); return 0; }
EXPORT void PyErr_BadInternalCall(void) { HIT(18); }
EXPORT void PyErr_Print(void) { HIT(19); }
EXPORT void PyErr_PrintEx(int last) { HIT(20); CHECK(last == 1); }
EXPORT int PyErr_WarnEx(void *t, const char *m, intptr_t level) {
    HIT(21); CHECK(t == T && strcmp(m, "message") == 0 && level == 7); return -1;
}
EXPORT int PyErr_WarnFormat(void *t, intptr_t level, const char *format, ...) {
    HIT(22); CHECK(t == T && level == 9 && strcmp(format, "%s") == 0);
    va_list args; va_start(args, format);
    CHECK(strcmp(va_arg(args, const char *), "message") == 0); va_end(args); return -1;
}
EXPORT void *PyObject_GetAttr(void *o, void *a) { HIT(23); ATTR(o, a); return N; }
EXPORT void *PyObject_GetAttrString(void *o, const char *a) { HIT(24); ATTRSTR(o, a); return N; }
EXPORT int PyObject_SetAttr(void *o, void *a, void *v) {
    HIT(25); ATTR(o, a); CHECK(!v || v == N); if (!v) deleted |= 1; return -21;
}
EXPORT int PyObject_SetAttrString(void *o, const char *a, void *v) {
    HIT(26); ATTRSTR(o, a); CHECK(!v || v == N); if (!v) deleted |= 2; return -22;
}
EXPORT int PyObject_HasAttr(void *o, void *a) { HIT(27); ATTR(o, a); return 1; }
EXPORT int PyObject_HasAttrString(void *o, const char *a) { HIT(28); ATTRSTR(o, a); return 1; }
EXPORT int PyObject_HasAttrWithError(void *o, void *a) { HIT(29); ATTR(o, a); return -1; }
EXPORT int PyObject_HasAttrStringWithError(void *o, const char *a) { HIT(30); ATTRSTR(o, a); return -1; }
EXPORT int PyObject_GetOptionalAttr(void *o, void *a, void **v) { HIT(31); ATTR(o, a); *v = N; return 1; }
EXPORT int PyObject_GetOptionalAttrString(void *o, const char *a, void **v) { HIT(32); ATTRSTR(o, a); *v = N; return 1; }
EXPORT void *PyObject_GenericGetAttr(void *o, void *a) { HIT(33); ATTR(o, a); return G; }
EXPORT int PyObject_GenericSetAttr(void *o, void *a, void *v) {
    HIT(34); ATTR(o, a); CHECK(v == G); return -23;
}
EXPORT void *_PyObject_GenericGetAttrWithDict(void *o, void *a, void *d, int suppress) {
    HIT(35); ATTR(o, a); CHECK(d == D && suppress == 1); return D;
}
EXPORT void *PyObject_GenericGetDict(void *o, void *context) {
    HIT(36); CHECK(o == O && context == P); return D;
}
EXPORT int PyObject_GenericSetDict(void *o, void *v, void *context) {
    HIT(37); CHECK(o == O && v == D && context == P); return -24;
}
extern int probe(void **, void **);
int main(void) {
    void *symbols[] = {/* EXCEPTION_SYMBOL_VALUES */};
    int result = probe(tokens, symbols);
    if (result) return result;
    if (bad || deleted != 3 || seen != (UINT64_C(1) << 38) - 1) return 100;
    for (int i = 0; i < 8; ++i) {
        if (storage[i][0] != 41 + i) return 101;
    }
    return 0;
}
""".replace("/* EXCEPTION_SYMBOL_DEFINITIONS */", symbol_definitions).replace(
            "/* EXCEPTION_SYMBOL_VALUES */", symbol_values
        ),
        encoding="utf-8",
    )
    includes = _source_extension_include_dirs_for_abi_tier(
        molt_root=root,
        abi_tier="cpython-abi" if transport == "abi-linked" else "source-compat",
    )
    output = tmp_path / ("probe.exe" if sys.platform == "win32" else "probe")
    command = [clang, "-O2", "-std=c11", *(f"-I{path}" for path in includes)]
    if transport == "source-host":
        command.append("-DMOLT_EXTENSION_HOST_ABI")
        if sys.platform == "darwin":
            command.append("-Wl,-export_dynamic")
        elif sys.platform != "win32":
            command.extend(["-Wl,--export-dynamic", "-ldl"])
    result = run_cli_test_process(
        [*command, str(probe), str(owner), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    result = run_cli_test_process(
        [str(output)], capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr


def test_owned_header_closure_preserves_search_order_cycles_and_system_boundary(
    tmp_path: Path,
) -> None:
    public, shared = tmp_path / "public", tmp_path / "shared"
    public.mkdir()
    shared.mkdir()
    header = public / "Python.h"
    header.write_text(
        '#include "local.h"\n#include <_exports.h>\n#include <sys/types.h>\n'
        '/* #include "missing-comment.h" */\n// #include "missing-line.h"\n',
        encoding="utf-8",
    )
    (public / "local.h").write_text('#include "Python.h"\n', encoding="utf-8")
    (shared / "local.h").write_text(
        '#include "wrong-search-order.h"\n', encoding="utf-8"
    )
    (shared / "_exports.h").write_text("extern int PyOwned(void);\n", encoding="utf-8")
    (shared / "unrelated.h").write_text(
        '#include "must-not-be-scanned.h"\n', encoding="utf-8"
    )

    assert c_api_header_closure(header, include_dirs=(shared, public)) == tuple(
        sorted(
            (header, public / "local.h", shared / "_exports.h"),
            key=lambda p: p.as_posix(),
        )
    )


@pytest.mark.parametrize("include", ['"missing.h"', "<_missing.h>"])
def test_missing_owned_include_fails_closed(tmp_path: Path, include: str) -> None:
    header = tmp_path / "Python.h"
    header.write_text(f"#include {include}\n", encoding="utf-8")
    with pytest.raises(CAPIHeaderClosureError, match="missing local header"):
        c_api_header_closure(header, include_dirs=(tmp_path,))
    surface, returned_header, error = _load_c_api_scan_surface(
        tmp_path, header_path=header
    )
    assert surface is None
    assert returned_header == header
    assert error is not None and "missing local header" in error


@pytest.mark.parametrize("abi_tier", ["source-compat", "cpython-abi"])
def test_scan_surface_follows_selected_tier_private_declarations(
    tmp_path: Path,
    abi_tier: str,
) -> None:
    header = _source_extension_python_header_for_abi_tier(
        molt_root=tmp_path, abi_tier=abi_tier
    )
    roots = _source_extension_include_dirs_for_abi_tier(
        molt_root=tmp_path, abi_tier=abi_tier
    )
    header.parent.mkdir(parents=True, exist_ok=True)
    shared = tmp_path / "include" / "molt" / "shared"
    shared.mkdir(parents=True, exist_ok=True)
    private = shared / "_module_callable_exports.h"
    private.write_text(
        "extern void *PyModule_Create2(void *, int);\n", encoding="utf-8"
    )
    (shared / "unrelated.h").write_text(
        "extern void PyUnrelated(void);\n", encoding="utf-8"
    )
    include = (
        "<_module_callable_exports.h>"
        if abi_tier == "cpython-abi"
        else '"shared/_module_callable_exports.h"'
    )
    header.write_text(f"#include {include}\n#include <stdint.h>\n", encoding="utf-8")

    surface, returned_header, error = _load_c_api_scan_surface(
        tmp_path, header_path=header
    )

    assert error is None and surface is not None
    assert returned_header == header
    assert surface.status_for("PyModule_Create2") == "runtime_backed"
    assert surface.status_for("PyUnrelated") == "missing"
    assert private in c_api_header_closure(header, include_dirs=roots)


def test_explicit_fixture_header_does_not_borrow_a_canonical_sdk(
    tmp_path: Path,
) -> None:
    canonical = _source_extension_python_header_for_abi_tier(
        molt_root=tmp_path, abi_tier="source-compat"
    )
    canonical.parent.mkdir(parents=True)
    canonical.write_text("extern int PyCanonicalOnly(void);\n", encoding="utf-8")
    custom = tmp_path / "fixture" / "Python.h"
    custom.parent.mkdir()
    custom.write_text('#include "_custom.h"\n', encoding="utf-8")
    (custom.parent / "_custom.h").write_text(
        "extern int PyCustomOnly(void);\n", encoding="utf-8"
    )

    surface, _, error = _load_c_api_scan_surface(tmp_path, header_path=custom)

    assert error is None and surface is not None
    assert surface.status_for("PyCustomOnly") == "runtime_backed"
    assert surface.status_for("PyCanonicalOnly") == "missing"


@pytest.mark.parametrize("transport", ["source-linked", "source-host", "abi-linked"])
def test_gc_headers_delegate_allocation_controls_and_queries(
    tmp_path: Path, transport: str
) -> None:
    """The headers must reach the same owner, including host symbol loading.

    This transport fixture does not stand in for runtime GC execution. Opaque
    tokens ensure the source facade cannot manufacture a second object layout;
    the runtime C-consumer tests exercise actual collection and finalization.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for C-API transport execution")
    root = Path(__file__).resolve().parents[2]
    header = "Python.h" if transport == "abi-linked" else "molt/Python.h"
    probe = tmp_path / "probe.c"
    probe.write_text(
        f"#include <{header}>\n"
        + r"""
#define REQUIRE(expr) do { if (!(expr)) return __LINE__; } while (0)
int probe(void **tokens) {
    PyTypeObject *type = (PyTypeObject *)tokens[0];
    PyObject *fixed = _PyObject_GC_New(type);
    PyVarObject *variable = _PyObject_GC_NewVar(type, 3);
    REQUIRE(fixed == tokens[1] && variable == tokens[2]);
    REQUIRE(PyObject_GC_IsTracked(fixed) == 0);
    REQUIRE(PyObject_GC_IsFinalized(fixed) == 1);
    REQUIRE(PyObject_GC_IsFinalized((PyObject *)variable) == 0);
    PyObject_GC_Track(fixed);
    PyObject_GC_Track(variable);
    REQUIRE(PyObject_GC_IsTracked(fixed) == 1);
    REQUIRE(PyObject_GC_IsTracked((PyObject *)variable) == 1);
    PyObject_GC_UnTrack(fixed);
    PyObject_GC_UnTrack(variable);
    REQUIRE(PyObject_GC_IsTracked(fixed) == 0);
    PyObject_GC_Del(fixed);
    PyObject_GC_Del(variable);
    REQUIRE(PyGC_IsEnabled() == 1);
    REQUIRE(PyGC_Disable() == 1 && PyGC_IsEnabled() == 0);
    REQUIRE(PyGC_Enable() == 0 && PyGC_IsEnabled() == 1);
    REQUIRE(PyGC_Collect() == (Py_ssize_t)(UINTPTR_MAX >> 2));
#ifdef MOLT_SOURCE_GC_PROBE
    /* Registered private headers have no GC membership. Never pass their
     * non-PyObject storage to the linked native owner. */
    REQUIRE(PyObject_GC_IsTracked((PyObject *)tokens[3]) == 0);
    REQUIRE(PyObject_GC_IsFinalized((PyObject *)tokens[3]) == 0);
    PyObject_GC_UnTrack(tokens[3]);
#endif
    return 0;
}
""",
        encoding="utf-8",
    )
    owner = tmp_path / "owner.c"
    owner.write_text(
        r"""#include <stdint.h>
#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#else
#define EXPORT __attribute__((visibility("default")))
#endif
static uintptr_t type[16], fixed[16], variable[16], private_header[16];
static unsigned int calls;
static int invalid, fixed_tracked, variable_tracked, enabled = 1;
static int *tracking(void *object) {
    if (object == fixed) return &fixed_tracked;
    if (object == variable) return &variable_tracked;
    ++invalid;
    return &invalid;
}
EXPORT int32_t molt_c_heap_contains(uintptr_t object) {
    return object == (uintptr_t)private_header;
}
EXPORT void *_PyObject_GC_New(void *value) {
    calls |= 1U << 0;
    if (value != type) ++invalid;
    return fixed;
}
EXPORT void *_PyObject_GC_NewVar(void *value, intptr_t size) {
    calls |= 1U << 1;
    if (value != type || size != 3) ++invalid;
    return variable;
}
EXPORT void PyObject_GC_Track(void *value) { calls |= 1U << 2; *tracking(value) = 1; }
EXPORT void PyObject_GC_UnTrack(void *value) { calls |= 1U << 3; *tracking(value) = 0; }
EXPORT void PyObject_GC_Del(void *value) {
    calls |= 1U << 4;
    if (*tracking(value)) ++invalid;
}
EXPORT int PyObject_GC_IsTracked(void *value) { calls |= 1U << 5; return *tracking(value); }
EXPORT int PyObject_GC_IsFinalized(void *value) {
    calls |= 1U << 6;
    if (value != fixed && value != variable) ++invalid;
    return value == fixed;
}
EXPORT intptr_t PyGC_Collect(void) { calls |= 1U << 7; return (intptr_t)(UINTPTR_MAX >> 2); }
EXPORT int PyGC_Enable(void) { int old = enabled; calls |= 1U << 8; enabled = 1; return old; }
EXPORT int PyGC_Disable(void) { int old = enabled; calls |= 1U << 9; enabled = 0; return old; }
EXPORT int PyGC_IsEnabled(void) { calls |= 1U << 10; return enabled; }
extern int probe(void **);
int main(void) {
    void *tokens[] = {type, fixed, variable, private_header};
    int result = probe(tokens);
    if (result) return result;
    return invalid || calls != ((1U << 11) - 1);
}
""",
        encoding="utf-8",
    )
    includes = _source_extension_include_dirs_for_abi_tier(
        molt_root=root,
        abi_tier="cpython-abi" if transport == "abi-linked" else "source-compat",
    )
    output = tmp_path / ("probe.exe" if sys.platform == "win32" else "probe")
    command = [clang, "-O2", "-std=c11", *(f"-I{path}" for path in includes)]
    if transport != "abi-linked":
        command.append("-DMOLT_SOURCE_GC_PROBE")
    if transport == "source-host":
        command.append("-DMOLT_EXTENSION_HOST_ABI")
        if sys.platform == "darwin":
            command.append("-Wl,-export_dynamic")
        elif sys.platform != "win32":
            command.extend(["-Wl,--export-dynamic", "-ldl"])
    result = run_cli_test_process(
        [*command, str(probe), str(owner), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    result = run_cli_test_process(
        [str(output)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("transport", ["source-linked", "source-host", "abi-linked"])
@pytest.mark.parametrize("shared_owner", [False, True], ids=["static", "windows-dll"])
@pytest.mark.parametrize(
    "private_copy", [False, True], ids=["exported", "private-mutant"]
)
def test_optimize_flag_headers_share_data_across_translation_units(
    tmp_path: Path, transport: str, shared_owner: bool, private_copy: bool
) -> None:
    """Each facade must read/write one external owner, including DLL imports.

    The negative replaces only the data declaration with the historical private
    binding. It must compile and fail the independent address oracle, not merely
    fail compilation or observe the same initial zero in every translation unit.
    """
    if shared_owner and sys.platform != "win32":
        pytest.skip("actual Windows DLL data import requires Windows")
    windows = sys.platform == "win32"
    driver_name = "clang-cl" if windows else "clang"
    compiler = shutil.which(driver_name)
    if compiler is None:
        pytest.skip(f"{driver_name} is required for C-API transport execution")
    root = Path(__file__).resolve().parents[2]
    tier = "cpython-abi" if transport == "abi-linked" else "source-compat"
    includes = _source_extension_include_dirs_for_abi_tier(
        molt_root=root, abi_tier=tier
    )
    header = _source_extension_python_header_for_abi_tier(molt_root=root, abi_tier=tier)
    if private_copy:
        # Preserve the real distributed header/include family; mutate only the
        # data binding under test in a private top-level header.
        source = header.read_text(encoding="utf-8")
        declaration = "PyAPI_DATA(int) Py_OptimizeFlag;"
        assert source.count(declaration) == 1
        mutant = tmp_path / "private_python.h"
        mutant.write_text(
            source.replace(declaration, "static int Py_OptimizeFlag = 0;"),
            encoding="utf-8",
        )
        # Quoted includes still resolve from the actual header's directory.
        includes = (header.parent, *includes)
        header = mutant
    consumers = []
    for name in ("left", "right"):
        consumer = tmp_path / f"{name}.c"
        consumer.write_text(
            f'#include "{header.as_posix()}"\n'
            f"int *{name}_address(void) {{ return &Py_OptimizeFlag; }}\n"
            f"int {name}_read(void) {{ return Py_OptimizeFlag; }}\n"
            f"void {name}_write(int value) {{ Py_OptimizeFlag = value; }}\n",
            encoding="utf-8",
        )
        consumers.append(consumer)
    owner = tmp_path / "owner.c"
    owner.write_text(
        "#ifdef _WIN32\n__declspec(dllexport)\n#endif\nint Py_OptimizeFlag = 17;\n",
        encoding="utf-8",
    )
    driver = tmp_path / "driver.c"
    driver.write_text(
        """#if defined(_WIN32) && defined(MOLT_CPYTHON_ABI_SHARED)
__declspec(dllimport)
#endif
extern int Py_OptimizeFlag;
extern int *left_address(void), *right_address(void);
extern int left_read(void), right_read(void);
extern void left_write(int), right_write(int);
int main(void) {
    if (left_address() != &Py_OptimizeFlag || right_address() != &Py_OptimizeFlag) return 41;
    if (left_read() != 17 || right_read() != 17) return 42;
    left_write(23);
    if (Py_OptimizeFlag != 23 || right_read() != 23) return 43;
    right_write(31);
    if (Py_OptimizeFlag != 31 || left_read() != 31) return 44;
    Py_OptimizeFlag = 47;
    if (left_read() != 47 || right_read() != 47) return 45;
    return 0;
}
""",
        encoding="utf-8",
    )
    command = [compiler, *(("/std:c11", "/O2") if windows else ("-std=c11", "-O2"))]
    for include in includes:
        command.extend(["/I" if windows else "-I", str(include)])
    if transport == "source-host":
        command.append(f"{'/D' if windows else '-D'}MOLT_EXTENSION_HOST_ABI=1")
    if shared_owner:
        # Exercise the native Windows MSVC driver and linker;
        # consume its real import library, not a preprocessor-only DLL mock.
        library = tmp_path / "owner.lib"
        build = run_cli_test_process(
            [
                compiler,
                "/LD",
                str(owner),
                f"/Fe{tmp_path / 'owner.dll'}",
                "/link",
                f"/IMPLIB:{library}",
            ],
            cwd=tmp_path,
            capture_output=True,
            text=True,
            check=False,
        )
        assert build.returncode == 0, build.stderr
        command.append("/DMOLT_CPYTHON_ABI_SHARED=1")
        owner_input = library
    else:
        owner_input = owner
    output = tmp_path / ("probe.exe" if sys.platform == "win32" else "probe")
    build = run_cli_test_process(
        [
            *command,
            *(str(path) for path in consumers),
            str(driver),
            str(owner_input),
            *([f"/Fe{output}"] if windows else ["-o", str(output)]),
        ],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr
    result = run_cli_test_process(
        [str(output)], cwd=tmp_path, capture_output=True, text=True, check=False
    )
    assert result.returncode == (41 if private_copy else 0), result.stderr
