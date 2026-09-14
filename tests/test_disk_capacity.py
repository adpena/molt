from __future__ import annotations

from decimal import localcontext
import json
from pathlib import Path

import pytest

from molt.disk_capacity import (
    DEFAULT_MINIMUM_HEADROOM_BYTES,
    DISK_GUARD_HIGH_WATER_ENV,
    DiskCapacityError,
    minimum_headroom_bytes,
    require_build_capacity,
)


def test_minimum_headroom_default_and_fractional_override_rounds_up() -> None:
    assert minimum_headroom_bytes({}) == 25 * 1024**3
    assert DEFAULT_MINIMUM_HEADROOM_BYTES == 25 * 1024**3
    assert minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: "0.0000000001"}) == 1


def test_minimum_headroom_is_independent_of_decimal_context() -> None:
    with localcontext() as context:
        context.prec = 1
        assert (
            minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: "25.0000000001"})
            == DEFAULT_MINIMUM_HEADROOM_BYTES + 1
        )


def test_extreme_positive_fraction_has_one_byte_minimum_without_large_power() -> None:
    assert minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: "1e-1000000"}) == 1


@pytest.mark.parametrize(
    "configured",
    [
        "",
        " ",
        "not-a-number",
        "0",
        "-1",
        "nan",
        "NaN",
        "inf",
        "+Infinity",
        "1e999999",
        "\u0660.\u0660",
        "\uff11",
        "1e\u0662",
    ],
)
def test_malformed_nonpositive_or_nonfinite_threshold_rejects(
    configured: str,
) -> None:
    with pytest.raises(DiskCapacityError) as caught:
        minimum_headroom_bytes({DISK_GUARD_HIGH_WATER_ENV: configured})

    error = caught.value
    assert error.diagnostic["status"] == "invalid-configuration"
    assert error.diagnostic["configured_value"] == configured
    assert "positive finite" in str(error)
    assert "diagnostic=" in str(error)


def test_capacity_equal_to_threshold_is_admitted(tmp_path: Path) -> None:
    output = tmp_path / "not-created" / "target"
    required = minimum_headroom_bytes({})

    receipt = require_build_capacity(
        [output],
        env={},
        measure_free_bytes=lambda path: required,
    )

    assert not output.exists()
    assert receipt.required_bytes == required
    assert receipt.probes[0].requested_path == output.resolve()
    assert receipt.probes[0].measured_path == tmp_path.resolve()
    assert receipt.probes[0].free_bytes == required
    assert receipt.as_dict()["status"] == "admitted"


def test_capacity_above_threshold_is_admitted(tmp_path: Path) -> None:
    required = minimum_headroom_bytes({})

    receipt = require_build_capacity(
        [tmp_path / "target"],
        env={},
        measure_free_bytes=lambda path: required + 1,
    )

    assert receipt.probes[0].free_bytes == required + 1


def test_one_byte_below_threshold_rejects_with_serialized_diagnostic(
    tmp_path: Path,
) -> None:
    required = minimum_headroom_bytes({})

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [tmp_path / "target"],
            env={},
            measure_free_bytes=lambda path: required - 1,
        )

    error = caught.value
    assert issubclass(DiskCapacityError, ValueError)
    assert error.diagnostic["status"] == "rejected"
    assert error.diagnostic["probes"][0]["free_bytes"] == required - 1
    serialized = str(error).split("diagnostic=", 1)[1]
    assert json.loads(serialized) == error.diagnostic
    assert "Reclaim verified inactive artifacts" in str(error)
    assert "explicitly permitted build output root" in str(error)


def test_multiple_roots_are_deterministic_and_one_low_rejects(
    tmp_path: Path,
) -> None:
    first = tmp_path / "a-root"
    second = tmp_path / "z-root"
    first.mkdir()
    second.mkdir()
    required = minimum_headroom_bytes({})
    free_by_path = {
        first.resolve(): required - 1,
        second.resolve(): required + 1,
    }

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [second / "out", first / "out"],
            env={},
            measure_free_bytes=lambda path: free_by_path[path],
        )

    probes = caught.value.diagnostic["probes"]
    assert [probe["requested_path"] for probe in probes] == [
        str((first / "out").resolve()),
        str((second / "out").resolve()),
    ]
    assert [probe["free_bytes"] for probe in probes] == [
        required - 1,
        required + 1,
    ]


def test_measurement_failure_rejects_and_records_existing_ancestor(
    tmp_path: Path,
) -> None:
    output = tmp_path / "missing" / "nested" / "target"

    def fail_measurement(path: Path) -> int:
        raise OSError("volume is unavailable")

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [output],
            env={},
            measure_free_bytes=fail_measurement,
        )

    probe = caught.value.diagnostic["probes"][0]
    assert probe["requested_path"] == str(output.resolve())
    assert probe["measured_path"] == str(tmp_path.resolve())
    assert probe["free_bytes"] is None
    assert "volume is unavailable" in probe["error"]
    assert not output.exists()


def test_unmeasurable_ancestor_rejects_without_falling_back(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output = (tmp_path / "blocked" / "target").resolve()
    original_stat = Path.stat
    measured: list[Path] = []

    def denied_stat(path: Path, *args: object, **kwargs: object):
        if path == output:
            raise PermissionError("ancestor access denied")
        return original_stat(path, *args, **kwargs)

    monkeypatch.setattr(Path, "stat", denied_stat)

    with pytest.raises(DiskCapacityError) as caught:
        require_build_capacity(
            [output],
            env={},
            measure_free_bytes=lambda path: measured.append(path) or 10**15,
        )

    probe = caught.value.diagnostic["probes"][0]
    assert probe["measured_path"] is None
    assert "ancestor access denied" in probe["error"]
    assert measured == []
    monkeypatch.undo()
    assert not output.exists()


def test_repeated_resolved_output_is_measured_once(tmp_path: Path) -> None:
    output = tmp_path / "target"
    observed: list[Path] = []

    receipt = require_build_capacity(
        [output, tmp_path / "." / "target"],
        env={},
        measure_free_bytes=lambda path: observed.append(path) or 10**15,
    )

    assert len(receipt.probes) == 1
    assert observed == [tmp_path.resolve()]
    assert not output.exists()


def test_invalid_configuration_fails_before_filesystem_measurement(
    tmp_path: Path,
) -> None:
    observed: list[Path] = []

    with pytest.raises(DiskCapacityError):
        require_build_capacity(
            [tmp_path / "target"],
            env={DISK_GUARD_HIGH_WATER_ENV: "0"},
            measure_free_bytes=lambda path: observed.append(path) or 10**15,
        )

    assert observed == []
    assert not (tmp_path / "target").exists()


def test_no_output_paths_rejects() -> None:
    with pytest.raises(DiskCapacityError, match="at least one output path"):
        require_build_capacity([], env={}, measure_free_bytes=lambda path: 10**15)
