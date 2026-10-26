from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path
from molt.process_environment import ProcessEnvironment

from molt.dx import (
    ARTIFACT_ROOT_ENV,
    CANONICAL_RUN_ENV_KEYS,
    development_artifact_env,
)


def build_molt_conformance_env(
    project_root: Path,
    session_id: str | None,
    base: Mapping[str, str],
    *,
    create_dirs: bool = False,
) -> ProcessEnvironment:
    """Resolve a conformance run's DX roots from the caller's environment.

    The caller's artifact-root and custody selectors (``MOLT_EXT_ROOT``, CI's
    ephemeral custody root) decide where artifacts go; resolving from the
    session ID alone put them in the checkout, which CI custody refuses. The
    roots derived from them are recomputed, so ambient values never leak in.

    A ``session_id`` pins the run's session, which scopes its Cargo target.
    ``None`` lets the run context generate one, so the run builds in the warm
    shared target.

    Python environment selection belongs to the caller. DX alone creates
    writable artifact directories; only the image publisher creates managed
    Python environments and their read-only bytecode namespaces.
    """
    selectors = ProcessEnvironment(base).without(
        [
            key
            for key in CANONICAL_RUN_ENV_KEYS
            if key not in {ARTIFACT_ROOT_ENV, "UV_PROJECT_ENVIRONMENT"}
        ]
        + ["PYTHONPATH"]
    )
    if session_id is not None:
        selectors = selectors.updated({"MOLT_SESSION_ID": session_id})
    return development_artifact_env(
        project_root,
        selectors,
        session_prefix="conformance",
        create_dirs=create_dirs,
    )


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
