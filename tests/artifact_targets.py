"""Artifact targets for tests that name a target by its spelling.

Production resolves a build's target once, at its layout boundary
(`build_output_layout._runtime_facts_target`). A test that asks a fact about
one target admits that spelling here; None resolves the host now, so a test
that replaces the host resolver sees its replacement.
"""

from __future__ import annotations

from molt.compilation_target import (
    ArtifactTarget,
    TargetKind,
    admit_target,
    resolve_artifact_target,
)


def artifact_target(triple: str | None) -> ArtifactTarget:
    return resolve_artifact_target(
        TargetKind.NATIVE if triple is None else admit_target(triple)
    )
