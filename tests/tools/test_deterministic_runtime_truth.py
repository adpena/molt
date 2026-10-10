from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

import pytest

from tools import check_deterministic_runtime as determinism


def test_determinism_requires_two_observations(tmp_path: Path) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")

    result = determinism.check_determinism(str(source), 1, "dev")

    assert result["status"] == "error"
    assert result["error"] == "runs must be at least 2"


def test_stderr_is_part_of_deterministic_observable(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "source.py"
    binary = tmp_path / "program"
    source.write_text("print('ok')\n", encoding="utf-8")
    binary.write_bytes(b"binary")
    observations = iter([(b"same", b"first", 0), (b"same", b"second", 0)])

    def build_program(
        source, profile, *, deterministic, cache_dir, cwd, hash_seed, build_timeout
    ):
        return str(binary), "", {"status": "ok"}, {"status": "completed"}

    def run_binary(binary, run_index, timeout, *, deterministic, cwd):
        return *next(observations), {"status": "completed"}

    monkeypatch.setattr(determinism, "build_program", build_program)
    monkeypatch.setattr(determinism, "run_binary", run_binary)

    result = determinism.check_determinism(str(source), 2, "dev")

    assert result["status"] == "fail"
    assert result["deterministic"] is False
    assert result["diffs"][0]["stderr_changed"] is True


@pytest.mark.parametrize("mode", ["default", "deterministic"])
@pytest.mark.parametrize("temporary_alias", [False, True])
def test_receipt_environment_matches_real_build_and_guest_children(
    tmp_path: Path, monkeypatch, mode: str, temporary_alias: bool
) -> None:
    """Real guarded children prove metadata transport, not Molt code generation."""
    if temporary_alias:
        import tempfile

        physical = tmp_path / "physical-temporary-root"
        physical.mkdir()
        alias = tmp_path / "temporary-alias"
        try:
            alias.symlink_to(physical, target_is_directory=True)
        except OSError as error:
            pytest.skip(f"host does not permit directory symlinks: {error}")
        monkeypatch.setattr(tempfile, "tempdir", str(alias))
    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    witnesses = []
    real_guard = determinism.harness_memory_guard.guarded_completed_process
    monkeypatch.setenv("MOLT_DETERMINISTIC", "ambient-poison")
    monkeypatch.setenv("PROVENANCE_TEST_SECRET", "must-not-be-serialized")
    monkeypatch.delenv("MOLT_BACKEND_DAEMON", raising=False)

    def payload_guard(command, **kwargs):
        if command[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            phase = "build"
            binary = Path(kwargs["cwd"]) / "observed-program"
            tail = (
                f"Path({str(binary)!r}).write_bytes(b'constant fixture artifact'); "
                f"print(json.dumps({{'output': {str(binary)!r}}}))"
            )
        elif len(command) == 1 and Path(command[0]).name == "observed-program":
            phase = "runtime"
            tail = "print('stable')"
        else:
            return real_guard(command, **kwargs)
        witness = tmp_path / f"{len(witnesses)}-{phase}.json"
        witnesses.append((phase, witness))
        # Literal child-side projection is independent of the reporting helper.
        observer = (
            "import json, os, sys; from pathlib import Path; "
            "e = {k: os.environ.get(k) for k in "
            "('PYTHONPATH','PYTHONHASHSEED','MOLT_DETERMINISTIC','MOLT_CACHE',"
            "'MOLT_EXT_ROOT','MOLT_TARGET_ROOT','CARGO_TARGET_DIR',"
            "'MOLT_BACKEND_DAEMON','MOLT_BACKEND_DAEMON_SOCKET_DIR','TMP','TEMP')}; "
            f"Path({str(witness)!r}).write_text(json.dumps("
            "{'environment': e, 'cwd': os.getcwd(), 'argv': sys.orig_argv}), "
            "encoding='utf-8'); " + tail
        )
        return real_guard([sys.executable, "-B", "-c", observer], **kwargs)

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", payload_guard
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_deterministic_runtime.py",
            str(source),
            "--runs",
            "2",
            "--mode",
            mode,
            "--json-out",
            str(receipt),
        ],
    )

    assert determinism.main() == 0
    raw = receipt.read_text(encoding="utf-8")
    payload = json.loads(raw)
    assert payload["schema"] == "molt.deterministic-runtime-proof.v3"
    assert payload["selected"] == payload["executed"] == payload["passed"] == 1
    assert "PROVENANCE_TEST_SECRET" not in raw
    assert "must-not-be-serialized" not in raw
    assert [phase for phase, _ in witnesses] == ["build", "runtime", "build", "runtime"]
    rows = payload["results"][0]["observations"]
    assert len(rows) == 2
    for index, row in enumerate(rows):
        assert row["source"] == source.name
        assert "environment" not in row
        for offset, phase in enumerate(("build", "runtime")):
            observed = json.loads(
                witnesses[2 * index + offset][1].read_text(encoding="utf-8")
            )
            launch = row[phase]
            assert launch["environment"] == observed["environment"]
            assert launch["argv"] == observed["argv"]
            assert launch["cwd"] == observed["cwd"]
            if temporary_alias:
                assert Path(launch["cwd"]).parent == physical
            assert launch["status"] == "completed"
            assert launch["returncode"] == launch["child_returncode"] == 0
            assert launch["environment"]["PYTHONHASHSEED"] == (
                "0" if phase == "build" else str(index + 1)
            )
            assert launch["environment"]["MOLT_DETERMINISTIC"] == (
                "1" if mode == "deterministic" else None
            )
        assert row["build"]["environment"]["MOLT_BACKEND_DAEMON"] == "0"
        assert row["runtime"]["environment"]["MOLT_BACKEND_DAEMON"] is None
    assert (
        rows[0]["build"]["environment"]["MOLT_CACHE"]
        != rows[1]["build"]["environment"]["MOLT_CACHE"]
    )


