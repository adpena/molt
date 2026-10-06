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
from collections import OrderedDict
from dataclasses import dataclass, field, replace
from pathlib import Path
from collections.abc import Callable, Iterator
from typing import Literal, Sequence

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.cli.llvm_wasi_tools import _tool_version, llvm_tool_candidates
from molt.cli.static_archive_identity import (
    StaticArchiveMemberIdentity,
    static_archive_member_identities,
)
from molt.compiler_distribution import installed_compiler
from molt.source_root import compiler_source_root
from molt.toolchain_identity import (
    StableRegularFileHandle,
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_executable_probe,
    stable_regular_file_handle_identity,
    stable_regular_file_identity,
    verify_stable_regular_file_content,
    verify_stable_regular_file_identity,
)
from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    WasmLlvmNmVerification,
    verify_wasm_llvm_nm,
)


_NativeObjectSymbolSets = tuple[set[str], set[str]]
# Prior generations could attach a supplied digest after metadata-only checks.
# Do not admit their persistent tables even when current bytes match that key.
_NATIVE_SYMBOL_FACTS_PROTOCOL = "molt.native-symbol-facts.v3"


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

_SYMBOL_WHITESPACE = re.compile(r"\s")


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


def _decode_symbol_table(
    value: object, *, validated_symbols: set[str]
) -> _NativeGlobalSymbolFacts | None:
    if not isinstance(value, dict) or set(value) != set(_SYMBOL_SET_FIELDS):
        return None
    tables: list[frozenset[str]] = []
    for name in _SYMBOL_SET_FIELDS:
        symbols = value[name]
        if not isinstance(symbols, list):
            return None
        previous: str | None = None
        for symbol in symbols:
            if not isinstance(symbol, str) or not symbol:
                return None
            # Strict ordering proves both canonical order and uniqueness without
            # sorting or constructing another temporary set for every table.
            if previous is not None and symbol <= previous:
                return None
            if symbol not in validated_symbols:
                # Unicode \s has the same whitespace semantics as str.isspace;
                # the regex engine scans each distinct symbol outside Python.
                if _SYMBOL_WHITESPACE.search(symbol) is not None:
                    return None
                validated_symbols.add(symbol)
            previous = symbol
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
    # Repeated strings across fields and archive members share lexical admission
    # only inside this payload. Member identity and table structure stay checked.
    validated_symbols: set[str] = set()
    if members is None:
        if set(value) != {"object"}:
            return None
        facts = _decode_symbol_table(
            value["object"], validated_symbols=validated_symbols
        )
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
        symbols = _decode_symbol_table(
            row["symbols"], validated_symbols=validated_symbols
        )
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


class NativeSymbolArtifactError(NativeSymbolInspectionError):
    """Artifact bytes, framing or custody failed before facts could be admitted.

    Cache consumers may reject this artifact and rebuild. Reader provisioning,
    execution and symbol-output failures retain NativeSymbolInspectionError and
    must surface as operational failures instead of a cache miss.
    """


NmReaderFamily = Literal["llvm", "gnu"]


def nm_reader_family_from_banner(banner: str | None) -> NmReaderFamily | None:
    """Classify the first line of ``nm --version``.

    ``llvm-nm`` announces itself as ``llvm-nm, compatible with GNU nm``; Xcode's
    ``nm`` is an llvm-nm and prints the same line. GNU binutils announces
    ``GNU nm (GNU Binutils ...)``. Any other reader is not one this module knows
    how to drive, so its candidate fails admission with the banner it printed.
    """
    if banner is None:
        return None
    text = banner.strip()
    if text.startswith("llvm-nm"):
        return "llvm"
    if text.startswith("GNU nm"):
        return "gnu"
    return None


@functools.lru_cache(maxsize=16)
def _cached_nm_reader_family(
    path_text: str,
    sha256: str,
) -> tuple[NmReaderFamily | None, str | None]:
    # One ``--version`` per distinct reader binary; the digest keys the cache so
    # a replaced executable is classified again.
    del sha256
    banner = _tool_version(Path(path_text))
    return nm_reader_family_from_banner(banner), banner


