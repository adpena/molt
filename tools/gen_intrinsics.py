#!/usr/bin/env python3
"""Generate intrinsics registry artifacts from the canonical manifest."""

from __future__ import annotations

import argparse
from collections import OrderedDict
import difflib
import importlib.util
from pathlib import Path
import re
import sys
import tempfile

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

MANIFEST = ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
CATEGORIES_TOML = ROOT / "runtime/molt-runtime/src/intrinsics/categories.toml"
OUT_PYI = ROOT / "src/molt/_intrinsics.pyi"
OUT_RS = ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs"
OUT_RS_RESOLVERS_DIR = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
MATH_LEAF_RESOLVERS_DIR = ROOT / "runtime/molt-runtime-math/src/intrinsics_generated"
MATH_LEAF_RESOLVER_INDEX = MATH_LEAF_RESOLVERS_DIR / "mod.rs"
LEAF_RESOLVER_REGISTRIES = {
    "stringprep": {
        "output": ROOT / "runtime/molt-runtime-stringprep/src/intrinsics_generated.rs",
        "crate_path": "molt_runtime_stringprep",
        "symbol_path_prefix": "molt_runtime_stringprep::stringprep",
        "function_path_prefix": "crate::stringprep",
    },
    "cmath": {
        "output": MATH_LEAF_RESOLVERS_DIR / "cmath_resolver.rs",
        "module_index": MATH_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_math",
        "crate_resolver_path": "molt_runtime_math::intrinsics_generated::cmath_resolver",
        "symbol_path_prefix": "molt_runtime_math::cmath_mod",
        "function_path_prefix": "crate::cmath_mod",
    },
    "colorsys": {
        "output": MATH_LEAF_RESOLVERS_DIR / "colorsys_resolver.rs",
        "module_index": MATH_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_math",
        "crate_resolver_path": "molt_runtime_math::intrinsics_generated::colorsys_resolver",
        "symbol_path_prefix": "molt_runtime_math::colorsys",
        "function_path_prefix": "crate::colorsys",
    },
    "fractions": {
        "output": MATH_LEAF_RESOLVERS_DIR / "fractions_resolver.rs",
        "module_index": MATH_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_math",
        "crate_resolver_path": "molt_runtime_math::intrinsics_generated::fractions_resolver",
        "symbol_path_prefix": "molt_runtime_math::fractions",
        "function_path_prefix": "crate::fractions",
    },
    "math": {
        "output": MATH_LEAF_RESOLVERS_DIR / "math_resolver.rs",
        "module_index": MATH_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_math",
        "crate_resolver_path": "molt_runtime_math::intrinsics_generated::math_resolver",
        "symbol_path_prefix": "molt_runtime_math::math",
        "function_path_prefix": "crate::math",
    },
    "random": {
        "output": MATH_LEAF_RESOLVERS_DIR / "random_resolver.rs",
        "module_index": MATH_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_math",
        "crate_resolver_path": "molt_runtime_math::intrinsics_generated::random_resolver",
        "symbol_path_prefix": "molt_runtime_math::random_mod",
        "function_path_prefix": "crate::random_mod",
    },
    "statistics": {
        "output": MATH_LEAF_RESOLVERS_DIR / "statistics_resolver.rs",
        "module_index": MATH_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_math",
        "crate_resolver_path": "molt_runtime_math::intrinsics_generated::statistics_resolver",
        "symbol_path_prefix": "molt_runtime_math::math",
        "function_path_prefix": "crate::math",
    },
}
OUT_BACKEND_OVERRIDES_RS = (
    ROOT / "runtime/molt-backend/src/intrinsic_symbol_overrides.rs"
)
_HARNESS_MEMORY_GUARD = None
_CHECK_MODE = False
_CHECK_DIFFS: list[str] = []


def _load_harness_memory_guard():
    global _HARNESS_MEMORY_GUARD
    if _HARNESS_MEMORY_GUARD is None:
        try:
            from tools import harness_memory_guard
        except (
            ModuleNotFoundError
        ):  # pragma: no cover - direct script import from tools/
            import harness_memory_guard  # type: ignore
        _HARNESS_MEMORY_GUARD = harness_memory_guard
    return _HARNESS_MEMORY_GUARD


