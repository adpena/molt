"""Each build configuration owns its intermediates; one build holds them at a time.

Two different programs whose entries share a file name once shared
``<MOLT_HOME>/build/<stem>``, and concurrent builds linked each other's objects:
4 of 9 binaries ran another program's code.
"""

from __future__ import annotations

from molt.compilation_target import admit_target

from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import sys

import pytest

from molt.cli import build_output_layout as layout
from molt.cli.output import read_json_payload
from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[2]


def _identity(project: Path, source: Path, target="native", profile="dev") -> str:
    return layout.artifacts_identity(
        project_root=project,
        source_path=source,
        target=admit_target(target),
        profile=profile,
    )


def test_identity_names_the_program_and_its_configuration(tmp_path: Path) -> None:
    first, second = tmp_path / "a" / "main.py", tmp_path / "b" / "main.py"
    for source in (first, second):
        source.parent.mkdir()
        source.write_text("print(1)\n", encoding="utf-8")
    alias = tmp_path / "alias"
    alias.symlink_to(first.parent, target_is_directory=True)

    base = _identity(tmp_path, first)
    assert base == _identity(tmp_path, alias / "main.py")
    assert (
        len(
            {
                base,
                _identity(tmp_path, second),
                _identity(tmp_path / "a", first),
                _identity(tmp_path, first, target="wasm"),
                _identity(tmp_path, first, profile="release"),
            }
        )
        == 5
    )


def test_same_named_programs_get_their_own_artifacts_roots(tmp_path: Path) -> None:
    roots = []
    with layout.build_scope():
        for name in ("a", "b"):
            source = tmp_path / name / "main.py"
            roots.append(
                layout._resolve_output_roots(
                    tmp_path,
                    tmp_path / "out",
                    "main",
                    identity=_identity(tmp_path, source),
                )[0]
            )
    assert roots[0] != roots[1]
    assert all(root.parent == tmp_path / "out" / ".molt_build" for root in roots)


def test_an_artifacts_root_is_claimed_only_inside_a_build(tmp_path: Path) -> None:
    with pytest.raises(RuntimeError, match="only inside build_scope"):
        layout._resolve_output_roots(
            tmp_path, tmp_path / "out", "main", identity="0" * 16
        )


def _held_elsewhere(lock: Path) -> bool:
    probe = (
        "import sys\n"
        "from pathlib import Path\n"
        "from molt.file_locks import _release_file_lock, _try_acquire_file_lock\n"
        "handle = _try_acquire_file_lock(Path(sys.argv[1]))\n"
        "print('held' if handle is None else 'free')\n"
        "if handle is not None:\n"
        "    _release_file_lock(handle)\n"
    )
    result = run_guarded_test_process(
        [sys.executable, "-c", probe, str(lock)],
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONPATH": str(ROOT / "src")},
        timeout=60.0,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout.strip() == "held"


def test_a_build_holds_its_root_until_it_returns(tmp_path: Path) -> None:
    with layout.build_scope():
        root = layout._resolve_output_roots(
            tmp_path, tmp_path / "out", "main", identity="1" * 16
        )[0]
        # Resolving the same root again in one build does not wait on itself.
        assert (
            layout._resolve_output_roots(
                tmp_path, tmp_path / "out", "main", identity="1" * 16
            )[0]
            == root
        )
        assert _held_elsewhere(root / ".molt-build.lock")
    assert not _held_elsewhere(root / ".molt-build.lock")


_PROGRAMS = {
    "a": 'x = 10\nprint("big" if x > 5 else "small")\n',
    "b": "val = 7\nprint(val * 2)\n",
    "c": "x = 5\nprint(x * 2 + x)\n",
}
# The shared root linked another program's objects in 4 of 9 builds.
_ROUNDS = 3


@pytest.mark.slow
def test_concurrent_builds_of_same_named_programs_run_their_own_code(
    tmp_path: Path,
) -> None:
    # Guest optimization and host compiler optimization are independent. This
    # artifact-ownership proof reuses the dev-fast compiler admitted by the
    # developer daemon proofs; a separate release-fast host costs another build.
    build_env = {
        **os.environ,
        "MOLT_BACKEND_PROFILE": "dev",
        "MOLT_DEV_BACKEND_CARGO_PROFILE": "dev-fast",
    }

    def build_and_run(job: tuple[int, str]) -> tuple[str, str]:
        round_index, key = job
        project = tmp_path / f"{key}{round_index}"
        source = project / "src" / "original.py"
        source.parent.mkdir(parents=True)
        source.write_text(_PROGRAMS[key], encoding="utf-8")
        binary = project / "out" / "original"
        build = run_guarded_test_process(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                "dev",
                "--json",
                "--output",
                str(binary),
                str(source),
            ],
            capture_output=True,
            text=True,
            env=build_env,
            timeout=900.0,
        )
        # The daemon can append peer-request log lines to a failure. Retain
        # the complete structured diagnostic instead of losing its cause to
        # a successful peer's trailing output.
        assert build.returncode == 0, (
            f"build {job} exited {build.returncode}\n{build.stdout}\n{build.stderr}"
        )
        profiles = read_json_payload(build.stdout)["data"]["observed_toolchain"][
            "selected_profiles"
        ]
        assert profiles["compiler_profile"] == "dev-fast"
        assert profiles["guest_profile"] == "dev"
        # A build must transfer its shared compiler daemon to the suite. Even
        # when all programs finish first, command cleanup must not kill it.
        assert not build.orphaned_process_groups, build.termination_reports
        expected = run_guarded_test_process(
            [sys.executable, str(source)],
            capture_output=True,
            text=True,
            timeout=60.0,
            check=True,
        ).stdout
        actual = run_guarded_test_process(
            [str(binary)], capture_output=True, text=True, timeout=60.0, check=True
        ).stdout
        return expected, actual

    for round_index in range(_ROUNDS):
        with ThreadPoolExecutor(len(_PROGRAMS)) as pool:
            jobs = [(round_index, key) for key in _PROGRAMS]
            results = list(pool.map(build_and_run, jobs))
        for expected, actual in results:
            assert actual == expected
