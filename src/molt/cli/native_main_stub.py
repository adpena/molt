from __future__ import annotations

import json
from pathlib import Path
from typing import Sequence

from molt.capability_manifest import ResolvedRuntimePolicy
from molt.cli.native_launch_abi_generated import NATIVE_LAUNCH_C_DECLARATIONS
from molt.cli.native_launch_plan import NativeLaunchPlan


def _native_main_stub_snippets(
    *,
    plan: NativeLaunchPlan,
) -> tuple[str, str, str, str]:
    windows_updates = "\n".join(
        f"    if (_putenv_s({json.dumps(key)}, {json.dumps(value)}) != 0) "
        "molt_capability_policy_env_failure();"
        for key, value in plan.environment.items()
    )
    posix_updates = "\n".join(
        f"    if (setenv({json.dumps(key)}, {json.dumps(value)}, 1) != 0) "
        "molt_capability_policy_env_failure();"
        for key, value in plan.environment.items()
    )
    policy_snippet = f"""
static void molt_capability_policy_env_failure() {{
    fprintf(stderr, "molt: failed to install resolved capability policy\\n");
    _Exit(125);
}}

static void molt_set_capability_policy() {{
#ifdef _WIN32
{windows_updates}
#else
{posix_updates}
#endif
}}
"""
    module_roots_snippet = ""
    module_roots_call = ""
    return (
        policy_snippet,
        "    molt_set_capability_policy();\n",
        module_roots_snippet,
        module_roots_call,
    )


def _c_string_literal(text: str) -> str:
    """The exact WTF-8 bytes of ``text``, Molt's string encoding, as a C string
    literal; JSON escapes are not C (a non-BMP `\\u` pair is invalid C)."""
    parts = []
    for byte in text.encode("utf-8", "surrogatepass"):
        if byte in b'"\\':
            parts.append("\\" + chr(byte))
        elif 0x20 <= byte < 0x7F and byte != ord("?"):  # `??x` could be a trigraph
            parts.append(chr(byte))
        else:
            parts.append(f"\\{byte:03o}")
    return '"' + "".join(parts) + '"'


def _compiled_sources_snippets(compiled_source_paths: Sequence[str]) -> tuple[str, str]:
    """Register the program's own source files with the runtime: tracebacks may
    show their lines without the `fs.read` capability. The set is fixed before
    the runtime starts, and no Python-reachable call can extend it."""
    if not compiled_source_paths:
        return "", "    molt_register_compiled_sources(NULL, 0);\n"
    literals = ",\n    ".join(_c_string_literal(path) for path in compiled_source_paths)
    snippet = (
        f"static const char* const molt_compiled_sources[] = {{\n    {literals}\n}};\n"
    )
    call = (
        "    molt_register_compiled_sources(molt_compiled_sources, "
        f"{len(compiled_source_paths)}ULL);\n"
    )
    return snippet, call


