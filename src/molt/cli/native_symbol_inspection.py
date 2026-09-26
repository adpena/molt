"""Shared native object/archive symbol facts and reader-generation custody.

Admission, source-extension validation and backend artifact caching consume this
same inspection authority. Backend cache publication and locking live elsewhere.
"""

from __future__ import annotations

import contextlib
import functools
import hashlib
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Sequence

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.cli.llvm_wasi_tools import llvm_tool_candidates
from molt.cli.static_archive_identity import (
    StaticArchiveMemberIdentity,
    static_archive_member_identities,
)
from molt.file_hashing import content_change_time_ns
from molt.source_root import compiler_source_root
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_executable_probe,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    WasmLlvmNmVerification,
    verify_wasm_llvm_nm,
)


_NativeObjectSymbolSets = tuple[set[str], set[str]]
_NATIVE_SYMBOL_FACTS_PROTOCOL = "molt.native-symbol-facts.v2"


@dataclass(frozen=True, slots=True)
class _NativeGlobalSymbolFacts:
    defined: frozenset[str]
    undefined: frozenset[str]
    defined_functions: frozenset[str]
    # Weak undefined references are neither providers nor required link inputs.
    weak_undefined: frozenset[str] = frozenset()
    artifact_digest: str | None = None
    members: tuple[_NativeArchiveMemberSymbolFacts, ...] | None = None
    weak_defined: frozenset[str] = frozenset()

    def __post_init__(self) -> None:
        # For archives these are projections, never separately authored facts.
        if self.members is not None:
            for name in _SYMBOL_SET_FIELDS:
                object.__setattr__(
                    self,
                    name,
                    frozenset().union(
                        *(getattr(member.symbols, name) for member in self.members)
                    ),
                )

    def symbol_sets(self) -> _NativeObjectSymbolSets:
        return set(self.defined), set(self.undefined)


@dataclass(frozen=True, slots=True)
class _NativeArchiveMemberSymbolFacts:
    identity: StaticArchiveMemberIdentity
    symbols: _NativeGlobalSymbolFacts


_SYMBOL_SET_FIELDS = (
    "defined",
    "undefined",
    "defined_functions",
    "weak_undefined",
    "weak_defined",
)


def _symbol_table_payload(facts: _NativeGlobalSymbolFacts) -> dict[str, object]:
    return {name: sorted(getattr(facts, name)) for name in _SYMBOL_SET_FIELDS}


def _symbol_facts_payload(facts: _NativeGlobalSymbolFacts) -> dict[str, object]:
    if facts.members is None:
        return {"object": _symbol_table_payload(facts)}
    return {
        "members": [
            {
                "ordinal": item.identity.ordinal,
                "name": item.identity.member.name,
                "offset": item.identity.member.content_offset,
                "size": item.identity.member.size,
                "sha256": item.identity.sha256,
                "symbols": _symbol_table_payload(item.symbols),
            }
            for item in facts.members
        ]
    }


def _decode_symbol_table(value: object) -> _NativeGlobalSymbolFacts | None:
    if not isinstance(value, dict) or set(value) != set(_SYMBOL_SET_FIELDS):
        return None
    tables: list[frozenset[str]] = []
    for name in _SYMBOL_SET_FIELDS:
        symbols = value[name]
        if not isinstance(symbols, list) or not all(
            isinstance(symbol, str) and symbol and not any(c.isspace() for c in symbol)
            for symbol in symbols
        ):
            return None
        if symbols != sorted(set(symbols)):
            return None
        tables.append(frozenset(symbols))
    facts = _NativeGlobalSymbolFacts(
        tables[0], tables[1], tables[2], tables[3], weak_defined=tables[4]
    )
    return (
        facts
        if (facts.defined_functions | facts.weak_defined) <= facts.defined
        else None
    )


def _decode_symbol_facts(
    value: object,
    *,
    artifact_digest: str,
    members: tuple[StaticArchiveMemberIdentity, ...] | None,
) -> _NativeGlobalSymbolFacts | None:
    if not isinstance(value, dict):
        return None
    if members is None:
        if set(value) != {"object"}:
            return None
        facts = _decode_symbol_table(value["object"])
        return (
            None if facts is None else replace(facts, artifact_digest=artifact_digest)
        )
    if set(value) != {"members"} or not isinstance(value["members"], list):
        return None
    rows = value["members"]
    if len(rows) != len(members):
        return None
    bound: list[_NativeArchiveMemberSymbolFacts] = []
    for row, identity in zip(rows, members):
        if not isinstance(row, dict) or set(row) != {
            "ordinal",
            "name",
            "offset",
            "size",
            "sha256",
            "symbols",
        }:
            return None
        if any(type(row[key]) is not int for key in ("ordinal", "offset", "size")):
            return None
        if (row["ordinal"], row["name"], row["offset"], row["size"], row["sha256"]) != (
            identity.ordinal,
            identity.member.name,
            identity.member.content_offset,
            identity.member.size,
            identity.sha256,
        ):
            return None
        symbols = _decode_symbol_table(row["symbols"])
        if symbols is None:
            return None
        bound.append(_NativeArchiveMemberSymbolFacts(identity, symbols))
    return _NativeGlobalSymbolFacts(
        frozenset(),
        frozenset(),
        frozenset(),
        artifact_digest=artifact_digest,
        members=tuple(bound),
    )


