"""Canonical symbol-prefix → Cargo-feature gate mapping.

This module is the **single source of truth** for "which runtime intrinsic
symbol is gated behind which `stdlib_*` / `builtin_*` Cargo feature." It is
consumed by two layers that must never drift:

1. ``tools/gen_intrinsics.py`` — emits the per-symbol ``#[cfg(feature = ...)]``
   guards into ``runtime/molt-runtime/src/intrinsics/generated.rs`` so the
   linker can drop unreachable domains. When a feature is disabled the
   ``resolve_symbol`` arm for that intrinsic is excluded and the symbol is
   absent from the linked archive.
2. ``molt.cli`` — at static import-graph resolution time, refuses *loudly at
   compile time* when a statically-imported stdlib module requires an intrinsic
   whose gating feature the selected build profile excludes. Without this the
   excluded symbol surfaces only as an undefined-symbol **linker error**
   (``Undefined symbols: _molt_ast_get_docstring`` …), which is opaque and
   non-actionable.

Because both consumers read this one table, the compile-time refusal in the
frontend is guaranteed to agree with the runtime's actual ``#[cfg]`` gating:
they are derived from the same data. ``tests/test_gen_intrinsics.py`` pins the
generated Rust in sync, so this table is kept honest against the compiled
runtime.

Ordering note: longest matching prefix wins (an intrinsic may share a shorter
prefix with a different feature group, e.g. ``molt_logging_record_`` is gated by
``stdlib_logging_ext`` while the bare ``molt_logging_`` prefix is
``stdlib_logging``).
"""

from __future__ import annotations

