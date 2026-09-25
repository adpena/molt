"""Target-addressed build control, independent of disposable output placement."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path

from molt.exact_json import canonical_json_sha256
from molt.file_publication import resolve_owned_path


def build_state_root(
    *, project_root: Path, cargo_target: Path, environment: Mapping[str, str]
) -> Path:
    """Project all consumers onto the same control root for one Cargo target.

    Explicit state selection keeps its public project-relative semantics. With
    an artifact root, payload location only addresses control: it never owns it.
    Per-run receipt roots and temporary directories are not identity inputs.
    This projection creates no directories and grants no cleanup authority.
    """
    explicit = environment.get("MOLT_BUILD_STATE_DIR", "").strip()
    if explicit:
        path = Path(explicit).expanduser()
        return path if path.is_absolute() else (project_root / path).absolute()
    artifact = environment.get("MOLT_EXT_ROOT", "").strip()
    if not artifact:
        return cargo_target / ".molt_state"
    target = resolve_owned_path(cargo_target)
    address = canonical_json_sha256(
        {
            "schema": "molt.build-state-target.v1",
            "cargo_target": os.path.normcase(str(target)),
        }
    )
    root = Path(artifact).expanduser()
    if not root.is_absolute():
        root = project_root / root
    return resolve_owned_path(root / "tmp" / "build-control" / address)
