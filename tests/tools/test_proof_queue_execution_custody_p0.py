from __future__ import annotations

import json
import hashlib
import os
import runpy
import sys
import time
from pathlib import Path

import pytest

from tests.process_guard_common import install_module_view, run_custody_subject_process

from tools.proof_queue_pkg import (
    execution_custody,
    supervisor_custody,
    windows_createprocess,
)


@pytest.mark.skipif(os.name == "nt", reason="CPython POSIX exec-path contract")
@pytest.mark.parametrize(
    "selection",
    [
        "inherit",
        "missing",
        "empty",
        "relative",
        "lowercase",
        "bytes-missing",
        "bytes-empty",
        "bytes-lowercase",
    ],
)
def test_python_hook_broker_matches_cpython_child_path_selection(
    tmp_path: Path, selection: str
) -> None:
    # Use real native interpreter aliases: the independent ordinary CPython
    # launch is the oracle, and the broker sees the actual bootstrap hook.
    image = Path(sys.executable).resolve(strict=True)
    name = "molt-custody-child-" + tmp_path.name
    parent_bin = tmp_path / "parent-bin"
    child_cwd = tmp_path / "child-cwd"
    for directory in (parent_bin, child_cwd, child_cwd / "bin"):
        directory.mkdir(parents=True, exist_ok=True)
        (directory / name).symlink_to(image)
    child_environment = {
        "inherit": None,
        "missing": {},
        "empty": {"PATH": ""},
        "relative": {"PATH": "bin"},
        "lowercase": {"path": str(parent_bin)},
        "bytes-missing": {b"OTHER": b"present"},
        "bytes-empty": {b"PATH": b""},
        "bytes-lowercase": {b"path": os.fsencode(parent_bin)},
    }[selection]
    payload = (
        "import json, subprocess\n"
        "try:\n"
        f" result = subprocess.run([{name!r}, '-I', '-S', '-c', "
        "\"print('selected-native-child')\"], "
        f"env={child_environment!r}, cwd={str(child_cwd)!r}, "
        "capture_output=True, text=True, timeout=10)\n"
        " print(json.dumps({'returncode': result.returncode, 'stdout': result.stdout}))\n"
        "except OSError as exc:\n"
        " print(json.dumps({'error': type(exc).__name__}))\n"
    )
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith("MOLT_PROOF_CHILD_CUSTODY")
    }
    environment["PATH"] = str(parent_bin)
    oracle = run_custody_subject_process(
        [sys.executable, "-I", "-S", "-c", payload],
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert oracle.returncode == 0, oracle.stderr
    expected = json.loads(oracle.stdout)
    found = selection in {"inherit", "empty", "relative", "bytes-empty"}
    assert expected == (
        {"returncode": 0, "stdout": "selected-native-child\n"}
        if found
        else {"error": "FileNotFoundError"}
    )
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "declared-toolchains",
        "allowed": [
            {
                "toolchain": "python",
                "path": execution_custody._norm(directory / name),
                "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
            }
            for directory in (parent_bin, child_cwd, child_cwd / "bin")
        ],
    }
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )
    assert completed.returncode == 0, completed.stderr
    assert json.loads(completed.stdout) == (
        expected if found else {"error": "PermissionError"}
    )
    receipt = server.receipt()
    assert receipt["broker_complete"] is True, receipt
    assert receipt["errors"] == [], receipt
    decisions = [
        row for row in receipt["events"] if row.get("event") == "child-process"
    ]
    assert len(decisions) == 1, receipt
    assert decisions[0]["admitted"] is found, decisions
    selected_directory = (
        parent_bin
        if selection == "inherit"
        else child_cwd / "bin"
        if selection == "relative"
        else child_cwd
    )
    assert decisions[0]["resolved"] == (
        str(selected_directory / name) if found else None
    ), decisions
    assert bool(receipt["violations"]) is not found


# The Microsoft CreateProcessW documentation is the oracle for this table; the
# Windows test below checks the same model against real CreateProcessW.
_APP, _CWD = "C:\\Python", "C:\\work"
_SYSTEM, _WINDOWS = "C:\\Windows\\System32", "C:\\Windows"
_UNDETERMINABLE = "undeterminable"


def _createprocess_image(
    monkeypatch: pytest.MonkeyPatch,
    files: dict[str, str],
    *,
    application: str | None = None,
    command: str | None = None,
    path: str | None = "C:\\tools",
    searches_cwd: bool = True,
) -> str | None:
    entries = {location.casefold(): kind for location, kind in files.items()}
    monkeypatch.setattr(
        windows_createprocess,
        "_entry_kind",
        lambda location: entries.get(location.casefold()),
    )
    search = windows_createprocess.CallerSearch(
        image_directory=_APP,
        current_directory=_CWD,
        path=path,
        searches_current_directory=searches_cwd,
        system_directory=_SYSTEM,
        windows_directory=_WINDOWS,
    )
    try:
        return windows_createprocess.createprocess_image(application, command, search)
    except windows_createprocess.LaunchUndeterminable:
        return _UNDETERMINABLE