# The symbol-prefix -> Cargo-feature gate mapping is the single source of truth
# shared with the frontend's compile-time profile-availability refusal. It lives
# in the shipped `molt` package (`src/molt/_runtime_feature_gates.py`) so the
# build-time gate generation here depends on the shipped data — not the reverse.
# Load it by file path to avoid importing the `molt` package (and its
# `__init__` side effects) from this standalone dev tool.
def _load_runtime_feature_gates():
    gates_path = ROOT / "src/molt/_runtime_feature_gates.py"
    spec = importlib.util.spec_from_file_location(
        "molt_gen_intrinsics_feature_gates", gates_path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load feature-gate table: {gates_path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_FEATURE_GATES_MODULE = _load_runtime_feature_gates()

# Re-exported under the historical names so callers/tests in this module keep
# working. Ordering matters: longest prefix wins (resolved by
# `feature_gate_for_symbol`).
_SYMBOL_FEATURE_GATES: list[tuple[str, str]] = list(
    _FEATURE_GATES_MODULE.RUNTIME_FEATURE_GATES
)
_feature_gate_for_symbol = _FEATURE_GATES_MODULE.feature_gate_for_symbol


_DEF_RE = re.compile(
    r"^def\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\((?P<params>.*)\)\s*->\s*(?P<ret>[^:]+):\s*\.\.\.\s*$"
)


class IntrinsicEntry:
    __slots__ = ("name", "symbol", "arity", "defaults")

    def __init__(
        self,
        name: str,
        symbol: str,
        arity: int,
        defaults: tuple[str, ...] = (),
    ) -> None:
        self.name = name
        self.symbol = symbol
        self.arity = arity
        self.defaults = defaults

    def __iter__(self):
        # Keep existing tuple-style tests and helper comprehensions working
        # while generated metadata gains Python-call default information.
        yield self.name
        yield self.symbol
        yield self.arity


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


def _param_default_expr(param: str) -> str | None:
    depth = 0
    for i, ch in enumerate(param):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == "=" and depth == 0:
            return param[i + 1 :].strip()
    return None


def _rust_intrinsic_default(default_expr: str, intrinsic_name: str) -> str | None:
    if default_expr == "...":
        return None
    if default_expr == "None":
        return "IntrinsicDefaultValue::None"
    if default_expr == "True":
        return "IntrinsicDefaultValue::Bool(true)"
    if default_expr == "False":
        return "IntrinsicDefaultValue::Bool(false)"
    if re.fullmatch(r"-?\d+", default_expr):
        return f"IntrinsicDefaultValue::Int({int(default_expr)})"
    raise RuntimeError(
        f"unsupported concrete default for {intrinsic_name}: {default_expr!r}"
    )


def _parse_intrinsic_defaults(name: str, params: list[str]) -> tuple[str, ...]:
    parsed: list[str | None] = []
    for param in params:
        default_expr = _param_default_expr(param)
        parsed.append(
            None
            if default_expr is None
            else _rust_intrinsic_default(default_expr, name)
        )
    concrete_positions = [idx for idx, value in enumerate(parsed) if value is not None]
    if not concrete_positions:
        return ()
    first = concrete_positions[0]
    expected = list(range(first, len(parsed)))
    if concrete_positions != expected:
        raise RuntimeError(
            f"concrete defaults for {name} must form a trailing positional suffix"
        )
    return tuple(value for value in parsed[first:] if value is not None)


def _load_manifest() -> tuple[str, list[IntrinsicEntry]]:
    if not MANIFEST.exists():
        raise FileNotFoundError(f"manifest missing: {MANIFEST}")
    raw = MANIFEST.read_text(encoding="utf-8")
    defs = _iter_defs(raw)
    entries: list[IntrinsicEntry] = []
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
        entries.append(
            IntrinsicEntry(
                name=name,
                symbol=name,
                arity=arity,
                defaults=_parse_intrinsic_defaults(name, params),
            )
        )
    return raw, entries


def _validate_symbols(entries: list[IntrinsicEntry]) -> None:
    runtime_root = ROOT / "runtime"
    src_roots = sorted(path for path in runtime_root.glob("*/src") if path.is_dir())
    rs_files = [rs for src_root in src_roots for rs in src_root.rglob("*.rs")]
    corpus = "\n".join(path.read_text(encoding="utf-8") for path in rs_files)
    # Single-pass: extract all function names defined in the corpus — O(n+m)
    # instead of O(n*m) regex searches per symbol
    defined_fns = set(re.findall(r"\bfn\s+(\w+)", corpus))
    missing = [entry.symbol for entry in entries if entry.symbol not in defined_fns]
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


def _rustfmt(path: Path) -> None:
    result = _load_harness_memory_guard().guarded_completed_process(
        ["rustfmt", str(path)],
        prefix="MOLT_GENERATOR",
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=60.0,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "rustfmt failed for "
            f"{path}:\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )


def _write_text_if_changed(path: Path, text: str) -> bool:
    if path.exists() and path.read_text(encoding="utf-8") == text:
        return False
    if _CHECK_MODE:
        _record_check_diff(path, path.read_text(encoding="utf-8") if path.exists() else "", text)
        return True
    path.write_text(text, encoding="utf-8")
    return True


def _write_rust_if_changed(path: Path, text: str) -> bool:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and path.read_text(encoding="utf-8") == text:
        return False
    tmp: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            newline="\n",
            suffix=".rs",
            prefix=f"{path.stem}_",
            dir=path.parent,
            delete=False,
        ) as tmp_file:
            tmp = Path(tmp_file.name)
            tmp_file.write(text)
        _rustfmt(tmp)
        formatted = tmp.read_text(encoding="utf-8")
        if path.exists() and path.read_text(encoding="utf-8") == formatted:
            return False
        if _CHECK_MODE:
            _record_check_diff(
                path, path.read_text(encoding="utf-8") if path.exists() else "", formatted
            )
            return True
        tmp.replace(path)
        return True
    finally:
        if tmp is not None:
            tmp.unlink(missing_ok=True)


def _record_check_diff(path: Path, current: str, expected: str) -> None:
    try:
        label = str(path.relative_to(ROOT))
    except ValueError:
        label = str(path)
    _CHECK_DIFFS.extend(
        difflib.unified_diff(
            current.splitlines(keepends=True),
            expected.splitlines(keepends=True),
            fromfile=label,
            tofile=f"{label} (generated)",
        )
    )


def _resolver_module_name(module_name: str) -> str:
    ident = re.sub(r"[^A-Za-z0-9_]", "_", module_name)
    if not ident or ident[0].isdigit():
        ident = f"module_{ident}"
    return f"{ident}_resolver"


def _resolver_file_name(module_name: str) -> str:
    return f"{_resolver_module_name(module_name)}.rs"


def _append_resolver_arm(lines: list[str], symbol: str) -> None:
    first_line = (
        f'        "{symbol}" => Some(crate::builtins::functions::runtime_fn_addr(\n'
    )
    if len(first_line.rstrip("\n")) <= 98:
        lines.append(first_line)
        lines.append(f'            "crate::{symbol}",\n')
        lines.append(f"            crate::{symbol} as *const (),\n")
        lines.append("        )),\n")
        return

    lines.append(f'        "{symbol}" => {{\n')
    lines.append("            Some(crate::builtins::functions::runtime_fn_addr(\n")
    lines.append(f'                "crate::{symbol}",\n')
    lines.append(f"                crate::{symbol} as *const (),\n")
    lines.append("            ))\n")
    lines.append("        }\n")


def _append_leaf_resolver_arm(
    lines: list[str],
    symbol: str,
    *,
    symbol_path_prefix: str,
    function_path_prefix: str,
) -> None:
    lines.append(f'        "{symbol}" => Some(runtime_fn_addr(\n')
    lines.append(f'            "{symbol_path_prefix}::{symbol}",\n')
    lines.append(f"            {function_path_prefix}::{symbol} as *const (),\n")
    lines.append("        )),\n")


def _write_leaf_resolver_module(
    mod_name: str,
    symbols: list[str],
    leaf: dict[str, object],
) -> None:
    output = leaf["output"]
    if not isinstance(output, Path):
        raise TypeError(f"leaf resolver output for {mod_name} must be a Path")
    symbol_path_prefix = str(leaf["symbol_path_prefix"])
    function_path_prefix = str(leaf["function_path_prefix"])
    lines: list[str] = []
    lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    lines.append("#[inline(never)]\n")
    lines.append("#[cold]\n")
    lines.append("pub fn resolve_symbol_with(\n")
    lines.append("    symbol: &str,\n")
    lines.append("    runtime_fn_addr: fn(&str, *const ()) -> u64,\n")
    lines.append(") -> Option<u64> {\n")
    lines.append("    match symbol {\n")
    for sym in symbols:
        _append_leaf_resolver_arm(
            lines,
            sym,
            symbol_path_prefix=symbol_path_prefix,
            function_path_prefix=function_path_prefix,
        )
    lines.append("        _ => None,\n")
    lines.append("    }\n")
    lines.append("}\n")
    _write_rust_if_changed(output, "".join(lines))


def _write_leaf_facade_resolver_module(
    mod_name: str,
    leaf: dict[str, object],
    feature: str,
) -> None:
    crate_path = str(leaf["crate_path"])
    resolver_path = str(
        leaf.get("crate_resolver_path", f"{crate_path}::intrinsics_generated")
    )
    lines: list[str] = []
    lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    lines.append("#[inline(never)]\n")
    lines.append("#[cold]\n")
    lines.append("pub(super) fn resolve_symbol(symbol: &str) -> Option<u64> {\n")
    lines.append(f'    #[cfg(feature = "{feature}")]\n')
    lines.append("    {\n")
    lines.append(f"        {resolver_path}::resolve_symbol_with(\n")
    lines.append("            symbol,\n")
    lines.append("            crate::builtins::functions::runtime_fn_addr,\n")
    lines.append("        )\n")
    lines.append("    }\n")
    lines.append(f'    #[cfg(not(feature = "{feature}"))]\n')
    lines.append("    {\n")
    lines.append("        let _ = symbol;\n")
    lines.append("        None\n")
    lines.append("    }\n")
    lines.append("}\n")
    _write_rust_if_changed(
        OUT_RS_RESOLVERS_DIR / _resolver_file_name(mod_name),
        "".join(lines),
    )


def _write_leaf_resolver_indexes(indexes: dict[Path, set[str]]) -> None:
    for index_path, modules in indexes.items():
        module_dir = (
            index_path.parent if index_path.name == "mod.rs" else index_path.parent / index_path.stem
        )
        module_dir.mkdir(parents=True, exist_ok=True)
        expected_files = {f"{module}.rs" for module in modules}
        for stale in module_dir.glob("*_resolver.rs"):
            if stale.name not in expected_files:
                stale.unlink()
        lines: list[str] = []
        lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
        for module in sorted(modules):
            lines.append(f"pub mod {module};\n")
        _write_rust_if_changed(index_path, "".join(lines))


def _leaf_resolver_feature_gate(mod_name: str, symbols: list[str]) -> str:
    gates = {_feature_gate_for_symbol(sym) for sym in symbols}
    if len(gates) == 1 and None not in gates:
        gate = next(iter(gates))
        assert gate is not None
        return gate
    rendered = ", ".join(
        "<none>" if gate is None else gate
        for gate in sorted(gates, key=lambda value: "" if value is None else value)
    )
    raise RuntimeError(
        f"leaf resolver {mod_name!r} must map to one runtime feature gate; "
        f"found: {rendered or '<empty>'}"
    )


def _write_resolver_modules(
    module_symbols: OrderedDict[str, list[str]],
) -> None:
    OUT_RS_RESOLVERS_DIR.mkdir(parents=True, exist_ok=True)
    leaf_indexes: dict[Path, set[str]] = {}

    module_file_names = {_resolver_file_name(mod_name) for mod_name in module_symbols}
    for stale in OUT_RS_RESOLVERS_DIR.glob("*_resolver.rs"):
        if stale.name != "mod.rs" and stale.name not in module_file_names:
            stale.unlink()

    mod_lines: list[str] = []
    mod_lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    for mod_name in module_symbols:
        mod_lines.append(f"mod {_resolver_module_name(mod_name)};\n")
    mod_lines.append("\n")
    mod_lines.append("pub(crate) fn resolve_symbol(symbol: &str) -> Option<u64> {\n")
    for mod_name in module_symbols:
        resolver_mod = _resolver_module_name(mod_name)
        mod_lines.append(
            f"    if let Some(v) = {resolver_mod}::resolve_symbol(symbol) {{\n"
        )
        mod_lines.append("        return Some(v);\n")
        mod_lines.append("    }\n")
    mod_lines.append("    None\n")
    mod_lines.append("}\n")
    _write_rust_if_changed(OUT_RS_RESOLVERS_DIR / "mod.rs", "".join(mod_lines))

    for mod_name, symbols in module_symbols.items():
        leaf = LEAF_RESOLVER_REGISTRIES.get(mod_name)
        if leaf is not None:
            feature = _leaf_resolver_feature_gate(mod_name, symbols)
            _write_leaf_resolver_module(mod_name, symbols, leaf)
            _write_leaf_facade_resolver_module(mod_name, leaf, feature)
            module_index = leaf.get("module_index")
            if isinstance(module_index, Path):
                output = leaf["output"]
                if not isinstance(output, Path):
                    raise TypeError(
                        f"leaf resolver output for {mod_name!r} must be a Path"
                    )
                leaf_indexes.setdefault(module_index, set()).add(output.stem)
            continue

        lines: list[str] = []
        lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
        lines.append("#[inline(never)]\n")
        lines.append("#[cold]\n")

        # Collect feature gates, preserving symbol order.
        gated: dict[str | None, list[str]] = {}
        for sym in symbols:
            gate = _feature_gate_for_symbol(sym)
            gated.setdefault(gate, []).append(sym)

        lines.append("pub(super) fn resolve_symbol(symbol: &str) -> Option<u64> {\n")
        lines.append("    match symbol {\n")
        for gate, syms in gated.items():
            for sym in syms:
                if gate:
                    lines.append(f'        #[cfg(feature = "{gate}")]\n')
                _append_resolver_arm(lines, sym)
        lines.append("        _ => None,\n")
        lines.append("    }\n")
        lines.append("}\n")
        _write_rust_if_changed(
            OUT_RS_RESOLVERS_DIR / _resolver_file_name(mod_name), "".join(lines)
        )
    _write_leaf_resolver_indexes(leaf_indexes)


def _write_generated_rs(entries: list[IntrinsicEntry]) -> None:
    builtin_symbols, internal_prefixes, stdlib_modules = _load_categories()

    # Classify every unique symbol into a module bucket
    module_symbols: OrderedDict[str, list[str]] = OrderedDict()
    seen_symbols: set[str] = set()
    for entry in entries:
        symbol = entry.symbol
        if symbol in seen_symbols:
            continue
        seen_symbols.add(symbol)
        mod = _classify_symbol(
            symbol, builtin_symbols, internal_prefixes, stdlib_modules
        )
        module_symbols.setdefault(mod, []).append(symbol)

    module_symbols = OrderedDict(sorted(module_symbols.items()))

    lines: list[str] = []
    lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    lines.append('#[path = "generated_resolvers/mod.rs"]\n')
    lines.append("mod generated_resolvers;\n")
    lines.append("\n")
    lines.append("pub(crate) use generated_resolvers::resolve_symbol;\n\n")
    lines.append("#[derive(Clone, Copy)]\n")
    lines.append("pub(crate) enum IntrinsicDefaultValue {\n")
    lines.append("    None,\n")
    lines.append("    Bool(bool),\n")
    lines.append("    Int(i64),\n")
    lines.append("}\n\n")
    lines.append("#[derive(Clone, Copy)]\n")
    lines.append("pub(crate) struct IntrinsicSpec {\n")
    lines.append("    pub name: &'static str,\n")
    lines.append("    pub symbol: &'static str,\n")
    lines.append("    pub arity: u8,\n")
    lines.append("    pub defaults: &'static [IntrinsicDefaultValue],\n")
    lines.append("}\n\n")
    lines.append("pub(crate) const INTRINSICS: &[IntrinsicSpec] = &[\n")
    for entry in entries:
        lines.append("    IntrinsicSpec {\n")
        lines.append(f'        name: "{entry.name}",\n')
        lines.append(f'        symbol: "{entry.symbol}",\n')
        lines.append(f"        arity: {entry.arity},\n")
        defaults = ", ".join(entry.defaults)
        lines.append(f"        defaults: &[{defaults}],\n")
        lines.append("    },\n")
    lines.append("];\n")

    _write_rust_if_changed(OUT_RS, "".join(lines))
    _write_resolver_modules(module_symbols)


def _write_pyi(raw_manifest: str) -> None:
    body = _strip_manifest_header(raw_manifest)
    header = (
        "# @generated by tools/gen_intrinsics.py from "
        "runtime/molt-runtime/src/intrinsics/manifest.pyi\n"
    )
    _write_text_if_changed(OUT_PYI, header + body)


def _remove_backend_overrides_rs() -> None:
    if _CHECK_MODE and OUT_BACKEND_OVERRIDES_RS.exists():
        _record_check_diff(
            OUT_BACKEND_OVERRIDES_RS, OUT_BACKEND_OVERRIDES_RS.read_text(encoding="utf-8"), ""
        )
        return
    OUT_BACKEND_OVERRIDES_RS.unlink(missing_ok=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="check without writing")
    args = parser.parse_args(argv)

    global _CHECK_MODE, _CHECK_DIFFS
    _CHECK_MODE = args.check
    _CHECK_DIFFS = []

    raw, entries = _load_manifest()
    _validate_symbols(entries)
    _write_generated_rs(entries)
    _write_pyi(raw)
    _remove_backend_overrides_rs()
    if args.check:
        if _CHECK_DIFFS:
            sys.stderr.writelines(_CHECK_DIFFS)
            return 1
        print("intrinsics registry: in sync")
    return 0


if __name__ == "__main__":
    sys.exit(main())
