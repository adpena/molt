import pytest
from molt import target_python_policy
from molt.target_python import resolve_target_python_for_oracle


@pytest.mark.parametrize("minor", [12, 13, 14])
def test_oracle_target_matches_each_supported_minor(minor):
    assert resolve_target_python_for_oracle((3, minor)).short == f"3.{minor}"


@pytest.mark.parametrize(
    "short", target_python_policy.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
)
def test_runtime_release_matches_pinned_source_independently_of_frontend(
    monkeypatch, short
):
    from packaging.version import Version
    from types import SimpleNamespace
    from molt import target_python
    from tools.cpython_regrtest import load_cpython_sources

    # The pin tag is an independent release oracle, not the generated header
    # parser or the interpreter running the compiler/test.
    upstream = Version(load_cpython_sources()[short].tag.removeprefix("v"))
    level, serial = (
        (
            {"a": "alpha", "b": "beta", "rc": "candidate"}[upstream.pre[0]],
            upstream.pre[1],
        )
        if upstream.pre
        else ("final", 0)
    )
    monkeypatch.setattr(
        target_python, "sys", SimpleNamespace(version_info=(9, 99, 999))
    )
    target = target_python._parse_target_python_version(short)
    assert (
        target.major,
        target.minor,
        target.micro,
        target.release,
        target.serial,
    ) == (upstream.major, upstream.minor, upstream.micro, level, serial)
    assert resolve_target_python_for_oracle(target.feature_version) == target
    assert target_python._parse_target_python_version(f"{short}.0") == target


@pytest.mark.parametrize(
    "oracle,explicit",
    [(None, None), ((3, 11), None), ((3, 14), "3.12"), ((3, 12), "3.12.0")],
)
def test_oracle_target_rejects_missing_unsupported_or_mismatched_coordinate(
    oracle, explicit
):
    with pytest.raises(ValueError):
        resolve_target_python_for_oracle(oracle, explicit)


@pytest.mark.parametrize("profile", ["release-fast", "relase", "unknown"])
def test_differential_profile_typo_cannot_silently_run_dev(monkeypatch, profile):
    from tests import molt_diff

    monkeypatch.setenv("MOLT_DIFF_BUILD_PROFILE", profile)
    with pytest.raises(ValueError, match="MOLT_DIFF_BUILD_PROFILE"):
        molt_diff._diff_build_profile()


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_differential_profile_preserves_explicit_selection(monkeypatch, profile):
    from tests import molt_diff

    monkeypatch.setenv("MOLT_DIFF_BUILD_PROFILE", profile)
    assert molt_diff._diff_build_profile() == profile


def test_compact_target_tags_preserve_full_catalog_identity(monkeypatch):
    from molt import target_python as authority

    # The live catalog remains the admission authority: no future target is
    # accepted merely because its dotted or compact spelling can be parsed.
    with pytest.raises(ValueError):
        authority._parse_target_python_version("4.0")
    with pytest.raises(ValueError):
        authority._parse_target_python_version("py40")
    versions = (
        authority.TargetPythonVersion(3, 0, 0),
        authority.TargetPythonVersion(4, 0, 0),
    )
    monkeypatch.setattr(authority, "_SUPPORTED_TARGET_PYTHON_VERSIONS", versions)
    monkeypatch.setattr(
        authority,
        "_SUPPORTED_TARGET_PYTHON_BY_SHORT",
        {target.short: target for target in versions},
    )
    monkeypatch.setattr(
        target_python_policy, "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS", ("3.0", "4.0")
    )
    assert authority._parse_target_python_version("py30").feature_version == (3, 0)
    assert authority._parse_target_python_version("py40").feature_version == (4, 0)
    assert authority._parse_target_python_version("4.0").feature_version == (4, 0)
    assert authority.resolve_target_python_for_oracle(
        (4, 0), "4.0"
    ).feature_version == (4, 0)
    with pytest.raises(ValueError, match="canonical <major>.<minor>"):
        authority.resolve_target_python_for_oracle((4, 0), "4.0.0")
    with pytest.raises(ValueError):
        authority._parse_target_python_version("4.1")
