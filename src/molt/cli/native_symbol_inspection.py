"""Shared native object/archive symbol facts and reader-generation custody.

Admission, source-extension validation and backend artifact caching consume this
same inspection authority. Backend cache publication and locking live elsewhere.

``molt.native_symbol_table`` reads ELF, Mach-O, COFF and WebAssembly symbol
tables in-process, with no subprocess and no wall-clock bound. Only LLVM
bitcode objects and archive members go to an admitted ``llvm-nm``, because
only an ``llvm-nm`` at least as new as the producer can read them.
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
from collections.abc import Callable, Generator, Iterable
from typing import Literal, Sequence, TypeVar

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.command_runtime import _run_completed_command
from molt.default_paths import _default_molt_cache
from molt.cli.llvm_wasi_tools import _tool_version, llvm_tool_candidates
from molt.cli.static_archive_identity import (
    StaticArchiveMemberIdentity,
    open_static_archive_members,
    static_archive_member_identities,
)
from molt.compiler_distribution import installed_compiler
from molt.native_artifact_header import NativeReader
from molt.native_symbol_table import (
    NativeSymbolRow,
    SymbolInputFormat,
    is_llvm_bitcode,
    read_symbol_rows,
    symbol_input_format,
)
from molt.native_target_shape import (
    NativeArtifactShape,
    NativeObjectFormat,
    native_artifact_shape,
)
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
    WasiSdkInstallation,
    managed_wasm_llvm_nm,
    resolve_wasi_sdk_tool,
    verify_selected_wasm_llvm_nm,
    wasi_sdk_host_asset,
)


_NativeObjectSymbolSets = tuple[set[str], set[str]]
_ParsedNmOutput = TypeVar("_ParsedNmOutput")
# v4: ELF, Mach-O, COFF and WebAssembly facts come from the in-process reader;
# llvm-nm reads only LLVM bitcode. Earlier generations never match this key.
_NATIVE_SYMBOL_FACTS_PROTOCOL = "molt.native-symbol-facts.v4"
_IN_PROCESS_SYMBOL_READER = "in-process:molt.native_symbol_table"
# A healthy llvm-nm read of bitcode takes well under a second. This bound only
# stops a hung reader; the in-process reader has no wall-clock bound (HF-F173).
_LLVM_NM_BITCODE_READ_TIMEOUT_S = 600.0


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
    ``GNU nm (GNU Binutils ...)``. Only an llvm-nm reads LLVM bitcode, so only
    an llvm-nm passes admission; the banner names any other reader.
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
    """One llvm-nm entrypoint for LLVM bitcode, or the reason it was refused."""

    command: tuple[str, ...]
    executable_identity: StableRegularFileIdentity | WasiSdkInstallation | None = None
    admission_error: str | None = None

    def cache_identity(self) -> str:
        return json.dumps(
            {
                "command": self.command,
                "sha256": (
                    None
                    if self.executable_identity is None
                    else str(self.executable_identity.tool_fact("llvm-nm")["sha256"])
                    if isinstance(self.executable_identity, WasiSdkInstallation)
                    else self.executable_identity.sha256
                ),
                "error": self.admission_error,
            },
            sort_keys=True,
            separators=(",", ":"),
        )


@dataclass(frozen=True, slots=True)
class _NativeSymbolReader:
    """The llvm-nm ladder for artifacts that contain LLVM bitcode."""

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


def _in_process_reader_identity(
    requirement: NativeSymbolRequirement,
) -> tuple[str, ...]:
    """Facts identity for artifacts without bitcode: no external reader.

    The in-process reader is part of this source tree, so the protocol
    generation alone names it. Host tools and PATH never change these facts.
    """
    return (
        _NATIVE_SYMBOL_FACTS_PROTOCOL,
        _IN_PROCESS_SYMBOL_READER,
        requirement.cache_identity(),
    )


@functools.lru_cache(maxsize=8)
def _cached_wasm_llvm_nm_verification(
    selected_path: Path,
    expected_version: str,
) -> WasmLlvmNmVerification[StableRegularFileIdentity]:
    return verify_selected_wasm_llvm_nm(
        selected_path, expected_version=expected_version
    )


def _verified_wasm_llvm_nm(
    environment: dict[str, str],
) -> WasmLlvmNmVerification[StableRegularFileIdentity | WasiSdkInstallation]:
    source_root = compiler_source_root()
    selected_path = resolve_wasi_sdk_tool(source_root, "llvm-nm", environ=environment)
    if managed := managed_wasm_llvm_nm(source_root, selected_path, environ=environment):
        return managed
    expected_version = wasi_sdk_host_asset(source_root).llvm_version
    verification = _cached_wasm_llvm_nm_verification(selected_path, expected_version)
    try:
        with stable_executable_probe(
            verification.path,
            label="verified WebAssembly llvm-nm",
            identity=verification.executable_identity,
        ):
            pass
    except (OSError, ValueError):
        _cached_wasm_llvm_nm_verification.cache_clear()
        verification = _cached_wasm_llvm_nm_verification(
            selected_path, expected_version
        )
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
    if reader_family != "llvm":
        return _NativeSymbolReaderCandidate(
            (str(entrypoint), *command[1:]),
            executable_identity=identity,
            admission_error=(
                "nm reader printed no --version banner"
                if banner is None
                else f"GNU nm cannot read LLVM bitcode: {banner!r}"
                if reader_family == "gnu"
                else f"unrecognized nm reader banner: {banner!r}"
            ),
        )
    return _NativeSymbolReaderCandidate(
        (str(entrypoint), *command[1:]), executable_identity=identity
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
            *(
                ("managed-sdk:" + verification.executable_identity.tree_sha256,)
                if isinstance(verification.executable_identity, WasiSdkInstallation)
                else ()
            ),
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
        if not isinstance(candidate.executable_identity, StableRegularFileIdentity):
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
) -> Generator[tuple[StableRegularFileHandle, StableRegularFileIdentity]]:
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


@dataclass(frozen=True, slots=True)
class _SymbolTargetPolicy:
    """Target facts that shape symbol reading, never the host's facts."""

    triple: str
    macho_decoration: bool
    # Selects the slice of a universal Mach-O input; None when the target
    # architecture has no Mach-O encoding.
    macho_shape: NativeArtifactShape | None