@pytest.mark.parametrize("failure", ["exit", "timeout", "launch-error"])
def test_failed_build_receipt_does_not_claim_guest_execution(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    commands = []
    real_guard = determinism.harness_memory_guard.guarded_completed_process

    def failed_guard(command, **kwargs):
        if command[:4] != [sys.executable, "-m", "molt.cli", "build"]:
            return real_guard(command, **kwargs)
        commands.append(command)
        if failure == "launch-error":
            raise OSError("child could not start")
        return determinism.harness_memory_guard.GuardedCompletedProcess(
            command,
            124 if failure == "timeout" else 23,
            "",
            "failed",
            elapsed_s=0.01,
            timed_out=failure == "timeout",
            child_returncode=23,
            child_stderr="failed",
        )

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", failed_guard
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_deterministic_runtime.py",
            str(source),
            "--runs",
            "2",
            "--mode",
            "default",
            "--json-out",
            str(receipt),
        ],
    )

    assert determinism.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["selected"] == payload["errors"] == 1
    assert payload["executed"] == payload["passed"] == payload["failed"] == 0
    assert len(commands) == 1
    rows = payload["results"][0]["observations"]
    assert len(rows) == 1
    assert rows[0]["runtime"] is None
    assert rows[0]["build_receipt"] is None
    assert rows[0]["build"]["argv"] == commands[0]
    assert rows[0]["build"]["environment"]["PYTHONHASHSEED"] == "0"
    assert (
        rows[0]["build"]["status"]
        == {
            "exit": "completed",
            "timeout": "timeout",
            "launch-error": "error",
        }[failure]
    )
    assert "binary_sha256" not in rows[0]


def test_timeout_exception_keeps_attempted_guest_environment(
    tmp_path: Path, monkeypatch
) -> None:
    binary = str(tmp_path / "binary")
    real_guard = determinism.harness_memory_guard.guarded_completed_process

    def timeout_guard(command, **kwargs):
        if command != [binary]:
            return real_guard(command, **kwargs)
        raise subprocess.TimeoutExpired(command, kwargs["timeout"])

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", timeout_guard
    )
    stdout, stderr, rc, launch = determinism.run_binary(
        binary, 7, cwd=tmp_path, deterministic=False
    )
    assert (stdout, stderr, rc) == (b"", b"", None)
    assert launch["argv"] == [binary]
    assert launch["cwd"] == str(tmp_path)
    assert launch["status"] == "timeout"
    assert launch["returncode"] is launch["child_returncode"] is None
    assert launch["environment"]["PYTHONHASHSEED"] == "7"
    assert launch["environment"]["MOLT_DETERMINISTIC"] is None


