import pytest
from molt.target_python import resolve_target_python_for_oracle


@pytest.mark.parametrize("minor", [12, 13, 14])
def test_oracle_target_matches_each_supported_minor(minor):
    assert resolve_target_python_for_oracle((3, minor)).short == f"3.{minor}"


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
