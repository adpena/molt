from __future__ import annotations

import json
from pathlib import Path
from typing import Sequence


def _native_main_stub_snippets(
    *,
    trusted: bool,
    capabilities_list: Sequence[str] | None,
) -> tuple[str, str, str, str, str, str]:
    trusted_snippet = ""
    trusted_call = ""
    if trusted:
        trusted_snippet = """
static void molt_set_trusted() {
#ifdef _WIN32
    _putenv_s("MOLT_TRUSTED", "1");
#else
    setenv("MOLT_TRUSTED", "1", 1);
#endif
}
"""
        trusted_call = "    molt_set_trusted();\n"
    capabilities_snippet = ""
    capabilities_call = ""
    if capabilities_list is not None:
        caps_literal = json.dumps(",".join(capabilities_list))
        capabilities_snippet = f"""
static void molt_set_capabilities() {{
#ifdef _WIN32
    _putenv_s("MOLT_CAPABILITIES", {caps_literal});
#else
    setenv("MOLT_CAPABILITIES", {caps_literal}, 1);
#endif
}}
"""
        capabilities_call = "    molt_set_capabilities();\n"
    module_roots_snippet = ""
    module_roots_call = ""
    return (
        trusted_snippet,
        trusted_call,
        capabilities_snippet,
        capabilities_call,
        module_roots_snippet,
        module_roots_call,
    )


def _render_native_main_stub(
    *,
    trusted: bool,
    capabilities_list: Sequence[str] | None,
    runtime_module_roots: Sequence[Path] = (),
) -> str:
    runtime_module_roots_literals = tuple(
        json.dumps(str(path.resolve())) for path in dict.fromkeys(runtime_module_roots)
    )
    (
        trusted_snippet,
        trusted_call,
        capabilities_snippet,
        capabilities_call,
        module_roots_snippet,
        module_roots_call,
    ) = _native_main_stub_snippets(
        trusted=trusted,
        capabilities_list=capabilities_list,
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
extern unsigned long long molt_raise(unsigned long long exc_bits);
extern void molt_dec_ref(unsigned long long bits);
extern void molt_dec_ref_obj(unsigned long long bits);
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
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
/* Per-app intrinsic resolver: the backend emits molt_app_resolve_intrinsic into
 * the user object covering only the intrinsics this app reaches by name. The
 * runtime resolves intrinsics through it instead of the staticlib's
 * resolve_symbol, which keeps resolve_symbol/resolve_core_symbol native-
 * unreachable so the linker dead-strips every unused intrinsic. This MUST be
 * registered before molt_runtime_init() so the resolver is in place before any
 * intrinsic lookup runs. */
extern unsigned long long molt_app_resolve_intrinsic(const char* name, unsigned long long len);
extern unsigned long long molt_set_app_intrinsic_resolver(unsigned long long fn_ptr);
/* MOLT_TRUSTED_SNIPPET */
/* MOLT_CAPABILITIES_SNIPPET */
/* MOLT_RUNTIME_MODULE_ROOTS_SNIPPET */

static int molt_finish() {
    unsigned long long pending = molt_exception_pending();
    const char* debug_exc = getenv("MOLT_DEBUG_MAIN_EXCEPTION");
    if (debug_exc != NULL && debug_exc[0] != '\\0' && strcmp(debug_exc, "0") != 0) {
        fprintf(stderr, "molt main finish pending=%d\\n", pending != 0);
    }
    if (pending != 0) {
        unsigned long long exc = molt_exception_last();
        molt_raise(exc);
        molt_frame_pop();  /* pop frame after traceback formatting */
        molt_dec_ref_obj(exc);
        molt_runtime_exit(1);
        _Exit(1);
    }
    molt_runtime_exit(0);
    _Exit(0);
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    /* MOLT_TRUSTED_CALL */
    /* MOLT_CAPABILITIES_CALL */
    /* MOLT_RUNTIME_MODULE_ROOTS_CALL */
    molt_set_app_intrinsic_resolver((unsigned long long)(void*)molt_app_resolve_intrinsic);
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv_utf16(argc, (const wchar_t**)argv);
    molt_main();
    return molt_finish();
}
#else
int main(int argc, char** argv) {
    /* MOLT_TRUSTED_CALL */
    /* MOLT_CAPABILITIES_CALL */
    /* MOLT_RUNTIME_MODULE_ROOTS_CALL */
    molt_set_app_intrinsic_resolver((unsigned long long)(void*)molt_app_resolve_intrinsic);
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv(argc, (const char**)argv);
    molt_main();
    return molt_finish();
}
#endif
"""
    main_c_content = main_c_content.replace(
        "/* MOLT_TRUSTED_SNIPPET */", trusted_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITIES_SNIPPET */", capabilities_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_RUNTIME_MODULE_ROOTS_SNIPPET */", module_roots_snippet
    )
    main_c_content = main_c_content.replace("/* MOLT_TRUSTED_CALL */", trusted_call)
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITIES_CALL */", capabilities_call
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_RUNTIME_MODULE_ROOTS_CALL */", module_roots_call
    )
    return main_c_content