@dataclass(frozen=True, slots=True)
class NativeSymbolRequirement:
    """Typed consumer admission included in every reader/fact cache identity."""

    function_prefix: str | None = None
    excluded_functions: frozenset[str] = frozenset()

    def accepts(self, facts: _NativeGlobalSymbolFacts) -> bool:
        if self.function_prefix is None:
            return True
        return any(
            name.startswith(self.function_prefix)
            and name not in self.excluded_functions
            for name in facts.defined_functions
        )

    def cache_identity(self) -> str:
        return json.dumps(
            {
                "function_prefix": self.function_prefix,
                "excluded_functions": sorted(self.excluded_functions),
            },
            sort_keys=True,
            separators=(",", ":"),
        )


class NativeSymbolInspectionError(OSError):
    """Required symbol evidence was unavailable, never an empty symbol table."""

    def __init__(self, path: Path, attempts: Sequence[str]) -> None:
        self.path = path
        self.attempts = tuple(attempts)
        super().__init__(
            f"Cannot inspect native symbols for {path}: " + "; ".join(self.attempts)
        )


@dataclass(frozen=True, slots=True)
class _NativeSymbolReaderCandidate:
    command: tuple[str, ...]
    executable_identity: StableRegularFileIdentity | None = None
    admission_error: str | None = None

    def cache_identity(self) -> str:
        return json.dumps(
            {
                "command": self.command,
                "sha256": (
                    None
                    if self.executable_identity is None
                    else self.executable_identity.sha256
                ),
                "error": self.admission_error,
            },
            sort_keys=True,
            separators=(",", ":"),
        )


@dataclass(frozen=True, slots=True)
class _NativeSymbolReader:
    candidates: tuple[_NativeSymbolReaderCandidate, ...]
    input_identity: tuple[str, ...]
    requirement: NativeSymbolRequirement
    cache_identity: tuple[str, ...] = field(init=False)

    def __post_init__(self) -> None:
        # Object sidecars and central archive facts share one parsing protocol
        # generation, independently of their persistence envelope versions.
        object.__setattr__(
            self,
            "cache_identity",
            (_NATIVE_SYMBOL_FACTS_PROTOCOL, *self.input_identity),
        )


@functools.lru_cache(maxsize=8)
def _cached_wasm_llvm_nm_verification(
    source_root: Path,
    environment_items: tuple[tuple[str, str], ...],
) -> WasmLlvmNmVerification:
    return verify_wasm_llvm_nm(source_root, environ=dict(environment_items))


def _verified_wasm_llvm_nm(
    environment: dict[str, str],
) -> WasmLlvmNmVerification:
    environment_items = tuple(sorted(environment.items()))
    source_root = compiler_source_root()
    verification = _cached_wasm_llvm_nm_verification(source_root, environment_items)
    try:
        with stable_executable_probe(
            verification.path,
            label="verified WebAssembly llvm-nm",
            identity=verification.executable_identity,
        ):
            pass
    except (OSError, ValueError):
        _cached_wasm_llvm_nm_verification.cache_clear()
        verification = _cached_wasm_llvm_nm_verification(source_root, environment_items)
    return verification


@functools.lru_cache(maxsize=64)
def _cached_symbol_reader_entrypoint_identity(
    path_text: str,
) -> tuple[Path, StableRegularFileIdentity]:
    with stable_executable_probe(Path(path_text), label="native symbol reader") as (
        entrypoint,
        identity,
    ):
        return entrypoint, identity


def _native_symbol_reader_candidate(
    command: tuple[str, ...],
) -> _NativeSymbolReaderCandidate:
    if not command:
        return _NativeSymbolReaderCandidate(command, admission_error="empty command")
    try:
        entrypoint, identity = _cached_symbol_reader_entrypoint_identity(command[0])
        with stable_executable_probe(
            entrypoint,
            label="native symbol reader",
            identity=identity,
        ):
            pass
    except (OSError, ValueError):
        _cached_symbol_reader_entrypoint_identity.cache_clear()
        try:
            entrypoint, identity = _cached_symbol_reader_entrypoint_identity(command[0])
        except (OSError, ValueError) as refreshed_exc:
            return _NativeSymbolReaderCandidate(
                command,
                admission_error=f"{type(refreshed_exc).__name__}: {refreshed_exc}",
            )
    return _NativeSymbolReaderCandidate(
        (str(entrypoint), *command[1:]),
        executable_identity=identity,
    )


