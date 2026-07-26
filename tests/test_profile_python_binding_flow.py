from tools.profile_python_binding_flow import _fit_nonnegative_log_log_slope


def _samples(values: list[int | None]) -> list[dict[str, object]]:
    return [
        {
            "ast_nodes": 100 * (2**index),
            "memory": {"bytes": value},
        }
        for index, value in enumerate(values)
    ]


def test_nonnegative_slope_fails_closed_when_telemetry_is_unavailable() -> None:
    assert (
        _fit_nonnegative_log_log_slope(
            _samples([100, None, 400]), "memory", "bytes"
        )
        is None
    )


def test_nonnegative_slope_accepts_zero_only_telemetry() -> None:
    assert (
        _fit_nonnegative_log_log_slope(
            _samples([0, 0, 0]), "memory", "bytes"
        )
        == 0.0
    )


def test_nonnegative_slope_rejects_mixed_zero_telemetry() -> None:
    assert (
        _fit_nonnegative_log_log_slope(
            _samples([0, 100, 200]), "memory", "bytes"
        )
        is None
    )