@dataclass(frozen=True, slots=True)
class _NativeSymbolReaderCandidate:
    command: tuple[str, ...]
    executable_identity: StableRegularFileIdentity | None = None
    admission_error: str | None = None
    # ``None`` only together with ``admission_error``: an admitted reader always
    # has a known family, because the family selects its command line.
    reader_family: NmReaderFamily | None = None

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
    reader_family, banner = _cached_nm_reader_family(str(entrypoint), identity.sha256)
    if reader_family is None:
        return _NativeSymbolReaderCandidate(
            (str(entrypoint), *command[1:]),
            executable_identity=identity,
            admission_error=(
                "nm reader printed no --version banner"
                if banner is None
                else f"unrecognized nm reader banner: {banner!r}"
            ),
        )
    return _NativeSymbolReaderCandidate(
        (str(entrypoint), *command[1:]),
        executable_identity=identity,
        reader_family=reader_family,
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
        reader_family="llvm",
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
        raise NativeSymbolArtifactError(path, [str(error)]) from error


def _require_unchanged_symbol_artifact(
    path: Path, identity: StableRegularFileIdentity
) -> None:
    try:
        if path.resolve(strict=True) != identity.path.resolve(strict=True):
            raise ValueError("artifact path no longer names its captured generation")
        verify_stable_regular_file_identity(
            identity, label="native symbol artifact", hash_content=True
        )
    except (OSError, ValueError) as error:
        raise NativeSymbolArtifactError(
            path,
            [
                "artifact changed during symbol inspection; no facts were published",
                str(error),
            ],
        ) from error


@contextlib.contextmanager
def _open_native_symbol_artifact(
    path: Path, identity: StableRegularFileIdentity | None = None
) -> Iterator[tuple[StableRegularFileHandle, StableRegularFileIdentity]]:
    """Admit current bytes and retain their descriptor through every consumer.

    Windows excludes writes/deletion while this handle is owned. POSIX retains
    the shared path/handle/change fences, not an atomic filesystem snapshot.
    Cache publication happens only after this context's closing fences pass.
    """
    consumer_error: OSError | ValueError | None = None
    try:
        resolved = path.resolve(strict=True)
        if identity is not None and identity.path != resolved:
            raise ValueError("artifact path no longer names its captured generation")
        with open_stable_regular_file(
            resolved, label="native symbol artifact", observed=identity
        ) as opened:
            current = stable_regular_file_handle_identity(
                opened, label="native symbol artifact"
            )
            if identity is not None:
                verify_stable_regular_file_content(
                    identity,
                    sha256=current.sha256,
                    size=current.size,
                    label="native symbol artifact",
                )
            try:
                yield opened, current
            except (OSError, ValueError) as error:
                consumer_error = error
                raise
    except NativeSymbolInspectionError:
        raise
    except (OSError, ValueError) as error:
        if error is consumer_error:
            raise
        raise NativeSymbolArtifactError(
            path,
            [
                "artifact changed during symbol inspection; no facts were published",
                str(error),
            ],
        ) from error


@dataclass(frozen=True, slots=True)
class _NativeSymbolFactsCacheKey:
    size: int
    symbol_target: str
    reader_identity: tuple[str, ...]
    artifact_digest: str


_NATIVE_OBJECT_SYMBOL_SETS_CACHE: OrderedDict[
    _NativeSymbolFactsCacheKey,
    _NativeGlobalSymbolFacts,
] = OrderedDict()
_NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT = 256
_NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION = 6
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT = 32
_NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION = 6
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE: OrderedDict[
    _NativeSymbolFactsCacheKey,
    _NativeGlobalSymbolFacts,
] = OrderedDict()


def _remember_native_symbol_facts(
    cache: OrderedDict[_NativeSymbolFactsCacheKey, _NativeGlobalSymbolFacts],
    key: _NativeSymbolFactsCacheKey,
    facts: _NativeGlobalSymbolFacts,
    *,
    limit: int,
) -> None:
    cache[key] = facts
    cache.move_to_end(key)
    while len(cache) > limit:
        cache.popitem(last=False)


def _symbol_target_policy(target_triple: str | None) -> tuple[str, bool]:
    from molt.cli.native_link_plan import (
        NativeObjectFormat,
        resolve_native_target_spec,
        target_is_wasm,
    )

    if target_triple is not None and target_is_wasm(target_triple):
        return target_triple.strip().lower(), False
    target = resolve_native_target_spec(target_triple)
    return target.triple, target.object_format is NativeObjectFormat.MACHO


def _target_uses_macho_symbol_decoration(target_triple: str | None) -> bool:
    return _symbol_target_policy(target_triple)[1]


def _normalize_native_symbol_name(
    name: str,
    *,
    target_triple: str | None = None,
) -> str:
    if _target_uses_macho_symbol_decoration(target_triple) and name.startswith("_"):
        return name[1:]
    return name


def _symbol_normalization_target(target_triple: str | None) -> str:
    return f"target:{_symbol_target_policy(target_triple)[0]}"


def _native_nm_command(
    nm_command: Sequence[str],
    path: Path,
    *,
    reader_family: NmReaderFamily,
) -> list[str]:
    """``-g`` reads the global symbol table, and only the symbol table.

    Rust's sysroot objects for Apple targets carry an embedded ``__LLVM,__bitcode``
    section. llvm-nm's default bitcode reader then also lists the IR symbols,
    with a dash placeholder instead of an address, which is not a symbol-table
    row. ``--no-llvm-bc`` keeps llvm-nm (Xcode's ``nm`` included) on the native
    symbol table. GNU nm has no bitcode reader and no such flag.
    """
    if reader_family == "llvm":
        return [*nm_command, "-g", "--no-llvm-bc", str(path)]
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
    _opened: StableRegularFileHandle | None = None,
) -> _NativeGlobalSymbolFacts:
    if _opened is None:
        with _open_native_symbol_artifact(path) as (opened, _identity):
            members = _symbol_artifact_members(opened.path, opened=opened)
            if archive_members is not None and archive_members != members:
                raise NativeSymbolArtifactError(
                    path, ["supplied archive framing differs from admitted bytes"]
                )
            return _read_native_global_symbol_facts(
                opened.path,
                timeout=timeout,
                nm_command=nm_command,
                target_triple=target_triple,
                _reader=_reader,
                requirement=requirement,
                archive_members=members,
                _opened=opened,
            )
    if _opened.path != path.expanduser().absolute():
        raise NativeSymbolArtifactError(path, ["symbol handle belongs to another path"])
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
        assert candidate.reader_family is not None
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
                        _native_nm_command(
                            (str(entrypoint), *command[1:]),
                            path,
                            reader_family=candidate.reader_family,
                        ),
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