def _native_symbol_reader(
    *,
    nm_command: Sequence[str] | None,
    target_triple: str | None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
) -> _NativeSymbolReader:
    command = tuple(nm_command) if nm_command is not None else None
    if target_triple is None or not target_triple.lower().startswith("wasm"):
        commands = (
            (command,)
            if command is not None
            else tuple((candidate,) for candidate in _nm_candidate_binaries())
        )
        candidates = tuple(_native_symbol_reader_candidate(item) for item in commands)
        return _NativeSymbolReader(
            candidates,
            (
                *tuple(candidate.cache_identity() for candidate in candidates),
                requirement.cache_identity(),
            ),
            requirement,
        )

    if command is not None and len(command) != 1:
        raise NativeSymbolInspectionError(
            Path(command[0] if command else "llvm-nm"),
            [
                "WASM symbol inspection requires one llvm-nm executable without arguments"
            ],
        )
    environment = dict(os.environ)
    configured = environment.get("MOLT_LLVM_NM", "").strip()
    try:
        if command is not None:
            command_environment = dict(environment)
            command_environment["MOLT_LLVM_NM"] = command[0]
            command_verification = _verified_wasm_llvm_nm(command_environment)
            if configured:
                configured_verification = _verified_wasm_llvm_nm(environment)
                if (
                    os.path.normcase(os.fspath(configured_verification.path.absolute()))
                    != os.path.normcase(os.fspath(command_verification.path.absolute()))
                    or configured_verification.fact.sha256
                    != command_verification.fact.sha256
                ):
                    raise LlvmToolchainConfigError(
                        "captured nm command disagrees with MOLT_LLVM_NM"
                    )
            verification = command_verification
        else:
            verification = _verified_wasm_llvm_nm(environment)
    except LlvmToolchainConfigError as exc:
        raise NativeSymbolInspectionError(
            Path(command[0] if command else configured or "llvm-nm"),
            [str(exc)],
        ) from exc
    candidate = _NativeSymbolReaderCandidate(
        (str(verification.path),),
        executable_identity=verification.executable_identity,
    )
    return _NativeSymbolReader(
        (candidate,),
        (
            "wasm-llvm-nm",
            str(verification.path),
            verification.fact.version,
            verification.fact.sha256,
            configured,
            requirement.cache_identity(),
        ),
        requirement,
    )


def _require_unchanged_symbol_reader(
    artifact: Path,
    reader: _NativeSymbolReader,
) -> None:
    for candidate in reader.candidates:
        if candidate.executable_identity is None:
            continue
        try:
            with stable_executable_probe(
                Path(candidate.command[0]),
                label="native symbol reader",
                identity=candidate.executable_identity,
            ):
                pass
        except (OSError, ValueError) as exc:
            raise NativeSymbolInspectionError(
                artifact,
                ["verified symbol reader changed during symbol inspection", str(exc)],
            ) from exc


def _native_symbol_artifact_identity(path: Path) -> StableRegularFileIdentity:
    """Use the shared direct-file content and mutation identity authority."""
    try:
        return stable_regular_file_identity(
            path.resolve(strict=True), label="native symbol artifact"
        )
    except (OSError, ValueError) as error:
        raise NativeSymbolInspectionError(path, [str(error)]) from error


def _require_unchanged_symbol_artifact(
    path: Path, identity: StableRegularFileIdentity
) -> None:
    try:
        if path.resolve(strict=True) != identity.path.resolve(strict=True):
            raise ValueError("artifact path no longer names its captured generation")
        verify_stable_regular_file_identity(identity, label="native symbol artifact")
    except (OSError, ValueError) as error:
        raise NativeSymbolInspectionError(
            path,
            [
                "artifact changed during symbol inspection; no facts were published",
                str(error),
            ],
        ) from error


_NativeObjectSymbolCacheKey = tuple[
    str, int, int, int, str, str, str, str, str, tuple[str, ...]
]
_NativeArchiveSymbolCacheKey = tuple[
    str, int, int, int, str, str, str, str, tuple[str, ...], str
]
_NATIVE_OBJECT_SYMBOL_SETS_CACHE: dict[
    _NativeObjectSymbolCacheKey,
    _NativeGlobalSymbolFacts,
] = {}
_NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT = 256
_NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION = 6
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT = 32
_NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION = 5
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE: dict[
    _NativeArchiveSymbolCacheKey,
    _NativeGlobalSymbolFacts,
] = {}