@pytest.mark.parametrize(
    "build_json", [[], None, {"output": 23}, {"data": []}, {"data": {"output": 23}}]
)
def test_wrong_build_json_shape_retains_launch_without_runtime(
    tmp_path: Path, monkeypatch, build_json: object
) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    calls = []
    real_guard = determinism.harness_memory_guard.guarded_completed_process

    def build_guard(command, **kwargs):
        if command[:4] != [sys.executable, "-m", "molt.cli", "build"]:
            return real_guard(command, **kwargs)
        launched = real_guard(
            [sys.executable, "-B", "-c", f"print({json.dumps(build_json)!r})"], **kwargs
        )
        calls.append(launched.args)
        return launched

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", build_guard
    )
    result = determinism.check_determinism(str(source), 2, "dev")
    assert result["status"] == "build_error"
    assert "build artifact error" in result["error"]
    assert result["completed_runs"] == 0
    assert len(calls) == len(result["observations"]) == 1
    row = result["observations"][0]
    assert row["build"]["argv"] == calls[0]
    assert row["build"]["status"] == "completed"
    assert row["build"]["child_returncode"] == 0
    assert row["runtime"] is None


@pytest.mark.parametrize(
    "failure",
    [
        "artifact",
        "cleanup",
        "failed-build-cleanup",
        "identity",
        "failed-build-identity",
    ],
)
def test_later_runtime_failure_retains_launches_and_other_completed_cell(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    """Real child completion survives a subsequent parent-side file failure."""
    from contextlib import contextmanager

    bad = tmp_path / "bad.py"
    good = tmp_path / "good.py"
    for path in (bad, good):
        path.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    real_guard = determinism.harness_memory_guard.guarded_completed_process
    real_temp = determinism.OwnedTemporaryDirectory
    real_hash = determinism._sha256_file
    launches = []
    retained_allocations = []

    def payload_guard(command, **kwargs):
        if command[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            artifact = Path(kwargs["cwd"]) / (Path(command[-1]).stem + "-program")
            script = (
                "import json; from pathlib import Path; "
                f"Path({str(artifact)!r}).write_bytes(b'fixture'); "
                f"print(json.dumps({{'output': {str(artifact)!r}}}))"
            )
        elif len(command) == 1 and Path(command[0]).name in {
            "good-program",
            "bad-program",
        }:
            script = "print('stable')"
        else:
            return real_guard(command, **kwargs)
        if (
            failure in {"failed-build-cleanup", "failed-build-identity"}
            and command[:4] == [sys.executable, "-m", "molt.cli", "build"]
            and Path(command[-1]).name == "bad.py"
            and Path(kwargs["cwd"]).name.startswith("runtime_repeat_1_")
        ):
            script = "raise SystemExit(23)"
        launched = real_guard([sys.executable, "-B", "-c", script], **kwargs)
        launches.append(launched.args)
        return launched

    def artifact_hash(path):
        if (
            failure == "artifact"
            and path.name == "bad-program"
            and path.parent.name.startswith("runtime_repeat_1_")
        ):
            raise OSError()
        return real_hash(path)

    @contextmanager
    def temporary_directory(*, prefix):
        with real_temp(prefix=prefix, dir=tmp_path) as directory:
            yield directory
            if (
                failure in {"identity", "failed-build-identity"}
                and prefix == "runtime_repeat_1_"
                and (Path(directory) / "bad.py").exists()
            ):
                original = Path(directory)
                retained = original.with_name(original.name + "-retained")
                original.rename(retained)
                original.mkdir()
                (original / "replacement").write_bytes(b"other owner")
                retained_allocations.append((original, retained))
            should_fail = (
                failure in {"cleanup", "failed-build-cleanup"}
                and prefix == "runtime_repeat_1_"
                and (Path(directory) / "bad.py").exists()
            )
        if should_fail:
            raise OSError()

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", payload_guard
    )
    monkeypatch.setattr(determinism, "_sha256_file", artifact_hash)
    # Replace only this consumer binding, preserving the shared allocation owner.
    monkeypatch.setattr(determinism, "OwnedTemporaryDirectory", temporary_directory)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_deterministic_runtime.py",
            "--batch",
            str(bad),
            str(good),
            "--mode",
            "default",
            "--runs",
            "2",
            "--json-out",
            str(receipt),
        ],
    )

    assert determinism.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert (
        payload["selected"],
        payload["executed"],
        payload["passed"],
        payload["failed"],
        payload["errors"],
    ) == (2, 1, 1, 0, 1)
    failed, passed = payload["results"]
    assert passed["status"] == "pass" and passed["completed_runs"] == 2
    assert failed["status"] == "error" and failed["completed_runs"] == 1
    first, second = failed["observations"]
    assert first["observable_sha256"]
    assert second["error_phase"] == ("artifact" if failure == "artifact" else "cleanup")
    assert second["error"] == (
        "temporary directory allocation changed before cleanup"
        if failure in {"identity", "failed-build-identity"}
        else "OSError"
    )
    assert second["build"]["status"] == "completed"
    assert (
        second["build"]["returncode"]
        == second["build"]["child_returncode"]
        == (23 if failure in {"failed-build-cleanup", "failed-build-identity"} else 0)
    )
    assert second["build"]["argv"] in launches
    if failure in {"artifact", "failed-build-cleanup", "failed-build-identity"}:
        assert second["runtime"] is None
    else:
        assert second["runtime"]["status"] == "completed"
        assert second["runtime"]["argv"] in launches
        assert second["runtime"]["environment"]["PYTHONHASHSEED"] == "2"
        assert second["observable_sha256"] == first["observable_sha256"]

    if failure in {"identity", "failed-build-identity"}:
        assert len(retained_allocations) == 1
        original, retained = retained_allocations[0]
        assert (original / "replacement").read_bytes() == b"other owner"
        assert (retained / "bad.py").read_bytes() == b"print('stable')\n"


