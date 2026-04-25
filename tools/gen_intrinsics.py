#!/usr/bin/env python3
"""Generate intrinsics registry artifacts from the canonical manifest."""

from __future__ import annotations

from collections import OrderedDict
from pathlib import Path
import re
import sys

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
CATEGORIES_TOML = ROOT / "runtime/molt-runtime/src/intrinsics/categories.toml"
OUT_PYI = ROOT / "src/molt/_intrinsics.pyi"
OUT_RS = ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs"

SYMBOL_OVERRIDES = {
    "molt_async_sleep": "molt_async_sleep_new",
}

# Map symbol prefixes to Cargo feature flags. When a feature is disabled the
# resolve_symbol entry is excluded so the linker can drop the corresponding
# code.  Ordering matters: longest prefix wins.
_SYMBOL_FEATURE_GATES: list[tuple[str, str]] = [
    # crypto: hashlib, hmac, secrets, pbkdf2, scrypt, compare_digest
    ("molt_hash_", "stdlib_crypto"),
    ("molt_hmac_", "stdlib_crypto"),
    ("molt_compare_digest", "stdlib_crypto"),
    ("molt_pbkdf2_hmac", "stdlib_crypto"),
    ("molt_scrypt", "stdlib_crypto"),
    ("molt_secrets_", "stdlib_crypto"),
    # compression: bz2, lzma, deflate, inflate, gzip, tarfile, zlib
    ("molt_bz2_", "stdlib_compression"),
    ("molt_lzma_", "stdlib_compression"),
    ("molt_deflate_", "stdlib_compression"),
    ("molt_inflate_", "stdlib_compression"),
    ("molt_gzip_", "stdlib_compression"),
    ("molt_tarfile_", "stdlib_compression"),
    ("molt_zlib_", "stdlib_compression"),
    ("molt_compression_streams_", "stdlib_compression"),
    # serialization: cbor, msgpack
    ("molt_cbor_", "stdlib_serialization"),
    ("molt_msgpack_", "stdlib_serialization"),
    # ast
    ("molt_ast_", "stdlib_ast"),
    # fs_extra: glob, tempfile
    ("molt_glob_glob", "stdlib_fs_extra"),
    ("molt_glob_iglob", "stdlib_fs_extra"),
    ("molt_glob_pattern", "stdlib_fs_extra"),
    ("molt_tempfile_", "stdlib_fs_extra"),
    # archive: zipfile
    ("molt_zipfile_", "stdlib_archive"),
    # tk: tkinter GUI bindings
    ("molt_tk_", "stdlib_tk"),
    # networking: ssl, http, urllib
    ("molt_ssl_", "stdlib_net"),
    ("molt_http_", "stdlib_net"),
    ("molt_urllib_", "stdlib_net"),
    ("molt_ipaddress_", "stdlib_net"),
    # asyncio: event loop, futures, tasks, queues, streams, transports
    ("molt_asyncio_", "stdlib_asyncio"),
    ("molt_event_loop_", "stdlib_asyncio"),
    ("molt_pipe_transport_", "stdlib_asyncio"),
    # email
    ("molt_email_", "stdlib_email"),
    # decimal
    ("molt_decimal_", "stdlib_decimal"),
    # logging
    ("molt_logging_", "stdlib_logging"),
    # concurrent
    ("molt_concurrent_", "stdlib_concurrent"),
    # dbm
    ("molt_dbm_", "stdlib_dbm"),
    # importlib.resources and importlib.metadata
    ("molt_importlib_resources_", "stdlib_importlib_extra"),
    ("molt_importlib_metadata_", "stdlib_importlib_extra"),
    # csv
    ("molt_csv_", "stdlib_csv"),
    # signal
    ("molt_signal_", "stdlib_signal"),
    # select
    ("molt_select_", "stdlib_select"),
    # low-level tinygrad GPU primitive bridge
    ("molt_gpu_prim_", "molt_gpu_primitives"),
    # sqlite3 driver: pulls in rusqlite + bundled SQLite via molt-db.
    ("molt_sqlite3_", "sqlite"),
]


def _feature_gate_for_symbol(symbol: str) -> str | None:
    """Return the Cargo feature gate for *symbol*, or None if ungated."""
    for prefix, feature in _SYMBOL_FEATURE_GATES:
        if symbol.startswith(prefix):
            return feature
    return None


_DEF_RE = re.compile(
    r"^def\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\((?P<params>.*)\)\s*->\s*(?P<ret>[^:]+):\s*\.\.\.\s*$"
)