def test_createprocess_model_searches_the_documented_order(monkeypatch) -> None:
    # Image directory, current directory, system, 16-bit system, Windows, PATH.
    order = [
        f"{_APP}\\tool.exe",
        f"{_CWD}\\tool.exe",
        f"{_SYSTEM}\\tool.exe",
        f"{_WINDOWS}\\System\\tool.exe",
        f"{_WINDOWS}\\tool.exe",
        "C:\\tools\\tool.exe",
    ]
    for index, expected in enumerate(order):
        files = dict.fromkeys(order[index:], "file")
        assert _createprocess_image(monkeypatch, files, command="tool") == expected
    # NoDefaultCurrentDirectoryInExePath removes the current directory for a
    # name without a backslash.
    files = dict.fromkeys(order[1:], "file")
    assert (
        _createprocess_image(monkeypatch, files, command="tool", searches_cwd=False)
        == order[2]
    )
    # The child's PATH and cwd never reach the model, so nothing found is None.
    assert _createprocess_image(monkeypatch, {}, command="tool") is None


@pytest.mark.parametrize(
    ("files", "launch", "expected"),
    [
        # Only .exe is appended to a name without an extension; PATHEXT has no role.
        (
            {
                "C:\\tools\\tool.com": "file",
                "C:\\tools\\tool": "file",
                "D:\\more\\tool.exe": "file",
            },
            {"command": "tool", "path": "C:\\tools;D:\\more"},
            "D:\\more\\tool.exe",
        ),
        (
            {"C:\\tools\\tool.com": "file"},
            {"command": "tool.com"},
            "C:\\tools\\tool.com",
        ),
        (
            {"C:\\tools\\tool.exe": "file"},
            {"command": "tool --flag"},
            "C:\\tools\\tool.exe",
        ),
        # lpApplicationName is completed from the current directory, never searched.
        ({"C:\\tools\\tool.exe": "file"}, {"application": "tool.exe"}, None),
        (
            {f"{_CWD}\\tool.exe": "file"},
            {"application": "tool.exe", "command": "x"},
            f"{_CWD}\\tool.exe",
        ),
        (
            {"D:\\abs\\tool.exe": "file"},
            {"application": "D:\\abs\\tool.exe"},
            "D:\\abs\\tool.exe",
        ),
        # A quoted module name ends at the quote; an unquoted one is tried
        # prefix by prefix, as in the documented "c:\program files" example.
        (
            {"C:\\Program Files\\x\\tool.exe": "file"},
            {"command": '"C:\\Program Files\\x\\tool.exe" --flag'},
            "C:\\Program Files\\x\\tool.exe",
        ),
        (
            {"C:\\Program Files\\x\\tool.exe": "file"},
            {"command": "C:\\Program Files\\x\\tool.exe --flag"},
            "C:\\Program Files\\x\\tool.exe",
        ),
        # The example appends .exe to a path; the text says it does not. Both
        # readings run a different image here, so custody refuses.
        (
            {"C:\\Program.exe": "file", "C:\\Program Files\\x\\tool.exe": "file"},
            {"command": "C:\\Program Files\\x\\tool.exe --flag"},
            _UNDETERMINABLE,
        ),
        ({"C:\\x\\tool.exe": "file"}, {"command": "C:\\x\\tool"}, "C:\\x\\tool.exe"),
        ({"C:\\x\\tool": "file"}, {"command": "C:\\x\\tool"}, "C:\\x\\tool"),
        (
            {"C:\\x\\tool": "file", "C:\\x\\tool.exe": "file"},
            {"command": "C:\\x\\tool"},
            _UNDETERMINABLE,
        ),
        (
            {"C:\\bin\\tool.exe": "file"},
            {"command": "\\bin\\tool.exe"},
            "C:\\bin\\tool.exe",
        ),
        (
            {"\\\\srv\\share\\tool.exe": "file"},
            {"command": "\\\\srv\\share\\tool.exe"},
            "\\\\srv\\share\\tool.exe",
        ),
        # A relative name with a separator: searched, or completed from the
        # current directory. Custody admits only an image both readings allow.
        (
            {f"{_CWD}\\sub\\tool.exe": "file"},
            {"command": "sub\\tool.exe"},
            f"{_CWD}\\sub\\tool.exe",
        ),
        (
            {"C:\\tools\\sub\\tool.exe": "file"},
            {"command": "sub\\tool.exe"},
            "C:\\tools\\sub\\tool.exe",
        ),
        (
            {f"{_APP}\\sub\\tool.exe": "file", f"{_CWD}\\sub\\tool.exe": "file"},
            {"command": "sub\\tool.exe"},
            _UNDETERMINABLE,
        ),
        ({f"{_CWD}\\tool.exe": "file"}, {"command": "./tool"}, f"{_CWD}\\tool.exe"),
        # Undocumented PATH entries refuse only when they could hold the image.
        (
            {f"{_CWD}\\tool.exe": "file"},
            {"command": "tool", "path": "C:\\tools;;D:\\more", "searches_cwd": False},
            _UNDETERMINABLE,
        ),
        (
            {f"{_CWD}\\rel\\tool.exe": "file"},
            {"command": "tool", "path": "rel;D:\\more"},
            _UNDETERMINABLE,
        ),
        (
            {"C:\\quoted\\tool.exe": "file"},
            {"command": "tool", "path": '"C:\\quoted";D:\\more'},
            _UNDETERMINABLE,
        ),
        (
            {"D:\\more\\tool.exe": "file"},
            {"command": "tool", "path": 'C:\\tools;;rel;"C:\\quoted";D:\\more'},
            "D:\\more\\tool.exe",
        ),
        # A directory that shadows the name stops the documented search.
        (
            {f"{_APP}\\tool.exe": "other", "C:\\tools\\tool.exe": "file"},
            {"command": "tool"},
            _UNDETERMINABLE,
        ),
    ],
)
def test_createprocess_model_matches_the_documented_cases(
    monkeypatch, files, launch, expected
) -> None:
    assert _createprocess_image(monkeypatch, files, **launch) == expected


