"""A compiled program never spawns itself as an interpreter (HF-519).

The probe is built once and run as itself: without `MOLT_SYS_EXECUTABLE`,
`sys.executable` names the program's own binary. CPython would run
`[sys.executable, "-c", source]`; the compiled binary has no interpreter and
would run `main` again, so Molt refuses that launch with `OSError(ENOEXEC)`.
This is a declared divergence under the dynamic execution policy (spec 0216).

A child that runs `main` again only reports and exits, so a missed refusal
costs one extra process, never a recursion on the test host.
"""

from __future__ import annotations

import errno
import os
import sys
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process

_PROBE = r"""
import asyncio
import multiprocessing
import os
import subprocess
import sys

CHILD = "HF519_PROBE_CHILD"


def outcome(launch):
    try:
        result = launch()
    except OSError as exc:
        return f"refused errno={exc.errno} type={type(exc).__name__}"
    return f"ran {result!r}"


def run_capture(argv, env):
    done = subprocess.run(argv, env=env, capture_output=True, text=True)
    return (done.returncode, done.stdout.strip())


async def asyncio_launch(argv, env):
    proc = await asyncio.create_subprocess_exec(
        *argv, env=env, stdout=asyncio.subprocess.PIPE
    )
    out, _ = await proc.communicate()
    return (proc.returncode, out.decode().strip())


def popen_launch(argv, env):
    with subprocess.Popen(argv, env=env, stdout=subprocess.PIPE, text=True) as proc:
        out, _ = proc.communicate()
    return (proc.returncode, out.strip())


def report(name, launch):
    print(name, outcome(launch), flush=True)


def interpreter(env):
    for flag, payload in (("-c", "print(1)"), ("-m", "json.tool")):
        argv = [sys.executable, flag, payload]
        report(f"run{flag}", lambda: run_capture(argv, env))
    argv = [sys.executable, "-u", "-c", "print(1)"]
    report("popen-c", lambda: popen_launch(argv, env))
    report("check_call-c", lambda: subprocess.check_call(argv, env=env))
    report("check_output-c", lambda: subprocess.check_output(argv, env=env))
    report("asyncio-c", lambda: asyncio.run(asyncio_launch(argv, env)))


def worker():
    print("worker ran", flush=True)


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "interpreter"
    if os.environ.get(CHILD) == "1":
        print("child", mode, flush=True)
        if mode == "nested":
            # Depth 1: a self-spawn without a declared role is refused.
            report("grandchild", lambda: run_capture([sys.executable, "leaf"], dict(os.environ)))
        return
    env = dict(os.environ, **{CHILD: "1"})
    if mode == "interpreter":
        interpreter(env)
    elif mode == "nested":
        # Depth 0: the self-spawn runs; its child reports the depth-1 refusal.
        report("nested", lambda: run_capture([sys.executable, "nested"], env))
    elif mode == "other":
        other = os.environ["HF519_OTHER_EXE"]
        report("other", lambda: run_capture([other, "-c", "print('other')"], env))
    elif mode == "multiprocessing":
        proc = multiprocessing.Process(target=worker)
        proc.start()
        proc.join()
        print("multiprocessing exit", proc.exitcode, flush=True)


if __name__ == "__main__":
    multiprocessing.freeze_support()
    main()
"""

_ENOEXEC = f"refused errno={errno.ENOEXEC} type=OSError"


@pytest.fixture(scope="module")
def probe(tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("hf519")
    source = root / "self_spawn_probe.py"
    source.write_text(_PROBE, encoding="utf-8")
    binary = root / "self_spawn_probe"
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--profile",
            "dev",
            "--output",
            str(binary),
            str(source),
        ],
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert build.returncode == 0, f"{build.stdout}\n{build.stderr}"
    return binary


def _run(binary: Path, *args: str) -> list[str]:
    env = {
        key: value for key, value in os.environ.items() if key != "MOLT_SYS_EXECUTABLE"
    }
    env.pop("HF519_PROBE_CHILD", None)
    env.pop("MOLT_SELF_SPAWN_DEPTH", None)
    env["HF519_OTHER_EXE"] = sys.executable
    result = run_native_test_process(
        [str(binary), *args], capture_output=True, text=True, timeout=120, env=env
    )
    assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"
    return result.stdout.splitlines()


@pytest.mark.slow
def test_every_spawn_api_refuses_an_interpreter_launch_of_its_own_binary(
    probe: Path,
) -> None:
    lines = _run(probe, "interpreter")
    assert lines == [
        f"run-c {_ENOEXEC}",
        f"run-m {_ENOEXEC}",
        f"popen-c {_ENOEXEC}",
        f"check_call-c {_ENOEXEC}",
        f"check_output-c {_ENOEXEC}",
        f"asyncio-c {_ENOEXEC}",
    ]


@pytest.mark.slow
def test_a_self_spawn_runs_at_depth_zero_and_is_refused_without_a_role_at_depth_one(
    probe: Path,
) -> None:
    lines = _run(probe, "nested")
    assert lines == [f"nested ran (0, 'child nested\\ngrandchild {_ENOEXEC}')"]


@pytest.mark.slow
def test_another_executable_and_multiprocessing_still_launch(probe: Path) -> None:
    assert _run(probe, "other") == ["other ran (0, 'other')"]
    assert _run(probe, "multiprocessing") == ["worker ran", "multiprocessing exit 0"]