# Map symbol prefixes to Cargo feature flags. When a feature is disabled the
# resolve_symbol entry is excluded so the linker can drop the corresponding
# code.  Ordering matters: longest prefix wins.
RUNTIME_FEATURE_GATES: tuple[tuple[str, str], ...] = (
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
    # serial crate: binary codecs, configparser, datetime, and struct helpers
    # live only in molt-runtime-serial. Disabled profiles must refuse these
    # imports loudly instead of falling through to deleted in-core copies.
    ("molt_base64_", "stdlib_serial"),
    ("molt_binascii_", "stdlib_serial"),
    ("molt_configparser_", "stdlib_serial"),
    ("molt_datetime_", "stdlib_serial"),
    ("molt_date_", "stdlib_serial"),
    ("molt_timedelta_", "stdlib_serial"),
    ("molt_timezone_", "stdlib_serial"),
    ("molt_struct_", "stdlib_serial"),
    ("molt_uu_codec", "stdlib_serial"),
    # math-family intrinsics live only in molt-runtime-math. Disabled profiles
    # must refuse imports instead of relying on deleted in-core fallback copies.
    ("molt_cmath_", "stdlib_math"),
    ("molt_colorsys_", "stdlib_math"),
    ("molt_fraction_", "stdlib_math"),
    ("molt_math_", "stdlib_math"),
    ("molt_random_", "stdlib_math"),
    ("molt_statistics_", "stdlib_math"),
    # XML-family intrinsics live only in molt-runtime-xml. Disabled profiles
    # must refuse imports instead of relying on deleted in-core fallback copies.
    ("molt_xml_", "stdlib_xml"),
    # ast
    ("molt_ast_", "stdlib_ast"),
    # collections + argparse live in the extracted collections crate.
    ("molt_argparse_", "stdlib_collections"),
    ("molt_namedtuple_", "stdlib_collections"),
    ("molt_ordereddict_", "stdlib_collections"),
    ("molt_defaultdict_", "stdlib_collections"),
    ("molt_deque_", "stdlib_collections"),
    ("molt_counter_", "stdlib_collections"),
    ("molt_chainmap_", "stdlib_collections"),
    # fs_extra: glob, tempfile
    ("molt_glob_glob", "stdlib_fs_extra"),
    ("molt_glob_iglob", "stdlib_fs_extra"),
    ("molt_glob_pattern", "stdlib_fs_extra"),
    ("molt_tempfile_", "stdlib_fs_extra"),
    # archive: zipfile
    ("molt_zipfile_", "stdlib_archive"),
    ("molt_imghdr_", "stdlib_archive"),
    # tk: tkinter GUI bindings
    ("molt_tk_", "stdlib_tk"),
    # stringprep: RFC 3454 table helpers live in the extracted leaf crate.
    ("molt_stringprep_", "stdlib_stringprep"),
    # text: html and unicodedata live in the extracted text leaf crate.
    ("molt_html_", "stdlib_text"),
    ("molt_unicodedata_", "stdlib_text"),
    # zoneinfo: IANA TZif helpers live in the extracted zoneinfo leaf crate.
    ("molt_zoneinfo_", "stdlib_zoneinfo"),
    # HTTP-family intrinsics live in the extracted molt-runtime-http crate.
    # Networking transport support is still gated by stdlib_net inside the
    # implementation; symbol ownership is governed by stdlib_http.
    ("molt_ctypes_", "stdlib_http"),
    ("molt_http_", "stdlib_http"),
    ("molt_socketserver_", "stdlib_http"),
    ("molt_urllib_", "stdlib_http"),
    # Advanced regex compiler/engine intrinsics live in the extracted
    # molt-runtime-regex crate. Lower-level literal/charclass helpers remain
    # core runtime intrinsics and intentionally stay ungated.
    ("molt_re_finditer_collect", "stdlib_regex"),
    ("molt_re_fullmatch_check", "stdlib_regex"),
    ("molt_re_match_groupdict", "stdlib_regex"),
    ("molt_re_match_groups", "stdlib_regex"),
    ("molt_re_match_group", "stdlib_regex"),
    ("molt_re_named_backref_advance", "stdlib_regex"),
    ("molt_re_negative_", "stdlib_regex"),
    ("molt_re_pattern_info", "stdlib_regex"),
    ("molt_re_positive_", "stdlib_regex"),
    ("molt_re_strip_verbose", "stdlib_regex"),
    ("molt_re_sub_callable", "stdlib_regex"),
    ("molt_re_compile", "stdlib_regex"),
    ("molt_re_execute", "stdlib_regex"),
    ("molt_re_escape", "stdlib_regex"),
    ("molt_re_split", "stdlib_regex"),
    ("molt_re_sub", "stdlib_regex"),
    # networking: IP address helpers. SSL keeps an always-linkable ABI because
    # asyncio imports ssl eagerly even in micro profiles; runtime operations
    # without net support raise from the Rust intrinsic implementation.
    ("molt_ipaddress_", "stdlib_net"),
    # asyncio: event loop, futures, tasks, queues, streams, transports
    ("molt_asyncio_", "stdlib_asyncio"),
    ("molt_event_loop_", "stdlib_asyncio"),
    ("molt_pipe_transport_", "stdlib_asyncio"),
    # email
    ("molt_email_", "stdlib_email"),
    ("molt_quopri_", "stdlib_email"),
    # decimal
    ("molt_decimal_", "stdlib_decimal"),
    # logging core lives behind stdlib_logging; stateful LogRecord/Logger/etc.
    # registries live in the extracted stdlib_logging_ext crate.
    ("molt_logging_record_", "stdlib_logging_ext"),
    ("molt_logging_formatter_", "stdlib_logging_ext"),
    ("molt_logging_handler_", "stdlib_logging_ext"),
    ("molt_logging_stream_handler_", "stdlib_logging_ext"),
    ("molt_logging_logger_", "stdlib_logging_ext"),
    ("molt_logging_manager_", "stdlib_logging_ext"),
    ("molt_logging_root_logger", "stdlib_logging_ext"),
    ("molt_logging_basic_config", "stdlib_logging_ext"),
    ("molt_logging_shutdown", "stdlib_logging_ext"),
    ("molt_logging_get_level_name", "stdlib_logging_ext"),
    ("molt_logging_add_level_name", "stdlib_logging_ext"),
    ("molt_logging_level_to_int", "stdlib_logging_ext"),
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
)


