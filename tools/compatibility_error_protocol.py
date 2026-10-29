"""Closed CPython compatibility outcomes; runtime projection and audit authority.

This is an executable semantic protocol, not an exception-site exemption list.
Every outcome checks its applicability before it can emit NotImplementedError.
The audit verifies both complete projections and inventories all consumer calls.
Unrecognized contexts, modified projections and raw raises remain obligations.

--refresh-sources captures pin-bound source-body receipts for explicit review;
--check and --write remain offline. Receipt refresh is not semantic approval.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from functools import cache
from pathlib import Path
import re

import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools.generator_io import (  # noqa: E402
    generated_file_matches,
    generator_main,
    rustfmt_source,
    write_generated_text,
)
from tools.cpython_regrtest import (  # noqa: E402
    CPYTHON_SOURCES,
    CPythonSource,
    load_cpython_sources,
)

PYTHON_PATH = "src/molt/stdlib/_compatibility_errors.py"
RUST_PATH = "runtime/molt-runtime/src/builtins/compatibility_error.rs"
TARGETS = ("3.12", "3.13", "3.14")
SOURCE_RECEIPTS = "config/python_compatibility_error_sources.json"
PATHLIB_REFERENCES = {
    "3.12": ("Lib/pathlib.py",),
    "3.13": ("Lib/pathlib/_local.py", "Modules/posixmodule.c", "Python/fileutils.c"),
    "3.14": ("Lib/pathlib/__init__.py", "Modules/posixmodule.c", "Python/fileutils.c"),
}
REFERENCE_FILES = (
    "Lib/collections/__init__.py",
    "Lib/asyncio/events.py",
    "Lib/asyncio/windows_events.py",
    "Lib/asyncio/base_events.py",
    "Lib/asyncio/selector_events.py",
    "Lib/asyncio/proactor_events.py",
    "Objects/memoryobject.c",
    "Modules/arraymodule.c",
)


@dataclass(frozen=True)
class Outcome:
    operations: tuple[str, ...]
    versions: tuple[str, ...]
    platforms: tuple[str, ...]
    predicate: str
    reference: str
    witness: str
    pinned_source_paths: tuple[str, ...] = ()


OUTCOMES = {
    "ImportlibCacheTagUnavailable": Outcome(
        ("importlib.util.cache_from_source",),
        (*TARGETS, "3.15"),
        ("all",),
        "the live sys.implementation.cache_tag is None after source path admission",
        "Lib/importlib/_bootstrap_external.py:cache_from_source,_get_cached",
        "tests/differential/stdlib/importlib_module_spec.py",
        pinned_source_paths=("Lib/importlib/_bootstrap_external.py",),
    ),
    "RawIoBaseReadinto": Outcome(
        ("io.RawIOBase.readinto",),
        (*TARGETS, "3.15"),
        ("all",),
        "abstract default body is unconditionally undefined after receiver admission and keyword rejection, with unrestricted positional arity",
        "Modules/_io/iobase.c:rawiobase_readinto,rawiobase_methods",
        "tests/differential/stdlib/io_bufferedreader_readinto.py",
        pinned_source_paths=("Modules/_io/iobase.c",),
    ),
    "LoggingHandlerEmit": Outcome(
        ("logging.Handler.emit",),
        (*TARGETS, "3.15"),
        ("all",),
        "abstract default body is unconditionally undefined after Python method argument binding",
        "Lib/logging/__init__.py:Handler.emit",
        "tests/differential/stdlib/logging_percent_style_intrinsic.py",
        pinned_source_paths=("Lib/logging/__init__.py",),
    ),
    "PathGlobNonRelative": Outcome(
        ("pathlib.Path.glob", "pathlib.Path.rglob"),
        TARGETS,
        ("all",),
        "selected native or legacy callback parser produces a true non-relative fact in the version-specific root truth order, after type/normalization checks and before tail callbacks, selector validation, base encoding or traversal",
        "Lib/pathlib.py:Path.glob,rglob (3.12);Lib/pathlib/_local.py:Path.glob,rglob (3.13);Lib/pathlib/__init__.py:PurePath._parse_pattern,Path.glob,rglob (3.14);Lib/ntpath.py:splitroot;Lib/posixpath.py:splitroot",
        "tests/differential/stdlib/pathlib_glob_relative.py",
    ),
    "CounterFromkeys": Outcome(
        ("collections.Counter.fromkeys",),
        TARGETS,
        ("all",),
        "operation is unconditionally undefined, before inspecting arguments",
        "Lib/collections/__init__.py:Counter.fromkeys",
        "tests/differential/stdlib/collections_counter.py",
    ),
    "WindowsSignalHandler": Outcome(
        ("add_signal_handler", "remove_signal_handler"),
        TARGETS,
        ("win32",),
        "Windows loops inherit AbstractEventLoop signal methods",
        "Lib/asyncio/events.py:AbstractEventLoop;Lib/asyncio/windows_events.py",
        "tests/differential/stdlib/asyncio_signal_handler_semantics.py",
    ),
    "AbstractSignalHandler": Outcome(
        ("add_signal_handler", "remove_signal_handler"),
        TARGETS,
        ("all",),
        "abstract event-loop signal methods are unconditionally undefined",
        "Lib/asyncio/events.py:AbstractEventLoop",
        "tests/differential/stdlib/asyncio_signal_handler_semantics.py",
    ),
    "MemoryviewSubView": Outcome(
        ("getitem", "setitem", "iter", "count"),
        TARGETS,
        ("all",),
        "nonzero rank with fewer integer indices than dimensions",
        "Objects/memoryobject.c:memory_item,memory_item_multi,memory_ass_sub,memory_iter",
        "tests/differential/basic/memoryview_multidim.py",
    ),
    "MemoryviewSlice": Outcome(
        ("getitem", "setitem"),
        TARGETS,
        ("all",),
        "nonempty tuple of slices, or assignment of a slice to rank greater than one",
        "Objects/memoryobject.c:memory_subscript,memory_ass_sub",
        "tests/differential/basic/memoryview_multidim.py",
    ),
    "MemoryviewLookup": Outcome(
        ("index",),
        ("3.14",),
        ("all",),
        "rank greater than one",
        "Objects/memoryobject.c:memoryview_index_impl",
        "tests/differential/basic/memoryview_multidim.py",
    ),
    "MemoryviewFormatSyntax": Outcome(
        ("getitem", "setitem", "iter", "tolist", "count", "index"),
        TARGETS,
        ("all",),
        "format is not one byte optionally preceded by @",
        "Objects/memoryobject.c:adjust_fmt",
        "tests/differential/basic/memoryview_format_codes.py",
    ),
    "MemoryviewFormatScalar": Outcome(
        ("getitem", "setitem", "next", "tolist", "count", "index"),
        TARGETS,
        ("all",),
        "syntactically native format code outside bBhHiIlLqQnNefd?cP",
        "Objects/memoryobject.c:unpack_single,pack_single",
        "tests/differential/basic/memoryview_format_codes.py",
    ),
}

PUBLISHED_OUTCOMES = {
    "CounterFromkeys": (1, ("counter_fromkeys_error",)),
    "WindowsSignalHandler": (
        2,
        ("windows_add_signal_handler_error", "windows_remove_signal_handler_error"),
    ),
    "AbstractSignalHandler": (
        3,
        ("abstract_add_signal_handler_error", "abstract_remove_signal_handler_error"),
    ),
    "LoggingHandlerEmit": (4, ("logging_handler_emit_error",)),
}

PYTHON_SOURCE = (
    '''"""Intrinsic-backed CPython compatibility outcomes; generated by tools/compatibility_error_protocol.py."""
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_COMPATIBILITY_ERROR = _require_intrinsic("molt_compatibility_error")
'''
    + "".join(
        f"\n\ndef {function}():\n    return _MOLT_COMPATIBILITY_ERROR({tag})\n"
        for tag, functions in PUBLISHED_OUTCOMES.values()
        for function in functions
    )
    + "\n\ndel _require_intrinsic\n"
)


def _version_pattern(versions: Iterable[str]) -> str:
    """Project exact declared coordinates without discarding their major.

    Compress only contiguous minor ranges within the same major; grouping must
    never authorize the same minor under an undeclared major.
    """
    by_major: dict[int, set[int]] = {}
    for version in versions:
        major, minor = (int(part) for part in version.split("."))
        by_major.setdefault(major, set()).add(minor)
    if not by_major:
        raise ValueError("compatibility outcome needs a declared target coordinate")
    patterns = []
    for major, values in sorted(by_major.items()):
        minors = sorted(values)
        if len(minors) > 2 and minors == list(range(minors[0], minors[-1] + 1)):
            pattern = f"{minors[0]}..={minors[-1]}"
        else:
            pattern = " | ".join(str(minor) for minor in minors)
        patterns.append(f"({major}, {pattern})")
    return " | ".join(patterns)


RUST_SOURCE = (
    """// @generated by tools/compatibility_error_protocol.py. DO NOT EDIT.