def _strip_manifest_header(text: str) -> str:
    lines = text.splitlines()
    idx = 0
    while idx < len(lines) and lines[idx].lstrip().startswith("#"):
        idx += 1
    if idx < len(lines) and lines[idx].strip() == "":
        idx += 1
    return "\n".join(lines[idx:]).rstrip() + "\n"


def _iter_defs(text: str) -> list[str]:
    lines = text.splitlines()
    defs: list[str] = []
    buf: list[str] = []
    in_def = False
    paren_depth = 0
    for raw in lines:
        line = raw.strip()
        if not in_def:
            if line.startswith("def "):
                in_def = True
                buf = [line]
                paren_depth = line.count("(") - line.count(")")
                if paren_depth <= 0 and line.endswith("..."):
                    defs.append(" ".join(buf))
                    in_def = False
            continue
        if not line:
            continue
        buf.append(line)
        paren_depth += line.count("(") - line.count(")")
        if paren_depth <= 0 and line.endswith("..."):
            defs.append(" ".join(buf))
            in_def = False
    if in_def:
        raise RuntimeError("unterminated def block in manifest")
    return defs


def _split_params(raw: str) -> list[str]:
    params = raw.strip()
    if not params:
        return []
    out: list[str] = []
    cur: list[str] = []
    depth = 0
    for ch in params:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            part = "".join(cur).strip()
            out.append(part)
            cur = []
            continue
        cur.append(ch)
    tail = "".join(cur).strip()
    if tail:
        out.append(tail)
    return [p for p in out if p and p != "*"]


def _load_manifest() -> tuple[str, list[tuple[str, str, int]]]:
    if not MANIFEST.exists():
        raise FileNotFoundError(f"manifest missing: {MANIFEST}")
    raw = MANIFEST.read_text()
    defs = _iter_defs(raw)
    entries: list[tuple[str, str, int]] = []
    seen: set[str] = set()
    for item in defs:
        m = _DEF_RE.match(item)
        if not m:
            raise RuntimeError(f"failed to parse def: {item}")
        name = m.group("name")
        if name in seen:
            raise RuntimeError(f"duplicate intrinsic name: {name}")
        seen.add(name)
        params = _split_params(m.group("params"))
        arity = len(params)
        symbol = SYMBOL_OVERRIDES.get(name, name)
        entries.append((name, symbol, arity))
    return raw, entries


def _validate_symbols(entries: list[tuple[str, str, int]]) -> None:
    src_root = ROOT / "runtime/molt-runtime/src"
    rs_files = list(src_root.rglob("*.rs"))
    corpus = "\n".join(path.read_text() for path in rs_files)
    # Single-pass: extract all function names defined in the corpus — O(n+m)
    # instead of O(n*m) regex searches per symbol
    defined_fns = set(re.findall(r"\bfn\s+(\w+)", corpus))
    missing = [sym for _name, sym, _arity in entries if sym not in defined_fns]
    if missing:
        missing_list = ", ".join(sorted(set(missing)))
        raise RuntimeError(f"missing intrinsic symbols in runtime: {missing_list}")


# ---------------------------------------------------------------------------
# Module categorization for per-module resolver splitting
# ---------------------------------------------------------------------------


def _load_categories() -> tuple[
    list[str],  # builtin_symbols (exact names)
    list[str],  # internal_prefixes
    OrderedDict[str, list[str]],  # module_name -> prefix list
]:
    """Load categories.toml and return classification rules.

    Returns (builtin_symbols, internal_prefixes, stdlib_modules) where
    *stdlib_modules* is an OrderedDict mapping module name to its list of
    symbol prefixes (with the ``molt_`` prefix already prepended).
    """
    raw = CATEGORIES_TOML.read_bytes()
    data = tomllib.loads(raw.decode())

    # Builtin: exact symbol names from all sub-keys
    builtin_symbols: list[str] = []
    for _key, val in data.get("builtin", {}).items():
        if isinstance(val, list):
            builtin_symbols.extend(val)

    # Internal: prefixes (need ``molt_`` prepended)
    internal_prefixes: list[str] = [
        f"molt_{p}" for p in data.get("internal", {}).get("prefixes", [])
    ]

    # Stdlib modules: each [stdlib.<mod>] has a ``prefixes`` list
    stdlib_modules: OrderedDict[str, list[str]] = OrderedDict()
    for mod_name, mod_data in data.get("stdlib", {}).items():
        prefixes = [f"molt_{p}" for p in mod_data.get("prefixes", [])]
        if prefixes:
            stdlib_modules[mod_name] = prefixes

    return builtin_symbols, internal_prefixes, stdlib_modules