def feature_gate_for_symbol(symbol: str) -> str | None:
    """Return the Cargo feature gate for *symbol*, or None if ungated.

    Longest matching prefix wins. ``None`` means the symbol is always linked
    (no feature gate) — e.g. core builtins or the deliberately-ungated
    ``molt_ssl_*`` ABI — and therefore importing the owning module never causes
    a profile-driven link failure.
    """
    best: tuple[int, str] | None = None
    for prefix, feature in RUNTIME_FEATURE_GATES:
        if symbol.startswith(prefix):
            prefix_len = len(prefix)
            if best is None or prefix_len > best[0]:
                best = (prefix_len, feature)
    return best[1] if best is not None else None


# Features whose Cargo gate, when disabled, actually REMOVES the intrinsic
# *symbol definitions* from the linked runtime archive — so requiring one of
# their intrinsics on a profile that excludes the feature is an undefined-symbol
# **link error**.
#
# A feature is link-affecting iff it either (a) gates a `mod` declaration inside
# the `molt-runtime` crate (the module — and its `#[unsafe(no_mangle)]` exports
# — is `#[cfg]`-compiled out) or (b) is `dep:`-backed (its intrinsics live in,
# or transitively pull, an optional crate/dependency that is dropped when the
# feature is off).
#
# The COMPLEMENT — features defined as an empty `[]` group in Cargo.toml
# (``stdlib_logging``, ``stdlib_concurrent``, ``stdlib_dbm``,
# ``stdlib_importlib_extra``, ``stdlib_signal``, ``stdlib_select``) — gate only
# the per-app *resolver arm* in ``generated.rs``. Their
# ``#[unsafe(no_mangle)]`` functions are compiled into the core runtime
# unconditionally and are ALWAYS present in every profile's archive, so
# importing the owning module never causes a link failure (at most a runtime
# "intrinsic unavailable" if reached dynamically). They must NOT trigger a
# compile-time refusal, or every micro build that pulls core import machinery
# (``importlib.machinery`` requires the ``stdlib_importlib_extra``-gated
# ``molt_importlib_resources_*`` resolver arms, yet their symbols are always
# defined) would be wrongly refused.
#
# `tests/test_runtime_feature_gates.py` re-derives this set mechanically from
# `runtime/molt-runtime/Cargo.toml` + the crate's `#[cfg(feature=...)]`-gated
# `mod` declarations and asserts equality, so a newly-added gated module or
# dep-backed feature that is not classified here fails loudly at test time.
LINK_AFFECTING_FEATURES: frozenset[str] = frozenset(
    {
        "molt_gpu_primitives",
        "sqlite",
        "stdlib_archive",
        "stdlib_ast",
        "stdlib_asyncio",
        "stdlib_collections",
        "stdlib_compression",
        "stdlib_crypto",
        "stdlib_csv",
        "stdlib_decimal",
        "stdlib_email",
        "stdlib_fs_extra",
        "stdlib_http",
        "stdlib_logging_ext",
        "stdlib_math",
        "stdlib_net",
        "stdlib_regex",
        "stdlib_serial",
        "stdlib_serialization",
        "stdlib_stringprep",
        "stdlib_text",
        "stdlib_tk",
        "stdlib_xml",
        "stdlib_zoneinfo",
    }
)


def link_affecting_feature_gate_for_symbol(symbol: str) -> str | None:
    """Return *symbol*'s feature gate only when disabling it breaks the link.

    Like :func:`feature_gate_for_symbol`, but returns ``None`` for
    resolver-only (always-defined) features. This is the precise predicate for
    "would requiring this intrinsic on a profile that excludes its feature
    cause an undefined-symbol link error?" — the basis for the frontend's
    compile-time profile-availability refusal.
    """
    feature = feature_gate_for_symbol(symbol)
    if feature is None or feature not in LINK_AFFECTING_FEATURES:
        return None
    return feature
