from __future__ import annotations

from pathlib import Path
import shutil
import subprocess
import sys

from molt.cli.command_runtime import _run_completed_command


def _crate_name_from_archive_member(member_name: str) -> str | None:
    """Return the normalized Rust crate name encoded in a staticlib member.

    Rust archives contain members such as
    ``molt_runtime_core-<hash>.molt_runtime_core.<hash>-cgu.0.rcgu.o``.
    Cargo build output directories use package names with ``-`` separators,
    while object members use crate names with ``_`` separators. Normalizing
    both forms lets the custom linker ignore stale build-script outputs from
    crates that are not actually present in the runtime archive.
    """
    name = member_name.strip()
    if not name or name == "__.SYMDEF":
        return None
    stem = name.split(".", 1)[0]
    if stem.endswith("-static"):
        return None
    if "-" in stem:
        stem = stem.rsplit("-", 1)[0]
    if not stem or stem[0].isdigit():
        return None
    return stem.replace("-", "_")


def _runtime_archive_crate_names(runtime_lib: Path) -> frozenset[str]:
    """Return normalized crate names present in a built Rust staticlib."""
    archive_tool = shutil.which("llvm-ar") or shutil.which("ar")
    if archive_tool is None:
        return frozenset()
    try:
        result = _run_completed_command(
            [archive_tool, "-t", str(runtime_lib)],
            capture_output=True,
            timeout=30,
            env=None,
            cwd=runtime_lib.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
    except (OSError, subprocess.SubprocessError):
        return frozenset()
    if result.returncode != 0:
        return frozenset()
    crates = {
        crate
        for line in result.stdout.splitlines()
        if (crate := _crate_name_from_archive_member(line)) is not None
    }
    return frozenset(crates)


def _crate_name_from_cargo_build_dir(entry_name: str) -> str:
    """Normalize a Cargo ``target/<profile>/build/<pkg-hash>`` directory name."""
    package = entry_name
    if "-" in entry_name:
        head, suffix = entry_name.rsplit("-", 1)
        if suffix and all(ch in "0123456789abcdefABCDEF" for ch in suffix):
            package = head
    return package.replace("-", "_")


def _collect_cargo_native_link_deps(runtime_lib: Path) -> tuple[list[str], list[str]]:
    """Collect native library link flags from cargo build-script output files.

    When the runtime static library is built by cargo, crate build scripts
    (e.g. lzma-sys) emit ``cargo:rustc-link-lib=`` and
    ``cargo:rustc-link-search=`` directives. Cargo normally consumes these
    during its own link step, but Molt performs a custom link, so we must
    forward them ourselves.

    Returns ``(search_paths, link_libs)`` -- lists of ``-L<path>`` and
    ``-l<lib>`` flags ready to append to the linker command.
    """
    search_paths: list[str] = []
    link_libs: list[str] = []
    profile_dir = runtime_lib.parent
    build_dir = profile_dir / "build"
    if not build_dir.is_dir():
        return search_paths, link_libs
    active_crates = _runtime_archive_crate_names(runtime_lib)
    seen_libs: set[str] = set()
    for entry in build_dir.iterdir():
        if active_crates:
            build_crate = _crate_name_from_cargo_build_dir(entry.name)
            if build_crate not in active_crates:
                continue
        output_file = entry / "output"
        if not output_file.is_file():
            continue
        try:
            text = output_file.read_text(errors="replace")
        except OSError:
            continue
        for line in text.splitlines():
            if line.startswith("cargo:rustc-link-search="):
                raw = line[len("cargo:rustc-link-search=") :]
                if "=" in raw:
                    raw = raw.split("=", 1)[1]
                if raw and raw not in seen_libs:
                    search_paths.append(f"-L{raw}")
                    seen_libs.add(raw)
            elif line.startswith("cargo:rustc-link-lib="):
                raw = line[len("cargo:rustc-link-lib=") :]
                if raw.startswith("static=") or raw.startswith("static:"):
                    continue
                kind = None
                if "=" in raw:
                    kind, raw = raw.split("=", 1)
                elif ":" in raw:
                    kind, raw = raw.split(":", 1)
                if not raw:
                    continue
                if kind in {"framework", "weak_framework"}:
                    key = f"{kind}:{raw}"
                    if key not in seen_libs:
                        if kind == "weak_framework":
                            link_libs.extend(["-weak_framework", raw])
                        else:
                            link_libs.extend(["-framework", raw])
                        seen_libs.add(key)
                else:
                    if raw not in seen_libs:
                        link_libs.append(f"-l{raw}")
                        seen_libs.add(raw)
    return search_paths, link_libs


def _native_target_is_windows(target_triple: str | None) -> bool:
    triple = (target_triple or "").lower()
    return (
        ("windows" in triple or "msvc" in triple)
        if target_triple
        else sys.platform == "win32"
    )


def _native_windows_system_link_libs(target_triple: str | None) -> list[str]:
    """Return Windows system libraries required by Molt's Rust runtime surface."""
    if not _native_target_is_windows(target_triple):
        return []
    return ["-lws2_32", "-lntdll", "-luserenv", "-ladvapi32"]
