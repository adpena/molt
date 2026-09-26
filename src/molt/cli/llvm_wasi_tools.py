from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
import functools
import os
from pathlib import Path
import subprocess
from typing import Literal

from molt.dx import TOOLCHAINS_DIRNAME

from molt.cli.command_runtime import _run_completed_command
from molt.cli.wasm_link_inputs import _wasi_sdk_root_for_sysroot
from molt.file_hashing import _sha256_file
from molt.rust_toolchain import RustToolSearch, rustc_host, rustc_printed_sysroot
from molt.toolchain_identity import (
    executable_candidates,
    executable_environment_value,
    executable_name_candidates,
    executable_search_directories,
    expand_user_path,
    find_executable,
    resolve_executable,
)
from molt.llvm_linker_roles import (
    LlvmLinkerRole,
    executable_selects_linker_role,
    lexical_executable_path,
)


LlvmToolRole = Literal["cc", "cxx", "wasm_ld", "ar", "ranlib", "nm", "strip"]
LlvmTargetFamily = Literal["native", "wasm"]
_PathIdentity = tuple[int, int, int, int, int]
_ProvenanceObservations = dict[str, _PathIdentity]

_LLVM_TOOL_ROLES: tuple[LlvmToolRole, ...] = (
    "cc",
    "cxx",
    "wasm_ld",
    "ar",
    "ranlib",
    "nm",
    "strip",
)
_LLVM_TOOL_NAMES: dict[LlvmToolRole, tuple[str, ...]] = {
    "cc": ("clang",),
    "cxx": ("clang++",),
    "wasm_ld": ("wasm-ld",),
    "ar": ("llvm-ar", "ar"),
    "ranlib": ("llvm-ranlib", "ranlib"),
    "nm": ("llvm-nm", "nm"),
    "strip": ("llvm-strip", "strip"),
}


@dataclass(frozen=True)
class ResolvedLlvmTool:
    role: LlvmToolRole
    command: tuple[str, ...]
    path: Path
    version: str | None
    sha256: str

    def metadata(self) -> dict[str, object]:
        return {
            "command": list(self.command),
            "path": str(self.path),
            "version": self.version,
            "sha256": self.sha256,
        }


@dataclass(frozen=True)
class LlvmWasiToolFamily:
    cc: ResolvedLlvmTool | None
    cxx: ResolvedLlvmTool | None
    wasm_ld: ResolvedLlvmTool | None
    ar: ResolvedLlvmTool | None
    ranlib: ResolvedLlvmTool | None
    nm: ResolvedLlvmTool | None
    strip: ResolvedLlvmTool | None

    def missing_roles(self) -> tuple[LlvmToolRole, ...]:
        return tuple(role for role in _LLVM_TOOL_ROLES if getattr(self, role) is None)

    def metadata(self) -> dict[str, object]:
        return {
            role: tool.metadata() if (tool := getattr(self, role)) is not None else None
            for role in _LLVM_TOOL_ROLES
        }


def _absolute_tool_path(path: Path) -> Path:
    """Normalize an executable path without erasing its invoked entrypoint.

    LLVM distributions commonly expose ``wasm-ld`` as a symlink to the generic
    ``lld`` driver.  Resolving the symlink changes which driver basename is
    invoked and therefore changes the tool's role.  Keep the lexical executable
    identity while still making relative PATH entries deterministic.
    """

    return lexical_executable_path(path)


def _dedupe_tool_paths(paths: Iterable[Path]) -> tuple[Path, ...]:
    seen: set[str] = set()
    result: list[Path] = []
    for path in paths:
        absolute = _absolute_tool_path(path)
        key = os.path.normcase(os.fspath(absolute))
        if key in seen:
            continue
        seen.add(key)
        result.append(absolute)
    return tuple(result)


