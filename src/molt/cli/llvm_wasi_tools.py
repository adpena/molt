from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import shlex
import shutil
import subprocess
from typing import Literal

from molt.cli.command_runtime import _run_completed_command
from molt.cli.file_hashing import _sha256_file


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
        return (str(direct_path.resolve()),)
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
        return (str(path.resolve()), *argv[1:])
    resolved = shutil.which(executable)
    if resolved is None:
        raise ValueError(f"{label} executable not found on PATH: {executable}")
    return (resolved, *argv[1:])


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


def _source_checkout_roots() -> tuple[Path, ...]:
    """Return the loaded checkout and its Git common checkout, if worktree-backed."""
    checkout = Path(__file__).resolve().parents[3]
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


def _managed_llvm_bin_directories(target_root: Path | None) -> tuple[Path, ...]:
    roots: list[Path] = []
    if target_root is not None:
        roots.append(target_root)
    raw_target_root = os.environ.get("MOLT_TARGET_ROOT", "").strip()
    if raw_target_root:
        roots.append(Path(raw_target_root))
    roots.extend(checkout / "target" for checkout in _source_checkout_roots())

    candidates: list[Path] = []
    for root in _dedupe_paths(roots):
        toolchains = root / "toolchains"
        candidates.extend((root / "bin", toolchains / "wasi-sdk" / "bin"))
        if toolchains.is_dir():
            candidates.extend(
                child / "bin"
                for child in sorted(toolchains.iterdir(), reverse=True)
                if child.is_dir()
                and (
                    child.name.startswith("llvm-")
                    or child.name.startswith("wasi-sdk-")
                )
            )
    return _dedupe_paths(candidates)


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
    if os.name != "nt" or Path(name).suffix:
        return (name,)
    raw_extensions = os.environ.get("PATHEXT", ".COM;.EXE;.BAT;.CMD")
    extensions = tuple(
        extension.lower() for extension in raw_extensions.split(";") if extension
    )
    return (name, *(name + extension for extension in extensions))


def _directory_candidates(directory: Path, names: Sequence[str]) -> Iterable[Path]:
    for name in names:
        for executable_name in _executable_names(name):
            candidate = directory / executable_name
            if candidate.is_file():
                yield candidate


def llvm_tool_candidates(
    role: LlvmToolRole,
    *,
    explicit_commands: Sequence[tuple[str, ...]] = (),
    sibling_directories: Sequence[Path] = (),
    target_root: Path | None = None,
    include_rust_toolchain: bool = False,
) -> tuple[Path, ...]:
    """Return one deterministic candidate ladder for every LLVM/WASI consumer."""
    return llvm_named_tool_candidates(
        *_LLVM_TOOL_NAMES[role],
        explicit_commands=explicit_commands,
        sibling_directories=sibling_directories,
        target_root=target_root,
        include_rust_toolchain=include_rust_toolchain,
    )


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
    explicit_paths = (Path(command[0]) for command in explicit_commands if command)
    directories: list[Path] = list(sibling_directories)
    if include_rust_toolchain:
        directories.extend(_rust_llvm_bin_directories())
    directories.extend(_managed_llvm_bin_directories(target_root))
    paths: list[Path] = list(explicit_paths)
    for directory in _dedupe_paths(directories):
        paths.extend(_directory_candidates(directory, names))
    for name in names:
        resolved = shutil.which(name)
        if resolved is not None:
            paths.append(Path(resolved))
    return _dedupe_paths(paths)


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
        key = os.path.normcase(os.fspath(path))
        if key not in identity_by_path:
            identity_by_path[key] = (_tool_version(path), _sha256_file(path))
        version, sha256 = identity_by_path[key]
        resolved[role] = ResolvedLlvmTool(
            role=role,
            command=command if command is not None else (str(path),),
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
