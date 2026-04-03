from __future__ import annotations

import json
from pathlib import Path


def build_molt_conformance_env(project_root: Path, session_id: str) -> dict[str, str]:
    target_dir = project_root / "target"
    tmp_dir = project_root / "tmp"
    return {
        "MOLT_EXT_ROOT": str(project_root),
        "CARGO_TARGET_DIR": str(target_dir),
        "MOLT_DIFF_CARGO_TARGET_DIR": str(target_dir),
        "MOLT_CACHE": str(project_root / ".molt_cache"),
        "MOLT_DIFF_ROOT": str(tmp_dir / "diff"),
        "MOLT_DIFF_TMPDIR": str(tmp_dir),
        "UV_CACHE_DIR": str(project_root / ".uv-cache"),
        "TMPDIR": str(tmp_dir),
        "PYTHONPATH": str(project_root / "src"),
        "MOLT_SESSION_ID": session_id,
    }


def ensure_molt_conformance_dirs(env: dict[str, str]) -> None:
    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
    ):
        Path(env[key]).mkdir(parents=True, exist_ok=True)


def load_molt_conformance_suite(
    corpus_dir: Path, suite: str, smoke_manifest: Path
) -> list[Path]:
    if suite == "full":
        return sorted(path for path in corpus_dir.glob("*.py") if path.is_file())
    if suite != "smoke":
        raise ValueError(f"unknown suite: {suite}")

    selected: list[Path] = []
    for raw_line in smoke_manifest.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        selected.append(corpus_dir / line)
    return selected


def write_molt_conformance_summary(path: Path, summary: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(summary, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )


def conformance_exit_code(summary: dict[str, object]) -> int:
    for key in ("failed", "compile_error", "timeout"):
        value = summary.get(key, 0)
        if not isinstance(value, int):
            raise TypeError(f"summary field {key!r} must be an int")
        if value > 0:
            return 1
    return 0
