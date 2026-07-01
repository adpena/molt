#!/usr/bin/env python3
from __future__ import annotations

import os
import subprocess
from pathlib import Path

import bench


def _resolve_pypy(arg: str) -> str | None:
    """Resolve a PyPy interpreter path (explicit, or auto-detect 3.11/3.10)."""
    import shutil

    if arg and arg != "__auto__":
        return arg if Path(arg).exists() else shutil.which(arg)
    for cand in (
        "/opt/homebrew/bin/pypy3.11",
        "/opt/homebrew/bin/pypy3.10",
        "/opt/homebrew/bin/pypy3",
        shutil.which("pypy3.11") or "",
        shutil.which("pypy3.10") or "",
        shutil.which("pypy3") or "",
    ):
        if cand and Path(cand).exists():
            return cand
    return None


def _resolve_codon(arg: str) -> str | None:
    """Resolve a Codon binary path (explicit, or auto-detect ~/.codon/bin)."""
    import shutil

    if arg and arg != "__auto__":
        return arg if Path(arg).exists() else shutil.which(arg)
    default = Path.home() / ".codon" / "bin" / "codon"
    if default.exists():
        return str(default)
    return shutil.which("codon")


def _probe_interp_version(interp_bin: str | None) -> str | None:
    if not interp_bin:
        return None
    from perf_scoreboard import _metadata_probe

    res = _metadata_probe([interp_bin, "--version"], timeout_s=30)
    if res is None:
        return None
    out = (res.stdout or res.stderr or "").strip().splitlines()
    return out[0].replace("Python ", "") if out else None


def _probe_codon_version(codon_bin: str | None) -> str | None:
    if not codon_bin:
        return None
    from perf_scoreboard import _metadata_probe

    res = _metadata_probe([codon_bin, "--version"], timeout_s=30)
    if res is None:
        return None
    out = (res.stdout or res.stderr or "").strip()
    return f"codon {out}" if out else None


def _path_executable_candidates(name: str) -> list[str]:
    path = Path(name)
    if path.is_absolute() or path.parent != Path("."):
        return [name]

    suffixes = [""]
    if os.name == "nt" and not path.suffix:
        suffixes = [
            ext.lower()
            for ext in os.environ.get("PATHEXT", ".COM;.EXE;.BAT;.CMD").split(
                os.pathsep
            )
            if ext
        ]

    out: list[str] = []
    seen: set[str] = set()
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        if not directory:
            continue
        for suffix in suffixes:
            candidate = Path(directory) / f"{name}{suffix}"
            key = str(candidate).lower()
            if key in seen:
                continue
            seen.add(key)
            if candidate.is_file():
                out.append(str(candidate))
    return out


def _canonical_interpreter_cmd(raw_cmd: tuple[str, ...]) -> tuple[str, ...]:
    if not raw_cmd or not raw_cmd[0]:
        raise FileNotFoundError("empty CPython candidate command")
    return (bench._canonical_interpreter(raw_cmd[0]), *raw_cmd[1:])


def _is_project_managed_interpreter(path: str) -> bool:
    normalized = path.replace("\\", "/").lower()
    return (
        "/.venv/" in normalized
        or "/target/sessions/" in normalized
        or "/sessions/" in normalized
    )


def _normalize_arch(machine: str) -> str:
    normalized = machine.strip().lower().replace("-", "_").replace(" ", "")
    if normalized in {"amd64", "x64", "x86_64"}:
        return "x86_64"
    if normalized in {"arm64", "aarch64"}:
        return "aarch64"
    if normalized in {"i386", "i486", "i586", "i686", "x86"}:
        return "x86"
    return normalized or "unknown"


def _python_version_key(version: str) -> tuple[int, int, int]:
    parts: list[int] = []
    for raw in version.split(".")[:3]:
        digits = "".join(ch for ch in raw if ch.isdigit())
        parts.append(int(digits) if digits else 0)
    while len(parts) < 3:
        parts.append(0)
    return (parts[0], parts[1], parts[2])


def _format_cmd(cmd: tuple[str, ...]) -> str:
    return " ".join(cmd)


def _probe_tail(res: subprocess.CompletedProcess[str]) -> str:
    lines = [
        line.strip()
        for text in (res.stdout, res.stderr)
        for line in (text or "").splitlines()
        if line.strip()
    ]
    return " | ".join(lines[-2:])[:240] or "no output"
