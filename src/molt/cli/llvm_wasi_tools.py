from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
import functools
import os
from pathlib import Path
import shlex
import shutil
import subprocess
from typing import Literal

from molt.cli.command_runtime import _run_completed_command
from molt.file_hashing import _sha256_file
from molt.llvm_linker_roles import (
    LlvmLinkerRole,
    executable_selects_linker_role,
    lexical_executable_path,
)


LlvmToolRole = Literal["cc", "cxx", "wasm_ld", "ar", "ranlib", "nm", "strip"]

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


def _path_like_command(command: str) -> bool:
    return (
        Path(command).is_absolute()
        or "/" in command
        or "\\" in command
        or (os.altsep is not None and os.altsep in command)
    )


def resolve_explicit_tool_command(raw_command: str, *, label: str) -> tuple[str, ...]:
    """Parse and resolve one explicitly configured executable command."""
    direct_path = Path(raw_command).expanduser()
    if direct_path.is_file():
        return (str(_absolute_tool_path(direct_path)),)
    try:
        argv = shlex.split(raw_command, posix=os.name != "nt")
    except ValueError as exc:
        raise ValueError(f"{label} is not a valid shell command: {exc}") from exc
    if os.name == "nt":
        argv = [
            argument[1:-1]
            if len(argument) >= 2 and argument[0] == argument[-1] == '"'
            else argument
            for argument in argv
        ]
    if not argv:
        raise ValueError(f"{label} is empty")
    executable = argv[0]
    if _path_like_command(executable):
        path = Path(executable).expanduser()
        if not path.exists() or not path.is_file():
            raise ValueError(f"{label} executable not found: {executable}")
        return (str(_absolute_tool_path(path)), *argv[1:])
    resolved = shutil.which(executable)
    if resolved is None:
        raise ValueError(f"{label} executable not found on PATH: {executable}")
    return (str(_absolute_tool_path(Path(resolved))), *argv[1:])


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
        resolved = path.expanduser().resolve(strict=False)
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
) -> tuple[Path, ...]:
    del toolchain_directory_identities
    candidates: list[Path] = []
    for root_string in roots:
        root = Path(root_string)
        toolchains = root / "toolchains"
        candidates.extend((root / "bin", toolchains / "wasi-sdk" / "bin"))
        if toolchains.is_dir():
            candidates.extend(
                child / "bin"
                for child in sorted(toolchains.iterdir(), reverse=True)
                if child.is_dir()
                and (
                    child.name.startswith("llvm-") or child.name.startswith("wasi-sdk-")
                )
            )
    return _dedupe_paths(candidates)


def _managed_llvm_bin_directories(target_root: Path | None) -> tuple[Path, ...]:
    roots: list[Path] = []
    if target_root is not None:
        roots.append(target_root)
    raw_target_root = os.environ.get("MOLT_TARGET_ROOT", "").strip()
    if raw_target_root:
        roots.append(Path(raw_target_root))
    roots.extend(checkout / "target" for checkout in _source_checkout_roots())

    normalized_roots = tuple(map(os.fspath, _dedupe_search_directories(roots)))
    identities = tuple(
        _directory_identity(Path(root) / "toolchains") for root in normalized_roots
    )
    return _cached_managed_llvm_bin_directories(normalized_roots, identities)


def _rust_llvm_bin_directories() -> tuple[Path, ...]:
    """Return rustc-matched LLVM tool directories for LTO object readers."""
    try:
        result = _run_completed_command(
            ["rustc", "--print", "sysroot"],
            capture_output=True,
            timeout=10,
            env=None,
            cwd=None,
            memory_guard_prefix=None,
        )
    except (OSError, subprocess.SubprocessError):
        return ()
    if result.returncode != 0 or not result.stdout.strip():
        return ()
    sysroot = Path(result.stdout.strip())
    return _dedupe_paths(
        path.parent for path in sysroot.glob("lib/rustlib/*/bin/llvm-nm*")
    )


def _executable_names(name: str) -> tuple[str, ...]:
    if os.name != "nt":
        return (name,)
    raw_extensions = os.environ.get("PATHEXT", ".COM;.EXE;.BAT;.CMD")
    extensions = tuple(
        extension.lower() for extension in raw_extensions.split(";") if extension
    )
    if name.lower().endswith(extensions):
        return (name,)
    return (name, *(name + extension for extension in extensions))