def _symbol_target_policy(target_triple: str | None) -> _SymbolTargetPolicy:
    from molt.cli.native_link_plan import resolve_native_target_spec, target_is_wasm

    if target_triple is not None and target_is_wasm(target_triple):
        return _SymbolTargetPolicy(target_triple.strip().lower(), False, None)
    target = resolve_native_target_spec(target_triple)
    try:
        macho_shape: NativeArtifactShape | None = native_artifact_shape(
            target.arch,
            target_triple=target.triple,
            object_format=NativeObjectFormat.MACHO,
        )
    except RuntimeError:
        macho_shape = None
    return _SymbolTargetPolicy(
        target.triple,
        target.object_format is NativeObjectFormat.MACHO,
        macho_shape,
    )


def _target_uses_macho_symbol_decoration(target_triple: str | None) -> bool:
    return _symbol_target_policy(target_triple).macho_decoration


def _normalize_native_symbol_name(
    name: str,
    *,
    target_triple: str | None = None,
) -> str:
    if _target_uses_macho_symbol_decoration(target_triple) and name.startswith("_"):
        return name[1:]
    return name


def _symbol_normalization_target(target_triple: str | None) -> str:
    return f"target:{_symbol_target_policy(target_triple).triple}"


_KNOWN_SYMBOL_KINDS = frozenset("AaBbCcDdGgIiRrSsTtUuVvWw")
_FUNCTION_SYMBOL_KINDS = frozenset("TtWi")


def _facts_from_symbol_rows(
    rows: Iterable[NativeSymbolRow], *, macho_decoration: bool
) -> _NativeGlobalSymbolFacts:
    """Project ``llvm-nm`` type letters into the shared symbol facts.

    The in-process reader and the llvm-nm bitcode reader both end here, so
    one table owns what each letter means. Mach-O targets drop one leading
    underscore, as the target's C symbol decoration requires.
    """
    defined: set[str] = set()
    undefined: set[str] = set()
    defined_functions: set[str] = set()
    weak_undefined: set[str] = set()
    weak_defined: set[str] = set()

    def undecorated(name: str) -> str:
        if not name or _SYMBOL_WHITESPACE.search(name) is not None:
            raise ValueError(f"symbol name is empty or contains whitespace: {name!r}")
        return name[1:] if macho_decoration and name.startswith("_") else name

    for row in rows:
        kind = row.kind
        if len(kind) != 1 or kind not in _KNOWN_SYMBOL_KINDS:
            raise ValueError(f"unsupported symbol type {kind!r} for {row.name!r}")
        symbol = undecorated(row.name)
        if row.indirect is not None:
            if kind != "I":
                raise ValueError(f"indirect target on a non-indirect symbol {symbol!r}")
            undefined.add(undecorated(row.indirect))
        if kind == "U":
            undefined.add(symbol)
        elif kind in {"w", "v"}:
            weak_undefined.add(symbol)
        else:
            defined.add(symbol)
            if kind in {"V", "W"}:
                weak_defined.add(symbol)
            if kind in _FUNCTION_SYMBOL_KINDS:
                defined_functions.add(symbol)
    return _NativeGlobalSymbolFacts(
        defined=frozenset(defined),
        undefined=frozenset(undefined),
        defined_functions=frozenset(defined_functions),
        weak_undefined=frozenset(weak_undefined),
        weak_defined=frozenset(weak_defined),
    )