def _target_uses_macho_symbol_decoration(target_triple: str | None) -> bool:
    from molt.cli.native_link_plan import (
        NativeObjectFormat,
        resolve_native_target_spec,
        target_is_wasm,
    )

    if target_triple is not None and target_is_wasm(target_triple):
        return False
    return (
        resolve_native_target_spec(target_triple).object_format
        is NativeObjectFormat.MACHO
    )


def _normalize_native_symbol_name(
    name: str,
    *,
    target_triple: str | None = None,
) -> str:
    if _target_uses_macho_symbol_decoration(target_triple) and name.startswith("_"):
        return name[1:]
    return name


def _symbol_normalization_target(target_triple: str | None) -> str:
    from molt.cli.native_link_plan import resolve_native_target_spec

    target = (
        resolve_native_target_spec(None).triple
        if target_triple is None
        else target_triple.strip().lower()
    )
    return f"target:{target}"


def _native_nm_command(nm_command: Sequence[str], path: Path) -> list[str]:
    return [*nm_command, "-g", str(path)]


def _nm_line_reports_no_symbols(
    line: str,
    result: subprocess.CompletedProcess[str],
    archive_member_names: frozenset[str] | None = None,
) -> bool:
    argv = result.args
    if isinstance(argv, str) or not argv:
        return False
    artifact = str(argv[-1])
    tool = str(argv[0])
    if line == "no symbols":
        return not archive_member_names
    for name in {tool, Path(tool).name}:
        if line.startswith(f"{name}: "):
            line = line[len(name) + 2 :]
            break
    if not line.endswith(": no symbols"):
        return False
    owner = line[: -len(": no symbols")]
    for prefix in {artifact, Path(artifact).name}:
        if owner == prefix:
            return not archive_member_names
        if not owner.startswith(prefix):
            continue
        suffix = owner[len(prefix) :]
        if suffix.startswith("(") and suffix.endswith(")"):
            member = suffix[1:-1]
        elif suffix.startswith(":"):
            # LLVM uses archive:member, GNU/BSD use archive(member). Match the
            # known input first: colons inside Windows paths are not separators.
            member = suffix[1:]
        else:
            continue
        # Diagnostic separators (': ') and missing members are not archive
        # ownership. Do not promote a nested error to a benign empty-member row.
        if member and member == member.strip() and ": " not in member:
            return archive_member_names is None or member in archive_member_names
    return False


def _nm_result_reports_no_symbols(result: subprocess.CompletedProcess[str]) -> bool:
    # rc=1 is accepted only for a wholly empty artifact; partial archive output
    # plus a failed member must never be promoted into complete evidence.
    lines = [
        line.strip()
        for line in f"{result.stdout}\n{result.stderr}".splitlines()
        if line.strip()
    ]
    return bool(lines) and all(
        _nm_line_reports_no_symbols(line, result) for line in lines
    )


def _nm_read_timeout(default: float) -> float:
    """Resolve the ``nm``/``llvm-nm`` object-symbol read timeout.

    ``llvm-nm -g <object>`` is a bounded, read-only, non-spawning leaf tool, but
    on slow volumes (network/OneDrive-backed checkouts, antivirus-scanned exFAT
    build roots) a single spawn + read can exceed a few seconds. Expose the
    ceiling via ``MOLT_NM_TIMEOUT_SEC`` so an operator on a slow host can raise
    it without patching; the tight default keeps healthy hosts fast.
    """
    raw = os.environ.get("MOLT_NM_TIMEOUT_SEC")
    if raw:
        try:
            value = float(raw)
        except ValueError:
            value = 0.0
        if value > 0:
            return value
    return default