@pytest.mark.parametrize(
    "launch",
    [
        {"command": "C:tool.exe"},
        {"command": "\\\\?\\C:\\x\\tool.exe"},
        {"command": "\\\\.\\pipe\\tool"},
        {"command": "..\\tool.exe"},
        {"command": "sub\\..\\tool.exe"},
        {"command": "tool.exe:stream"},
        {"command": "to*l"},
        {"command": "tool."},
        {"command": '"C:\\unterminated'},
        {"command": " tool"},
        {"command": ""},
        {"application": "C:relative.exe"},
        {},
    ],
)
def test_createprocess_model_refuses_names_it_cannot_resolve(monkeypatch, launch):
    files = {f"{_CWD}\\tool.exe": "file", "C:\\tools\\tool.exe": "file"}
    assert _createprocess_image(monkeypatch, files, **launch) == _UNDETERMINABLE


_WINDOWS_LAUNCH_PAYLOAD = r"""
import ctypes, json, os, subprocess, sys
from ctypes import wintypes

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.QueryFullProcessImageNameW.argtypes = [
    wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)
]
kernel32.QueryFullProcessImageNameW.restype = wintypes.BOOL
CREATE_SUSPENDED = 0x4
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def image_of(pid):
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        size = wintypes.DWORD(32768)
        buffer = ctypes.create_unicode_buffer(size.value)
        if not kernel32.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)):
            raise ctypes.WinError(ctypes.get_last_error())
        return buffer.value
    finally:
        kernel32.CloseHandle(handle)


results = []
for case in json.loads(sys.argv[1]):
    saved = {name: os.environ.get(name) for name in case["environ"]}
    for name, value in case["environ"].items():
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value
    for name, value in case["putenv"].items():
        os.putenv(name, value)
    try:
        process = subprocess.Popen(
            case["args"],
            executable=case["executable"],
            env=case["env"],
            cwd=case["cwd"],
            creationflags=CREATE_SUSPENDED,
        )
    except OSError as exc:
        results.append({"error": type(exc).__name__})
    else:
        try:
            results.append({"image": image_of(process.pid)})
        finally:
            process.kill()
            process.wait()
    for name, value in saved.items():
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value
    for name in case["putenv"]:
        os.putenv(name, os.environ[name])
print(json.dumps(results))
"""


