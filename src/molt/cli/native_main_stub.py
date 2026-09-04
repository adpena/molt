from __future__ import annotations

import json
from pathlib import Path
from typing import Sequence

from molt.capability_manifest import ResolvedRuntimePolicy


def _native_main_stub_snippets(
    *,
    resolved_capability_policy: ResolvedRuntimePolicy,
) -> tuple[str, str, str, str]:
    runtime_environment = {
        **resolved_capability_policy.to_env_vars(),
        "MOLT_EXECUTION_TARGET": "native",
    }
    windows_updates = "\n".join(
        f"    if (_putenv_s({json.dumps(key)}, {json.dumps(value)}) != 0) "
        "molt_capability_policy_env_failure();"
        for key, value in sorted(runtime_environment.items())
    )
    posix_updates = "\n".join(
        f"    if (setenv({json.dumps(key)}, {json.dumps(value)}, 1) != 0) "
        "molt_capability_policy_env_failure();"
        for key, value in sorted(runtime_environment.items())
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


def _render_native_main_stub(
    *,
    resolved_capability_policy: ResolvedRuntimePolicy,
    runtime_module_roots: Sequence[Path] = (),
) -> str:
    runtime_module_roots_literals = tuple(
        json.dumps(str(path.resolve())) for path in dict.fromkeys(runtime_module_roots)
    )
    (
        capability_policy_snippet,
        capability_policy_call,
        module_roots_snippet,
        module_roots_call,
    ) = _native_main_stub_snippets(
        resolved_capability_policy=resolved_capability_policy,
    )
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
extern unsigned long long molt_runtime_init();
extern void molt_runtime_ensure_gil();
extern unsigned long long molt_runtime_shutdown();
extern unsigned long long molt_runtime_exit(unsigned long long code);
extern void molt_set_argv(int argc, const char** argv);
#ifdef _WIN32
extern void molt_set_argv_utf16(int argc, const wchar_t** argv);
#endif
extern void molt_main();
extern unsigned long long molt_frame_pop();
extern unsigned long long molt_exception_pending();
extern unsigned long long molt_exception_last();
extern unsigned long long molt_exception_report_uncaught(unsigned long long exc_bits);
extern void molt_dec_ref(unsigned long long bits);
extern void molt_dec_ref_obj(unsigned long long bits);
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);

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
extern unsigned long long molt_alloc(long size);
extern long molt_block_on(void* task);
extern void molt_spawn(void* task);
extern void* molt_chan_new(unsigned long long capacity);
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern long molt_chan_try_send(void* chan, long val);
extern long molt_chan_try_recv(void* chan);
extern long molt_chan_send_blocking(void* chan, long val);
extern long molt_chan_recv_blocking(void* chan);
extern void molt_print_obj(unsigned long long val);
/* Per-app callable resolver: the backend emits molt_app_resolve_callable into
 * the user object for the native app-callable manifest, and WASM emits the
 * analogous callable table resolver for intrinsics plus reachable builtin
 * callables. The runtime resolves name-based callable materialization through
 * this app-owned hook instead of monolithic generated resolvers, keeping unused
 * callables dead-strippable. This MUST be registered before molt_runtime_init()
 * so the resolver is in place before any lookup runs. */
extern unsigned long long molt_app_resolve_callable(const char* name, unsigned long long len);
extern unsigned long long molt_set_app_callable_resolver(unsigned long long fn_ptr);
/* Per-build module registry blob (import bedrock, design doc 69): the backend
 * emits the registry (module identity, MODULE_INIT_TABLE function pointers,
 * sorted name table) into the application object as one relocated data
 * symbol. It MUST be installed before molt_runtime_init() so module identity
 * resolution and molt_module_ensure are in place before any import runs. */
extern const unsigned char molt_module_registry_blob[];
extern unsigned long long molt_module_registry_install(const unsigned char* blob);
/* MOLT_CAPABILITY_POLICY_SNIPPET */
/* MOLT_RUNTIME_MODULE_ROOTS_SNIPPET */

static int molt_finish() {
    unsigned long long pending = molt_exception_pending();
    if (molt_env_enabled("MOLT_DEBUG_MAIN_EXCEPTION")) {
        fprintf(stderr, "molt main finish pending=%d\\n", pending != 0);
    }
    if (pending != 0) {
        unsigned long long exc = molt_exception_last();
        unsigned long long exit_code = molt_exception_report_uncaught(exc);
        molt_frame_pop();  /* pop frame after traceback formatting */
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
    return main_c_content