def _render_native_main_stub(
    *,
    resolved_capability_policy: ResolvedRuntimePolicy,
    compiled_source_paths: Sequence[str] = (),
    runtime_module_roots: Sequence[Path] = (),
) -> str:
    """Render the program's ``NativeLaunchPlan`` as an AOT executable's C main."""
    plan = NativeLaunchPlan.for_program(
        resolved_capability_policy=resolved_capability_policy,
        compiled_source_paths=compiled_source_paths,
        runtime_module_roots=runtime_module_roots,
    )
    runtime_module_roots_literals = tuple(
        json.dumps(root) for root in plan.runtime_module_roots
    )
    (
        capability_policy_snippet,
        capability_policy_call,
        module_roots_snippet,
        module_roots_call,
    ) = _native_main_stub_snippets(plan=plan)
    if runtime_module_roots_literals:
        roots_array = ", ".join(runtime_module_roots_literals)
        roots_count = len(runtime_module_roots_literals)
        module_roots_snippet = f"""
static char* molt_join_runtime_module_roots() {{
    const char* roots[{roots_count}] = {{{roots_array}}};
    size_t total = 1;
    for (size_t i = 0; i < {roots_count}; i++) {{
        total += strlen(roots[i]);
        if (i + 1 < {roots_count}) {{
            total += 1;
        }}
    }}
    char* joined = (char*)malloc(total);
    if (joined == NULL) {{
        return NULL;
    }}
    size_t offset = 0;
    for (size_t i = 0; i < {roots_count}; i++) {{
        size_t len = strlen(roots[i]);
        memcpy(joined + offset, roots[i], len);
        offset += len;
        if (i + 1 < {roots_count}) {{
#ifdef _WIN32
            joined[offset++] = ';';
#else
            joined[offset++] = ':';
#endif
        }}
    }}
    joined[offset] = '\\0';
    return joined;
}}

static void molt_set_runtime_module_roots() {{
    char* roots = molt_join_runtime_module_roots();
    if (roots == NULL) {{
        fprintf(stderr, "molt: failed to allocate runtime module roots\\n");
        _Exit(125);
    }}
    const char* existing = getenv("MOLT_MODULE_ROOTS");
    if (existing == NULL || existing[0] == '\\0') {{
#ifdef _WIN32
        if (_putenv_s("MOLT_MODULE_ROOTS", roots) != 0) {{
#else
        if (setenv("MOLT_MODULE_ROOTS", roots, 1) != 0) {{
#endif
            free(roots);
            fprintf(stderr, "molt: failed to set runtime module roots\\n");
            _Exit(125);
        }}
        free(roots);
        return;
    }}
    size_t roots_len = strlen(roots);
    size_t existing_len = strlen(existing);
    char* merged = (char*)malloc(roots_len + 1 + existing_len + 1);
    if (merged == NULL) {{
        free(roots);
        fprintf(stderr, "molt: failed to allocate runtime module roots\\n");
        _Exit(125);
    }}
    memcpy(merged, roots, roots_len);
#ifdef _WIN32
    merged[roots_len] = ';';
#else
    merged[roots_len] = ':';
#endif
    memcpy(merged + roots_len + 1, existing, existing_len + 1);
#ifdef _WIN32
    if (_putenv_s("MOLT_MODULE_ROOTS", merged) != 0) {{
#else
    if (setenv("MOLT_MODULE_ROOTS", merged, 1) != 0) {{
#endif
        free(roots);
        free(merged);
        fprintf(stderr, "molt: failed to merge runtime module roots\\n");
        _Exit(125);
    }}
    free(roots);
    free(merged);
}}
"""
        module_roots_call = "    molt_set_runtime_module_roots();\n"
    main_c_content = """
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <wchar.h>
#endif
/* The launch ABI (runtime/native_launch_abi.toml). */
/* MOLT_NATIVE_LAUNCH_DECLARATIONS */
extern unsigned long long molt_runtime_shutdown();
#ifdef _WIN32
extern void molt_set_argv_utf16(int argc, const wchar_t** argv);
#endif
extern void molt_main();
extern void molt_dec_ref(unsigned long long bits);
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern unsigned long long molt_get_attr_generic(void* obj, const char* attr, unsigned long long len);

static int molt_env_enabled(const char* name) {
#ifdef _WIN32
    char* value = NULL;
    size_t value_len = 0;
    if (_dupenv_s(&value, &value_len, name) != 0 || value == NULL) {
        free(value);
        return 0;
    }
    int enabled = value[0] != '\\0' && strcmp(value, "0") != 0;
    free(value);
    return enabled;
#else
    const char* value = getenv(name);
    return value != NULL && value[0] != '\\0' && strcmp(value, "0") != 0;
#endif
}
/* Per-app callable resolver: the backend emits molt_app_resolve_callable into
 * the user object for the native app-callable manifest, and WASM emits the
 * analogous callable table resolver for intrinsics plus reachable builtin
 * callables. The runtime resolves name-based callable materialization through
 * this app-owned hook instead of monolithic generated resolvers, keeping unused
 * callables dead-strippable. This MUST be registered before molt_runtime_init()
 * so the resolver is in place before any lookup runs. */
extern unsigned long long molt_app_resolve_callable(const char* name, unsigned long long len);
/* Per-build module registry blob (import bedrock, design doc 69): the backend
 * emits the registry (module identity, MODULE_INIT_TABLE function pointers,
 * sorted name table) into the application object as one relocated data
 * symbol. It MUST be installed before molt_runtime_init() so module identity
 * resolution and molt_module_ensure are in place before any import runs. */
extern const unsigned char molt_module_registry_blob[];
/* MOLT_CAPABILITY_POLICY_SNIPPET */
/* MOLT_RUNTIME_MODULE_ROOTS_SNIPPET */
/* MOLT_COMPILED_SOURCES_SNIPPET */

static int molt_finish() {
    unsigned long long pending = molt_exception_pending();
    if (molt_env_enabled("MOLT_DEBUG_MAIN_EXCEPTION")) {
        fprintf(stderr, "molt main finish pending=%d\\n", pending != 0);
    }
    if (pending != 0) {
        unsigned long long exc = molt_exception_last_pending();
        unsigned long long exit_code = molt_exception_report_uncaught(exc);
        molt_dec_ref_obj(exc);
        molt_runtime_exit(exit_code);
        _Exit(1);
    }
    molt_runtime_exit(0);
    _Exit(0);
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    /* MOLT_CAPABILITY_POLICY_CALL */
    /* MOLT_RUNTIME_MODULE_ROOTS_CALL */
    /* MOLT_COMPILED_SOURCES_CALL */
    molt_set_app_callable_resolver((unsigned long long)(void*)molt_app_resolve_callable);
    molt_module_registry_install(molt_module_registry_blob);
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv_utf16(argc, (const wchar_t**)argv);
    molt_main();
    return molt_finish();
}
#else
int main(int argc, char** argv) {
    /* MOLT_CAPABILITY_POLICY_CALL */
    /* MOLT_RUNTIME_MODULE_ROOTS_CALL */
    /* MOLT_COMPILED_SOURCES_CALL */
    molt_set_app_callable_resolver((unsigned long long)(void*)molt_app_resolve_callable);
    molt_module_registry_install(molt_module_registry_blob);
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv(argc, (const char**)argv);
    molt_main();
    return molt_finish();
}
#endif
"""
    main_c_content = main_c_content.replace(
        "/* MOLT_NATIVE_LAUNCH_DECLARATIONS */", NATIVE_LAUNCH_C_DECLARATIONS
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITY_POLICY_SNIPPET */", capability_policy_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_RUNTIME_MODULE_ROOTS_SNIPPET */", module_roots_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITY_POLICY_CALL */", capability_policy_call
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_RUNTIME_MODULE_ROOTS_CALL */", module_roots_call
    )
    compiled_sources_snippet, compiled_sources_call = _compiled_sources_snippets(
        plan.compiled_source_paths
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_COMPILED_SOURCES_SNIPPET */", compiled_sources_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_COMPILED_SOURCES_CALL */", compiled_sources_call
    )
    return main_c_content