@pytest.mark.skipif(os.name != "nt", reason="real Windows CreateProcessW oracle")
def test_python_hook_broker_admits_the_image_createprocess_runs(tmp_path: Path):
    # Each launch runs twice through the same Popen call: once plain, so
    # Windows itself names the image, and once under the real hook and broker.
    # A suspended child never runs, and the kernel reports its mapped image.
    import shutil

    source = Path(sys.executable).resolve(strict=True)
    directory = {
        name: tmp_path / name
        for name in (
            "caller-cwd",
            "child-cwd",
            "caller-path",
            "child-path",
            "live-path",
            "peb-path",
            "spaced dir",
        )
    }
    for path in directory.values():
        path.mkdir()
    (directory["caller-path"] / "sub").mkdir()

    def image(where: str, name: str) -> Path:
        path = directory[where] / name
        shutil.copyfile(source, path)
        return path

    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith("MOLT_PROOF_CHILD_CUSTODY")
        and name.casefold() != "nodefaultcurrentdirectoryinexepath"
    }
    environment["PATH"] = str(directory["caller-path"])
    probe = run_custody_subject_process(
        [
            sys.executable,
            "-I",
            "-S",
            "-c",
            "import _winapi; print(_winapi.GetModuleFileName(0))",
        ],
        env=environment,
        cwd=directory["caller-cwd"],
        check=True,
        capture_output=True,
        text=True,
        timeout=20,
    )
    caller_image = Path(probe.stdout.strip())

    def case(name, args, real, *, expect="admit", decoys=(), allow=None, **launch):
        return {
            "id": name,
            "args": args,
            "executable": launch.get("executable"),
            "env": launch.get("env"),
            "cwd": str(launch["cwd"]) if "cwd" in launch else None,
            "environ": launch.get("environ", {}),
            "putenv": launch.get("putenv", {}),
            "expect": expect,
            "real": None if real is None else str(real),
            "decoys": [str(path) for path in decoys],
            "allow": [str(path) for path in (allow if allow is not None else [real])],
        }

    child_env = {"PATH": str(directory["child-path"])}
    path_no_extension = image("caller-path", "hf137k.exe")
    ambiguous_image = image("caller-path", "hf137l.exe")
    ambiguous_bare = image("caller-path", "hf137l")
    subdirectory_image = image("caller-path", "sub\\hf137o.exe")
    cases = [
        # The child's PATH holds a decoy; Windows searches the caller's PATH.
        case(
            "child-path",
            ["hf137a"],
            image("caller-path", "hf137a.exe"),
            decoys=[image("child-path", "hf137a.exe")],
            env=child_env,
            cwd=directory["child-cwd"],
        ),
        # Admitting the child-PATH decoy would let the real image run unadmitted.
        case(
            "child-path-decoy-allowed",
            ["hf137b"],
            image("caller-path", "hf137b.exe"),
            expect="deny",
            decoys=[decoy := image("child-path", "hf137b.exe")],
            allow=[decoy],
            env=child_env,
            cwd=directory["child-cwd"],
        ),
        # Windows searches the caller's current directory, not the child's cwd.
        case(
            "child-cwd",
            ["hf137c"],
            image("caller-cwd", "hf137c.exe"),
            decoys=[image("child-cwd", "hf137c.exe")],
            env={"PATH": "."},
            cwd=directory["child-cwd"],
        ),
        # Windows appends only .exe; PATHEXT would select the .com first.
        case(
            "pathext",
            ["hf137d"],
            image("caller-path", "hf137d.exe"),
            decoys=[image("caller-path", "hf137d.com")],
        ),
        # The caller's image directory precedes every other directory.
        case(
            "image-directory",
            ["python"],
            caller_image,
            decoys=[image("caller-path", "python.exe")],
        ),
        # PATH is read when the launch happens, through os.environ ...
        case(
            "live-path",
            ["hf137e"],
            image("live-path", "hf137e.exe"),
            decoys=[image("caller-path", "hf137e.exe")],
            environ={"PATH": str(directory["live-path"])},
        ),
        # ... or set by os.putenv, which os.environ never sees.
        case(
            "putenv-path",
            ["hf137f"],
            image("peb-path", "hf137f.exe"),
            decoys=[image("caller-path", "hf137f.exe")],
            putenv={"PATH": str(directory["peb-path"])},
        ),
        case(
            "no-default-current-directory",
            ["hf137g"],
            image("caller-path", "hf137g.exe"),
            decoys=[image("caller-cwd", "hf137g.exe")],
            environ={"NoDefaultCurrentDirectoryInExePath": "1"},
        ),
        # lpApplicationName is completed from the caller's current directory.
        case(
            "application-name",
            ["hf137-argv0"],
            image("caller-cwd", "hf137h.exe"),
            decoys=[
                image("child-cwd", "hf137h.exe"),
                image("child-path", "hf137h.exe"),
            ],
            executable="hf137h.exe",
            env=child_env,
            cwd=directory["child-cwd"],
        ),
        case(
            "quoted-absolute",
            [str(spaced := image("spaced dir", "hf137i.exe")), "x"],
            spaced,
        ),
        case("string-command", "hf137j --flag", image("caller-path", "hf137j.exe")),
        case(
            "dot-relative",
            [".\\hf137m.exe"],
            image("caller-cwd", "hf137m.exe"),
            decoys=[image("child-cwd", "hf137m.exe")],
            cwd=directory["child-cwd"],
        ),
        # Microsoft's text says Windows does not append .exe to a name with a
        # path; its own example appends it. This launch records which one runs.
        case(
            "path-without-extension",
            [str(path_no_extension.with_suffix(""))],
            path_no_extension,
        ),
        case(
            "dot-relative-without-extension",
            ["./hf137n"],
            image("caller-cwd", "hf137n.exe"),
        ),
        case(
            "path-without-extension-ambiguous",
            [str(ambiguous_bare)],
            None,
            expect="undeterminable",
            allow=[ambiguous_bare, ambiguous_image],
        ),
        # Searched, or completed from the current directory: either is safe.
        case(
            "relative-subdirectory",
            ["sub\\hf137o.exe"],
            subdirectory_image,
            expect="either",
        ),
    ]
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "declared-toolchains",
        "allowed": [
            {
                "toolchain": "python",
                "path": execution_custody._norm(path),
                "sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest(),
            }
            for path in sorted({path for row in cases for path in row["allow"]})
        ],
    }
    arguments = json.dumps(cases)
    oracle_run = run_custody_subject_process(
        [sys.executable, "-I", "-S", "-c", _WINDOWS_LAUNCH_PAYLOAD, arguments],
        env=environment,
        cwd=directory["caller-cwd"],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert oracle_run.returncode == 0, oracle_run.stderr
    oracle = json.loads(oracle_run.stdout)
    server = execution_custody.ChildCustodyEventServer("python", policy)
    custody_environment = {
        **environment,
        execution_custody.CHILD_POLICY_ENV: json.dumps(policy),
        **server.environment(),
    }
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    with server:
        custody_run = run_custody_subject_process(
            [
                sys.executable,
                bootstrap,
                "command",
                "0",
                _WINDOWS_LAUNCH_PAYLOAD,
                arguments,
            ],
            env=custody_environment,
            cwd=directory["caller-cwd"],
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )
    assert custody_run.returncode == 0, custody_run.stderr
    observed = json.loads(custody_run.stdout)
    receipt = server.receipt()
    assert receipt["broker_complete"] is True, receipt
    decisions = [
        row for row in receipt["events"] if row.get("event") == "child-process"
    ]
    table = [
        {
            "case": row["id"],
            "expect": row["expect"],
            "real": row["real"],
            "oracle": truth,
            "decision": decision,
            "observed": seen,
        }
        for row, truth, decision, seen in zip(cases, oracle, decisions, observed)
    ]
    detail = json.dumps(table, indent=1)
    assert len(decisions) == len(observed) == len(oracle) == len(cases), detail

    def same(left: object, right: object) -> bool:
        return (
            isinstance(left, str)
            and isinstance(right, str)
            and os.path.samefile(left, right)
        )

    for row, truth, decision, seen in zip(cases, oracle, decisions, observed):
        resolved = decision["resolved"]
        # An admitted launch runs exactly the admitted image, or nothing.
        if decision["admitted"]:
            assert same(seen.get("image"), resolved) or (
                row["expect"] == "either" and "error" in seen and seen == truth
            ), detail
        else:
            assert seen == {"error": "PermissionError"}, detail
        assert not any(same(resolved, decoy) for decoy in row["decoys"]), detail
        if row["expect"] in {"admit", "deny"}:
            # Windows itself runs the expected image, and custody names it.
            assert same(truth.get("image"), row["real"]), detail
            assert same(resolved, row["real"]), detail
            assert decision["admitted"] is (row["expect"] == "admit"), detail
        elif row["expect"] == "undeterminable":
            assert resolved is None, detail
            assert decision["reason"].startswith("identity-unavailable:"), detail
        else:
            assert truth == seen or same(truth.get("image"), resolved), detail


@pytest.mark.skipif(os.name != "nt", reason="libuv Windows executable search")
def test_node_selection_names_the_image_libuv_runs(tmp_path: Path) -> None:
    # The Node hook launches the broker's selection. libuv runs a path as named
    # only when the name has an extension; it appends .com or .exe otherwise.
    import shutil

    node_path = shutil.which("node")
    if node_path is None:
        pytest.skip("node is unavailable")
    node = Path(node_path).resolve(strict=True)
    bare, suffixed = tmp_path / "hf137node", tmp_path / "hf137node.exe"
    for path in (bare, suffixed):
        shutil.copyfile(node, path)
    script = (
        "const r=require('child_process').spawnSync(process.argv[1],"
        "['-e','process.stdout.write(process.execPath)'],{encoding:'utf8'});"
        "process.stdout.write(JSON.stringify("
        "{stdout:r.stdout,error:r.error?r.error.message:null}));"
    )
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith("MOLT_PROOF_CHILD_CUSTODY") and name != "NODE_OPTIONS"
    }
    plain = run_custody_subject_process(
        [node, "-e", script, str(bare)],
        env=environment,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    oracle = json.loads(plain.stdout)
    assert os.path.samefile(oracle["stdout"], suffixed), oracle

    # Admitting the extensionless file would let libuv run the .exe beside it.
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "declared-toolchains",
        "allowed": [
            {
                "toolchain": "node",
                "path": execution_custody._norm(bare),
                "sha256": hashlib.sha256(bare.read_bytes()).hexdigest(),
            }
        ],
    }
    server = execution_custody.ChildCustodyEventServer("node", policy)
    hook = Path(execution_custody.__file__).with_name("node_child_custody.cjs")
    custody_environment = {
        **environment,
        execution_custody.CHILD_POLICY_ENV: json.dumps(policy),
        **server.environment(),
        "NODE_OPTIONS": f"--no-global-search-paths --require={hook}",
    }
    with server:
        guarded = run_custody_subject_process(
            [node, "-e", script, str(bare)],
            env=custody_environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
    receipt = server.receipt()
    decisions = [
        row for row in receipt["events"] if row.get("event") == "child-process"
    ]
    assert len(decisions) == 1, receipt
    assert decisions[0]["admitted"] is False, receipt
    assert "has no extension" in decisions[0]["reason"], receipt
    assert guarded.returncode != 0, guarded
    assert "has no extension" in guarded.stderr, guarded


def test_python_payload_cannot_replace_private_audit_enforcement(
    tmp_path: Path,
) -> None:
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    marker = tmp_path / "escaped"
    child = f"from pathlib import Path; Path({str(marker)!r}).touch()"
    payload = (
        "import subprocess,sys; "
        "assert '_molt_proof_execution_custody' not in sys.modules; "
        "blocked=False\n"
        "try:\n"
        f" subprocess.run([sys.executable,'-c',{child!r}],check=True)\n"
        "except PermissionError:\n"
        " blocked=True\n"
        "assert blocked"
    )
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "forbidden",
        "allowed": [],
    }
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment = dict(os.environ)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())

    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )

    receipt = server.receipt()
    assert completed.returncode == 0, completed.stderr
    assert not marker.exists()
    assert receipt["broker_complete"] is True
    assert receipt["process_closure_complete"] is False
    assert receipt["scope"] == "runtime-hook-broker"
    assert receipt["violations"]
    assert not execution_custody.child_receipt_is_admitted(receipt)