def _dedupe_paths(paths: Iterable[Path]) -> tuple[Path, ...]:
    seen: set[str] = set()
    result: list[Path] = []
    for path in paths:
        resolved = path.resolve(strict=False)
        key = os.path.normcase(os.fspath(resolved))
        if key in seen:
            continue
        seen.add(key)
        result.append(resolved)
    return tuple(result)


@functools.lru_cache(maxsize=8)
def _cached_source_checkout_roots(module_file: str) -> tuple[Path, ...]:
    """Return the loaded checkout and its Git common checkout, if worktree-backed."""
    checkout = Path(module_file).resolve().parents[3]
    roots = [checkout]
    git_marker = checkout / ".git"
    if git_marker.is_file():
        try:
            marker = git_marker.read_text(encoding="utf-8", errors="strict").strip()
        except (OSError, UnicodeDecodeError):
            marker = ""
        if marker.startswith("gitdir:"):
            raw_git_dir = marker.removeprefix("gitdir:").strip()
            git_dir = Path(raw_git_dir)
            if not git_dir.is_absolute():
                git_dir = checkout / git_dir
            resolved_git_dir = git_dir.resolve(strict=False)
            common_dot_git = next(
                (
                    candidate
                    for candidate in (resolved_git_dir, *resolved_git_dir.parents)
                    if candidate.name == ".git"
                ),
                None,
            )
            if common_dot_git is not None:
                roots.append(common_dot_git.parent)
    return _dedupe_paths(roots)


def _source_checkout_roots() -> tuple[Path, ...]:
    return _cached_source_checkout_roots(os.path.abspath(__file__))


def _directory_identity(path: Path) -> tuple[int, int, int, int, int]:
    """Return a portable identity that changes when a search directory mutates.

    A missing directory has an explicit identity; creating it therefore changes
    the snapshot without probing every possible executable name on every plan
    build.
    """
    try:
        stat = os.stat(path)
    except OSError:
        return (-1, 0, 0, 0, 0)
    return (
        0,
        int(stat.st_dev),
        int(stat.st_ino),
        int(stat.st_mtime_ns),
        int(stat.st_size),
    )


@functools.lru_cache(maxsize=32)
def _cached_managed_llvm_bin_directories(
    roots: tuple[str, ...],
    toolchain_directory_identities: tuple[tuple[int, int, int, int, int], ...],
    target_family: LlvmTargetFamily,
) -> tuple[Path, ...]:
    del toolchain_directory_identities
    candidates: list[Path] = []
    for root_string in roots:
        root = Path(root_string)
        toolchains = root / TOOLCHAINS_DIRNAME
        candidates.append(root / "bin")
        if target_family == "wasm":
            candidates.append(toolchains / "wasi-sdk" / "bin")
        if toolchains.is_dir():
            candidates.extend(
                child / "bin"
                for child in sorted(toolchains.iterdir(), reverse=True)
                if child.is_dir()
                and (
                    child.name.startswith("llvm-")
                    or (target_family == "wasm" and child.name.startswith("wasi-sdk-"))
                )
            )
    return _dedupe_paths(candidates)


def _managed_llvm_bin_directories(
    target_root: Path | None,
    *,
    environment: Mapping[str, str],
    target_family: LlvmTargetFamily,
) -> tuple[Path, ...]:
    roots: list[Path] = []
    if target_root is not None:
        roots.append(target_root)
    raw_target_root = executable_environment_value(
        environment, "MOLT_TARGET_ROOT"
    ).strip()
    if raw_target_root:
        roots.append(Path(raw_target_root))
    roots.extend(checkout / "target" for checkout in _source_checkout_roots())

    normalized_roots = tuple(
        map(os.fspath, _dedupe_search_directories(roots, environment=environment))
    )
    identities = tuple(
        _directory_identity(Path(root) / TOOLCHAINS_DIRNAME)
        for root in normalized_roots
    )
    return _cached_managed_llvm_bin_directories(
        normalized_roots, identities, target_family
    )


