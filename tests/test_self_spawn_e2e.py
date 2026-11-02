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
    print("worker stdin", repr(sys.stdin.read()), flush=True)
    print("worker stdout", flush=True)
    print("worker stderr", file=sys.stderr, flush=True)


def descriptor_inheritance():
    if sys.platform == "win32":
        report("pass-fds", lambda: subprocess.Popen([os.environ["HF519_OTHER_EXE"]], pass_fds=(3,)))
        return
    path = os.environ["HF627_FD_FILE"]
    with open(path, "wb") as stream:
        stream.write(b"descriptor witness")
    with open(path, "rb") as stream:
        fd = stream.fileno()
        os.set_inheritable(fd, True)
        env = dict(os.environ)
        env["HF627_FD"] = str(fd)
        # This child is an explicitly selected external interpreter, not a
        # compiled application's attempt to interpret Python source itself.
        source = "import os; fd=int(os.environ['HF627_FD']); path=os.environ['HF627_FD_FILE']; "
        source += "\ntry:\n s=os.fstat(fd); p=os.stat(path); print('open' if (s.st_dev,s.st_ino)==(p.st_dev,p.st_ino) else 'closed')"
        source += "\nexcept OSError:\n print('closed')"
        command = [os.environ["HF519_OTHER_EXE"], "-c", source]
        for name, close_fds, passed in (
            ("keep", True, (fd, fd)),
            ("close", True, ()),
            ("inherit", False, ()),
        ):
            with subprocess.Popen(command, env=env, close_fds=close_fds,
                                  pass_fds=passed, stdout=subprocess.PIPE, text=True) as proc:
                out, _ = proc.communicate()
            print(name, proc.returncode, out.strip(), flush=True)
        # Inheritance changes belong to the child, including on spawn failure.
        print("parent-inheritable", os.get_inheritable(fd), flush=True)
        try:
            subprocess.Popen([path + ".missing"], pass_fds=(fd,))
        except OSError as exc:
            print("missing", exc.errno, flush=True)
        print("parent-after-failure", os.get_inheritable(fd), flush=True)


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
    elif mode == "descriptors":
        descriptor_inheritance()


if __name__ == "__main__":
    multiprocessing.freeze_support()
    main()
"""

_ENOEXEC = f"refused errno={errno.ENOEXEC} type=OSError"
# The probe reads its environment, launches processes, and its multiprocessing
# child reads and writes its spawn state.
_CAPABILITIES = ",".join(
    (
        "env.read",
        "env.write",
        "env.snapshot",
        "fs.read",
        "fs.write",
        "process.exec",
        "process.spawn",
        "process.asyncio",
    )
)


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
            # A compiled binary takes its grants at build time.
            "--capabilities",
            _CAPABILITIES,
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


def _run(
    binary: Path,
    *args: str,
    expected_stderr: str | None = None,
    input_data: str | None = None,
) -> list[str]:
    env = {
        key: value for key, value in os.environ.items() if key != "MOLT_SYS_EXECUTABLE"
    }
    env.pop("HF519_PROBE_CHILD", None)
    env.pop("MOLT_SELF_SPAWN_DEPTH", None)
    env["HF519_OTHER_EXE"] = sys.executable
    env["HF627_FD_FILE"] = str(binary.parent / "descriptor-witness")
    result = run_native_test_process(
        [str(binary), *args],
        capture_output=True,
        text=True,
        timeout=120,
        env=env,
        input=input_data,
    )
    assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"
    if expected_stderr is not None:
        assert result.stderr == expected_stderr
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
    assert _run(
        probe,
        "multiprocessing",
        expected_stderr="worker stderr\n",
        input_data="parent program input must not reach the worker\n",
    ) == [
        "worker stdin ''",
        "worker stdout",
        "multiprocessing exit 0",
    ]


@pytest.mark.slow
@pytest.mark.skipif(sys.platform == "win32", reason="pass_fds is a POSIX interface")
def test_pass_fds_and_close_fds_preserve_parent_descriptor_state(probe: Path) -> None:
    assert _run(probe, "descriptors", expected_stderr="") == [
        "keep 0 open",
        "close 0 closed",
        "inherit 0 open",
        "parent-inheritable True",
        f"missing {errno.ENOENT}",
        "parent-after-failure True",
    ]