@pytest.mark.parametrize("runtime", ["python", "node"])
def test_child_admission_distinguishes_handshakes_from_launch_decisions(runtime):
    receipt = {
        "broker_complete": True,
        "errors": [],
        "violations": [],
        "events": [
            {"event": "hook-start", "runtime": runtime, "connection_id": 0},
            {"event": "child-process", "admitted": True},
            {"event": "hook-end", "runtime": runtime, "connection_id": 0},
        ],
    }
    assert execution_custody.child_receipt_is_admitted(receipt)
    for bad_event in (
        {"event": "child-process", "admitted": False},
        {"event": "child-process"},
        {"event": "unknown", "admitted": True},
        {"event": "hook-start"},
        {"event": [], "admitted": True},
        None,
    ):
        assert not execution_custody.child_receipt_is_admitted(
            {**receipt, "events": [*receipt["events"], bad_event]}
        )
    for field, value in (
        ("broker_complete", False),
        ("errors", ["broken transport"]),
        ("violations", [{"event": "child-process", "admitted": False}]),
    ):
        assert not execution_custody.child_receipt_is_admitted(
            {**receipt, field: value}
        )


def test_derived_child_admission_reuses_supervisor_provenance(tmp_path: Path):
    source = tmp_path / "source"
    source.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    provenance = supervisor_custody._derived_root_provenance(
        descendants="declared-toolchains",
        env={supervisor_custody.PROOF_SCRATCH_ROOT_ENV: str(scratch)},
        source_root=source,
        result_path=tmp_path / "receipt.json",
    )
    envelope = {"process_closure": {"descendants": "declared-toolchains"}}
    policy = execution_custody.child_policy(
        envelope, {}, environment_executables={}, derived_roots=provenance
    )
    role = supervisor_custody.SCRATCH_OUTPUT_ROLE
    assert policy["derived_roots"] == [{"role": role, "path": str(scratch.resolve())}]
    with pytest.raises(ValueError, match="run-owned provenance"):
        execution_custody.child_policy(
            envelope,
            {},
            environment_executables={},
            derived_roots=[{**provenance[0], "run_owned": False}],
        )
    with pytest.raises(ValueError, match="run-owned provenance"):
        execution_custody.child_policy(
            {"process_closure": {"descendants": "forbidden"}},
            {},
            environment_executables={},
            derived_roots=provenance,
        )

    image = scratch / "candidate.exe"
    image.write_bytes(b"generated native image")
    outside = tmp_path / "scratch-sibling"
    outside.mkdir()
    escaped_image = outside / "candidate.exe"
    escaped_image.write_bytes(image.read_bytes())
    server = execution_custody.ChildCustodyEventServer(None, policy)
    with server:
        # The Node hook launches the broker's selection, so an absolute
        # selection is the image on every host.
        admitted = server._decide_child({"requested": str(image)}, "node")
        assert admitted["admitted"] is True
        assert admitted["derived_role"] == role
        assert admitted["resolved"] == str(image.resolve())
        assert admitted["sha256"] == hashlib.sha256(image.read_bytes()).hexdigest()
        denied = server._decide_child({"requested": str(escaped_image)}, "node")
        assert denied["admitted"] is False


