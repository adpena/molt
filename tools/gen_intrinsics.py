#!/usr/bin/env python3
"""Generate intrinsics registry artifacts from the canonical manifest.

Usage::

    python tools/gen_intrinsics.py --write
    python tools/gen_intrinsics.py --check
"""

from __future__ import annotations

from collections import OrderedDict
from collections.abc import Mapping, Sequence
from pathlib import Path
import re
import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]


from tools.wasm_abi_gen.intrinsic_availability import (  # noqa: E402
    IntrinsicAvailability,
    load_intrinsic_availability,
)
from tools.generator_io import (  # noqa: E402
    display_path,
    generator_main,
    rustfmt_sources,
)
from tools.intrinsic_manifest import (  # noqa: E402
    IntrinsicEntry,
    parse_intrinsic_manifest,
)

MANIFEST = ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
CATEGORIES_TOML = ROOT / "runtime/molt-runtime/src/intrinsics/categories.toml"
RUNTIME_CARGO_TOML = ROOT / "runtime/molt-runtime/Cargo.toml"
_RUNTIME_AVAILABILITY = load_intrinsic_availability(MANIFEST)
OUT_PYI = ROOT / "src/molt/_intrinsics.pyi"
OUT_INTRINSIC_SYMBOLS_PY = ROOT / "src/molt/_intrinsic_symbols.py"
OUT_RUNTIME_FEATURE_GATES_PY = ROOT / "src/molt/_runtime_feature_gates.py"
OUT_RS = ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs"
OUT_RS_RESOLVERS_DIR = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
MATH_LEAF_RESOLVERS_DIR = ROOT / "runtime/molt-runtime-math/src/intrinsics_generated"
MATH_LEAF_RESOLVER_INDEX = MATH_LEAF_RESOLVERS_DIR / "mod.rs"
XML_LEAF_RESOLVERS_DIR = ROOT / "runtime/molt-runtime-xml/src/intrinsics_generated"
XML_LEAF_RESOLVER_INDEX = XML_LEAF_RESOLVERS_DIR / "mod.rs"
GRAPHLIB_LEAF_RESOLVERS_DIR = (
    ROOT / "runtime/molt-stdlib-graphlib/src/intrinsics_generated"
)
GRAPHLIB_LEAF_RESOLVER_INDEX = GRAPHLIB_LEAF_RESOLVERS_DIR / "mod.rs"
IPADDRESS_LEAF_RESOLVERS_DIR = (
    ROOT / "runtime/molt-runtime-ipaddress/src/intrinsics_generated"
)
IPADDRESS_LEAF_RESOLVER_INDEX = IPADDRESS_LEAF_RESOLVERS_DIR / "mod.rs"
TK_LEAF_RESOLVERS_DIR = ROOT / "runtime/molt-runtime-tk/src/intrinsics_generated"
TK_LEAF_RESOLVER_INDEX = TK_LEAF_RESOLVERS_DIR / "mod.rs"
COLLECTIONS_LEAF_RESOLVERS_DIR = (
    ROOT / "runtime/molt-runtime-collections/src/intrinsics_generated"
)
COLLECTIONS_LEAF_RESOLVER_INDEX = COLLECTIONS_LEAF_RESOLVERS_DIR / "mod.rs"
SERIAL_LEAF_RESOLVERS_DIR = (
    ROOT / "runtime/molt-runtime-serial/src/intrinsics_generated"
)
SERIAL_LEAF_RESOLVER_INDEX = SERIAL_LEAF_RESOLVERS_DIR / "mod.rs"
CRYPTO_LEAF_RESOLVERS_DIR = (
    ROOT / "runtime/molt-runtime-crypto/src/intrinsics_generated"
)
CRYPTO_LEAF_RESOLVER_INDEX = CRYPTO_LEAF_RESOLVERS_DIR / "mod.rs"
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
    "xml_etree": {
        "output": XML_LEAF_RESOLVERS_DIR / "xml_etree_resolver.rs",
        "module_index": XML_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_xml",
        "crate_resolver_path": "molt_runtime_xml::intrinsics_generated::xml_etree_resolver",
        "symbol_path_prefix": "molt_runtime_xml::xml_etree",
        "function_path_prefix": "crate::xml_etree",
    },
    "xml_sax": {
        "output": XML_LEAF_RESOLVERS_DIR / "xml_sax_resolver.rs",
        "module_index": XML_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_xml",
        "crate_resolver_path": "molt_runtime_xml::intrinsics_generated::xml_sax_resolver",
        "symbol_path_prefix": "molt_runtime_xml::xml_sax",
        "function_path_prefix": "crate::xml_sax",
    },
    "graphlib": {
        "output": GRAPHLIB_LEAF_RESOLVERS_DIR / "graphlib_resolver.rs",
        "module_index": GRAPHLIB_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_stdlib_graphlib",
        "crate_resolver_path": "molt_stdlib_graphlib::intrinsics_generated::graphlib_resolver",
        "symbol_path_prefix": "molt_stdlib_graphlib::graphlib",
        "function_path_prefix": "crate::graphlib",
    },
    "ipaddress": {
        "output": IPADDRESS_LEAF_RESOLVERS_DIR / "ipaddress_resolver.rs",
        "module_index": IPADDRESS_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_ipaddress",
        "crate_resolver_path": "molt_runtime_ipaddress::intrinsics_generated::ipaddress_resolver",
        "symbol_path_prefix": "molt_runtime_ipaddress::ipaddress",
        "function_path_prefix": "crate::ipaddress",
    },
    "tk": {
        "output": TK_LEAF_RESOLVERS_DIR / "tk_resolver.rs",
        "module_index": TK_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_tk",
        "crate_resolver_path": "molt_runtime_tk::intrinsics_generated::tk_resolver",
        "symbol_path_prefix": "molt_runtime_tk::intrinsics",
        "function_path_prefix": "crate::intrinsics",
    },
    "collections": {
        "output": COLLECTIONS_LEAF_RESOLVERS_DIR / "collections_resolver.rs",
        "module_index": COLLECTIONS_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_collections",
        "crate_resolver_path": (
            "molt_runtime_collections::intrinsics_generated::collections_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_collections::collections_ext",
        "function_path_prefix": "crate::collections_ext",
    },
    "archive": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "archive_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::archive_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::zipfile",
        "function_path_prefix": "crate::zipfile",
    },
    "base64": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "base64_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::base64_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::base64_mod",
        "function_path_prefix": "crate::base64_mod",
    },
    "binascii": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "binascii_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::binascii_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::binascii",
        "function_path_prefix": "crate::binascii",
    },
    "configparser": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "configparser_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::configparser_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::configparser",
        "function_path_prefix": "crate::configparser",
    },
    "csv": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "csv_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": "molt_runtime_serial::intrinsics_generated::csv_resolver",
        "symbol_path_prefix": "molt_runtime_serial::csv",
        "function_path_prefix": "crate::csv",
    },
    "datetime": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "datetime_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::datetime_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::datetime",
        "function_path_prefix": "crate::datetime",
    },
    "decimal": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "decimal_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::decimal_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::decimal",
        "function_path_prefix": "crate::decimal",
    },
    "email": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "email_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::email_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::email",
        "function_path_prefix": "crate::email",
    },
    "quopri": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "quopri_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::quopri_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::email",
        "function_path_prefix": "crate::email",
    },
    "struct": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "struct_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": (
            "molt_runtime_serial::intrinsics_generated::struct_resolver"
        ),
        "symbol_path_prefix": "molt_runtime_serial::structs",
        "function_path_prefix": "crate::structs",
    },
    "uu": {
        "output": SERIAL_LEAF_RESOLVERS_DIR / "uu_resolver.rs",
        "module_index": SERIAL_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_serial",
        "crate_resolver_path": "molt_runtime_serial::intrinsics_generated::uu_resolver",
        "symbol_path_prefix": "molt_runtime_serial::binascii",
        "function_path_prefix": "crate::binascii",
    },
    "crypto": {
        "output": CRYPTO_LEAF_RESOLVERS_DIR / "crypto_resolver.rs",
        "module_index": CRYPTO_LEAF_RESOLVER_INDEX,
        "crate_path": "molt_runtime_crypto",
        "crate_resolver_path": (
            "molt_runtime_crypto::intrinsics_generated::crypto_resolver"
        ),
        "path_prefixes": (
            ("molt_hash_", "molt_runtime_crypto::hashlib", "crate::hashlib"),
            ("molt_hmac_", "molt_runtime_crypto::hmac", "crate::hmac"),
            ("molt_compare_digest", "molt_runtime_crypto::hmac", "crate::hmac"),
            ("molt_pbkdf2_", "molt_runtime_crypto::hashlib", "crate::hashlib"),
            ("molt_scrypt", "molt_runtime_crypto::hashlib", "crate::hashlib"),
            ("molt_secrets_", "molt_runtime_crypto::secrets", "crate::secrets"),
        ),
    },
}
# Outputs this generator once rendered and no longer does. Retirement is part of
# the authority: rendering fails closed while any of them (or an orphaned
# ``*_resolver.rs`` in a directory whose resolvers this generator owns) exists.
RETIRED_OUTPUTS = (ROOT / "runtime/molt-backend/src/intrinsic_symbol_overrides.rs",)
# Formatting runs one guarded rustfmt per batch of scratch files; keep each
# command line well inside the 32,767-character Windows limit.