use crate::{
    BuiltinExc, ExceptionSentinel, MoltObject, PyToken, obj_from_bits, raise_exception,
    runtime_state,
};

pub(crate) enum CompatibilityError<'a> {
    ImportlibCacheTagUnavailable {
        is_none: bool,
    },
    PathGlobNonRelative {
        non_relative: bool,
    },
    CounterFromkeys,
    WindowsSignalHandler,
    AbstractSignalHandler,
    RawIoBaseReadinto,
    LoggingHandlerEmit,
    MemoryviewSubView {
        rank: usize,
        indices: usize,
        tuple: bool,
        assignment: bool,
    },
    MemoryviewSlice {
        rank: usize,
        slices: usize,
        tuple: bool,
        assignment: bool,
    },
    MemoryviewLookup {
        rank: usize,
    },
    MemoryviewFormatSyntax {
        format: &'a str,
    },
    MemoryviewFormatScalar {
        format: &'a str,
    },
}

impl CompatibilityError<'_> {
    fn message(self, py: &PyToken<'_>) -> Result<String, &'static str> {
        let target = crate::object::ops_sys::runtime_target_python_info(runtime_state(py));
        let target_version = (target.major, target.minor);
        let supported = match &self {
            Self::RawIoBaseReadinto => matches!(target_version, __RAW_IO_VERSIONS__),
            Self::LoggingHandlerEmit => matches!(target_version, __LOGGING_HANDLER_VERSIONS__),
            Self::ImportlibCacheTagUnavailable { .. } => matches!(target_version, __CACHE_TAG_VERSIONS__),
            _ => matches!(target_version, __COMMON_VERSIONS__),
        };
        if !supported {
            return Err("unsupported compatibility-error target");
        }
        let message = match self {
            Self::ImportlibCacheTagUnavailable { is_none } if is_none => {
                "sys.implementation.cache_tag is None".to_owned()
            }
            Self::PathGlobNonRelative { non_relative } if non_relative => {
                "Non-relative patterns are unsupported".to_owned()
            }
            Self::CounterFromkeys => {
                "Counter.fromkeys() is undefined.  Use Counter(iterable) instead.".to_owned()
            }
            Self::WindowsSignalHandler if cfg!(target_os = "windows") => String::new(),
            Self::AbstractSignalHandler => String::new(),
            Self::RawIoBaseReadinto => String::new(),
            Self::LoggingHandlerEmit => {
                "emit must be implemented by Handler subclasses".to_owned()
            }
            Self::MemoryviewSubView {
                rank,
                indices,
                tuple,
                assignment,
            } if rank > 0 && indices < rank => {
                if tuple || assignment {
                    "sub-views are not implemented".to_owned()
                } else {
                    "multi-dimensional sub-views are not implemented".to_owned()
                }
            }
            Self::MemoryviewSlice {
                rank,
                slices,
                tuple,
                assignment,
            } if rank > 0 && slices > 0 && (tuple || (assignment && rank > 1)) => {
                if assignment {
                    "memoryview slice assignments are currently restricted to ndim = 1".to_owned()
                } else {
                    "multi-dimensional slicing is not implemented".to_owned()
                }
            }
            Self::MemoryviewLookup { rank } if rank > 1 && matches!(target_version, __MEMORYVIEW_LOOKUP_VERSIONS__) => {
                "multi-dimensional lookup is not implemented".to_owned()
            }
            Self::MemoryviewFormatSyntax { format }
                if format.strip_prefix('@').unwrap_or(format).len() != 1 =>
            {
                format!("memoryview: unsupported format {format}")
            }
            Self::MemoryviewFormatScalar { format }
                if format.len() == 1 && !b"bBhHiIlLqQnNefd?cP".contains(&format.as_bytes()[0]) =>
            {
                format!("memoryview: format {format} not supported")
            }
            _ => {
                return Err("inapplicable compatibility-error outcome");
            }
        };
        Ok(message)
    }

    pub(crate) fn raise<T: ExceptionSentinel>(self, py: &PyToken<'_>) -> T {
        match self.message(py) {
            Ok(message) => raise_exception(py, BuiltinExc::NotImplementedError, &message),
            Err(message) => raise_exception(py, BuiltinExc::SystemError, message),
        }
    }

    fn exception(self, py: &PyToken<'_>) -> u64 {
        let message = match self.message(py) {
            Ok(message) => message,
            Err(message) => return raise_exception(py, BuiltinExc::SystemError, message),
        };
        let ptr = crate::builtins::exceptions::alloc_exception(py, BuiltinExc::NotImplementedError, &message);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compatibility_error(outcome_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let outcome = match obj_from_bits(outcome_bits).as_int() {
__ARMS__
            _ => {
                return raise_exception(
                    py,
                    BuiltinExc::SystemError,
                    "inapplicable compatibility-error outcome",
                );
            }
        };
        outcome.exception(py)
    })
}
#[cfg(test)]
mod tests {
    use super::CompatibilityError;