# Additional prefix-to-module mapping for modules NOT yet in categories.toml.
# These are checked *after* categories.toml rules so the TOML file wins.
_EXTRA_PREFIX_MODULES: list[tuple[str, str]] = [
    ("molt_math_", "math"),
    ("molt_json_", "json"),
    ("molt_os_", "os"),
    ("molt_socket_", "socket"),
    ("molt_asyncio_", "asyncio"),
    ("molt_async_sleep", "asyncio"),
    ("molt_datetime_", "datetime"),
    ("molt_re_", "re"),
    ("molt_http_", "http"),
    ("molt_decimal_", "decimal"),
    ("molt_pathlib_", "pathlib"),
    ("molt_signal_", "signal"),
    ("molt_logging_", "logging"),
    ("molt_csv_", "csv"),
    ("molt_hashlib_", "hashlib"),
    ("molt_hash_", "crypto"),
    ("molt_hmac_", "crypto"),
    ("molt_compare_digest", "crypto"),
    ("molt_pbkdf2_", "crypto"),
    ("molt_scrypt", "crypto"),
    ("molt_secrets_", "crypto"),
    ("molt_zlib_", "compression"),
    ("molt_bz2_", "compression"),
    ("molt_lzma_", "compression"),
    ("molt_deflate_", "compression"),
    ("molt_inflate_", "compression"),
    ("molt_gzip_", "compression"),
    ("molt_tarfile_", "compression"),
    ("molt_compression_streams_", "compression"),
    ("molt_ssl_", "ssl"),
    ("molt_struct_", "struct"),
    ("molt_thread_", "threading"),
    ("molt_process_", "subprocess"),
    ("molt_stream_", "io"),
    ("molt_file_", "io"),
    ("molt_io_wait", "io"),
    ("molt_ws_", "websocket"),
    ("molt_importlib_", "importlib"),
    ("molt_bytes_", "bytes"),
    ("molt_bytearray_", "bytes"),
    ("molt_string_", "string"),
    ("molt_buffer2d_", "buffer"),
    ("molt_weakref_", "weakref"),
    ("molt_weakkeydict_", "weakref"),
    ("molt_weakvaluedict_", "weakref"),
    ("molt_weakset_", "weakref"),
    ("molt_contextlib_", "contextlib"),
    ("molt_cancel_token_", "cancel"),
    ("molt_cancel_current", "cancel"),
    ("molt_cancelled", "cancel"),
    ("molt_lock_", "lock"),
    ("molt_sqlite_", "sqlite"),
    ("molt_sqlite3_", "sqlite"),
    ("molt_db_", "sqlite"),
    ("molt_tk_", "tk"),
    ("molt_atexit_", "atexit"),
    ("molt_time_", "time"),
    ("molt_random_", "random"),
    ("molt_itertools_", "itertools"),
    ("molt_functools_", "functools"),
    ("molt_enum_", "enum"),
    ("molt_dataclasses_", "dataclasses"),
    ("molt_collections_abc_runtime", "collections"),
    ("molt_namedtuple_", "collections"),
    ("molt_ordereddict_", "collections"),
    ("molt_defaultdict_", "collections"),
    ("molt_deque_", "collections"),
    ("molt_counter_", "collections"),
    ("molt_chainmap_", "collections"),
    ("molt_heapq_", "heapq"),
    ("molt_bisect_", "bisect"),
    ("molt_insort_", "bisect"),
    ("molt_copy_", "copy"),
    ("molt_copyreg_", "copyreg"),
    ("molt_typing_", "typing"),
    ("molt_inspect_", "inspect"),
    ("molt_warnings_", "warnings"),
    ("molt_traceback_", "traceback"),
    ("molt_linecache_", "linecache"),
    ("molt_tokenize_", "tokenize"),
    ("molt_ast_", "ast"),
    ("molt_sys_", "sys"),
    ("molt_platform_", "platform"),
    ("molt_locale_", "locale"),
    ("molt_codecs_", "codecs"),
    ("molt_encodings_", "codecs"),
    ("molt_unicodedata_", "unicodedata"),
    ("molt_email_", "email"),
    ("molt_urllib_", "urllib"),
    ("molt_html_", "html"),
    ("molt_xml", "xml"),
    ("molt_pprint_", "pprint"),
    ("molt_textwrap_", "textwrap"),
    ("molt_difflib_", "difflib"),
    ("molt_shutil_", "shutil"),
    ("molt_shlex_", "shlex"),
    ("molt_fnmatch", "fnmatch"),
    ("molt_glob_", "glob"),
    ("molt_tempfile_", "tempfile"),
    ("molt_zipfile_", "archive"),
    ("molt_zipapp_", "archive"),
    ("molt_cbor_", "serialization"),
    ("molt_msgpack_", "serialization"),
    ("molt_pickle_", "serialization"),
    ("molt_uuid_", "uuid"),
    ("molt_binascii_", "binascii"),
    ("molt_base64_", "base64"),
    ("molt_quopri_", "quopri"),
    ("molt_uu_codec", "uu"),
    ("molt_ipaddress_", "ipaddress"),
    ("molt_select_", "select"),
    ("molt_socketserver_", "socketserver"),
    ("molt_socketpair", "socket"),
    ("molt_concurrent_", "concurrent"),
    ("molt_multiprocessing_", "multiprocessing"),
    ("molt_subprocess_", "subprocess"),
    ("molt_queue_", "queue"),
    ("molt_gc_", "gc"),
    ("molt_ctypes_", "ctypes"),
    ("molt_cmath_", "cmath"),
    ("molt_statistics_", "statistics"),
    ("molt_fraction_", "fractions"),
    ("molt_array_", "array"),
    ("molt_memoryview_", "memoryview"),
    ("molt_operator_", "operator"),
    ("molt_keyword_", "keyword"),
    ("molt_opcode_", "opcode"),
    ("molt_site_", "site"),
    ("molt_configparser_", "configparser"),
    ("molt_gettext_", "gettext"),
    ("molt_argparse_", "argparse"),
    ("molt_codeop_", "codeop"),
    ("molt_compileall_", "compileall"),
    ("molt_py_compile_", "py_compile"),
    ("molt_runpy_", "runpy"),
    ("molt_pkgutil_", "pkgutil"),
    ("molt_imghdr_", "imghdr"),
    ("molt_stat_", "stat"),
    ("molt_fcntl_", "fcntl"),
    ("molt_zoneinfo_", "zoneinfo"),
    ("molt_graphlib_", "graphlib"),
    ("molt_stringprep_", "stringprep"),
    ("molt_punycode_", "punycode"),
    ("molt_this_", "this"),
    ("molt_wsgiref_", "wsgiref"),
    ("molt_xmlrpc_", "xmlrpc"),
    ("molt_tomllib_", "tomllib"),
    ("molt_symtable_", "symtable"),
    ("molt_colorsys_", "colorsys"),
    ("molt_dbm_", "dbm"),
    ("molt_pipe_transport_", "asyncio"),
    ("molt_protocol_", "asyncio"),
    ("molt_event_loop_", "asyncio"),
    ("molt_event_", "asyncio"),
    ("molt_future_", "asyncio"),
    ("molt_asyncgen_", "asyncio"),
    ("molt_promise_", "asyncio"),
    ("molt_compile_", "compile"),
    ("molt_errno_", "errno"),
    ("molt_condition_", "threading"),
    ("molt_semaphore_", "threading"),
    ("molt_barrier_", "threading"),
    ("molt_rlock_", "threading"),
    ("molt_local_", "threading"),
    ("molt_spawn_", "subprocess"),
    ("molt_chan_", "chan"),
    ("molt_generic_alias_", "typing"),
    ("molt_text_io_wrapper_", "io"),
    ("molt_stringio_", "io"),
    ("molt_bytesio_", "io"),
    ("molt_buffered_", "io"),
    ("molt_http_client", "http"),
    ("molt_http_server", "http"),
    ("molt_http_cookiejar", "http"),
    ("molt_http_cookies", "http"),
    ("molt_http_message", "http"),
    ("molt_http_parse", "http"),
    ("molt_http_status", "http"),
    ("molt_logging_config", "logging"),
    ("molt_path_", "pathlib"),
    ("molt_timedelta_", "datetime"),
    ("molt_timezone_", "datetime"),
    ("molt_date_", "datetime"),
    ("molt_token_payload", "tokenize"),
    ("molt_repr_from", "reprlib"),
]


