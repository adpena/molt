"""Target-addressed build control, independent of disposable output placement."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path

from molt.dx import (
    configured_artifact_root,
    control_state_dir,
    project_cargo_target_dir,
)
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
    if configured_artifact_root(environment, relative_to=project_root) is None:
        return cargo_target / ".molt_state"
    target = resolve_owned_path(cargo_target)
    address = canonical_json_sha256(
        {
            "schema": "molt.build-state-target.v1",
            "cargo_target": os.path.normcase(str(target)),
        }
    )
    return resolve_owned_path(
        control_state_dir(project_root, f"build-control/{address}", environment)
    )


def project_build_state_root(
    project_root: Path, environment: Mapping[str, str]
) -> Path:
    """Build control for the project's Cargo target.

    The target is `molt.dx.project_cargo_target_dir`, the one the CLI builds
    into, so the backend daemon, its suite lease and CI consumers find the
    state a build leaves.
    """
    return build_state_root(
        project_root=project_root,
        cargo_target=project_cargo_target_dir(project_root, environment),
        environment=environment,
    )