def test_derived_child_admission_rejects_symlink_escape(tmp_path: Path):
    root = tmp_path / "owned"
    root.mkdir()
    outside = tmp_path / "outside.exe"
    outside.write_bytes(b"outside native image")
    link = root / "escape.exe"
    try:
        link.symlink_to(outside)
    except OSError as exc:
        if os.name == "nt" and exc.winerror == 1314:
            pytest.skip("Windows host lacks symbolic-link creation privilege")
        raise
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {},
        environment_executables={},
        derived_roots=[
            {"role": "scratch-output", "path": str(root), "run_owned": True}
        ],
    )
    with execution_custody.ChildCustodyEventServer(None, policy) as server:
        assert (
            server._decide_child({"requested": str(link)}, "node")["admitted"] is False
        )


@pytest.mark.parametrize("changed_field", [None, "path", "sha256", "roles", "class"])
def test_derived_child_binds_actual_executed_image(tmp_path: Path, changed_field):
    image = {
        "class": "derived",
        "path": str(tmp_path / "candidate.exe"),
        "sha256": hashlib.sha256(b"generated native image").hexdigest(),
        "roles": ["scratch-output"],
    }
    receipt = {
        "events": [
            {
                "event": "child-process",
                "admitted": True,
                "resolved": image["path"],
                "sha256": image["sha256"],
                "derived_role": "scratch-output",
            }
        ]
    }
    event_log = tmp_path / "events.jsonl"
    if changed_field is not None:
        image[changed_field] = (
            ["build-output"] if changed_field == "roles" else "changed"
        )
    event_log.write_text(
        json.dumps({"event": {"kind": "exec", "image": image}}) + "\n", encoding="utf-8"
    )
    if changed_field is None:
        execution_custody.require_derived_child_image_bindings(receipt, event_log)
    else:
        with pytest.raises(ValueError, match="executed image identity"):
            execution_custody.require_derived_child_image_bindings(receipt, event_log)
    event_log.write_text("", encoding="utf-8")
    with pytest.raises(ValueError, match="executed image identity"):
        execution_custody.require_derived_child_image_bindings(receipt, event_log)