def _validate_declared_features(availability: IntrinsicAvailability) -> None:
    """Every declared feature must be a molt-runtime Cargo feature."""
    cargo = tomllib.loads(RUNTIME_CARGO_TOML.read_text(encoding="utf-8"))
    known = set(cargo.get("features", {}))
    unknown = sorted(availability.features() - known)
    if unknown:
        raise RuntimeError(
            "intrinsic cfg declarations name features molt-runtime does not "
            f"define: {', '.join(unknown)}"
        )


def _strip_manifest_header(text: str) -> str:
    lines = text.splitlines()
    idx = 0
    while idx < len(lines) and lines[idx].lstrip().startswith("#"):
        idx += 1
    if idx < len(lines) and lines[idx].strip() == "":
        idx += 1
    return "\n".join(lines[idx:]).rstrip() + "\n"


def _load_manifest() -> tuple[str, list[IntrinsicEntry]]:
    raw = MANIFEST.read_text(encoding="utf-8")
    return raw, parse_intrinsic_manifest(raw)


def _validate_symbols(entries: list[IntrinsicEntry]) -> None:
    runtime_root = ROOT / "runtime"
    src_roots = sorted(path for path in runtime_root.glob("*/src") if path.is_dir())
    rs_files = [rs for src_root in src_roots for rs in src_root.rglob("*.rs")]
    corpus = "\n".join(path.read_text(encoding="utf-8") for path in rs_files)
    # Single-pass: extract all function names defined in the corpus â€” O(n+m)
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

    builtin_symbols: list[str] = []
    for category, symbols in data.get("builtin", {}).items():
        if not isinstance(symbols, list) or not all(
            isinstance(symbol, str) and symbol for symbol in symbols
        ):
            raise TypeError(f"builtin.{category} must be a list of non-empty strings")
        builtin_symbols.extend(symbols)
    builtin_symbols = list(dict.fromkeys(builtin_symbols))

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