    #[test]
    fn raw_readinto_and_logging_emit_target_support_does_not_widen_older_outcomes() {
        let transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for minor in 12..=15 {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(
                        CompatibilityError::RawIoBaseReadinto.message(py).as_deref(),
                        Ok("")
                    );
                    assert_eq!(
                        CompatibilityError::LoggingHandlerEmit.message(py).as_deref(),
                        Ok("emit must be implemented by Handler subclasses")
                    );
                    assert_eq!(
                        CompatibilityError::ImportlibCacheTagUnavailable { is_none: true }.message(py).as_deref(),
                        Ok("sys.implementation.cache_tag is None")
                    );
                    assert_eq!(
                        CompatibilityError::ImportlibCacheTagUnavailable { is_none: false }.message(py),
                        Err("inapplicable compatibility-error outcome")
                    );
                });
            }
            // Synthetic projection tests identity only, never target admission.
            for (coordinate, expected) in [((4, 0), true), ((3, 0), false)] {
                assert_eq!(matches!(coordinate, __SYNTHETIC_FOUR_ONLY__), expected);
            }
            for (major, minor) in [(3, 0), (4, 0), (4, 14)] {
                transaction.with_target_python(
                    py,
                    Some(crate::state::runtime_state::PythonVersionInfo {
                        major, minor, micro: 0, releaselevel: "final".to_string(), serial: 0,
                    }),
                    || {
                        assert_eq!(CompatibilityError::RawIoBaseReadinto.message(py),
                            Err("unsupported compatibility-error target"));
                        assert_eq!(CompatibilityError::CounterFromkeys.message(py),
                            Err("unsupported compatibility-error target"));
                        assert_eq!(CompatibilityError::MemoryviewLookup { rank: 2 }.message(py),
                            Err("unsupported compatibility-error target"));
                    },
                );
            }
            for minor in [11, 16] {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(
                        CompatibilityError::RawIoBaseReadinto.message(py),
                        Err("unsupported compatibility-error target"),
                    );
                    assert_eq!(
                        CompatibilityError::LoggingHandlerEmit.message(py),
                        Err("unsupported compatibility-error target"),
                    );
                    assert_eq!(
                        CompatibilityError::ImportlibCacheTagUnavailable { is_none: true }.message(py),
                        Err("unsupported compatibility-error target"),
                    );
                });
            }
            transaction.with_target_python_minor(py, 15, || {
                assert_eq!(
                    CompatibilityError::MemoryviewLookup { rank: 2 }.message(py),
                    Err("unsupported compatibility-error target"),
                );
                assert_eq!(
                    CompatibilityError::CounterFromkeys.message(py),
                    Err("unsupported compatibility-error target"),
                );
            });
        });
    }

    #[test]
    fn glob_outcome_rejects_relative_facts_and_unsupported_targets() {
        let transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for minor in 12..=14 {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(
                        CompatibilityError::PathGlobNonRelative { non_relative: true }
                            .message(py)
                            .as_deref(),
                        Ok("Non-relative patterns are unsupported"),
                    );
                    assert_eq!(
                        CompatibilityError::PathGlobNonRelative {
                            non_relative: false
                        }
                        .message(py),
                        Err("inapplicable compatibility-error outcome"),
                    );
                });
            }
            transaction.with_target_python_minor(py, 15, || {
                assert_eq!(
                    CompatibilityError::PathGlobNonRelative { non_relative: true }.message(py),
                    Err("unsupported compatibility-error target"),
                );
            });
        });
    }
}
""".replace(
        "__RAW_IO_VERSIONS__",
        _version_pattern(OUTCOMES["RawIoBaseReadinto"].versions),
    )
    .replace(
        "__LOGGING_HANDLER_VERSIONS__",
        _version_pattern(OUTCOMES["LoggingHandlerEmit"].versions),
    )
    .replace(
        "__CACHE_TAG_VERSIONS__",
        _version_pattern(OUTCOMES["ImportlibCacheTagUnavailable"].versions),
    )
    .replace(
        "__COMMON_VERSIONS__",
        _version_pattern(TARGETS),
    )
    .replace(
        "__MEMORYVIEW_LOOKUP_VERSIONS__",
        _version_pattern(OUTCOMES["MemoryviewLookup"].versions),
    )
    .replace("__SYNTHETIC_FOUR_ONLY__", _version_pattern(("4.0",)))
    .replace(
        "__ARMS__",
        "\n".join(
            f"            Some({tag}) => CompatibilityError::{outcome},"
            for outcome, (tag, _) in PUBLISHED_OUTCOMES.items()
        ),
    )
)


@cache
def _rust_projection(source: str) -> str:
    # Audits query the same projections for every source file. Format once per
    # source text through the shared, toolchain-keyed generator authority.
    return rustfmt_source(source, label=RUST_PATH)


def projections() -> dict[str, str]:
    return {PYTHON_PATH: PYTHON_SOURCE, RUST_PATH: _rust_projection(RUST_SOURCE)}


@dataclass(frozen=True)
class InventoryHit:
    line: int
    outcome: str
    proved: bool
    detail: str


def _reference_coordinates(
    root: Path, *, pins: dict[str, CPythonSource] | None = None
) -> dict[tuple[str, str], str | None]:
    """Source roles join the admitted references and their exact source pins."""
    from molt.verified_subset import load_verified_subset_policy

    policy = load_verified_subset_policy(root / "config/verified_subset.toml")
    references = dict(
        zip(policy.python_versions, policy.reference_cpython, strict=True)
    )
    if pins is None:
        pins = load_cpython_sources(root / CPYTHON_SOURCES.relative_to(_REPO_ROOT))
    for version in TARGETS:
        if pins[version].tag != "v" + references[version]:
            raise ValueError(f"CPython {version} reference and source pin disagree")
    coordinates: dict[tuple[str, str], str | None] = {
        (
            references[version] if path == "Objects/memoryobject.c" else version + ".0",
            path,
        ): pins[version].revision if path == "Objects/memoryobject.c" else None
        for version in TARGETS
        for path in (
            *REFERENCE_FILES,
            *PATHLIB_REFERENCES[version],
            "Lib/ntpath.py",
            "Lib/posixpath.py",
        )
    }
    coordinates.update(
        {
            (pins[version].tag.removeprefix("v"), path): pins[version].revision
            for facts in OUTCOMES.values()
            for version in facts.versions
            for path in facts.pinned_source_paths
        }
    )
    return coordinates


def _reference_url(version: str, path: str, revision: str | None) -> str:
    return (
        "https://raw.githubusercontent.com/python/cpython/"
        + (revision or "v" + version)
        + "/"
        + path
    )


def _receipt_sources(receipt: object) -> dict[tuple[str, str], dict[str, str]]:
    if not isinstance(receipt, dict) or receipt.get("schema_version") != 1:
        raise ValueError("invalid compatibility source receipt schema")
    rows = receipt.get("sources")
    if not isinstance(rows, list):
        raise ValueError("invalid compatibility source rows")
    result = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("invalid compatibility source row")
        version, path, revision = (
            row.get("version"),
            row.get("path"),
            row.get("revision"),
        )
        if (
            not isinstance(version, str)
            or re.fullmatch(r"3\.\d+\.\d+", version) is None
            or not isinstance(path, str)
            or not path
            or path.startswith("/")
            or any(part in ("", ".", "..") for part in path.split("/"))
            or (
                revision is not None
                and (
                    not isinstance(revision, str)
                    or re.fullmatch(r"[0-9a-f]{40}", revision) is None
                )
            )
            or row.get("url") != _reference_url(version, path, revision)
            or not isinstance(row.get("sha256"), str)
            or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            or (version, path) in result
        ):
            raise ValueError(
                "invalid compatibility source coordinate or content identity"
            )
        result[version, path] = row
    return result


def capture_reference_sources(
    version: str, paths: tuple[str, ...], revision: str | None, git_url: str
) -> dict[str, bytes]:
    """Read exact Git blobs using the existing guarded CPython capture owner."""
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from tools.gen_stdlib_module_union import _git, _git_blob_id

    with OwnedTemporaryDirectory(prefix="molt-compatibility-sources-") as scratch:
        repo = Path(scratch)
        _git(["init", "--quiet", "--bare", str(repo)], cwd=repo)

        def git(*args: str) -> bytes:
            return _git(["-C", str(repo), *args], cwd=repo)

        tag = "refs/tags/v" + version
        git("remote", "add", "origin", git_url)
        git(
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth=1",
            "--filter=blob:none",
            "origin",
            f"+{tag}:{tag}",
        )
        observed = git("rev-parse", "--verify", tag + "^{commit}").decode().strip()
        if re.fullmatch(r"[0-9a-f]{40}", observed) is None or (
            revision is not None and observed != revision
        ):
            raise ValueError(
                f"CPython {version} tag does not match its pinned revision"
            )

        def blob(path: str) -> bytes:
            object_id = (
                git("rev-parse", "--verify", f"{observed}:{path}").decode().strip()
            )
            data = git("cat-file", "blob", object_id)
            if _git_blob_id(data) != object_id:
                raise ValueError(
                    f"CPython {version} {path}: blob content identity differs"
                )
            return data

        header = blob("Include/patchlevel.h").decode("utf-8")
        numbers = [
            re.search(
                r"^#define\s+PY_" + field + r"_VERSION\s+(\d+)\b", header, re.MULTILINE
            )
            for field in ("MAJOR", "MINOR", "MICRO")
        ]
        if (
            any(match is None for match in numbers)
            or ".".join(match.group(1) for match in numbers if match is not None)
            != version
        ):
            raise ValueError(
                f"CPython {version} source release does not match its coordinate"
            )
        return {path: blob(path) for path in paths}


def refresh_sources(
    fetch: Callable[
        [str, tuple[str, ...], str | None, str], dict[str, bytes]
    ] = capture_reference_sources,
    *,
    root: Path = ROOT,
) -> int:
    """Capture all inputs before atomically replacing the receipt, never relabel it."""
    from molt.toolchain_identity import (
        capture_stable_regular_file,
        verify_stable_regular_file_identity,
    )

    policy_path = root / "config/verified_subset.toml"
    pins_path = root / CPYTHON_SOURCES.relative_to(_REPO_ROOT)
    inputs = [
        capture_stable_regular_file(path, label="compatibility source authority")[0]
        for path in (policy_path, pins_path)
    ]
    pins = load_cpython_sources(pins_path)
    expected = _reference_coordinates(root, pins=pins)
    for identity in inputs:
        verify_stable_regular_file_identity(
            identity, label="compatibility source authority", hash_content=True
        )
    destination = root / SOURCE_RECEIPTS
    try:
        destination.lstat()
    except FileNotFoundError:
        prior_identity, prior_bytes = None, None
    else:
        prior_identity, prior_bytes = capture_stable_regular_file(
            destination, label="compatibility source receipt"
        )
    prior = _receipt_sources(json.loads(prior_bytes)) if prior_bytes is not None else {}
    rows = []
    versions = sorted(
        {version for version, _path in expected},
        key=lambda version: tuple(map(int, version.split("."))),
    )
    for version in versions:
        paths = tuple(sorted(path for current, path in expected if current == version))
        revisions = {expected[version, path] for path in paths}
        if len(revisions) != 1:
            raise ValueError(f"CPython {version} has conflicting source revisions")
        revision = next(iter(revisions))
        minor = version.rsplit(".", 1)[0]
        fetched = fetch(version, paths, revision, pins[minor].git_url)
        if set(fetched) != set(paths) or any(
            not isinstance(data, bytes) for data in fetched.values()
        ):
            raise ValueError(f"CPython {version} capture is incomplete")
        for path in paths:
            row = {
                "version": version,
                "url": _reference_url(version, path, revision),
                "path": path,
                "sha256": hashlib.sha256(fetched[path]).hexdigest(),
            }
            if revision is not None:
                row["revision"] = revision
            previous = prior.get((version, path))
            if previous is not None and previous != row:
                raise ValueError(
                    f"CPython {version} {path}: unchanged coordinate differs from its receipt; nothing was written"
                )
            rows.append(row)
    text = json.dumps({"schema_version": 1, "sources": rows}, indent=2) + "\n"
    for identity in inputs:
        verify_stable_regular_file_identity(
            identity, label="compatibility source authority", hash_content=True
        )
    if prior_identity is not None:
        verify_stable_regular_file_identity(
            prior_identity, label="compatibility source receipt", hash_content=True
        )
    else:
        try:
            destination.lstat()
        except FileNotFoundError:
            pass
        else:
            raise ValueError(
                "compatibility source receipt appeared during capture; nothing was written"
            )
    if not generated_file_matches(destination, text):
        write_generated_text(destination, text)
        print(f"wrote {SOURCE_RECEIPTS}; now run --write")
    return 0


def source_errors(root: Path) -> list[str]:
    """Pinned CPython source receipts and differential witnesses for every outcome."""
    errors = []
    try:
        sources = _receipt_sources(
            json.loads((root / SOURCE_RECEIPTS).read_text(encoding="utf-8"))
        )
        expected_coordinates = _reference_coordinates(root)
        valid = sources.keys() == expected_coordinates.keys() and all(
            row.get("revision") == revision
            and row["url"] == _reference_url(version, path, revision)
            for (version, path), revision in expected_coordinates.items()
            for row in (sources[version, path],)
        )
        if not valid:
            errors.append("compatibility source coordinates are incomplete or unpinned")
    except (OSError, json.JSONDecodeError, TypeError):
        errors.append("compatibility source receipts are missing or malformed")
    except (ValueError, KeyError):
        errors.append("compatibility source coordinates are incomplete or unpinned")
    for outcome, facts in OUTCOMES.items():
        if not (root / facts.witness).is_file():
            errors.append(f"missing compatibility witness: {outcome}")
    return errors


def projection_errors(root: Path) -> list[str]:
    errors = source_errors(root)
    for name, expected in projections().items():
        # Canonical Rust is emitted in rustfmt form. The shared generator I/O
        # authority normalizes checkout newlines only; predicates and every
        # diagnostic byte remain exact, with no local equivalence parser.
        if not generated_file_matches(root / name, expected):
            errors.append(f"compatibility projection differs: {name}")
    return errors


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    errors = source_errors(ROOT)
    if errors:
        raise ValueError(
            "compatibility error protocol sources invalid:\n"
            + "\n".join(errors)
            + "\nFor reviewed canonical pin changes, capture body receipts with "
            "python tools/compatibility_error_protocol.py --refresh-sources; "
            "changed hashes still require semantic review."
        )
    return {ROOT / name: text for name, text in projections().items()}


def describe(outcome: str) -> str:
    facts = OUTCOMES[outcome]
    availability = ",".join(
        operation
        + "@"
        + "/".join(
            ("3.14",)
            if outcome.startswith("Memoryview") and operation in ("count", "index")
            else facts.versions
        )
        for operation in facts.operations
    )
    return (
        f"{outcome}; operations={','.join(facts.operations)}; "
        f"versions={','.join(facts.versions)}; platforms={','.join(facts.platforms)}; "
        f"availability={availability}; predicate={facts.predicate}; "
        f"CPython={facts.reference}; witness={facts.witness}"
    )


def python_inventory(text: str, proved: bool) -> list[InventoryHit]:
    # Every hit resolves through an import of _compatibility_errors, so a file
    # that never names the module has none; skip parsing it.
    if "_compatibility_errors" not in text:
        return []
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return []
    surfaces = {
        function: outcome
        for outcome, (_, functions) in PUBLISHED_OUTCOMES.items()
        for function in functions
    }
    aliases = {
        alias.asname or alias.name: surfaces.get(alias.name, "dynamic")
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module == "_compatibility_errors"
        for alias in node.names
    }
    modules = {
        alias.asname or alias.name
        for node in ast.walk(tree)
        if isinstance(node, ast.Import)
        for alias in node.names
        if alias.name == "_compatibility_errors"
    }
    # Shadowing invalidates symbol resolution. Unknown/dynamic contexts are debt,
    # including callers that retain the name but replace the imported emitter.
    shadowed = {
        node.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)
    }
    shadowed.update(node.arg for node in ast.walk(tree) if isinstance(node, ast.arg))
    shadowed.update(
        node.name
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))
    )
    hits = []
    direct_callees = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        symbol = None
        outcome = "dynamic"
        if isinstance(node.func, ast.Name) and node.func.id in aliases:
            symbol = node.func.id
            outcome = aliases[symbol]
            direct_callees.add(id(node.func))
        elif (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id in modules
        ):
            symbol = node.func.value.id
            outcome = surfaces.get(node.func.attr, "dynamic")
        if symbol is None:
            continue
        admitted = outcome in OUTCOMES and not node.args and not node.keywords
        valid = proved and admitted and symbol not in shadowed
        hits.append(
            InventoryHit(
                node.lineno,
                outcome,
                valid,
                describe(outcome)
                if valid
                else f"unproved compatibility call: {outcome}",
            )
        )
    for node in ast.walk(tree):
        if (
            isinstance(node, ast.Name)
            and isinstance(node.ctx, ast.Load)
            and node.id in aliases
            and id(node) not in direct_callees
        ):
            hits.append(
                InventoryHit(
                    node.lineno,
                    "dynamic",
                    False,
                    "compatibility emitter escapes static outcome classification",
                )
            )
    return hits


def rust_inventory(text: str, code: str, proved: bool) -> list[InventoryHit]:
    hits = []
    imports = list(
        re.finditer(
            r"\buse\s+(?:crate\s*::\s*builtins\s*::\s*)?compatibility_error\s*::\s*CompatibilityError(?:\s+as\s+(\w+))?\s*;",
            code,
        )
    )
    aliases = {match.group(1) or "CompatibilityError" for match in imports}
    names = "|".join(
        re.escape(name) for name in sorted(aliases | {"CompatibilityError"})
    )
    for match in re.finditer(
        r"\b(" + names + r")\s*::\s*([A-Za-z_][A-Za-z_0-9]*)", code
    ):
        outcome = match.group(2)
        valid = (
            proved
            and match.group(1) in aliases
            and outcome in OUTCOMES
            and outcome not in PUBLISHED_OUTCOMES
        )
        hits.append(
            InventoryHit(
                text.count("\n", 0, match.start()) + 1,
                outcome,
                valid,
                describe(outcome)
                if valid
                else f"unproved compatibility call: {outcome}",
            )
        )
    return hits


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="check offline projections")
    mode.add_argument("--write", action="store_true", help="rewrite stale projections")
    mode.add_argument(
        "--refresh-sources",
        action="store_true",
        help="capture pin-bound source receipts for review without approving semantics",
    )
    args = parser.parse_args(argv)
    if args.refresh_sources:
        return refresh_sources(root=ROOT)
    return generator_main(
        generated_outputs, ["--write" if args.write else "--check"], description=__doc__
    )


if __name__ == "__main__":
    raise SystemExit(main())
