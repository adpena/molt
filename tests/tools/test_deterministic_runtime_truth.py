from __future__ import annotations

import json
from pathlib import Path

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
    monkeypatch.setattr(
        determinism,
        "build_program",
        lambda *_args, **_kwargs: (str(binary), "", {"status": "ok"}),
    )
    monkeypatch.setattr(
        determinism, "run_binary", lambda *_args, **_kwargs: next(observations)
    )

    result = determinism.check_determinism(str(source), 2, "dev")

    assert result["status"] == "fail"
    assert result["deterministic"] is False
    assert result["diffs"][0]["stderr_changed"] is True


def test_observations_share_the_compiler_build_but_not_program_state(
    tmp_path: Path, monkeypatch
) -> None:
    """A per-observation Cargo target rebuilt the runtime for every build.

    Every observation must reuse the warm compiler roots it inherits and get
    its own cache, output path and no backend daemon.
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
        return determinism.subprocess.CompletedProcess(
            cmd, 0, stdout=json.dumps({"data": {"output": str(output)}}), stderr=""
        )

    monkeypatch.setattr(
        determinism.harness_memory_guard, "guarded_completed_process", fake_build
    )
    monkeypatch.setattr(
        determinism, "run_binary", lambda *_args, **_kwargs: (b"ok\n", b"", 0)
    )

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