def _selected_wasi_sdk_bins(environment: Mapping[str, str]) -> tuple[Path, ...]:
    """Project existing SDK/sysroot selectors without changing native PATH."""
    roots = []
    for name in ("WASI_SDK_PATH", "WASI_SDK_PREFIX"):
        if raw := executable_environment_value(environment, name).strip():
            roots.append(expand_user_path(raw, environment=environment))
    for name in ("MOLT_WASI_SYSROOT", "WASI_SYSROOT"):
        if raw := executable_environment_value(environment, name).strip():
            root = _wasi_sdk_root_for_sysroot(
                expand_user_path(raw, environment=environment)
            )
            if root is not None:
                roots.append(root)
    return _dedupe_search_directories(
        (root / "bin" for root in roots), environment=environment
    )


def _observe_provenance_path(
    path: Path, observations: _ProvenanceObservations
) -> _PathIdentity:
    key = os.path.normcase(os.path.abspath(path))
    if key not in observations:
        observations[key] = _directory_identity(path)
    return observations[key]


@functools.lru_cache(maxsize=512)
def _cached_provenance_path(
    path: str, identity: _PathIdentity, parent_identity: _PathIdentity
) -> Path:
    del identity, parent_identity
    return Path(path).resolve(strict=False)


def _resolved_provenance_path(
    path: Path, observations: _ProvenanceObservations
) -> Path:
    # Following stat observes alias retargets; the lexical parent also catches
    # a file alias retargeted between hardlinks with identical content identity.
    return _cached_provenance_path(
        os.path.normcase(os.path.abspath(path)),
        _observe_provenance_path(path, observations),
        _observe_provenance_path(path.parent, observations),
    )


@functools.lru_cache(maxsize=256)
def _cached_sdk_bin_layout(
    directory: str,
    shared_sysroot_identity: _PathIdentity,
    sysroot_identity: _PathIdentity,
) -> bool:
    del shared_sysroot_identity, sysroot_identity
    root = Path(directory).parent
    return (root / "share" / "wasi-sysroot").is_dir() or (
        root / "wasi-sysroot"
    ).is_dir()


def _is_wasi_sdk_bin(
    directory: Path,
    selected_bins: tuple[Path, ...],
    observations: _ProvenanceObservations,
) -> bool:
    """Classify SDK search provenance, not a compiler's guessed target.

    Cover managed SDK names, explicitly selected SDKs, and relocated SDK trees.
    Resolve directory aliases only for admission; invocation keeps its lexical
    entrypoint (wasm-ld may alias lld). Caller-supplied siblings are automatic
    search inputs, not authorization to cross the native/WASM family boundary.
    """
    resolved = _resolved_provenance_path(directory, observations)
    if any(
        resolved == _resolved_provenance_path(selected, observations)
        for selected in selected_bins
    ):
        return True
    if resolved.name.lower() != "bin":
        return False
    root = resolved.parent
    return (
        root.name.lower() == "wasi-sdk"
        or root.name.lower().startswith("wasi-sdk-")
        or _cached_sdk_bin_layout(
            os.fspath(resolved),
            _observe_provenance_path(root / "share" / "wasi-sysroot", observations),
            _observe_provenance_path(root / "wasi-sysroot", observations),
        )
    )


def _tool_is_wasi_sdk(
    path: Path, sdk_bins: tuple[Path, ...], observations: _ProvenanceObservations
) -> bool:
    return _is_wasi_sdk_bin(path.parent, sdk_bins, observations) or _is_wasi_sdk_bin(
        _resolved_provenance_path(path, observations).parent, sdk_bins, observations
    )


def llvm_tool_is_wasi_sdk(
    path: Path, *, environment: Mapping[str, str] | None = None
) -> bool:
    """Identify SDK provenance while retaining a tool's lexical entrypoint."""
    effective_environment = os.environ if environment is None else environment
    sdk_bins = _selected_wasi_sdk_bins(effective_environment)
    return _tool_is_wasi_sdk(path, sdk_bins, {})


