from __future__ import annotations

from pathlib import Path

import molt.cli as cli


def test_prepare_build_config_uses_dev_runtime_profile_for_dev_builds(
    tmp_path: Path,
) -> None:
    prepared, error = cli._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=False,
        profile="dev",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == "dev-fast"


def test_prepare_build_config_uses_release_runtime_profile_for_release_builds(
    tmp_path: Path,
) -> None:
    prepared, error = cli._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=False,
        profile="release",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == "release-fast"