def _classify_symbol(
    symbol: str,
    builtin_symbols: list[str],
    internal_prefixes: list[str],
    stdlib_modules: OrderedDict[str, list[str]],
) -> str:
    """Return the resolver module name for *symbol*.

    Returns ``"core"`` for builtins/internal, or a module name string.
    """
    # Check builtin exact matches
    if symbol in builtin_symbols:
        return "core"

    # Check internal prefixes
    for pfx in internal_prefixes:
        if symbol.startswith(pfx):
            return "core"

    # Check categories.toml stdlib modules (longest prefix wins)
    best_mod: str | None = None
    best_len = 0
    for mod_name, prefixes in stdlib_modules.items():
        for pfx in prefixes:
            if symbol.startswith(pfx) and len(pfx) > best_len:
                best_mod = mod_name
                best_len = len(pfx)

    # Check extra prefix table (longest prefix wins)
    for pfx, mod_name in _EXTRA_PREFIX_MODULES:
        if symbol.startswith(pfx) and len(pfx) > best_len:
            best_mod = mod_name
            best_len = len(pfx)

    if best_mod is not None:
        return best_mod

    # Fallback: core
    return "core"


def _write_generated_rs(entries: list[tuple[str, str, int]]) -> None:
    builtin_symbols, internal_prefixes, stdlib_modules = _load_categories()

    # Classify every unique symbol into a module bucket
    module_symbols: OrderedDict[str, list[str]] = OrderedDict()
    seen_symbols: set[str] = set()
    for _name, symbol, _arity in entries:
        if symbol in seen_symbols:
            continue
        seen_symbols.add(symbol)
        mod = _classify_symbol(
            symbol, builtin_symbols, internal_prefixes, stdlib_modules
        )
        module_symbols.setdefault(mod, []).append(symbol)

    # Ensure "core" comes first
    if "core" in module_symbols:
        ordered: OrderedDict[str, list[str]] = OrderedDict()
        ordered["core"] = module_symbols.pop("core")
        ordered.update(sorted(module_symbols.items()))
        module_symbols = ordered
    else:
        module_symbols = OrderedDict(sorted(module_symbols.items()))

    lines: list[str] = []
    lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    lines.append("#[derive(Clone, Copy)]\n")
    lines.append("pub(crate) struct IntrinsicSpec {\n")
    lines.append("    pub name: &'static str,\n")
    lines.append("    pub symbol: &'static str,\n")
    lines.append("    pub arity: u8,\n")
    lines.append("}\n\n")
    lines.append("pub(crate) const INTRINSICS: &[IntrinsicSpec] = &[\n")
    for name, symbol, arity in entries:
        lines.append(
            f'    IntrinsicSpec {{ name: "{name}", symbol: "{symbol}", arity: {arity} }},\n'
        )
    lines.append("];\n\n")

    # -- Dispatcher --------------------------------------------------------
    lines.append("pub(crate) fn resolve_symbol(symbol: &str) -> Option<u64> {\n")
    lines.append(
        "    // Try per-module resolvers. Each is #[inline(never)] + #[cold]\n"
    )
    lines.append("    // so --gc-sections can strip unreferenced module resolvers.\n")
    for mod_name in module_symbols:
        fn_name = f"resolve_{mod_name}_symbol"
        lines.append(f"    if let Some(v) = {fn_name}(symbol) {{ return Some(v); }}\n")
    lines.append("    None\n")
    lines.append("}\n\n")

    # -- Per-module resolvers ----------------------------------------------
    for mod_name, symbols in module_symbols.items():
        fn_name = f"resolve_{mod_name}_symbol"
        lines.append("#[inline(never)]\n")
        lines.append("#[cold]\n")

        # Collect feature gates, preserving symbol order
        gated: dict[str | None, list[str]] = {}
        for sym in symbols:
            gate = _feature_gate_for_symbol(sym)
            gated.setdefault(gate, []).append(sym)

        lines.append(f"fn {fn_name}(symbol: &str) -> Option<u64> {{\n")
        lines.append("    match symbol {\n")
        for gate, syms in gated.items():
            for sym in syms:
                if gate:
                    lines.append(f'        #[cfg(feature = "{gate}")]\n')
                lines.append(
                    f'        "{sym}" => Some(crate::builtins::functions::runtime_fn_addr("crate::{sym}", crate::{sym} as *const ())),\n'
                )
        lines.append("        _ => None,\n")
        lines.append("    }\n")
        lines.append("}\n\n")

    OUT_RS.write_text("".join(lines))


def _write_pyi(raw_manifest: str) -> None:
    body = _strip_manifest_header(raw_manifest)
    header = (
        "# @generated by tools/gen_intrinsics.py from "
        "runtime/molt-runtime/src/intrinsics/manifest.pyi\n"
    )
    OUT_PYI.write_text(header + body)


def main() -> int:
    raw, entries = _load_manifest()
    _validate_symbols(entries)
    _write_generated_rs(entries)
    _write_pyi(raw)
    return 0


if __name__ == "__main__":
    sys.exit(main())