def _native_search_environment(
    environment: Mapping[str, str],
    selected_bins: tuple[Path, ...],
    observations: _ProvenanceObservations,
) -> dict[str, str]:
    result = dict(environment)
    directories = executable_search_directories(environment=environment, cwd=Path.cwd())
    admitted = [
        directory
        for directory in directories
        if not _is_wasi_sdk_bin(directory, selected_bins, observations)
    ]
    if admitted != list(directories):
        # Reuse the executable authority's quoted-PATH and implicit-cwd rules.
        # Preserve captured key spelling, including equivalent Windows aliases.
        keys = [
            key for key in result if (key.upper() if os.name == "nt" else key) == "PATH"
        ]
        for key in keys or ["PATH"]:
            result[key] = os.pathsep.join(map(str, admitted))
        if os.name == "nt":
            result["NoDefaultCurrentDirectoryInExePath"] = "1"
    return result


def _rust_llvm_bin_directories(*, environment: Mapping[str, str]) -> tuple[Path, ...]:
    """Return rustc-matched LLVM tool directories for LTO object readers."""
    probe_cwd = Path.cwd()
    try:
        rustc = resolve_executable(
            executable_environment_value(environment, "RUSTC", "rustc"),
            environment=environment,
            label="Rust LLVM toolchain",
        )
        result = _run_completed_command(
            [str(rustc), "--print", "sysroot"],
            capture_output=True,
            timeout=10,
            env=dict(environment),
            cwd=probe_cwd,
            memory_guard_prefix=None,
        )
        if result.returncode != 0:
            return ()
        sysroot = rustc_printed_sysroot(result.stdout, cwd=probe_cwd)
        version = _run_completed_command(
            [str(rustc), "-vV"],
            capture_output=True,
            timeout=10,
            env=dict(environment),
            cwd=probe_cwd,
            memory_guard_prefix=None,
        )
        if version.returncode != 0:
            return ()
        search = RustToolSearch(rustc_host(version.stdout), sysroot, sysroot)
        return tuple(
            directory for directory in search.directories() if directory.is_dir()
        )
    except (OSError, ValueError, subprocess.SubprocessError):
        return ()


def _directory_candidates(
    directory: Path, names: Sequence[str], *, environment: Mapping[str, str]
) -> Iterable[Path]:
    for name in names:
        for executable_name in executable_name_candidates(
            name, environment=environment
        ):
            candidate = directory / executable_name
            if candidate.is_file():
                yield candidate


def _dedupe_search_directories(
    paths: Iterable[Path], *, environment: Mapping[str, str]
) -> tuple[Path, ...]:
    """Deduplicate caller search directories without filesystem probing.

    Candidate results are still canonicalized by ``_dedupe_paths`` on a cache
    miss.  Keeping cache-key construction free of ``Path.resolve`` prevents the
    memoization layer from repeating the filesystem work it exists to remove.
    """
    seen: set[str] = set()
    result: list[Path] = []
    for path in paths:
        absolute = Path(
            os.path.abspath(expand_user_path(path, environment=environment))
        )
        key = os.path.normcase(os.fspath(absolute))
        if key in seen:
            continue
        seen.add(key)
        result.append(absolute)
    return tuple(result)


@functools.lru_cache(maxsize=256)
def _observed_search_directories(
    search_directories: tuple[str, ...],
    environment: tuple[tuple[str, str], ...],
    cwd: str,
) -> tuple[Path, ...]:
    """Reuse normalized directory objects while their configuration is stable."""
    return _dedupe_search_directories(
        (
            *map(Path, search_directories),
            *executable_search_directories(
                environment=dict(environment), cwd=Path(cwd)
            ),
        ),
        environment=dict(environment),
    )


@dataclass(frozen=True)
class _LlvmCandidateSnapshot:
    paths: tuple[Path, ...]
    provenance: tuple[tuple[str, _PathIdentity], ...]