def _read_native_global_symbol_facts(
    path: Path,
    *,
    timeout: float,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    _reader: _NativeSymbolReader | None = None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
    archive_members: tuple[StaticArchiveMemberIdentity, ...] | None = None,
) -> _NativeGlobalSymbolFacts:
    reader = _reader or _native_symbol_reader(
        nm_command=nm_command,
        target_triple=target_triple,
        requirement=requirement,
    )
    if not reader.candidates:
        raise NativeSymbolInspectionError(
            path, ["no nm/llvm-nm candidate is available"]
        )
    read_timeout = _nm_read_timeout(timeout)
    _require_unchanged_symbol_reader(path, reader)
    failures: list[str] = []
    primary: BaseException | None = None
    for candidate in reader.candidates:
        command = candidate.command
        if candidate.admission_error is not None:
            failures.append(f"{command!r}: {candidate.admission_error}")
            continue
        assert candidate.executable_identity is not None
        execution_error: BaseException | None = None
        result: subprocess.CompletedProcess[str] | None = None
        try:
            # Reading a static object's global symbol table is a leaf,
            # non-spawning, read-only operation: it can neither orphan a process
            # tree nor run away on memory, so it does NOT go through the
            # process-tree memory guard. Guarding it here regressed on slow
            # hosts, where the guard's per-call repo-scoped orphan cleanup blew
            # past the read timeout and killed a healthy `llvm-nm` mid-output
            # (rc=124), stalling every source-recompiled extension seal at the
            # object-fact step. A plain subprocess timeout is the correct bound.
            with stable_executable_probe(
                Path(command[0]),
                label="native symbol reader",
                identity=candidate.executable_identity,
            ) as (entrypoint, _identity):
                try:
                    result = _run_completed_command(
                        _native_nm_command((str(entrypoint), *command[1:]), path),
                        capture_output=True,
                        timeout=read_timeout,
                        env=None,
                        cwd=path.parent,
                        memory_guard_prefix=None,
                        errors="strict",
                    )
                except (OSError, subprocess.SubprocessError, UnicodeError) as error:
                    execution_error = error
        except (OSError, ValueError) as error:
            raise NativeSymbolInspectionError(
                path,
                ["verified symbol reader changed during symbol inspection", str(error)],
            ) from error
        if execution_error is not None:
            if primary is None:
                primary = execution_error
            failures.append(
                f"{command!r}: {type(execution_error).__name__}: {execution_error}"
            )
            continue
        assert result is not None
        try:
            facts = _parse_native_nm_result(
                result,
                path=path,
                archive_members=archive_members,
                target_triple=target_triple,
            )
        except ValueError as error:
            if primary is None:
                primary = error
            failures.append(f"{command!r}: {error}")
            continue
        if reader.requirement.accepts(facts):
            return facts
        failures.append(
            f"{command!r}: no function definitions satisfy consumer requirement "
            f"{reader.requirement.cache_identity()}"
        )
    raise NativeSymbolInspectionError(path, failures) from primary


def _parse_native_nm_result(
    result: subprocess.CompletedProcess[str],
    *,
    path: Path,
    archive_members: tuple[StaticArchiveMemberIdentity, ...] | None,
    target_triple: str | None,
) -> _NativeGlobalSymbolFacts:
    if (
        archive_members is None
        and result.returncode in {0, 1}
        and _nm_result_reports_no_symbols(result)
    ):
        return _NativeGlobalSymbolFacts(frozenset(), frozenset(), frozenset())
    names = (
        None
        if archive_members is None
        else frozenset(item.member.name for item in archive_members)
    )
    if result.returncode != 0 or any(
        line.strip() and not _nm_line_reports_no_symbols(line.strip(), result, names)
        for line in result.stderr.splitlines()
    ):
        raise ValueError(
            f"exit {result.returncode}; stdout={result.stdout[:2048]!r}; "
            f"stderr={result.stderr[:2048]!r}"
        )
    output = "\n".join(
        line
        for line in result.stdout.splitlines()
        if not _nm_line_reports_no_symbols(line.strip(), result, names)
    )
    if archive_members is None:
        return _parse_native_nm_global_symbol_facts(output, target_triple=target_triple)
    return _parse_native_archive_symbol_facts(
        output, path=path, members=archive_members, target_triple=target_triple
    )


def _native_object_symbol_facts_sidecar_path(path: Path) -> Path:
    return path.with_suffix(".symbols.json")


def _native_object_symbol_cache_key(
    path: Path,
    object_digest: str,
    *,
    reader_identity: tuple[str, ...],
    target_triple: str | None,
) -> _NativeObjectSymbolCacheKey | None:
    try:
        resolved = path.resolve()
        stat = path.stat()
    except OSError:
        return None
    return (
        os.fspath(resolved),
        int(stat.st_size),
        int(stat.st_mtime_ns),
        int(getattr(stat, "st_ctime_ns", 0)),
        object_digest,
        os.environ.get("MOLT_TARGET_ROOT", ""),
        os.environ.get("PATH", ""),
        os.environ.get("MOLT_NM_TIMEOUT_SEC", ""),
        _symbol_normalization_target(target_triple),
        reader_identity,
    )


def _native_object_symbol_facts_payload(
    *,
    object_digest: str,
    facts: _NativeGlobalSymbolFacts,
    target_triple: str | None,
    reader_identity: tuple[str, ...],
) -> dict[str, object]:
    return {
        "schema": _NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION,
        "platform": sys.platform,
        "symbol_target": _symbol_normalization_target(target_triple),
        "object_digest": object_digest,
        "reader_identity": list(reader_identity),
        "facts": _symbol_facts_payload(facts),
    }