def _directory_candidates(directory: Path, names: Sequence[str]) -> Iterable[Path]:
    for name in names:
        for executable_name in _executable_names(name):
            candidate = directory / executable_name
            if candidate.is_file():
                yield candidate


def _dedupe_search_directories(paths: Iterable[Path]) -> tuple[Path, ...]:
    """Deduplicate caller search directories without filesystem probing.

    Candidate results are still canonicalized by ``_dedupe_paths`` on a cache
    miss.  Keeping cache-key construction free of ``Path.resolve`` prevents the
    memoization layer from repeating the filesystem work it exists to remove.
    """
    seen: set[str] = set()
    result: list[Path] = []
    for path in paths:
        absolute = Path(os.path.abspath(os.fspath(path.expanduser())))
        key = os.path.normcase(os.fspath(absolute))
        if key in seen:
            continue
        seen.add(key)
        result.append(absolute)
    return tuple(result)


@functools.lru_cache(maxsize=32)
def _path_search_directories(path_value: str, cwd: str) -> tuple[Path, ...]:
    """Return directories whose identity governs ``shutil.which`` results."""
    directories: list[Path] = [Path(cwd)]
    for raw in path_value.split(os.pathsep):
        value = raw.strip().strip('"')
        directories.append(Path(value) if value else Path(cwd))
    return _dedupe_search_directories(directories)


@functools.lru_cache(maxsize=256)
def _observed_search_directories(
    search_directories: tuple[str, ...], path_value: str, cwd: str
) -> tuple[Path, ...]:
    """Reuse normalized directory objects while their configuration is stable."""
    return _dedupe_search_directories(
        (
            *map(Path, search_directories),
            *_path_search_directories(path_value, cwd),
        )
    )


@functools.lru_cache(maxsize=256)
def _cached_llvm_named_tool_candidates(
    names: tuple[str, ...],
    explicit_commands: tuple[tuple[str, ...], ...],
    search_directories: tuple[str, ...],
    directory_identities: tuple[tuple[int, int, int, int, int], ...],
    target_root: str | None,
    include_rust_toolchain: bool,
    environment_target_root: str,
    path_value: str,
    path_extensions: str,
    rust_environment: tuple[str, str, str],
    cwd: str,
    module_file: str,
    which_identity: int,
) -> tuple[Path, ...]:
    """Resolve one immutable, filesystem-identified tool-search snapshot.

    All configuration and directory identities that can select a different
    ladder are part of the key. Selected paths are also existence-checked on
    every hit by the public wrapper.
    """
    del (
        directory_identities,
        environment_target_root,
        path_value,
        path_extensions,
        rust_environment,
        cwd,
        module_file,
        which_identity,
    )
    del target_root, include_rust_toolchain
    explicit_paths = (
        path
        for command in explicit_commands
        if command and (path := Path(command[0])).is_file()
    )
    paths = list(explicit_paths)
    for directory in map(Path, search_directories):
        paths.extend(_directory_candidates(directory, names))
    for name in names:
        # PATH is in the cache key.  Calling with the ambient default preserves
        # Python's exact platform-specific current-directory/PATHEXT semantics.
        resolved = shutil.which(name)
        if resolved is not None:
            paths.append(Path(resolved))
    return _dedupe_tool_paths(paths)


def clear_llvm_tool_candidate_cache() -> None:
    """Invalidate process-local tool selection after an explicit tool mutation."""
    _cached_source_checkout_roots.cache_clear()
    _cached_managed_llvm_bin_directories.cache_clear()
    _path_search_directories.cache_clear()
    _observed_search_directories.cache_clear()
    _cached_llvm_named_tool_candidates.cache_clear()


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
) -> tuple[Path, ...]:
    """Return one deterministic candidate ladder for every LLVM/WASI consumer."""
    if role == "wasm_ld":
        return llvm_linker_candidates(
            "wasm-ld",
            explicit_commands=explicit_commands,
            sibling_directories=sibling_directories,
            target_root=target_root,
            include_rust_toolchain=include_rust_toolchain,
        )
    return llvm_named_tool_candidates(
        *_LLVM_TOOL_NAMES[role],
        explicit_commands=explicit_commands,
        sibling_directories=sibling_directories,
        target_root=target_root,
        include_rust_toolchain=include_rust_toolchain,
    )