def _llvm_nm_bitcode_command(command: Sequence[str], path: Path) -> list[str]:
    """``-g`` lists the global symbols of each bitcode module.

    The bitcode reader stays enabled: it is the reason this reader runs. In
    a mixed archive llvm-nm also reads native members, but only the tables of
    bitcode members become facts; the in-process reader owns the others.
    """
    return [*command, "-g", str(path)]


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


def _artifact_reader(opened: StableRegularFileHandle) -> NativeReader:
    """Bounded reads through the one admitted handle; no reopen by path."""
    stream = opened.stream

    def read_at(offset: int, size: int) -> bytes:
        stream.seek(offset)
        return stream.read(size)

    return NativeReader(opened.stat.st_size, read_at)


def _symbol_artifact_has_llvm_bitcode(
    opened: StableRegularFileHandle, *, archive: bool
) -> bool:
    """Whether llvm-nm must read part of this artifact.

    Archives answer from member framing and four magic bytes per member, never
    from member hashing, so a warm content-cache hit stays cheap.
    """
    try:
        if not archive:
            opened.stream.seek(0)
            return is_llvm_bitcode(opened.stream.read(4))
        with open_static_archive_members(opened.path, opened=opened) as (
            members,
            stream,
        ):
            for member in members:
                stream.seek(member.content_offset)
                if is_llvm_bitcode(stream.read(min(4, member.size))):
                    return True
        return False
    except (OSError, ValueError) as error:
        raise NativeSymbolArtifactError(opened.path, [str(error)]) from error


def _read_native_global_symbol_facts(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    _reader: _NativeSymbolReader | None = None,
    requirement: NativeSymbolRequirement = NativeSymbolRequirement(),
    archive_members: tuple[StaticArchiveMemberIdentity, ...] | None = None,
    _opened: StableRegularFileHandle | None = None,
) -> _NativeGlobalSymbolFacts:
    """Read current facts for one object or archive through one owned handle.

    The in-process reader owns every non-bitcode object and member. A bitcode
    object, or an archive with a bitcode member, also runs the llvm-nm ladder
    once; its tables bind only the bitcode members.
    """
    if _opened is None:
        with _open_native_symbol_artifact(path) as (opened, _identity):
            members = _symbol_artifact_members(opened.path, opened=opened)
            if archive_members is not None and archive_members != members:
                raise NativeSymbolArtifactError(
                    path, ["supplied archive framing differs from admitted bytes"]
                )
            return _read_native_global_symbol_facts(
                opened.path,
                nm_command=nm_command,
                target_triple=target_triple,
                _reader=_reader,
                requirement=requirement,
                archive_members=members,
                _opened=opened,
            )
    if _opened.path != path.expanduser().absolute():
        raise NativeSymbolArtifactError(path, ["symbol handle belongs to another path"])
    policy = _symbol_target_policy(target_triple)
    artifact = _artifact_reader(_opened)

    def bitcode_reader() -> _NativeSymbolReader:
        return _reader or _native_symbol_reader(
            nm_command=nm_command,
            target_triple=target_triple,
            requirement=requirement,
        )

    if archive_members is None:
        try:
            prefix = artifact.read(0, min(8, artifact.size), "object magic")
            bitcode = symbol_input_format(prefix) is SymbolInputFormat.LLVM_BITCODE
            if not bitcode:
                facts = _facts_from_symbol_rows(
                    read_symbol_rows(artifact, macho_shape=policy.macho_shape),
                    macho_decoration=policy.macho_decoration,
                )
        except (OSError, ValueError) as error:
            raise NativeSymbolArtifactError(path, [str(error)]) from error
        if bitcode:
            facts = _run_llvm_nm_ladder(
                path,
                bitcode_reader(),
                parse=lambda result: _parse_llvm_nm_object_result(
                    result, policy=policy
                ),
            )
    else:
        facts = _read_archive_symbol_facts(
            path,
            artifact,
            archive_members,
            policy=policy,
            bitcode_reader=bitcode_reader,
        )
    if not requirement.accepts(facts):
        raise NativeSymbolInspectionError(
            path,
            [
                "no function definitions satisfy consumer requirement "
                f"{requirement.cache_identity()}"
            ],
        )
    return facts