def test_observations_share_the_compiler_build_but_not_program_state(
    tmp_path: Path, monkeypatch
) -> None:
    """A per-observation Cargo target rebuilt the runtime for every build.

    Every observation must reuse the warm compiler roots it inherits and get
    its own cache, output path and no backend daemon. Launch evidence records
    those same inputs.
    """
    shared = {
        "CARGO_TARGET_DIR": str(tmp_path / "shared-target"),
        "MOLT_EXT_ROOT": str(tmp_path / "shared-artifacts"),
        "MOLT_TARGET_ROOT": str(tmp_path / "shared-toolchains"),
    }
    for name, value in shared.items():
        monkeypatch.setenv(name, value)
    monkeypatch.delenv("MOLT_BACKEND_DAEMON", raising=False)
    source = tmp_path / "program.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    builds: list[tuple[list[str], dict[str, str], object, float]] = []

    def fake_build(cmd, *, env, cwd, timeout, **_kwargs):
        builds.append((list(cmd), dict(env), cwd, timeout))
        output = Path(cmd[cmd.index("--output") + 1])
        output.write_bytes(b"binary")
        return determinism.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            0,
            json.dumps({"data": {"output": str(output)}}),
            "",
            elapsed_s=0.0,
            child_returncode=0,
            child_stderr="",
        )

    def fake_run(binary, run_index, timeout, *, deterministic, cwd):
        return b"ok\n", b"", 0, {"status": "completed", "cwd": str(cwd)}

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", fake_build
    )
    monkeypatch.setattr(determinism, "run_binary", fake_run)

    result = determinism.check_determinism(str(source), 2, "dev", build_timeout=42)

    assert result["status"] == "pass"
    assert len(builds) == 2
    caches, outputs, cwds = set(), set(), set()
    for cmd, env, cwd, timeout in builds:
        assert {name: env.get(name) for name in shared} == shared
        assert env["MOLT_BACKEND_DAEMON"] == "0"
        assert timeout == 42
        output = Path(cmd[cmd.index("--output") + 1])
        assert output.parent == Path(cwd)
        caches.add(env["MOLT_CACHE"])
        outputs.add(output)
        cwds.add(cwd)
    assert len(caches) == len(outputs) == len(cwds) == 2
    for observation in result["observations"]:
        recorded = observation["build"]["environment"]
        assert {name: recorded[name] for name in shared} == shared
        assert recorded["MOLT_BACKEND_DAEMON"] == "0"