@functools.lru_cache(maxsize=256)
def _cached_llvm_named_tool_candidates(
    names: tuple[str, ...],
    explicit_commands: tuple[tuple[str, ...], ...],
    search_directories: tuple[str, ...],
    directory_identities: tuple[tuple[int, int, int, int, int], ...],
    environment_items: tuple[tuple[str, str], ...],
    cwd: str,
    module_file: str,
    finder_identity: int,
    target_family: LlvmTargetFamily,
) -> _LlvmCandidateSnapshot:
    """Resolve one immutable, filesystem-identified tool-search snapshot.

    All configuration and directory identities that can select a different
    ladder are part of the key. Selected paths are also existence-checked on
    every hit by the public wrapper.
    """
    del (
        directory_identities,
        module_file,
        finder_identity,
    )
    environment = dict(environment_items)
    sdk_bins = _selected_wasi_sdk_bins(environment)
    observations: _ProvenanceObservations = {}
    explicit_paths = (
        path
        for command in explicit_commands
        if command and (path := Path(command[0])).is_file()
    )
    paths = list(explicit_paths)
    for directory in map(Path, search_directories):
        paths.extend(_directory_candidates(directory, names, environment=environment))
    for name in names:
        resolved = find_executable(name, environment=environment, cwd=Path(cwd))
        if (
            resolved is not None
            and target_family == "native"
            and _tool_is_wasi_sdk(Path(resolved), sdk_bins, observations)
        ):
            # File aliases can cross the family boundary even when their PATH
            # directory is not SDK-owned. Continue the canonical executable
            # ladder instead of hiding a later native tool behind that alias.
            resolved = next(
                (
                    candidate
                    for candidate in executable_candidates(
                        name, environment=environment, cwd=Path(cwd)
                    )
                    if not _tool_is_wasi_sdk(candidate, sdk_bins, observations)
                ),
                None,
            )
        if resolved is not None:
            paths.append(Path(resolved))
    result = _dedupe_tool_paths(paths)
    if target_family == "native":
        explicit_selections = {
            Path(command[0]) for command in explicit_commands if command
        }
        result = tuple(
            path
            for path in result
            if path in explicit_selections
            or not _tool_is_wasi_sdk(path, sdk_bins, observations)
        )
    # Include rejected aliases: their resolved SDK can become native without
    # changing the lexical PATH directory or any previously selected candidate.
    return _LlvmCandidateSnapshot(result, tuple(observations.items()))


def clear_llvm_tool_candidate_cache() -> None:
    """Invalidate process-local tool selection after an explicit tool mutation."""
    _cached_source_checkout_roots.cache_clear()
    _cached_managed_llvm_bin_directories.cache_clear()
    _observed_search_directories.cache_clear()
    _cached_llvm_named_tool_candidates.cache_clear()
    _cached_provenance_path.cache_clear()
    _cached_sdk_bin_layout.cache_clear()


def llvm_tool_candidate_cache_info() -> dict[str, int]:
    """Expose bounded cache telemetry to profilers and contract tests."""
    info = _cached_llvm_named_tool_candidates.cache_info()
    return {
        "hits": info.hits,
        "misses": info.misses,
        "maxsize": int(info.maxsize or 0),
        "currsize": info.currsize,
    }


def llvm_tool_candidates(
    role: LlvmToolRole,
    *,
    explicit_commands: Sequence[tuple[str, ...]] = (),
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    include_rust_toolchain: bool = False,
    environment: Mapping[str, str] | None = None,
    target_family: LlvmTargetFamily = "native",
    target_triple: str | None = None,
) -> tuple[Path, ...]:
    """Return one deterministic candidate ladder for every LLVM/WASI consumer."""
    if role == "wasm_ld":
        return llvm_linker_candidates(
            "wasm-ld",
            explicit_commands=explicit_commands,
            sibling_directories=sibling_directories,
            target_root=target_root,
            include_rust_toolchain=include_rust_toolchain,
            environment=environment,
            target_family=target_family,
        )
    names = (
        ("clang-cl",)
        if role in {"cc", "cxx"}
        and target_triple is not None
        and target_triple.endswith("-windows-msvc")
        else _LLVM_TOOL_NAMES[role]
    )
    return llvm_named_tool_candidates(
        *names,
        explicit_commands=explicit_commands,
        sibling_directories=sibling_directories,
        target_root=target_root,
        include_rust_toolchain=include_rust_toolchain,
        environment=environment,
        target_family=target_family,
    )