def _read_archive_symbol_facts(
    path: Path,
    artifact: NativeReader,
    members: tuple[StaticArchiveMemberIdentity, ...],
    *,
    policy: _SymbolTargetPolicy,
    bitcode_reader: Callable[[], _NativeSymbolReader],
) -> _NativeGlobalSymbolFacts:
    tables: list[_NativeGlobalSymbolFacts | None] = []
    for identity in members:
        member = identity.member
        try:
            reader = artifact.slice(
                member.content_offset, member.size, "archive member"
            )
            prefix = reader.read(0, min(8, member.size), "archive member magic")
            if symbol_input_format(prefix) is SymbolInputFormat.LLVM_BITCODE:
                tables.append(None)
                continue
            tables.append(
                _facts_from_symbol_rows(
                    read_symbol_rows(reader, macho_shape=policy.macho_shape),
                    macho_decoration=policy.macho_decoration,
                )
            )
        except (OSError, ValueError) as error:
            raise NativeSymbolArtifactError(
                path,
                [f"archive member {identity.ordinal} ({member.name!r}): {error}"],
            ) from error
    bitcode = [ordinal for ordinal, facts in enumerate(tables) if facts is None]
    if bitcode:
        names = frozenset(item.member.name for item in members)

        def parse(result: subprocess.CompletedProcess[str]) -> list[str]:
            return _bind_nm_archive_tables(
                _validated_nm_output(result, archive_member_names=names),
                path=path,
                members=members,
            )

        bound = _run_llvm_nm_ladder(path, bitcode_reader(), parse=parse)
        for ordinal in bitcode:
            tables[ordinal] = _facts_from_nm_output(
                bound[ordinal], macho_decoration=policy.macho_decoration
            )
    bound_members: list[_NativeArchiveMemberSymbolFacts] = []
    for identity, facts in zip(members, tables):
        assert facts is not None
        bound_members.append(_NativeArchiveMemberSymbolFacts(identity, facts))
    return _NativeGlobalSymbolFacts(
        frozenset(), frozenset(), frozenset(), members=tuple(bound_members)
    )