def test_source_watch_records_create_execute_delete_transient(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    tracked = source / "tracked.py"
    tracked.write_text("VALUE = 1\n", encoding="utf-8")
    before = tracked.read_bytes()
    specs = execution_custody.watch_specs(
        source_root=source,
        tracked_paths=[tracked],
        identities=[],
        broad_roots=[],
    )
    assert specs == [execution_custody.WatchSpec(source.resolve(), None)]
    monitor = execution_custody.LiveCustodyMonitor(specs)

    with monitor:
        transient = source / "transient.py"
        transient.write_text("EXECUTED = True\n", encoding="utf-8")
        namespace = runpy.run_path(str(transient))
        assert namespace["EXECUTED"] is True
        transient.unlink()
        # Kernel delivery is asynchronous even though enqueue is synchronous.
        time.sleep(0.10)

    assert tracked.read_bytes() == before
    assert not transient.exists()
    receipt = monitor.receipt()
    assert receipt["stable"] is False
    assert any(
        Path(str(event["path"])).name == "transient.py" for event in receipt["events"]
    )


def test_linux_root_watch_is_installed_before_recursive_enumeration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "root"
    root.mkdir()
    monitor = execution_custody.LiveCustodyMonitor(
        [execution_custody.WatchSpec(root, None)]
    )
    add_calls: list[Path] = []

    class FakeFunction:
        def __init__(self, implementation):
            self.implementation = implementation

        def __call__(self, *args):
            return self.implementation(*args)

    class FakeLibc:
        inotify_init1 = FakeFunction(lambda _flags: 17)

        @staticmethod
        def _add(_fd, raw_path, _mask):
            add_calls.append(Path(os.fsdecode(raw_path)))
            return len(add_calls)

        inotify_add_watch = FakeFunction(_add)

    original_rglob = Path.rglob

    def asserting_rglob(path: Path, pattern: str):
        assert path == root
        assert add_calls == [root]
        return original_rglob(path, pattern)

    monkeypatch.setattr(execution_custody.ctypes, "CDLL", lambda *_a, **_k: FakeLibc())
    monkeypatch.setattr(Path, "rglob", asserting_rglob)
    install_module_view(
        monkeypatch,
        "os",
        os,
        execution_custody,
        O_NONBLOCK=0x800,
        O_CLOEXEC=0x80000,
        read=lambda *_a, **_k: (_ for _ in ()).throw(BlockingIOError()),
    )
    monitor._stop.set()

    monitor._run_linux()

    assert add_calls == [root]
    assert monitor._ready.is_set()


@pytest.mark.skipif(os.name == "nt", reason="POSIX native interpreter alias execution")
def test_python_path_and_cargo_hook_launches_share_native_environment_custody(tmp_path):
    import shutil
    from tools import proof_plan
    from tools.proof_queue_pkg import (
        command_admission,
        command_identity,
        execution_environment,
    )

    path_bin = tmp_path / "path"
    path_bin.mkdir()
    path_tool, hook_tool = path_bin / "cargo", tmp_path / "selected-cargo"
    for image in (path_tool, hook_tool):
        shutil.copyfile(Path(sys.executable).resolve(strict=True), image)
        image.chmod(0o755)
    (tmp_path / "pyvenv.cfg").write_text(
        f"home = {Path(sys._base_executable).resolve(strict=True).parent}\n",
        encoding="utf-8",
    )
    environment = {**os.environ, "PATH": str(path_bin), "CARGO": str(hook_tool)}
    environment, _ = execution_environment._deterministic_execution_environment(
        environment, override_names=["CARGO"]
    )
    envelope = command_admission.envelope_for_command([sys.executable, "-c", "pass"])
    identity = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "cargo",
        envelope,
        [sys.executable, "-c", "pass"],
        cwd=tmp_path,
        env=environment,
    )
    assert identity["path"] == str(path_tool)
    configured = execution_environment._execution_environment_executable_identities(
        environment, cwd=tmp_path
    )
    declared = {"process_closure": {"descendants": "declared-toolchains"}}
    policy = execution_custody.child_policy(
        declared, {"cargo": identity}, environment_executables=configured
    )
    _, fixed = supervisor_custody._supervisor_fixed_images(
        {"cargo": identity}, configured, [sys.executable]
    )
    wanted = {str(path_tool), str(hook_tool)}
    assert wanted <= {row["path"] for row in policy["allowed"]}
    assert wanted <= {row["path"] for row in fixed}
    payload = (
        "import os, subprocess\n"
        "for executable in ('cargo', os.environ['CARGO']):\n"
        " subprocess.run([executable, '-I', '-S', '-c', \"print('actual-child')\"], check=True)\n"
    )
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(policy)
    environment.update(server.environment())
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            cwd=tmp_path,
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.splitlines() == ["actual-child", "actual-child"]
    receipt = server.receipt()
    assert execution_custody.child_receipt_is_admitted(receipt), receipt
    assert {
        row["resolved"]
        for row in receipt["events"]
        if row.get("event") == "child-process"
    } == wanted
    # The recorded allowance binds bytes, rather than granting a directory.
    hook_tool.write_bytes(b"replacement")
    with execution_custody.ChildCustodyEventServer("python", policy) as changed:
        assert not changed._decide_child({"requested": str(hook_tool)}, "python")[
            "admitted"
        ]