def llvm_linker_candidates(
    role: LlvmLinkerRole,
    *,
    explicit_commands: Sequence[tuple[str, ...]] = (),
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    include_rust_toolchain: bool = False,
    environment: Mapping[str, str] | None = None,
    target_family: LlvmTargetFamily = "native",
) -> tuple[Path, ...]:
    """Return only entrypoints that select one exact LLVM linker role."""

    candidates = llvm_named_tool_candidates(
        role,
        explicit_commands=explicit_commands,
        sibling_directories=sibling_directories,
        target_root=target_root,
        include_rust_toolchain=include_rust_toolchain,
        environment=environment,
        target_family=target_family,
    )
    return tuple(
        path for path in candidates if executable_selects_linker_role(path, role)
    )


def _is_wasm_ld_entrypoint(path: Path) -> bool:
    """Require the role-selecting wasm-ld name on every host.

    A physical file may be shared with ``lld`` through a symlink or hardlink,
    but invoking the generic driver is not equivalent to invoking its wasm role.
    Accept the Windows executable suffix without letting a generic ``lld`` path
    cross the role boundary.
    """

    return executable_selects_linker_role(path, "wasm-ld")


def _command_selects_path(command: tuple[str, ...] | None, path: Path) -> bool:
    if not command:
        return False
    executable = Path(command[0])
    return executable.is_file() and _absolute_tool_path(executable) == path


def llvm_named_tool_candidates(
    *names: str,
    explicit_commands: Sequence[tuple[str, ...]] = (),
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    include_rust_toolchain: bool = False,
    environment: Mapping[str, str] | None = None,
    target_family: LlvmTargetFamily = "native",
) -> tuple[Path, ...]:
    """Resolve an LLVM utility through the canonical managed-tool ladder.

    The fixed role API above remains the authority for required toolchain roles.
    Inspection and profiling utilities such as ``llvm-readobj`` use this generic
    entry point so they do not grow a second PATH/managed-tool resolver.
    """
    if not names or any(not name for name in names):
        raise ValueError("at least one non-empty LLVM tool name is required")
    if target_family not in {"native", "wasm"}:
        raise ValueError(f"unknown LLVM target family: {target_family!r}")
    effective_environment = dict(os.environ if environment is None else environment)
    sdk_bins = _selected_wasi_sdk_bins(effective_environment)
    observations: _ProvenanceObservations = {}
    normalized_explicit_commands = tuple(
        _selected_tool_command(command, environment=effective_environment)
        for command in explicit_commands
    )
    search_directories: list[Path] = list(sibling_directories)
    if target_family == "wasm":
        search_directories.extend(sdk_bins)
    if include_rust_toolchain:
        search_directories.extend(
            _rust_llvm_bin_directories(environment=effective_environment)
        )
    search_directories.extend(
        _managed_llvm_bin_directories(
            target_root, environment=effective_environment, target_family=target_family
        )
    )
    normalized_search_directories = tuple(
        map(
            os.fspath,
            _dedupe_search_directories(
                search_directories, environment=effective_environment
            ),
        )
    )
    if target_family == "native":
        normalized_search_directories = tuple(
            directory
            for directory in normalized_search_directories
            if not _is_wasi_sdk_bin(Path(directory), sdk_bins, observations)
        )
        effective_environment = _native_search_environment(
            effective_environment, sdk_bins, observations
        )
    environment_items = tuple(sorted(effective_environment.items()))
    cwd = os.path.normcase(os.path.abspath(os.curdir))
    observed_directories = _observed_search_directories(
        (
            *normalized_search_directories,
            *(
                os.fspath(Path(command[0]).absolute().parent)
                for command in normalized_explicit_commands
                if command
            ),
        ),
        environment_items,
        cwd,
    )
    directory_identities = tuple(
        _observe_provenance_path(path, observations) for path in observed_directories
    )
    key = (
        tuple(names),
        normalized_explicit_commands,
        normalized_search_directories,
        directory_identities,
        environment_items,
        cwd,
        os.path.abspath(__file__),
        id(find_executable),
        target_family,
    )
    snapshot = _cached_llvm_named_tool_candidates(*key)
    if any(
        _observe_provenance_path(Path(path), observations) != identity
        for path, identity in snapshot.provenance
    ):
        # A file alias's target tree can change SDK classification independently
        # of the lexical search directories. Rebuild the same candidate ladder.
        _cached_llvm_named_tool_candidates.cache_clear()
        snapshot = _cached_llvm_named_tool_candidates(*key)
    if not all(path.is_file() for path in snapshot.paths):
        # Selected-path check backs up coarse directory timestamps.
        clear_llvm_tool_candidate_cache()
        snapshot = _cached_llvm_named_tool_candidates(*key)
    return snapshot.paths