def _native_symbol_facts_cache_key(
    identity: StableRegularFileIdentity,
    *,
    reader_identity: tuple[str, ...],
    target_triple: str | None,
) -> _NativeSymbolFactsCacheKey:
    return _NativeSymbolFactsCacheKey(
        size=identity.size,
        symbol_target=_symbol_normalization_target(target_triple),
        reader_identity=reader_identity,
        artifact_digest=identity.sha256,
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


@contextlib.contextmanager
def _native_symbol_facts_admission(
    path: Path,
    *,
    archive: bool = False,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
    publish: bool = False,
    validate_shape: Callable[[StableRegularFileHandle], None] | None = None,
) -> Iterator[
    tuple[StableRegularFileHandle, StableRegularFileIdentity, _NativeGlobalSymbolFacts]
]:
    """One owned admission for native shape, member framing and symbol facts.

    Supplied digests and all cache hits require current content admission.
    Member parsing and external nm retain that same handle. Closing fences
    precede either persistent or process-cache publication.
    """
    computed = False
    with _open_native_symbol_artifact(path, identity) as (opened, admitted):
        if validate_shape is not None:
            validate_shape(opened)
        # Format, not caller spelling or pathname, owns retention and nm policy.
        # A warm content hit needs only this header, never another member parse.
        archive = archive or _symbol_artifact_has_archive_header(opened)
        cache = (
            _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE
            if archive
            else _NATIVE_OBJECT_SYMBOL_SETS_CACHE
        )
        limit = (
            _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT
            if archive
            else _NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT
        )
        reader = _native_symbol_reader(
            nm_command=nm_command,
            target_triple=target_triple,
            requirement=requirement,
        )
        cache_key = _native_symbol_facts_cache_key(
            admitted,
            reader_identity=reader.cache_identity,
            target_triple=target_triple,
        )
        facts = cache.get(cache_key)
        persistent_cache_path = (
            _native_archive_symbol_cache_path(cache_key) if archive else None
        )
        if facts is None or not requirement.accepts(facts):
            members = _symbol_artifact_members(
                opened.path, opened=opened, require_archive=archive
            )
            if archive:
                assert members is not None and persistent_cache_path is not None
                facts = _read_native_archive_symbol_cache(
                    persistent_cache_path, cache_key=cache_key, members=members
                )
            else:
                facts = _read_native_object_symbol_facts(
                    path,
                    object_digest=admitted.sha256,
                    target_triple=target_triple,
                    reader_identity=reader.cache_identity,
                    members=members,
                )
            if facts is None or not requirement.accepts(facts):
                facts = _read_native_global_symbol_facts(
                    opened.path,
                    timeout=120 if archive else 5,
                    target_triple=target_triple,
                    _reader=reader,
                    archive_members=members,
                    _opened=opened,
                )
                facts = replace(facts, artifact_digest=admitted.sha256)
                computed = True
        _require_unchanged_symbol_reader(path, reader)
        yield opened, admitted, facts
    # Facts describe bytes read during the admitted interval. They remain valid
    # under that content key if the pathname changes after custody is released;
    # a later lookup must independently admit its then-current content.
    _remember_native_symbol_facts(cache, cache_key, facts, limit=limit)
    if archive:
        if computed:
            assert persistent_cache_path is not None
            with contextlib.suppress(OSError):
                _write_native_archive_symbol_cache(
                    persistent_cache_path, cache_key=cache_key, facts=facts
                )
    elif publish:
        with contextlib.suppress(OSError):
            _write_native_object_symbol_facts(
                path,
                object_digest=admitted.sha256,
                facts=facts,
                target_triple=target_triple,
                reader_identity=reader.cache_identity,
            )


def _native_object_global_symbol_facts(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
    publish: bool = False,
) -> _NativeGlobalSymbolFacts:
    with _native_symbol_facts_admission(
        path,
        nm_command=nm_command,
        target_triple=target_triple,
        identity=identity,
        requirement=requirement,
        publish=publish,
    ) as (_opened, _identity, facts):
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


def _symbol_artifact_has_archive_header(opened: StableRegularFileHandle) -> bool:
    try:
        opened.stream.seek(0)
        return opened.stream.read(8) in {b"!<arch>\n", b"!<thin>\n"}
    except OSError as error:
        raise NativeSymbolArtifactError(opened.path, [str(error)]) from error


def _symbol_artifact_members(
    path: Path,
    *,
    opened: StableRegularFileHandle,
    require_archive: bool = False,
) -> tuple[StaticArchiveMemberIdentity, ...] | None:
    try:
        if not require_archive and not _symbol_artifact_has_archive_header(opened):
            return None
        return static_archive_member_identities(path, opened=opened)
    except (OSError, ValueError) as error:
        raise NativeSymbolArtifactError(path, [str(error)]) from error


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
    """Admit provider archives without publishing into immutable toolchains."""
    with _native_symbol_facts_admission(
        path,
        archive=True,
        nm_command=nm_command,
        target_triple=target_triple,
        identity=identity,
        requirement=requirement,
    ) as (_opened, _identity, facts):
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
    cache_key: _NativeSymbolFactsCacheKey,
) -> dict[str, object]:
    # Resolve and admit the current readers before consulting this cache. Their
    # content identities, the parser protocol and the target already capture
    # the output-bearing selection. Ambient PATH and timeout spelling must not
    # invalidate facts produced by those same immutable reader/archive bytes.
    # The operation's separate file and reader fences still detect mutation.
    return {
        "size": cache_key.size,
        "symbol_target": cache_key.symbol_target,
        "nm_command": list(cache_key.reader_identity),
        "artifact_digest": cache_key.artifact_digest,
    }


def _native_archive_symbol_cache_path(
    cache_key: _NativeSymbolFactsCacheKey,
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
    cache_key: _NativeSymbolFactsCacheKey,
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
        artifact_digest=cache_key.artifact_digest,
        members=members,
    )


def _write_native_archive_symbol_cache(
    path: Path,
    *,
    cache_key: _NativeSymbolFactsCacheKey,
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
    # Installed runtime projections are shipped, but application objects and
    # source extensions still use this shared reader. Those consumers must not
    # rediscover Rust merely to inspect native objects with host LLVM/binutils.
    source_checkout = installed_compiler(compiler_source_root()) is None
    return [
        str(path)
        for path in llvm_tool_candidates("nm", include_rust_toolchain=source_checkout)
    ]