def _feature_gate_for_symbol(symbol: str) -> str | None:
    """Return the Cargo feature *symbol*'s manifest entry declares, if any."""
    return _RUNTIME_AVAILABILITY.feature_gate_for_symbol(symbol)


def _cfg_gate_for_symbol(symbol: str) -> str | None:
    """Return the complete Rust cfg predicate *symbol*'s entry declares."""
    return _RUNTIME_AVAILABILITY.cfg_for_symbol(symbol).rust_predicate()


# Additional prefix-to-module mapping for modules NOT yet in categories.toml.
# These are checked *after* categories.toml rules so the TOML file wins.
_EXTRA_PREFIX_MODULES: list[tuple[str, str]] = [
    ("molt_pathlib_", "pathlib"),
    ("molt_hashlib_", "hashlib"),
    ("molt_ssl_", "ssl"),
    ("molt_atexit_", "atexit"),
    ("molt_itertools_", "itertools"),
    ("molt_functools_", "functools"),
    ("molt_enum_", "enum"),
    ("molt_dataclasses_", "dataclasses"),
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
    ("molt_sys_", "sys"),
    ("molt_platform_", "platform"),
    ("molt_codecs_", "codecs"),
    ("molt_encodings_", "codecs"),
    ("molt_pprint_", "pprint"),
    ("molt_shutil_", "shutil"),
    ("molt_shlex_", "shlex"),
    ("molt_fnmatch", "fnmatch"),
    ("molt_pickle_", "serialization"),
    ("molt_uuid_", "uuid"),
    ("molt_socketpair", "socket"),
    ("molt_multiprocessing_", "multiprocessing"),
    ("molt_subprocess_", "subprocess"),
    ("molt_queue_", "queue"),
    ("molt_gc_", "gc"),
    ("molt_array_", "array"),
    ("molt_memoryview_", "memoryview"),
    ("molt_operator_", "operator"),
    ("molt_keyword_", "keyword"),
    ("molt_opcode_", "opcode"),
    ("molt_site_", "site"),
    ("molt_gettext_", "gettext"),
    ("molt_codeop_", "codeop"),
    ("molt_compileall_", "compileall"),
    ("molt_py_compile_", "py_compile"),
    ("molt_runpy_", "runpy"),
    ("molt_pkgutil_", "pkgutil"),
    ("molt_stat_", "stat"),
    ("molt_fcntl_", "fcntl"),
    ("molt_punycode_", "punycode"),
    ("molt_this_", "this"),
    ("molt_protocol_", "asyncio"),
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
    ("molt_path_", "pathlib"),
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


def _leaf_resolver_paths_for_symbol(
    mod_name: str,
    symbol: str,
    leaf: dict[str, object],
) -> tuple[str, str]:
    path_prefixes = leaf.get("path_prefixes")
    if path_prefixes is None:
        return str(leaf["symbol_path_prefix"]), str(leaf["function_path_prefix"])

    best: tuple[int, str, str] | None = None
    if not isinstance(path_prefixes, tuple | list):
        raise TypeError(f"leaf resolver {mod_name!r} path_prefixes must be a sequence")
    for entry in path_prefixes:
        if not isinstance(entry, tuple | list) or len(entry) != 3:
            raise TypeError(
                f"leaf resolver {mod_name!r} path_prefixes entries must be triples"
            )
        prefix, symbol_path_prefix, function_path_prefix = entry
        if not all(
            isinstance(value, str)
            for value in (prefix, symbol_path_prefix, function_path_prefix)
        ):
            raise TypeError(
                f"leaf resolver {mod_name!r} path_prefixes entries must be strings"
            )
        if symbol.startswith(prefix) and (best is None or len(prefix) > best[0]):
            best = (len(prefix), symbol_path_prefix, function_path_prefix)

    if best is None:
        raise RuntimeError(
            f"leaf resolver {mod_name!r} has no path_prefixes entry for {symbol!r}"
        )
    return best[1], best[2]


def _leaf_resolver_output(mod_name: str, leaf: dict[str, object]) -> Path:
    output = leaf["output"]
    if not isinstance(output, Path):
        raise TypeError(f"leaf resolver output for {mod_name!r} must be a Path")
    return output


def _render_leaf_resolver_module(
    mod_name: str,
    symbols: list[str],
    leaf: dict[str, object],
) -> str:
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
        symbol_path_prefix, function_path_prefix = _leaf_resolver_paths_for_symbol(
            mod_name, sym, leaf
        )
        _append_leaf_resolver_arm(
            lines,
            sym,
            symbol_path_prefix=symbol_path_prefix,
            function_path_prefix=function_path_prefix,
        )
    lines.append("        _ => None,\n")
    lines.append("    }\n")
    lines.append("}\n")
    return "".join(lines)


def _render_leaf_facade_resolver_module(
    leaf: dict[str, object],
    cfg_gate: str,
) -> str:
    crate_path = str(leaf["crate_path"])
    resolver_path = str(
        leaf.get("crate_resolver_path", f"{crate_path}::intrinsics_generated")
    )
    lines: list[str] = []
    lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    lines.append("#[inline(never)]\n")
    lines.append("#[cold]\n")
    lines.append("pub(super) fn resolve_symbol(symbol: &str) -> Option<u64> {\n")
    lines.append(f"    #[cfg({cfg_gate})]\n")
    lines.append("    {\n")
    lines.append(f"        {resolver_path}::resolve_symbol_with(\n")
    lines.append("            symbol,\n")
    lines.append("            crate::builtins::functions::runtime_fn_addr,\n")
    lines.append("        )\n")
    lines.append("    }\n")
    lines.append(f"    #[cfg(not({cfg_gate}))]\n")
    lines.append("    {\n")
    lines.append("        let _ = symbol;\n")
    lines.append("        None\n")
    lines.append("    }\n")
    lines.append("}\n")
    return "".join(lines)


def _leaf_index_module_dir(index_path: Path) -> Path:
    """The directory holding the resolver modules a leaf index declares."""
    if index_path.name == "mod.rs":
        return index_path.parent
    return index_path.parent / index_path.stem


def _render_leaf_resolver_index(modules: set[str]) -> str:
    lines: list[str] = []
    lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
    for module in sorted(modules):
        lines.append(f"pub mod {module};\n")
    return "".join(lines)


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


def _leaf_resolver_cfg_gate(mod_name: str, symbols: list[str]) -> str:
    gates = {_cfg_gate_for_symbol(sym) for sym in symbols}
    if len(gates) == 1 and None not in gates:
        gate = next(iter(gates))
        assert gate is not None
        return gate
    rendered = ", ".join(
        "<none>" if gate is None else gate
        for gate in sorted(gates, key=lambda value: "" if value is None else value)
    )
    raise RuntimeError(
        f"leaf resolver {mod_name!r} must map to one runtime cfg gate; "
        f"found: {rendered or '<empty>'}"
    )


def _append_declared_absence_probe(
    lines: list[str], gate: str, symbols: Sequence[str]
) -> None:
    """Prove that no build outside *gate* defines *symbols*.

    Outside the declared cfg each name must resolve to its sentinel in
    ``declared_absent``. A definition that survives (a hand stub, or a
    definition gated more widely than declared) is a second glob candidate,
    and rustc rejects the ambiguous name.
    """
    lines.append("\n")
    lines.append(f"#[cfg(not({gate}))]\n")
    lines.append("const _: () = {\n")
    lines.append("    #[allow(unused_imports)]\n")
    lines.append("    use crate::*;\n")
    lines.append("    use super::declared_absent::*;\n")
    for sym in symbols:
        lines.append(f"    let _ = {sym};\n")
    lines.append("};\n")


def _render_resolver_modules(
    module_symbols: OrderedDict[str, list[str]],
) -> tuple[dict[Path, str], tuple[Path, ...]]:
    """Render every resolver module as unformatted Rust.

    Returns the sources keyed by output path, plus the directories whose
    ``*_resolver.rs`` files this generator owns outright.
    """
    sources: dict[Path, str] = {}
    leaf_indexes: dict[Path, set[str]] = {}
    declared_absent: list[str] = []

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

    for mod_name, symbols in module_symbols.items():
        leaf = LEAF_RESOLVER_REGISTRIES.get(mod_name)
        if leaf is not None:
            cfg_gate = _leaf_resolver_cfg_gate(mod_name, symbols)
            output = _leaf_resolver_output(mod_name, leaf)
            sources[output] = _render_leaf_resolver_module(mod_name, symbols, leaf)
            sources[OUT_RS_RESOLVERS_DIR / _resolver_file_name(mod_name)] = (
                _render_leaf_facade_resolver_module(leaf, cfg_gate)
            )
            module_index = leaf.get("module_index")
            if isinstance(module_index, Path):
                leaf_indexes.setdefault(module_index, set()).add(output.stem)
            continue

        lines: list[str] = []
        lines.append("// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n")
        lines.append("#[inline(never)]\n")
        lines.append("#[cold]\n")

        # Collect complete feature/target gates, preserving symbol order.
        gated: dict[str | None, list[str]] = {}
        for sym in symbols:
            gate = _cfg_gate_for_symbol(sym)
            gated.setdefault(gate, []).append(sym)

        lines.append("pub(super) fn resolve_symbol(symbol: &str) -> Option<u64> {\n")
        lines.append("    match symbol {\n")
        for gate, syms in gated.items():
            for sym in syms:
                if gate:
                    lines.append(f"        #[cfg({gate})]\n")
                _append_resolver_arm(lines, sym)
        lines.append("        _ => None,\n")
        lines.append("    }\n")
        lines.append("}\n")
        for gate, syms in gated.items():
            if gate is None:
                continue
            declared_absent.extend(syms)
            _append_declared_absence_probe(lines, gate, syms)
        sources[OUT_RS_RESOLVERS_DIR / _resolver_file_name(mod_name)] = "".join(lines)
    if declared_absent:
        mod_lines.append("\n")
        mod_lines.append(
            "// One sentinel per declared-gated symbol. A build outside a symbol's\n"
            "// declared cfg resolves its probe here; a definition that is still\n"
            "// present makes the name ambiguous and fails to compile (HF-487).\n"
        )
        mod_lines.append("#[allow(dead_code, non_upper_case_globals)]\n")
        mod_lines.append("mod declared_absent {\n")
        # A marker type, not `()`: a probe's `let _ =` then binds no unit
        # value (clippy::let_unit_value).
        mod_lines.append("    pub(super) struct DeclaredAbsent;\n")
        for sym in sorted(declared_absent):
            mod_lines.append(
                f"    pub(super) const {sym}: DeclaredAbsent = DeclaredAbsent;\n"
            )
        mod_lines.append("}\n")
    sources[OUT_RS_RESOLVERS_DIR / "mod.rs"] = "".join(mod_lines)
    for index_path, modules in leaf_indexes.items():
        sources[index_path] = _render_leaf_resolver_index(modules)
    owned_dirs = (
        OUT_RS_RESOLVERS_DIR,
        *(_leaf_index_module_dir(index_path) for index_path in leaf_indexes),
    )
    return sources, owned_dirs


def _render_generated_rs(
    entries: list[IntrinsicEntry],
) -> tuple[dict[Path, str], tuple[Path, ...]]:
    """Render ``generated.rs`` and its resolver tree as unformatted Rust.

    Returns the sources keyed by output path and the resolver directories this
    generator owns (see :func:`_render_resolver_modules`).
    """
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
    sources, owned_dirs = _render_resolver_modules(module_symbols)

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
    lines.append("    pub python_name: Option<&'static str>,\n")
    lines.append("}\n\n")
    lines.append("pub(crate) const INTRINSICS: &[IntrinsicSpec] = &[\n")
    for entry in entries:
        lines.append("    IntrinsicSpec {\n")
        lines.append(f'        name: "{entry.name}",\n')
        lines.append(f'        symbol: "{entry.symbol}",\n')
        lines.append(f"        arity: {entry.arity},\n")
        defaults = ", ".join(entry.defaults)
        lines.append(f"        defaults: &[{defaults}],\n")
        python_name = f'Some("{entry.python_name}")' if entry.python_name else "None"
        lines.append(f"        python_name: {python_name},\n")
        lines.append("    },\n")
    lines.append("];\n")

    sources[OUT_RS] = "".join(lines)
    return sources, owned_dirs


def _render_pyi(raw_manifest: str) -> str:
    # Availability is a build fact owned by the manifest and its generated
    # gate table; the typing projection carries signatures only.
    body = "".join(
        line
        for line in _strip_manifest_header(raw_manifest).splitlines(keepends=True)
        if not line.startswith("# cfg: ")
    )
    header = (
        "# @generated by tools/gen_intrinsics.py from "
        "runtime/molt-runtime/src/intrinsics/manifest.pyi\n"
    )
    return header + body


def _render_intrinsic_symbols_py(entries: list[IntrinsicEntry]) -> str:
    lines: list[str] = []
    lines.append(
        "# @generated by tools/gen_intrinsics.py from "
        "runtime/molt-runtime/src/intrinsics/manifest.pyi\n"
    )
    lines.append("# DO NOT EDIT BY HAND.\n\n")
    lines.append("from __future__ import annotations\n\n")
    lines.append("INTRINSIC_SIGNATURES: dict[str, tuple[str, int]] = {\n")
    for entry in entries:
        lines.append(f'    "{entry.name}": ("{entry.symbol}", {entry.arity}),\n')
    lines.append("}\n\n\n")
    lines.append("def intrinsic_runtime_symbol_name(name: str) -> str:\n")
    lines.append("    signature = INTRINSIC_SIGNATURES.get(name)\n")
    lines.append("    return name if signature is None else signature[0]\n")
    return "".join(lines)


_RUNTIME_FEATURE_GATES_FUNCTIONS = """

def feature_for_symbol(symbol: str) -> str | None:
    \"\"\"Return the Cargo feature whose build defines *symbol*, or None.\"\"\"
    return RUNTIME_SYMBOL_FEATURES.get(symbol)


def target_arch_from_triple(target_triple: str | None) -> str | None:
    \"\"\"Return the Rust target architecture component.\"\"\"
    if target_triple is None:
        return None
    return target_triple.split("-", 1)[0]


def target_os_from_triple(target_triple: str | None) -> str:
    \"\"\"Return the target OS that availability facts name for *target_triple*.\"\"\"
    if target_triple == "wasm32-wasip1":
        return "wasi"
    if target_triple == "wasm32-unknown-unknown":
        return "none"
    from molt.native_target import resolve_native_target_spec

    return resolve_native_target_spec(target_triple).os


def unsupported_target_arches_for_symbol(symbol: str) -> tuple[str, ...]:
    \"\"\"Return architectures where *symbol* has no provider.\"\"\"
    return RUNTIME_SYMBOL_UNSUPPORTED_TARGET_ARCHES.get(symbol, ())


def supported_target_oses_for_symbol(symbol: str) -> tuple[str, ...]:
    \"\"\"Return required OSes, or () for no OS requirement.\"\"\"
    return RUNTIME_SYMBOL_SUPPORTED_TARGET_OSES.get(symbol, ())


def unsupported_target_oses_for_symbol(symbol: str) -> tuple[str, ...]:
    \"\"\"Return OSes where *symbol* has no provider.\"\"\"
    return RUNTIME_SYMBOL_UNSUPPORTED_TARGET_OSES.get(symbol, ())


def symbol_availability_depends_on_target_os(symbol: str) -> bool:
    \"\"\"Whether deciding *symbol*'s availability needs the target OS.\"\"\"
    return (
        symbol in RUNTIME_SYMBOL_SUPPORTED_TARGET_OSES
        or symbol in RUNTIME_SYMBOL_UNSUPPORTED_TARGET_OSES
    )


def runtime_symbol_available_on_target(
    symbol: str, *, target_triple: str | None, target_os: str | None
) -> bool:
    \"\"\"Whether *symbol* has a provider for *target_triple*.\"\"\"
    if target_arch_from_triple(target_triple) in unsupported_target_arches_for_symbol(
        symbol
    ):
        return False
    supported_oses = supported_target_oses_for_symbol(symbol)
    if supported_oses and target_os not in supported_oses:
        return False
    return target_os not in unsupported_target_oses_for_symbol(symbol)


def features_unsupported_on_target(
    *, target_arch: str | None, target_os: str | None
) -> frozenset[str]:
    \"\"\"Features with no provider on the target.\"\"\"
    unsupported = {
        feature
        for feature, arches in RUNTIME_FEATURE_TARGET_ARCH_EXCLUSIONS.items()
        if target_arch in arches
    }
    unsupported.update(
        feature
        for feature, oses in RUNTIME_FEATURE_TARGET_OS_EXCLUSIONS.items()
        if target_os in oses
    )
    return frozenset(unsupported)
"""


def _render_tuple(values: Sequence[str]) -> str:
    rendered = ", ".join(f'"{value}"' for value in values)
    return f"({rendered},)" if len(values) == 1 else f"({rendered})"


def _render_runtime_feature_gates_py(availability: IntrinsicAvailability) -> str:
    lines: list[str] = []
    lines.append(
        '"""Generated runtime intrinsic availability: one exact fact per symbol.\n\n'
        "Source authority: each entry's ``# cfg:`` declaration in\n"
        "runtime/molt-runtime/src/intrinsics/manifest.pyi. The generated Rust\n"
        "resolvers prove at compile time that each definition's cfg equals its\n"
        "declaration, so a runtime built without a symbol's feature never\n"
        "defines it, and reachability must select or refuse that feature.\n"
        '"""\n\n'
    )
    lines.append("# @generated by tools/gen_intrinsics.py. DO NOT EDIT.\n\n")
    lines.append("from __future__ import annotations\n\n")
    symbol_cfgs = availability.symbol_cfgs
    lines.append("RUNTIME_SYMBOL_FEATURES: dict[str, str] = {\n")
    for symbol, cfg in symbol_cfgs.items():
        if cfg.feature is not None:
            lines.append(f'    "{symbol}": "{cfg.feature}",\n')
    lines.append("}\n\n")
    for name, field in (
        ("RUNTIME_SYMBOL_UNSUPPORTED_TARGET_ARCHES", "unsupported_target_arches"),
        ("RUNTIME_SYMBOL_SUPPORTED_TARGET_OSES", "supported_target_oses"),
        ("RUNTIME_SYMBOL_UNSUPPORTED_TARGET_OSES", "unsupported_target_oses"),
    ):
        lines.append(f"{name}: dict[str, tuple[str, ...]] = {{\n")
        for symbol, cfg in symbol_cfgs.items():
            values = getattr(cfg, field)
            if values:
                lines.append(f'    "{symbol}": {_render_tuple(values)},\n')
        lines.append("}\n\n")
    for name, exclusions in (
        (
            "RUNTIME_FEATURE_TARGET_ARCH_EXCLUSIONS",
            availability.feature_target_arch_exclusions(),
        ),
        (
            "RUNTIME_FEATURE_TARGET_OS_EXCLUSIONS",
            availability.feature_target_os_exclusions(),
        ),
    ):
        lines.append(f"{name}: dict[str, frozenset[str]] = {{\n")
        for feature, values in exclusions:
            rendered = ", ".join(f'"{value}"' for value in values)
            lines.append(f'    "{feature}": frozenset({{{rendered}}}),\n')
        lines.append("}\n\n")
    lines.append("RUNTIME_FEATURES: frozenset[str] = frozenset(\n")
    lines.append("    {\n")
    for feature in sorted(availability.features()):
        lines.append(f'        "{feature}",\n')
    lines.append("    }\n")
    lines.append(")\n")
    lines.append(_RUNTIME_FEATURE_GATES_FUNCTIONS)
    return "".join(lines)


def _stale_generated_files(
    rendered: Mapping[Path, str], owned_dirs: Sequence[Path]
) -> list[Path]:
    """Files this generator owns that the current render no longer produces."""
    stale = [path for path in RETIRED_OUTPUTS if path.exists()]
    for directory in owned_dirs:
        stale.extend(
            path
            for path in sorted(directory.glob("*_resolver.rs"))
            if path not in rendered
        )
    return stale


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text.

    A file this generator owns but no longer renders (a retired output, or a
    resolver module whose category disappeared) is part of the authority too:
    rendering raises while one exists, so ``--check`` and ``--write`` both
    fail closed naming the files to delete.
    """
    raw, entries = _load_manifest()
    _validate_symbols(entries)
    _validate_declared_features(_RUNTIME_AVAILABILITY)
    rust_sources, owned_dirs = _render_generated_rs(entries)
    stale = _stale_generated_files(rust_sources, owned_dirs)
    if stale:
        raise RuntimeError(
            "delete generated files that tools/gen_intrinsics.py no longer "
            "renders: " + ", ".join(display_path(path) for path in stale)
        )
    outputs = rustfmt_sources(rust_sources, label="intrinsic Rust sources")
    outputs[OUT_RUNTIME_FEATURE_GATES_PY] = _render_runtime_feature_gates_py(
        _RUNTIME_AVAILABILITY
    )
    outputs[OUT_PYI] = _render_pyi(raw)
    outputs[OUT_INTRINSIC_SYMBOLS_PY] = _render_intrinsic_symbols_py(entries)
    return outputs


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