def _tool_version(
    path: Path, *, environment: Mapping[str, str] | None = None
) -> str | None:
    try:
        result = _run_completed_command(
            [str(path), "--version"],
            capture_output=True,
            timeout=10,
            env=None if environment is None else dict(environment),
            cwd=path.parent,
            memory_guard_prefix=None,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    lines = (result.stdout or result.stderr).splitlines()
    return lines[0].strip() if lines and lines[0].strip() else None


def _selected_tool_command(
    command: Sequence[str], *, environment: Mapping[str, str]
) -> tuple[str, ...]:
    if not command:
        return ()
    selected = _absolute_tool_path(
        expand_user_path(command[0], environment=environment)
    )
    return (str(selected), *command[1:])


def resolve_llvm_wasi_tool_family(
    *,
    target_family: LlvmTargetFamily,
    explicit_commands: Mapping[LlvmToolRole, tuple[str, ...]] | None = None,
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    environment: Mapping[str, str] | None = None,
) -> LlvmWasiToolFamily:
    """Resolve and attest the complete LLVM/WASI binary family exactly once."""
    effective_environment = dict(os.environ if environment is None else environment)
    explicit = {
        role: _selected_tool_command(command, environment=effective_environment)
        for role, command in (explicit_commands or {}).items()
    }
    resolved: dict[LlvmToolRole, ResolvedLlvmTool | None] = {}
    search_directories = list(sibling_directories)
    identity_by_path: dict[str, tuple[str | None, str]] = {}
    for role in _LLVM_TOOL_ROLES:
        command = explicit.get(role)
        candidates = llvm_tool_candidates(
            role,
            explicit_commands=(command,) if command is not None else (),
            sibling_directories=search_directories,
            target_root=target_root,
            environment=effective_environment,
            target_family=target_family,
        )
        if not candidates:
            resolved[role] = None
            continue
        path = candidates[0]
        search_directories.append(path.parent)
        key = os.path.normcase(os.path.realpath(path))
        if key not in identity_by_path:
            identity_by_path[key] = (
                _tool_version(path, environment=effective_environment),
                _sha256_file(path),
            )
        version, sha256 = identity_by_path[key]
        selected_command = (str(path),)
        if command is not None and _command_selects_path(command, path):
            selected_command = command
        resolved[role] = ResolvedLlvmTool(
            role=role,
            command=selected_command,
            path=path,
            version=version,
            sha256=sha256,
        )
    return LlvmWasiToolFamily(
        cc=resolved["cc"],
        cxx=resolved["cxx"],
        wasm_ld=resolved["wasm_ld"],
        ar=resolved["ar"],
        ranlib=resolved["ranlib"],
        nm=resolved["nm"],
        strip=resolved["strip"],
    )