@pytest.mark.parametrize(
    "selector",
    [
        "CARGO",
        "RUSTC",
        "RUSTFMT",
        "RUSTDOC",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTDOC",
    ],
)
def test_environment_rust_proxy_component_reaches_both_custody_consumers(
    tmp_path, monkeypatch, selector
):
    import subprocess
    from molt import process_guard
    from tools.proof_queue_pkg import (
        execution_environment,
        toolchain_capture,
        process_image_capture,
    )

    role = selector.removeprefix("CARGO_BUILD_").lower()
    proxy = tmp_path / (role + (".exe" if os.name == "nt" else ""))
    rustup = proxy.with_name("rustup.exe" if os.name == "nt" else "rustup")
    physical = tmp_path / "physical" / proxy.name
    physical.parent.mkdir()
    for path in (proxy, rustup, physical):
        path.write_bytes(b"proxy" if path != physical else b"actual component")
        path.chmod(0o755)
    calls = []

    def which(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, str(physical) + "\n", "")

    monkeypatch.setattr(process_guard, "run_completed_command", which)
    captured = execution_environment._execution_environment_executable_identities(
        {selector: str(proxy)}, cwd=tmp_path
    )
    assert calls == [[str(rustup), "which", role]]
    images = process_image_capture.environment_images(captured)
    expected_paths = {
        process_image_capture._image_path_key(proxy),
        process_image_capture._image_path_key(physical),
    }
    assert {row["path"] for row in images} == expected_paths
    assert {
        process_image_capture._image_path_key(Path(row.path))
        for row in toolchain_capture.frozen_files(captured)
    } >= expected_paths
    assert set(execution_custody._identity_paths(captured)) >= {proxy, physical}
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {},
        environment_executables=captured,
    )
    _, native = supervisor_custody._supervisor_fixed_images(
        {}, captured, [sys.executable]
    )
    expected = {(row["path"], row["sha256"]) for row in images}
    assert {(row["path"], row["sha256"]) for row in policy["allowed"]} == expected
    assert {
        (row["path"], row["sha256"])
        for row in native
        if row["role"] == f"env:{selector}"
    } == expected
    physical.write_bytes(b"changed component")
    assert (
        execution_environment._execution_environment_executable_identities(
            {selector: str(proxy)}, cwd=tmp_path
        )
        != captured
    )
    with pytest.raises(ValueError, match="changed while live custody"):
        process_image_capture.revalidate_images(images)
    incomplete = {
        selector: {
            key: value
            for key, value in captured[selector].items()
            if key != "process_images"
        }
    }
    with pytest.raises(ValueError, match="no process-image closure"):
        execution_custody.child_policy(
            {"process_closure": {"descendants": "declared-toolchains"}},
            {},
            environment_executables=incomplete,
        )
    with pytest.raises(ValueError, match="no process-image closure"):
        supervisor_custody._supervisor_fixed_images({}, incomplete, [sys.executable])


def test_watch_custody_retains_ancestor_alias_and_deleted_entry(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    target = tmp_path / "target"
    target.mkdir()
    tool = target / "tool"
    tool.write_bytes(b"same bytes")
    alias = tmp_path / "alias"
    try:
        alias.symlink_to(target, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink capability unavailable: {exc}")
    selected = alias / "tool"
    specs = execution_custody.watch_specs(
        source_root=source,
        tracked_paths=[],
        identities=[{"path": str(selected)}],
        broad_roots=[],
    )
    assert any(spec.owns(tool) for spec in specs if spec.root == target)
    alias_spec = next(spec for spec in specs if spec.root == tmp_path)
    assert alias_spec.owns(alias)
    alias.unlink()
    assert alias_spec.owns(alias), "removed selection must retain a mutation event"
    replacement = tmp_path / "replacement"
    replacement.mkdir()
    (replacement / "tool").write_bytes(tool.read_bytes())
    alias.symlink_to(replacement, target_is_directory=True)
    assert alias_spec.owns(alias), "same-byte alias retarget still changes selection"
