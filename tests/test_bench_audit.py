"""Actual audit consumers retain or close the canonical benchmark allocation."""

from __future__ import annotations

import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

from tools import bench  # noqa: E402
from tools import bench_audit as audit  # noqa: E402
from molt import temporary_artifacts  # noqa: E402


@pytest.mark.parametrize("phase", ["median", "perf-rebuild"])
@pytest.mark.parametrize("outcome", ["success", "unavailable", "error", "cancel"])
@pytest.mark.parametrize("cleanup_failure", [False, True])
def test_audit_consumers_close_or_retain_exact_owner(
    monkeypatch, tmp_path, phase, outcome, cleanup_failure
):
    owners = []
    original = (
        KeyboardInterrupt("independent audit cancellation")
        if outcome == "cancel"
        else OSError("independent audit observation failure")
    )
    target_count = 1 if phase == "median" else 2

    def build(*args, **kwargs):
        owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
        owners.append(owner)
        artifact = Path(owner.name) / "program"
        artifact.write_bytes(b"independent artifact")
        return bench.MoltBinary(artifact, owner, 0.1, 1.0)

    def observe(path, *args, **kwargs):
        assert Path(path).is_file()
        if len(owners) == target_count:
            if outcome in {"error", "cancel"}:
                raise original
            if outcome == "unavailable":
                return None
        return bench.RunSample(1.0, "", "")

    def perf(path):
        result = observe(path)
        return None if result is None else {"instructions": 123}

    monkeypatch.setattr(audit, "prepare_molt_binary", build)
    monkeypatch.setattr(audit, "measure_molt_run", observe)
    monkeypatch.setattr(audit, "_collect_perf_data", perf)
    monkeypatch.setattr(audit, "_cpython_median", lambda *a: 2.0)
    monkeypatch.setattr(audit, "platform", SimpleNamespace(system=lambda: "Linux"))
    monkeypatch.setattr(
        audit, "shutil", SimpleNamespace(which=lambda command: "/fixture/perf")
    )
    real_delete = temporary_artifacts.delete_path

    def delete(path):
        if len(owners) == target_count and path == Path(owners[-1].name):
            return False, "independent audit deletion refusal"
        return real_delete(path)

    unrelated = RuntimeError("unrelated caller exception")

    def run():
        try:
            raise unrelated
        except RuntimeError:
            if phase == "median":
                return audit._molt_median(str(tmp_path / "ordinary.py"), 1, 1.0)
            return audit.audit_benchmarks([str(tmp_path / "ordinary.py")], samples=1)

    with monkeypatch.context() as scoped:
        if cleanup_failure:
            scoped.setattr(temporary_artifacts, "delete_path", delete)
        if outcome in {"error", "cancel"} or cleanup_failure:
            with pytest.raises(
                type(original) if outcome in {"error", "cancel"} else OSError
            ) as caught:
                run()
            if outcome in {"error", "cancel"}:
                assert caught.value is original
            if cleanup_failure:
                assert caught.value.benchmark_temporary_directories == (owners[-1],)
                assert Path(owners[-1].name).is_dir()
        else:
            result = run()
            if phase == "median":
                assert result == (None if outcome == "unavailable" else 1.0, 0.1)
            else:
                assert result[0].perf_data == (
                    None if outcome == "unavailable" else {"instructions": 123}
                )
    assert not hasattr(unrelated, "benchmark_temporary_directories")
    assert len(owners) == target_count
    for owner in owners:
        if Path(owner.name).exists():
            assert cleanup_failure and owner is owners[-1]
            owner.cleanup()
        assert not Path(owner.name).exists()