def llvm_linker_candidates(
    role: LlvmLinkerRole,
    *,
    explicit_commands: Sequence[tuple[str, ...]] = (),
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    include_rust_toolchain: bool = False,
) -> tuple[Path, ...]:
    """Return only entrypoints that select one exact LLVM linker role."""

    candidates = llvm_named_tool_candidates(
        role,
        explicit_commands=explicit_commands,
        sibling_directories=sibling_directories,
        target_root=target_root,
        include_rust_toolchain=include_rust_toolchain,
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
    executable = Path(command[0]).expanduser()
    return executable.is_file() and _absolute_tool_path(executable) == path


def llvm_named_tool_candidates(
    *names: str,
    explicit_commands: Sequence[tuple[str, ...]] = (),
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    include_rust_toolchain: bool = False,
) -> tuple[Path, ...]:
    """Resolve an LLVM utility through the canonical managed-tool ladder.

    The fixed role API above remains the authority for required toolchain roles.
    Inspection and profiling utilities such as ``llvm-readobj`` use this generic
    entry point so they do not grow a second PATH/managed-tool resolver.
    """
    if not names or any(not name for name in names):
        raise ValueError("at least one non-empty LLVM tool name is required")
    normalized_explicit_commands = tuple(
        tuple(command) for command in explicit_commands
    )
    search_directories: list[Path] = list(sibling_directories)
    if include_rust_toolchain:
        search_directories.extend(_rust_llvm_bin_directories())
    search_directories.extend(_managed_llvm_bin_directories(target_root))
    normalized_search_directories = tuple(
        map(os.fspath, _dedupe_search_directories(search_directories))
    )
    normalized_target_root = (
        os.fspath(Path(os.path.abspath(os.fspath(target_root.expanduser()))))
        if target_root is not None
        else None
    )
    path_value = os.environ.get("PATH", os.defpath)
    path_extensions = os.environ.get("PATHEXT", "")
    cwd = os.path.normcase(os.path.abspath(os.curdir))
    observed_directories = _observed_search_directories(
        normalized_search_directories, path_value, cwd
    )
    directory_identities = tuple(
        _directory_identity(path) for path in observed_directories
    )
    key = (
        tuple(names),
        normalized_explicit_commands,
        normalized_search_directories,
        directory_identities,
        normalized_target_root,
        include_rust_toolchain,
        os.environ.get("MOLT_TARGET_ROOT", "").strip(),
        path_value,
        path_extensions,
        (
            os.environ.get("RUSTUP_TOOLCHAIN", ""),
            os.environ.get("RUSTUP_HOME", ""),
            os.environ.get("CARGO_HOME", ""),
        ),
        cwd,
        os.path.abspath(__file__),
        id(shutil.which),
    )
    result = _cached_llvm_named_tool_candidates(*key)
    if all(path.is_file() for path in result):
        return result

    # A direct selected-path check is the fail-closed backstop for filesystems
    # whose directory timestamp granularity cannot expose a rapid removal.
    clear_llvm_tool_candidate_cache()
    return _cached_llvm_named_tool_candidates(*key)


def _tool_version(path: Path) -> str | None:
    try:
        result = _run_completed_command(
            [str(path), "--version"],
            capture_output=True,
            timeout=10,
            env=None,
            cwd=path.parent,
            memory_guard_prefix=None,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    lines = (result.stdout or result.stderr).splitlines()
    return lines[0].strip() if lines and lines[0].strip() else None


def resolve_llvm_wasi_tool_family(
    *,
    explicit_commands: Mapping[LlvmToolRole, tuple[str, ...]] | None = None,
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
) -> LlvmWasiToolFamily:
    """Resolve and attest the complete LLVM/WASI binary family exactly once."""
    explicit = dict(explicit_commands or {})
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
        )
        if not candidates:
            resolved[role] = None
            continue
        path = candidates[0]
        search_directories.append(path.parent)
        key = os.path.normcase(os.path.realpath(path))
        if key not in identity_by_path:
            identity_by_path[key] = (_tool_version(path), _sha256_file(path))
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
