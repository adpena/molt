#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path


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
    from tools.perf_scoreboard import _metadata_probe

    res = _metadata_probe([interp_bin, "--version"], timeout_s=30)
    if res is None:
        return None
    out = (res.stdout or res.stderr or "").strip().splitlines()
    return out[0].replace("Python ", "") if out else None


def _probe_codon_version(codon_bin: str | None) -> str | None:
    if not codon_bin:
        return None
    from tools.perf_scoreboard import _metadata_probe

    res = _metadata_probe([codon_bin, "--version"], timeout_s=30)
    if res is None:
        return None
    out = (res.stdout or res.stderr or "").strip()
    return f"codon {out}" if out else None
