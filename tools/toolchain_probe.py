"""Shared fail-closed decoding for toolchain path-probe output."""

from __future__ import annotations

from pathlib import Path


def resolve_single_file_path(output: str, *, probe_cwd: Path) -> Path:
    """Resolve exactly one probe-emitted file path against its declared cwd."""

    candidates = tuple(line.strip() for line in output.splitlines() if line.strip())
    if len(candidates) != 1:
        raise ValueError("toolchain path probe must emit exactly one non-empty line")
    candidate = Path(candidates[0])
    if not candidate.is_absolute():
        candidate = probe_cwd / candidate
    resolved = candidate.resolve(strict=True)
    if not resolved.is_file():
        raise ValueError(f"toolchain path probe did not name a file: {resolved}")
    return resolved