def _run_llvm_nm_ladder(
    path: Path,
    reader: _NativeSymbolReader,
    *,
    parse: Callable[[subprocess.CompletedProcess[str]], _ParsedNmOutput],
) -> _ParsedNmOutput:
    """Run admitted llvm-nm candidates in order until one output parses.

    Reading bitcode is a leaf, non-spawning, read-only operation, so it does
    not go through the process-tree memory guard. Its timeout only stops a
    hung reader.
    """
    if not reader.candidates:
        raise NativeSymbolInspectionError(
            path, ["LLVM bitcode needs llvm-nm, and no llvm-nm candidate is available"]
        )
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
            custody = (
                contextlib.nullcontext(
                    (Path(command[0]), candidate.executable_identity)
                )
                if isinstance(candidate.executable_identity, WasiSdkInstallation)
                else stable_executable_probe(
                    Path(command[0]),
                    label="native symbol reader",
                    identity=candidate.executable_identity,
                )
            )
            with custody as (entrypoint, _identity):
                try:
                    result = _run_completed_command(
                        _llvm_nm_bitcode_command((str(entrypoint), *command[1:]), path),
                        capture_output=True,
                        timeout=_LLVM_NM_BITCODE_READ_TIMEOUT_S,
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
            return parse(result)
        except ValueError as error:
            if primary is None:
                primary = error
            failures.append(f"{command!r}: {error}")
    raise NativeSymbolInspectionError(path, failures) from primary


def _validated_nm_output(
    result: subprocess.CompletedProcess[str],
    *,
    archive_member_names: frozenset[str] | None,
) -> str:
    """Reject a failed or diagnostic-bearing run; drop benign empty rows."""
    if result.returncode != 0 or any(
        line.strip()
        and not _nm_line_reports_no_symbols(line.strip(), result, archive_member_names)
        for line in result.stderr.splitlines()
    ):
        raise ValueError(
            f"exit {result.returncode}; stdout={result.stdout[:2048]!r}; "
            f"stderr={result.stderr[:2048]!r}"
        )
    return "\n".join(
        line
        for line in result.stdout.splitlines()
        if not _nm_line_reports_no_symbols(line.strip(), result, archive_member_names)
    )


def _parse_llvm_nm_object_result(
    result: subprocess.CompletedProcess[str], *, policy: _SymbolTargetPolicy
) -> _NativeGlobalSymbolFacts:
    if result.returncode in {0, 1} and _nm_result_reports_no_symbols(result):
        return _NativeGlobalSymbolFacts(frozenset(), frozenset(), frozenset())
    return _facts_from_nm_output(
        _validated_nm_output(result, archive_member_names=None),
        macho_decoration=policy.macho_decoration,
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
) -> Generator[
    tuple[StableRegularFileHandle, StableRegularFileIdentity, _NativeGlobalSymbolFacts]
]:
    """One owned admission for native shape, member framing and symbol facts.

    Supplied digests and all cache hits require current content admission.
    Member parsing and the bitcode llvm-nm retain that same handle. Closing
    fences precede either persistent or process-cache publication.
    """
    computed = False
    with _open_native_symbol_artifact(path, identity) as (opened, admitted):
        if validate_shape is not None:
            validate_shape(opened)
        # Format, not caller spelling or pathname, owns retention and reader
        # policy. A warm content hit needs only the header and member magics.
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
        # Only bitcode makes the facts depend on an external reader.
        reader = (
            _native_symbol_reader(
                nm_command=nm_command,
                target_triple=target_triple,
                requirement=requirement,
            )
            if _symbol_artifact_has_llvm_bitcode(opened, archive=archive)
            else None
        )
        reader_identity = (
            _in_process_reader_identity(requirement)
            if reader is None
            else reader.cache_identity
        )
        cache_key = _native_symbol_facts_cache_key(
            admitted,
            reader_identity=reader_identity,
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
                    reader_identity=reader_identity,
                    members=members,
                )
            if facts is None or not requirement.accepts(facts):
                facts = _read_native_global_symbol_facts(
                    opened.path,
                    nm_command=nm_command,
                    target_triple=target_triple,
                    _reader=reader,
                    requirement=requirement,
                    archive_members=members,
                    _opened=opened,
                )
                facts = replace(facts, artifact_digest=admitted.sha256)
                computed = True
        if reader is not None:
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
                reader_identity=reader_identity,
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


_NM_ADDRESS = re.compile(r"[0-9a-fA-F]+|-+")
_NM_INDIRECT = re.compile(r"(.*) \(indirect for ([^\s]+)\)")


def _facts_from_nm_output(
    output: str, *, macho_decoration: bool
) -> _NativeGlobalSymbolFacts:
    """Parse one llvm-nm table; archive boundaries must never be dropped.

    llvm-nm prints dashes, not an address, for a defined bitcode symbol.
    """
    rows: list[NativeSymbolRow] = []
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.endswith(":"):
            raise ValueError(f"unexpected nm header without member custody: {line!r}")
        indirect_target: str | None = None
        indirect = _NM_INDIRECT.fullmatch(line)
        if indirect:
            line, indirect_target = indirect.groups()
        parts = line.split()
        if len(parts) == 2:
            kind, name = parts
        elif len(parts) == 3 and _NM_ADDRESS.fullmatch(parts[0]):
            _, kind, name = parts
        else:
            raise ValueError(f"unrecognized nm symbol row: {line[:512]!r}")
        rows.append(NativeSymbolRow(kind, name, indirect_target))
    return _facts_from_symbol_rows(rows, macho_decoration=macho_decoration)


def _parse_native_nm_global_symbol_facts(
    output: str,
    *,
    target_triple: str | None = None,
) -> _NativeGlobalSymbolFacts:
    return _facts_from_nm_output(
        output,
        macho_decoration=_target_uses_macho_symbol_decoration(target_triple),
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


def _bind_nm_archive_tables(
    output: str,
    *,
    path: Path,
    members: tuple[StaticArchiveMemberIdentity, ...],
) -> list[str]:
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
    return ["\n".join(lines) for lines in tables]


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
    """Ordered ``llvm-nm`` candidates for LLVM bitcode objects and members.

    The in-process reader owns every other format. Bitcode is readable only by
    an ``llvm-nm`` whose LLVM is at least as new as the producer's. Apple's
    Xcode ``nm`` (an older LLVM reader) rejects newer Rust bitcode with
    ``Unknown attribute kind``. Order newest and most capable readers first;
    the ladder admits each candidate by its banner and accepts the first
    clean, parseable read. GNU nm candidates fail admission.
    """
    # Installed runtime projections are shipped, but application objects and
    # source extensions still use this shared reader. Those consumers must not
    # rediscover Rust merely to read bitcode with host LLVM.
    source_checkout = installed_compiler(compiler_source_root()) is None
    return [
        str(path)
        for path in llvm_tool_candidates("nm", include_rust_toolchain=source_checkout)
    ]