def _read_native_object_symbol_facts(
    path: Path,
    *,
    object_digest: str,
    target_triple: str | None,
    reader_identity: tuple[str, ...],
    members: tuple[StaticArchiveMemberIdentity, ...] | None = None,
) -> _NativeGlobalSymbolFacts | None:
    try:
        payload = json.loads(
            _native_object_symbol_facts_sidecar_path(path).read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION:
        return None
    if payload.get("platform") != sys.platform:
        return None
    if payload.get("symbol_target") != _symbol_normalization_target(target_triple):
        return None
    if payload.get("object_digest") != object_digest:
        return None
    if payload.get("reader_identity") != list(reader_identity):
        return None
    return _decode_symbol_facts(
        payload.get("facts"),
        artifact_digest=object_digest,
        members=members,
    )


def _write_native_object_symbol_facts(
    path: Path,
    *,
    object_digest: str,
    facts: _NativeGlobalSymbolFacts,
    target_triple: str | None,
    reader_identity: tuple[str, ...],
) -> None:
    payload = _native_object_symbol_facts_payload(
        object_digest=object_digest,
        facts=facts,
        target_triple=target_triple,
        reader_identity=reader_identity,
    )
    _atomic_write_json(
        _native_object_symbol_facts_sidecar_path(path),
        payload,
        indent=2,
    )


def _native_object_global_symbol_facts(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
) -> _NativeGlobalSymbolFacts:
    if identity is None:
        identity = _native_symbol_artifact_identity(path)
    else:
        _require_unchanged_symbol_artifact(path, identity)
    object_digest = identity.sha256
    reader = _native_symbol_reader(
        nm_command=nm_command,
        target_triple=target_triple,
        requirement=requirement,
    )
    cache_key = _native_object_symbol_cache_key(
        path,
        object_digest,
        reader_identity=reader.cache_identity,
        target_triple=target_triple,
    )
    if cache_key is not None:
        cached = _NATIVE_OBJECT_SYMBOL_SETS_CACHE.get(cache_key)
        if cached is not None and requirement.accepts(cached):
            _require_unchanged_symbol_reader(path, reader)
            _require_unchanged_symbol_artifact(path, identity)
            return cached
    members = _symbol_artifact_members(path)
    if object_digest:
        symbol_facts = _read_native_object_symbol_facts(
            path,
            object_digest=object_digest,
            target_triple=target_triple,
            reader_identity=reader.cache_identity,
            members=members,
        )
        if symbol_facts is not None and requirement.accepts(symbol_facts):
            _require_unchanged_symbol_reader(path, reader)
            _require_unchanged_symbol_artifact(path, identity)
            if cache_key is not None:
                _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = symbol_facts
            return symbol_facts
    facts = _read_native_global_symbol_facts(
        path,
        timeout=5,
        nm_command=None,
        target_triple=target_triple,
        _reader=reader,
        archive_members=members,
    )
    _require_unchanged_symbol_artifact(path, identity)
    facts = replace(facts, artifact_digest=object_digest)
    if cache_key is not None:
        if (
            len(_NATIVE_OBJECT_SYMBOL_SETS_CACHE)
            >= _NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT
        ):
            _NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
        _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = facts
    if object_digest:
        with contextlib.suppress(OSError):
            _write_native_object_symbol_facts(
                path,
                object_digest=object_digest,
                facts=facts,
                target_triple=target_triple,
                reader_identity=reader.cache_identity,
            )
    _require_unchanged_symbol_artifact(path, identity)
    return facts


def _native_object_global_symbol_sets(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> _NativeObjectSymbolSets:
    facts = _native_object_global_symbol_facts(
        path,
        nm_command=nm_command,
        target_triple=target_triple,
        identity=identity,
    )
    return facts.symbol_sets()


def _parse_native_nm_global_symbol_facts(
    output: str,
    *,
    target_triple: str | None = None,
) -> _NativeGlobalSymbolFacts:
    """Parse one object's symbol table; archive boundaries must never be dropped."""

    defined: set[str] = set()
    undefined: set[str] = set()
    defined_functions: set[str] = set()
    weak_undefined: set[str] = set()
    weak_defined: set[str] = set()
    macho_decoration = _target_uses_macho_symbol_decoration(target_triple)
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.endswith(":"):
            raise ValueError(f"unexpected nm header without member custody: {line!r}")
        indirect_target: str | None = None
        indirect = re.fullmatch(r"(.*) \(indirect for ([^\s]+)\)", line)
        if indirect:
            line, indirect_target = indirect.groups()
        parts = line.split()
        if len(parts) == 2:
            kind, name = parts
        elif len(parts) == 3 and re.fullmatch(r"[0-9a-fA-F]+", parts[0]):
            _, kind, name = parts
        else:
            raise ValueError(f"unrecognized nm symbol row: {line[:512]!r}")
        if len(kind) != 1 or kind not in "AaBbCcDdGgIiRrSsTtUuVvWw":
            raise ValueError(f"unsupported nm symbol type in row: {line[:512]!r}")
        symbol = name[1:] if macho_decoration and name.startswith("_") else name
        if indirect_target is not None:
            if kind != "I":
                raise ValueError(
                    f"indirect target on a non-indirect nm row: {raw_line[:512]!r}"
                )
            undefined.add(
                indirect_target[1:]
                if macho_decoration and indirect_target.startswith("_")
                else indirect_target
            )
        if kind == "U":
            undefined.add(symbol)
        elif kind in {"w", "v"}:
            weak_undefined.add(symbol)
        else:
            defined.add(symbol)
            if kind in {"V", "W"}:
                weak_defined.add(symbol)
            if kind in {"T", "t", "W", "i"}:
                defined_functions.add(symbol)
    return _NativeGlobalSymbolFacts(
        defined=frozenset(defined),
        undefined=frozenset(undefined),
        defined_functions=frozenset(defined_functions),
        weak_undefined=frozenset(weak_undefined),
        weak_defined=frozenset(weak_defined),
    )


def _symbol_artifact_members(
    path: Path,
    *,
    require_archive: bool = False,
) -> tuple[StaticArchiveMemberIdentity, ...] | None:
    try:
        with path.open("rb") as stream:
            magic = stream.read(8)
        if not require_archive and magic not in {b"!<arch>\n", b"!<thin>\n"}:
            return None
        return static_archive_member_identities(path)
    except (OSError, ValueError) as error:
        raise NativeSymbolInspectionError(path, [str(error)]) from error


def _parse_native_archive_symbol_facts(
    output: str,
    *,
    path: Path,
    members: tuple[StaticArchiveMemberIdentity, ...],
    target_triple: str | None,
) -> _NativeGlobalSymbolFacts:
    """Bind each nm table to the same ordinal in the stable archive envelope.

    nm visits archives in stored member order (sorting only within tables).
    Names validate that traversal, but never identify a member by themselves.
    Missing, reordered, extra or unbound tables fail closed, including empties.
    """
    tables: list[list[str]] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.endswith(":"):
            ordinal = len(tables)
            if ordinal >= len(members):
                raise ValueError("nm emitted an extra archive member table")
            member = members[ordinal].member
            labels = {member.name}
            for owner in {str(path), path.name}:
                labels.update((f"{owner}({member.name})", f"{owner}:{member.name}"))
            if stripped[:-1] not in labels:
                raise ValueError(
                    f"nm archive member {ordinal} differs from framing: "
                    f"expected {member.name!r}, found {stripped!r}"
                )
            tables.append([])
        elif not tables:
            raise ValueError("nm symbol row has no archive member custody")
        else:
            tables[-1].append(line)
    if len(tables) != len(members):
        raise ValueError(
            f"nm archive member count differs: {len(tables)} != {len(members)}"
        )
    return _NativeGlobalSymbolFacts(
        frozenset(),
        frozenset(),
        frozenset(),
        members=tuple(
            _NativeArchiveMemberSymbolFacts(
                identity,
                _parse_native_nm_global_symbol_facts(
                    "\n".join(lines), target_triple=target_triple
                ),
            )
            for identity, lines in zip(members, tables)
        ),
    )


def _parse_native_nm_global_symbol_sets(
    output: str,
    *,
    target_triple: str | None = None,
) -> _NativeObjectSymbolSets:
    return _parse_native_nm_global_symbol_facts(
        output,
        target_triple=target_triple,
    ).symbol_sets()


def _native_archive_global_symbol_facts(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
) -> _NativeGlobalSymbolFacts:
    """Read one provider archive's globals without mutating the toolchain.

    Provider archives are immutable installation inputs, not build outputs.
    Their symbol facts therefore use bounded process caching plus one central,
    content-keyed cache under Molt's cache root; unlike object facts, this function
    never writes a ``*.symbols.json`` sibling into Rust or WASI SDK directories.
    """

    if identity is None:
        identity = _native_symbol_artifact_identity(path)
    else:
        _require_unchanged_symbol_artifact(path, identity)
    resolved = identity.path
    stat = resolved.stat()
    changed = content_change_time_ns(resolved, stat)
    if changed is None:
        raise NativeSymbolInspectionError(
            path, ["content-change identity is unavailable"]
        )
    symbol_target = _symbol_normalization_target(target_triple)
    reader = _native_symbol_reader(
        nm_command=nm_command,
        target_triple=target_triple,
        requirement=requirement,
    )
    cache_key: _NativeArchiveSymbolCacheKey = (
        os.fspath(resolved),
        identity.size,
        stat.st_mtime_ns,
        changed,
        os.environ.get("MOLT_TARGET_ROOT", ""),
        os.environ.get("PATH", ""),
        os.environ.get("MOLT_NM_TIMEOUT_SEC", ""),
        symbol_target,
        reader.cache_identity,
        identity.sha256,
    )
    cached = _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.get(cache_key)
    if cached is not None and requirement.accepts(cached):
        _require_unchanged_symbol_reader(path, reader)
        _require_unchanged_symbol_artifact(path, identity)
        return cached
    persistent_cache_path = _native_archive_symbol_cache_path(cache_key)
    members = _symbol_artifact_members(resolved, require_archive=True)
    assert members is not None
    persistent_facts = _read_native_archive_symbol_cache(
        persistent_cache_path,
        cache_key=cache_key,
        members=members,
    )
    if persistent_facts is not None and requirement.accepts(persistent_facts):
        _require_unchanged_symbol_reader(path, reader)
        _require_unchanged_symbol_artifact(path, identity)
        _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE[cache_key] = persistent_facts
        return persistent_facts
    facts = _read_native_global_symbol_facts(
        resolved,
        timeout=120,
        nm_command=None,
        target_triple=target_triple,
        _reader=reader,
        archive_members=members,
    )
    _require_unchanged_symbol_artifact(path, identity)
    facts = replace(facts, artifact_digest=identity.sha256)
    if (
        len(_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE)
        >= _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT
    ):
        _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE[cache_key] = facts
    with contextlib.suppress(OSError):
        _write_native_archive_symbol_cache(
            persistent_cache_path,
            cache_key=cache_key,
            facts=facts,
        )
    _require_unchanged_symbol_artifact(path, identity)
    return facts


def _native_archive_global_symbol_sets(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> _NativeObjectSymbolSets:
    facts = _native_archive_global_symbol_facts(
        path,
        nm_command=nm_command,
        target_triple=target_triple,
        identity=identity,
    )
    return facts.symbol_sets()


def _native_archive_symbol_cache_identity(
    cache_key: _NativeArchiveSymbolCacheKey,
) -> dict[str, object]:
    return {
        "path": cache_key[0],
        "size": cache_key[1],
        "mtime_ns": cache_key[2],
        "ctime_ns": cache_key[3],
        "target_root": cache_key[4],
        "path_env": cache_key[5],
        "timeout_env": cache_key[6],
        "symbol_target": cache_key[7],
        "nm_command": list(cache_key[8]),
        "artifact_digest": cache_key[9],
    }


def _native_archive_symbol_cache_path(
    cache_key: _NativeArchiveSymbolCacheKey,
) -> Path:
    identity = _native_archive_symbol_cache_identity(cache_key)
    digest = hashlib.sha256(
        json.dumps(identity, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return (
        _default_molt_cache()
        / "toolchain_symbol_facts"
        / f"v{_NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION}"
        / f"{digest}.json"
    )


def _read_native_archive_symbol_cache(
    path: Path,
    *,
    cache_key: _NativeArchiveSymbolCacheKey,
    members: tuple[StaticArchiveMemberIdentity, ...],
) -> _NativeGlobalSymbolFacts | None:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION:
        return None
    if payload.get("identity") != _native_archive_symbol_cache_identity(cache_key):
        return None
    return _decode_symbol_facts(
        payload.get("facts"),
        artifact_digest=cache_key[9],
        members=members,
    )


def _write_native_archive_symbol_cache(
    path: Path,
    *,
    cache_key: _NativeArchiveSymbolCacheKey,
    facts: _NativeGlobalSymbolFacts,
) -> None:
    _atomic_write_json(
        path,
        {
            "schema": _NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION,
            "identity": _native_archive_symbol_cache_identity(cache_key),
            "facts": _symbol_facts_payload(facts),
        },
        indent=None,
        sort_keys=True,
    )


def _nm_candidate_binaries() -> list[str]:
    """Ordered candidate `nm` binaries for reading the runtime staticlib.

    The staticlib's members are LLVM *bitcode* when the runtime profile builds
    with LTO, and bitcode is only readable by an ``llvm-nm`` whose LLVM is at
    least as new as the producing rustc's. Apple's Xcode ``nm`` (an older LLVM
    reader) rejects newer Rust bitcode with ``Unknown attribute kind`` — the
    failure that silently broke symbol extraction when the toolchain moved to
    Rust 1.96/LLVM 22 while ``shutil.which("nm")`` kept resolving to Xcode's.
    Order newest/most-capable readers first; the extraction loop validates each
    candidate (clean exit AND a non-empty ``molt_*`` set) before trusting it.
    """
    return [
        str(path) for path in llvm_tool_candidates("nm", include_rust_toolchain=True)
    ]
