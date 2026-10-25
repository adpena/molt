from __future__ import annotations

import json
import hashlib
import errno
import os
import runpy
import struct
import sys
import threading
import time
from pathlib import Path

import pytest

from tests.process_guard_common import (
    install_module_view,
    run_custody_subject_process,
)

from tools.proof_queue_pkg import (
    execution_custody,
    python_child_custody,
    supervisor_custody,
    windows_createprocess,
)


def _run_windows_raw_python_subject(command_line, *, executable, tmp_path, **kwargs):
    """Observe raw CreateProcess syntax inside the existing typed subject.

    The suspended-image oracle below uses this same Windows Popen boundary.
    A fresh driver prefix keeps the observation process out of the foreign
    cache namespace deliberately assigned to the actual raw child.
    """
    import tempfile

    program = (
        "import subprocess,sys;"
        "child=subprocess.run(sys.argv[1],executable=sys.argv[2],"
        "stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=15);"
        "sys.stdout.buffer.write(child.stdout);sys.stderr.buffer.write(child.stderr);"
        "sys.exit(child.returncode)"
    )
    with tempfile.TemporaryDirectory(dir=tmp_path, prefix="raw-driver-") as prefix:
        return run_custody_subject_process(
            [
                sys.executable,
                "-I",
                "-B",
                "-X",
                f"pycache_prefix={prefix}",
                "-c",
                program,
                command_line,
                str(executable),
            ],
            **kwargs,
        )


def _python_base_process_images():
    from tools.proof_queue_pkg import process_image_capture

    # uv's Python trampoline preserves its absolute embedded base selector.
    # A managed-Python directory junction and its physical image are distinct
    # captured coordinates, even though they currently name the same bytes.
    base = Path(os.path.abspath(sys._base_executable))
    images = [process_image_capture.capture_image("base-interpreter", base)]
    if execution_custody._norm(base) != images[0]["path"]:
        images.append(
            process_image_capture.capture_image(
                "interpreter-alias:base-selector", base, preserve_path=True
            )
        )
    return images


@pytest.mark.parametrize("runtime", ["python", "node"])
@pytest.mark.parametrize(
    ("interpreter_role", "alias_form"),
    [
        ("selected-interpreter", "selected"),
        ("interpreter-alias:python3.12", "copy"),
        pytest.param(
            "interpreter-alias:python3.12",
            "symlink",
            marks=pytest.mark.skipif(
                os.name != "nt", reason="actual Windows symlink alias subject"
            ),
        ),
    ],
)
def test_python_child_bytecode_namespace_is_bound_before_launch(
    tmp_path, runtime, interpreter_role, alias_form
):
    import shutil
    from molt.python_environment_location import environment_bytecode_prefix
    from tools.proof_command_argv import python_environment_command
    from tools.proof_queue_pkg import process_image_capture

    node = shutil.which("node") if runtime == "node" else None
    if runtime == "node" and node is None:
        pytest.skip("node is unavailable")
    selected = tmp_path / "selected-image"
    prefix = environment_bytecode_prefix(selected)
    prefix.mkdir(parents=True)
    child_executable = sys.executable
    if interpreter_role.startswith("interpreter-alias:"):
        if os.name == "nt":
            import venv

            # The genuine venv redirector preserves CPython's DLL/home setup
            # while its copied lexical alias exercises the actual image grant.
            venv.EnvBuilder(with_pip=False).create(selected)
            alias = selected / "Scripts" / "python3.12.exe"
            if alias_form == "symlink":
                try:
                    alias.symlink_to(selected / "Scripts" / "python.exe")
                except OSError as exc:
                    if exc.winerror == 1314:
                        pytest.skip(
                            "Windows host lacks symbolic-link creation privilege"
                        )
                    raise
            else:
                shutil.copyfile(selected / "Scripts" / "python.exe", alias)
        else:
            alias = selected / "python3.12"
            alias.symlink_to(Path(sys.executable).resolve(strict=True))
        child_executable = str(alias)
    foreign = tmp_path / "foreign-cache"
    source = tmp_path / "cache_probe.py"
    source.write_text("VALUE = 111\n", encoding="utf-8")
    stamp = source.stat()
    original = source.read_bytes()
    # CPython itself writes two executable, same-header wrong-code caches.
    # Both namespaces must remain untouched and unread by admitted children.
    source.write_text("VALUE = 999\n", encoding="utf-8")
    os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    caches = []
    for namespace in (None, str(foreign)):
        compiled = run_custody_subject_process(
            [
                sys.executable,
                "-I",
                "-S",
                "-B",
                "-X",
                f"pycache_prefix={namespace or ''}",
                "-c",
                "import py_compile;print(py_compile.compile("
                f"{str(source)!r},doraise=True,invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP))",
            ],
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert compiled.returncode == 0, compiled.stderr
        cache = Path(compiled.stdout.strip())
        caches.append((cache, cache.read_bytes()))
    source.write_bytes(original)
    os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    program = (
        "import json,sys;"
        f"sys.path.insert(0,{str(tmp_path)!r});"
        "import cache_probe;print(json.dumps([sys.pycache_prefix,"
        "sys.dont_write_bytecode,cache_probe.VALUE]))"
    )
    # Independent live CPython control proves each retained cache executes.
    for namespace in ("", str(foreign)):
        ordinary = run_custody_subject_process(
            [sys.executable, "-B", "-X", f"pycache_prefix={namespace}", "-c", program],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=20,
            check=False,
        )
        assert ordinary.returncode == 0, ordinary.stderr
        assert json.loads(ordinary.stdout)[2] == 999

    captured = process_image_capture.capture_image(
        interpreter_role, Path(child_executable), preserve_path=True
    )
    images = [captured, *_python_base_process_images()]
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {
            "python": {
                "location": {"prefix": str(selected)},
                "process_images": images,
            }
        },
        environment_executables={},
    )
    selected_grant = next(
        row
        for row in policy["allowed"]
        if row["path"] == execution_custody._norm(child_executable)
    )
    assert selected_grant["python_bytecode"] == {
        "environment_root": str(selected),
        "pycache_prefix": str(prefix),
    }
    assert selected_grant["python_launch"] == "interpreter"
    # A second exact executable grant must not bypass interpreter custody.
    policy["allowed"].insert(
        0,
        {
            "toolchain": "generic-tool",
            "path": selected_grant["path"],
            "sha256": selected_grant["sha256"],
        },
    )
    cases = [
        {
            "name": "environment",
            "argv": [child_executable, "-c", program],
            "allow": True,
        },
        *(
            {
                "name": flag,
                "argv": python_environment_command(
                    [child_executable, flag, "-c", program], environment_root=selected
                ),
                "allow": True,
            }
            for flag in ("-I", "-E")
        ),
        {
            "name": "conflict",
            "argv": [
                child_executable,
                "-B",
                "-X",
                f"pycache_prefix={foreign}",
                "-c",
                program,
            ],
            "allow": False,
        },
        {
            "name": "duplicate",
            "argv": [
                child_executable,
                "-B",
                "-X",
                f"pycache_prefix={prefix}",
                "-X",
                f"pycache_prefix={prefix}",
                "-c",
                program,
            ],
            "allow": False,
        },
        {
            "name": "ignored-environment",
            "argv": [child_executable, "-I", "-c", program],
            "allow": False,
        },
        {
            "name": "writes-enabled",
            "argv": [
                child_executable,
                "-E",
                "-X",
                f"pycache_prefix={prefix}",
                "-c",
                program,
            ],
            "allow": False,
        },
        {
            "name": "terminal",
            "argv": [child_executable, "--help", "-X", f"pycache_prefix={foreign}"],
            "allow": True,
        },
    ]
    environment = {
        **os.environ,
        "PYTHONPYCACHEPREFIX": str(prefix),
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    server = execution_custody.ChildCustodyEventServer(runtime, policy)
    environment.update(
        {execution_custody.CHILD_POLICY_ENV: json.dumps(policy), **server.environment()}
    )
    if runtime == "python":
        cases.append(
            {
                "name": "native-environment",
                "argv": [child_executable, "-c", program],
                "allow": False,
                "putenv": str(foreign),
            }
        )
        payload = (
            "import json,os,subprocess\n"
            f"cases={cases!r}\n"
            "rows=[]\n"
            "for case in cases:\n"
            " if 'putenv' in case: os.putenv('PYTHONPYCACHEPREFIX',case['putenv'])\n"
            " try:\n"
            "  r=subprocess.run(case['argv'],capture_output=True,text=True,timeout=15)\n"
            "  rows.append({'name':case['name'],'allowed':True,'rc':r.returncode,'stdout':r.stdout})\n"
            " except PermissionError:\n"
            "  rows.append({'name':case['name'],'allowed':False})\n"
            " finally:\n"
            "  if 'putenv' in case: os.putenv('PYTHONPYCACHEPREFIX',os.environ['PYTHONPYCACHEPREFIX'])\n"
            "print(json.dumps(rows))"
        )
        bootstrap = Path(execution_custody.__file__).with_name(
            "python_custody_bootstrap.py"
        )
        argv = [sys.executable, str(bootstrap), "command", "0", payload]
    else:
        environment["NODE_OPTIONS"] = execution_custody.node_bootstrap_options()
        payload = (
            "const cp=require('child_process');const rows=[];"
            f"const cases={json.dumps(cases)};"
            "for(const c of cases){try{const r=cp.spawnSync(c.argv[0],c.argv.slice(1),{encoding:'utf8',timeout:15000});"
            "rows.push({name:c.name,allowed:true,rc:r.status,stdout:r.stdout});}"
            "catch(e){if(!String(e).includes('python-bytecode-custody:'))throw e;"
            "rows.push({name:c.name,allowed:false});}}console.log(JSON.stringify(rows));"
        )
        argv = [node, "-e", payload]
    with server:
        completed = run_custody_subject_process(
            argv,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=60,
            check=False,
        )
    assert completed.returncode == 0, completed.stderr
    observed = json.loads(completed.stdout)
    assert [(r["name"], r["allowed"]) for r in observed] == [
        (r["name"], r["allow"]) for r in cases
    ]
    for row in observed:
        if row["allowed"]:
            assert row["rc"] == 0, row
            if row["name"] != "terminal":
                assert json.loads(row["stdout"]) == [str(prefix), True, 111]
    receipt = server.receipt()
    assert receipt["broker_complete"] is True, receipt
    decisions = [
        row for row in receipt["events"] if row.get("event") == "child-process"
    ]
    assert [row["admitted"] for row in decisions] == [row["allow"] for row in cases]
    assert all(
        row["reason"].startswith("python-bytecode-custody:")
        for row in decisions
        if not row["admitted"]
    )
    assert all(cache.read_bytes() == data for cache, data in caches)
    assert source.read_bytes() == original


def test_python_child_policy_rejects_unknown_launch_roles(tmp_path):
    from tools.proof_queue_pkg import process_image_capture

    captured = process_image_capture.capture_image(
        "unproven-python-launch", Path(sys.executable), preserve_path=True
    )
    with pytest.raises(ValueError, match="unknown launch role"):
        execution_custody.child_policy(
            {"process_closure": {"descendants": "declared-toolchains"}},
            {
                "python": {
                    "location": {"prefix": str(tmp_path)},
                    "process_images": [captured],
                }
            },
            environment_executables={},
        )


@pytest.mark.skipif(os.name == "nt", reason="POSIX kernel shebang image boundary")
def test_python_console_shebang_uses_explicit_interpreter_custody(tmp_path):
    from molt.python_environment_location import environment_bytecode_prefix
    from tools.proof_command_argv import python_environment_command
    from tools.proof_queue_pkg import process_image_capture

    marker = tmp_path / "executed"
    launcher = tmp_path / "console-probe"
    launcher.write_text(
        f"#!{sys.executable}\nfrom pathlib import Path\nPath({str(marker)!r}).write_text('ran')\n",
        encoding="utf-8",
    )
    launcher.chmod(0o755)
    # The same genuine source executes through both ordinary kernel shebang
    # launch and the explicit interpreter boundary used by proof consumers.
    ordinary = run_custody_subject_process([launcher], timeout=20, check=False)
    assert ordinary.returncode == 0
    assert marker.read_text(encoding="utf-8") == "ran"
    marker.unlink()
    images = [
        process_image_capture.capture_image(
            "selected-interpreter", Path(sys.executable), preserve_path=True
        ),
        process_image_capture.capture_image(
            "console-script:probe", launcher, preserve_path=True
        ),
        process_image_capture.capture_image(
            "distribution-executable:probe", launcher, preserve_path=True
        ),
    ]
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {"python": {"location": {"prefix": str(tmp_path)}, "process_images": images}},
        environment_executables={},
    )
    grants = [row for row in policy["allowed"] if row["path"] == str(launcher)]
    assert {row["python_launch"] for row in grants} == {"console-script", "native"}
    assert next(row for row in grants if row["python_launch"] == "console-script")[
        "python_bytecode"
    ] == {
        "environment_root": str(tmp_path),
        "pycache_prefix": str(environment_bytecode_prefix(tmp_path)),
    }
    commands = [
        [str(launcher)],
        python_environment_command(
            [sys.executable, str(launcher)], environment_root=tmp_path
        ),
    ]
    payload = (
        "import subprocess\n"
        f"commands={commands!r}\n"
        "for command in commands:\n"
        " try: subprocess.run(command,check=True); print('admitted')\n"
        " except PermissionError: print('refused')\n"
    )
    server = execution_custody.ChildCustodyEventServer("python", policy)
    environment = {
        **os.environ,
        "PYTHONPYCACHEPREFIX": str(environment_bytecode_prefix(tmp_path)),
        "PYTHONDONTWRITEBYTECODE": "1",
        execution_custody.CHILD_POLICY_ENV: json.dumps(policy),
        **server.environment(),
    }
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    with server:
        completed = run_custody_subject_process(
            [sys.executable, bootstrap, "command", "0", payload],
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.splitlines() == ["refused", "admitted"]
    assert marker.read_text(encoding="utf-8") == "ran"
    decisions = [
        row for row in server.receipt()["events"] if row.get("event") == "child-process"
    ]
    assert [row["admitted"] for row in decisions] == [False, True]
    assert decisions[0]["reason"] == "implicit-interpreter"


@pytest.mark.skipif(os.name != "nt", reason="actual Windows CPython CRT argv")
@pytest.mark.parametrize("tail", ['a"b"" c d', r'"ab\"c" "\\" d', '"" "unterminated'])
def test_windows_python_crt_decoder_matches_actual_child(tmp_path, tail):
    import subprocess
    from tools.proof_command_argv import split_windows_python_command_line

    program = "import json,sys;print(json.dumps(sys.orig_argv))"
    executable = Path(sys._base_executable).resolve(strict=True)
    command = (
        subprocess.list2cmdline(
            [
                str(executable),
                "-I",
                "-B",
                "-X",
                f"pycache_prefix={tmp_path / 'empty'}",
                "-c",
                program,
            ]
        )
        + " "
        + tail
    )
    actual = _run_windows_raw_python_subject(
        command,
        executable=executable,
        tmp_path=tmp_path,
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert actual.returncode == 0, actual.stderr
    assert json.loads(actual.stdout) == list(split_windows_python_command_line(command))


@pytest.mark.skipif(os.name != "nt", reason="actual Windows CPython startup options")
def test_windows_python_doubled_quote_warning_cannot_grant_hidden_flags(tmp_path):
    import subprocess
    from tools.proof_queue_pkg import process_image_capture
    from tools.proof_command_argv import split_windows_python_command_line
    from molt.python_environment_location import environment_bytecode_prefix

    prefix = str(environment_bytecode_prefix(tmp_path))
    foreign = str(tmp_path / "foreign")
    program = "import json,sys;print(json.dumps([sys.orig_argv,sys.pycache_prefix,sys.dont_write_bytecode]))"
    executable = Path(sys._base_executable).resolve(strict=True)
    command = (
        subprocess.list2cmdline([str(executable)])
        + f' -W"x"" -B -Xpycache_prefix={prefix} " -c '
        + subprocess.list2cmdline([program])
    )
    environment = {
        key: value
        for key, value in os.environ.items()
        if key.upper() not in {"PYTHONPYCACHEPREFIX", "PYTHONDONTWRITEBYTECODE"}
    }
    environment["PYTHONPYCACHEPREFIX"] = foreign
    actual = _run_windows_raw_python_subject(
        command,
        executable=executable,
        tmp_path=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert actual.returncode == 0, actual.stderr
    observed_argv, observed_prefix, observed_readonly = json.loads(actual.stdout)
    assert observed_argv == list(split_windows_python_command_line(command))
    assert (observed_prefix, observed_readonly) == (foreign, False)
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {
            "python": {
                "location": {"prefix": str(tmp_path)},
                "process_images": [
                    process_image_capture.capture_image(
                        "selected-interpreter", executable, preserve_path=True
                    )
                ],
            }
        },
        environment_executables={},
    )
    policy["allowed"].insert(
        0,
        {
            "toolchain": "generic",
            "path": policy["allowed"][0]["path"],
            "sha256": policy["allowed"][0]["sha256"],
        },
    )
    facts = python_child_custody._windows_caller_facts(
        python_child_custody._native_environment_reader()
    )()
    intent = {
        **facts,
        "application_name": str(executable),
        "command_line": command,
        "python_environment": {
            "PYTHONPYCACHEPREFIX": foreign,
            "PYTHONDONTWRITEBYTECODE": None,
        },
    }
    with execution_custody.ChildCustodyEventServer(None, policy) as server:
        decision = server._decide_child(intent, "python")
    assert decision["admitted"] is False
    assert decision["reason"].startswith("python-bytecode-custody:")


def test_python_console_startup_is_image_bound_and_payload_flags_cannot_weaken_it(
    tmp_path, monkeypatch
):
    import io
    import zipfile
    from molt.python_environment_location import environment_bytecode_prefix

    interpreter = tmp_path / "python.exe"
    interpreter.write_bytes(b"captured interpreter")
    launcher = tmp_path / "probe.exe"
    with io.BytesIO() as archive_bytes:
        with zipfile.ZipFile(archive_bytes, "w") as archive:
            archive.writestr("__main__.py", "print('console payload')\n")
        launcher.write_bytes(
            b"MZfixture" + f'#!"{interpreter}"\n'.encode() + archive_bytes.getvalue()
        )
    launcher.chmod(0o755)
    digest = hashlib.sha256(launcher.read_bytes()).hexdigest()
    # PE resource lookup is the independent platform service outside this
    # portable ZIP/shebang control. Real Windows launcher execution is below.
    monkeypatch.setattr(
        execution_custody, "_windows_uv_launcher_metadata", lambda path: None
    )
    startup, transport = execution_custody._windows_python_console_startup(
        launcher, {str(interpreter)}, sha256=digest
    )
    assert startup == [str(interpreter), str(launcher)]
    assert transport == "unicode"
    with pytest.raises(ValueError, match="outside capture"):
        execution_custody._windows_python_console_startup(
            launcher, set(), sha256=digest
        )
    with pytest.raises(ValueError, match="differs from its captured image"):
        execution_custody._windows_python_console_startup(
            launcher, {str(interpreter)}, sha256="0" * 64
        )
    authority = {
        "toolchain": "python",
        "python_launch": "console-script",
        "path": execution_custody._norm(launcher),
        "sha256": digest,
        "python_bytecode": {
            "environment_root": str(tmp_path),
            "pycache_prefix": str(environment_bytecode_prefix(tmp_path)),
        },
        "python_startup_argv": startup,
        "python_windows_transport": transport,
    }
    policy = {
        "schema": execution_custody.CHILD_POLICY_SCHEMA,
        "descendants": "declared-toolchains",
        "allowed": [
            {"toolchain": "generic", "path": authority["path"], "sha256": digest},
            {
                "toolchain": "python",
                "python_launch": "native",
                "path": authority["path"],
                "sha256": digest,
                "python_bytecode": None,
            },
            authority,
        ],
    }
    intent = {
        "requested": str(launcher),
        "argv": [
            str(launcher),
            "--help",
            "-B",
            "-X",
            f"pycache_prefix={environment_bytecode_prefix(tmp_path)}",
        ],
        "python_environment": {
            "PYTHONPYCACHEPREFIX": str(tmp_path / "foreign"),
            "PYTHONDONTWRITEBYTECODE": None,
        },
        "windows_verbatim_arguments": False,
    }
    with execution_custody.ChildCustodyEventServer(None, policy) as server:
        refused = server._decide_child(intent, "node")
        assert refused["admitted"] is False
        assert refused["reason"].startswith("python-bytecode-custody:")
        safe = {
            "PYTHONPYCACHEPREFIX": str(environment_bytecode_prefix(tmp_path)),
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        assert (
            server._decide_child({**intent, "python_environment": safe}, "node")[
                "admitted"
            ]
            is True
        )
        authority["python_startup_argv"] = [str(interpreter), "-I", str(launcher)]
        assert (
            server._decide_child({**intent, "python_environment": safe}, "node")[
                "admitted"
            ]
            is False
        )
    assert hashlib.sha256(launcher.read_bytes()).hexdigest() == digest


@pytest.mark.parametrize(
    "declaration,expected", [("UTF-8", True), ("Legacy", False), ("", False)]
)
def test_python_console_codepage_comes_from_launcher_manifest(declaration, expected):
    # This is the Windows application-manifest declaration, independent of the
    # broker process's own GetACP value or environment.
    raw = (
        '<assembly xmlns="urn:schemas-microsoft-com:asm.v1" xmlns:v3="urn:schemas-microsoft-com:asm.v3">'
        "<v3:application><v3:windowsSettings>"
        f'<activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">{declaration}</activeCodePage>'
        "</v3:windowsSettings></v3:application></assembly>"
    ).encode()
    assert execution_custody._windows_console_manifest_uses_utf8(raw) is expected
    assert not execution_custody._windows_console_manifest_uses_utf8(None)
    assert not execution_custody._windows_console_manifest_uses_utf8(
        b"<!DOCTYPE assembly><assembly/>"
    )


def test_python_console_rejects_zip64_before_constructing_zipfile(monkeypatch):
    import io
    import zipfile

    classic = struct.pack("<4s4H2LH", b"PK\x05\x06", 0, 0, 1, 1, 57, 40, 0)
    locator = struct.pack("<4sLQL", b"PK\x06\x07", 0, 10, 1)
    malformed = io.BytesIO(b"MZ" + b"x" * 64 + locator + classic)

    def forbidden(*args, **kwargs):
        raise AssertionError("ZIP64 must be refused before ZipFile reads its directory")

    monkeypatch.setattr(zipfile, "ZipFile", forbidden)
    with pytest.raises(ValueError, match="ZIP64 metadata"):
        execution_custody._distlib_python_console_prefix(malformed)


@pytest.mark.parametrize("layout", ["negative-offset", "unindexed-prefix"])
def test_python_console_requires_the_actual_classic_archive_base(layout):
    import io
    import zipfile

    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w") as writer:
        writer.writestr("__main__.py", "print('source')\n")
    zipped = bytearray(archive.getvalue())
    central = zipped.index(b"PK\x01\x02")
    end = len(zipped) - 22
    if layout == "negative-offset":
        # CPython adjusts header_offset by its inferred concatenation prefix.
        # This classic offset pushes that adjusted header below zero.
        struct.pack_into("<L", zipped, end + 16, 1024 * 1024)
        leading = b""
    else:
        # The native distlib launcher sees the original archive-base shebang;
        # a separately indexed second local entry must not select a fake one.
        leading = b"unindexed-local-block\n#!C:\\foreign\\python.exe -B\n"
        struct.pack_into("<L", zipped, central + 42, len(leading))
        original_directory = struct.unpack_from("<L", zipped, end + 16)[0]
        struct.pack_into("<L", zipped, end + 16, original_directory + len(leading))
    raw = (
        b"MZ"
        + b"x" * (300 * 1024)
        + b"#!C:\\selected\\python.exe\n"
        + leading
        + bytes(zipped)
    )

    class BoundedReader(io.BytesIO):
        def read(self, count=-1):
            remaining = len(raw) - self.tell()
            assert (
                remaining if count < 0 else min(count, remaining)
            ) <= 46 + 3 * 65535, "decoder attempted an unbounded image read"
            return super().read(count)

    with pytest.raises(ValueError, match="invalid archive offset"):
        execution_custody._distlib_python_console_prefix(BoundedReader(raw))


def test_python_interpreter_unicode_transport_is_image_bound(tmp_path, monkeypatch):
    interpreter = tmp_path / "python.exe"
    interpreter.write_bytes(b"captured image")
    digest = hashlib.sha256(interpreter.read_bytes()).hexdigest()
    metadata = {"kind": "python", "interpreter": str(interpreter), "utf8": False}
    monkeypatch.setattr(
        execution_custody, "_windows_uv_launcher_metadata", lambda path: metadata
    )
    interpreters = {execution_custody._norm(interpreter)}
    assert (
        execution_custody._windows_python_interpreter_transport(
            interpreter, interpreters, sha256=digest
        )
        == "ascii"
    )
    unicode_interpreter = tmp_path / "漢字.exe"
    unicode_interpreter.write_bytes(b"captured target")
    metadata["interpreter"] = str(unicode_interpreter)
    with pytest.raises(ValueError, match="captured UTF-8 manifest"):
        execution_custody._windows_python_interpreter_transport(
            interpreter,
            {*interpreters, execution_custody._norm(unicode_interpreter)},
            sha256=digest,
        )
    metadata["interpreter"] = str(interpreter)
    authority = {
        "toolchain": "python",
        "python_launch": "interpreter",
        "python_windows_transport": "ascii",
        "python_bytecode": {
            "environment_root": str(tmp_path),
            "pycache_prefix": str(tmp_path / "bytecode"),
        },
    }
    intent = {
        "command_line": f'python.exe -B -X pycache_prefix={tmp_path / "bytecode"} -c "print(1)"',
        "python_environment": {
            "PYTHONPYCACHEPREFIX": None,
            "PYTHONDONTWRITEBYTECODE": None,
        },
    }
    execution_custody._validate_python_child_launch(authority, intent, windows=True)
    with pytest.raises(ValueError, match="unproven Unicode transport"):
        execution_custody._validate_python_child_launch(
            authority,
            {**intent, "command_line": intent["command_line"] + " 漢字"},
            windows=True,
        )
    metadata["utf8"] = True
    assert (
        execution_custody._windows_python_interpreter_transport(
            interpreter, interpreters, sha256=digest
        )
        == "unicode"
    )
    with pytest.raises(ValueError, match="differs from its captured image"):
        execution_custody._windows_python_interpreter_transport(
            interpreter, interpreters, sha256="0" * 64
        )


@pytest.mark.parametrize("retarget", [False, True])
def test_python_launcher_transport_preserves_alias_generation(
    tmp_path, monkeypatch, retarget
):
    original = tmp_path / "original.exe"
    other = tmp_path / "other.exe"
    data = b"same captured executable bytes"
    original.write_bytes(data)
    other.write_bytes(data)
    alias = tmp_path / "python-alias.exe"
    try:
        alias.symlink_to(original)
    except OSError as exc:
        if os.name == "nt" and exc.winerror == 1314:
            pytest.skip("Windows host lacks symbolic-link creation privilege")
        raise

    def metadata(entrypoint):
        assert entrypoint == alias
        if retarget:
            alias.unlink()
            alias.symlink_to(other)
        return None

    monkeypatch.setattr(execution_custody, "_windows_uv_launcher_metadata", metadata)

    def capture():
        return execution_custody._windows_python_interpreter_transport(
            alias,
            {execution_custody._norm(alias)},
            sha256=hashlib.sha256(data).hexdigest(),
        )

    if retarget:
        with pytest.raises(ValueError, match="entrypoint changed"):
            capture()
    else:
        assert capture() == "unicode"
    assert original.read_bytes() == other.read_bytes() == data


@pytest.mark.skipif(
    os.name != "nt", reason="actual Windows Python forwarding launchers"
)
@pytest.mark.parametrize("family", ["uv", "cpython-venv"])
def test_windows_python_forwarder_cannot_remove_observed_startup_options(
    tmp_path, family
):
    import shutil
    import venv
    from tools.proof_queue_pkg import process_image_capture
    from tools.proof_command_argv import (
        split_windows_python_command_line,
        validate_python_environment_command,
    )

    image = tmp_path / "image"
    if family == "uv":
        uv = shutil.which("uv")
        assert uv is not None, (
            "the Windows uv-proxy proof requires the declared uv tool"
        )
        created = run_custody_subject_process(
            [
                uv,
                "venv",
                "--offline",
                "--no-config",
                "--python",
                sys._base_executable,
                str(image),
            ],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        assert created.returncode == 0, created.stderr
    else:
        venv.EnvBuilder(with_pip=False).create(image)
    proxy = image / "Scripts" / "python.exe"
    metadata = execution_custody._windows_uv_launcher_metadata(proxy)
    assert (
        (metadata is not None and metadata["kind"] == "python")
        if family == "uv"
        else metadata is None
    )
    images = [
        process_image_capture.capture_image(
            "selected-interpreter", proxy, preserve_path=True
        ),
        *_python_base_process_images(),
    ]
    # A pure ASCII namespace projection makes the raw counterexamples exact
    # regardless of spaces/nonASCII in the real disposable launcher location.
    # No child is run with this namespace: the ordinary control uses foreign.
    policy_root = Path(tmp_path.anchor) / "molt-custody-selected"
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {
            "python": {
                "location": {"prefix": str(policy_root)},
                "process_images": images,
            }
        },
        environment_executables={},
    )
    expected = str(policy_root / "bytecode")
    program = "print(__import__('json').dumps([__import__('sys').orig_argv,__import__('sys').pycache_prefix,__import__('sys').dont_write_bytecode]))"
    command = (
        f'"fake\\" -B -X pycache_prefix={expected} -c "dummy -c {program}"'
        if family == "uv"
        else f'"fake"-c{program} -B -X pycache_prefix={expected} -c dummy'
    )
    foreign = str(tmp_path / "foreign")
    environment = {
        key: value
        for key, value in os.environ.items()
        if key.upper() not in {"PYTHONPYCACHEPREFIX", "PYTHONDONTWRITEBYTECODE"}
    }
    environment["PYTHONPYCACHEPREFIX"] = foreign
    # The old CRT-only admission really sees apparently safe startup flags;
    # refusal must follow from forwarding, not a malformed cache-prefix fixture.
    validate_python_environment_command(
        split_windows_python_command_line(command),
        environment_root=policy_root,
        environment={"PYTHONPYCACHEPREFIX": foreign},
    )
    ordinary = _run_windows_raw_python_subject(
        command,
        executable=str(proxy),
        tmp_path=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert ordinary.returncode == 0, ordinary.stderr
    actual_argv, actual_prefix, actual_readonly = json.loads(ordinary.stdout)
    assert actual_argv[1].startswith("-c")
    assert (actual_prefix, actual_readonly) == (foreign, False)
    facts = python_child_custody._windows_caller_facts(
        python_child_custody._native_environment_reader()
    )()
    intent = {
        **facts,
        "application_name": str(proxy),
        "command_line": command,
        "python_environment": {
            "PYTHONPYCACHEPREFIX": foreign,
            "PYTHONDONTWRITEBYTECODE": None,
        },
    }
    with execution_custody.ChildCustodyEventServer(None, policy) as server:
        decision = server._decide_child(intent, "python")
    assert decision["admitted"] is False
    assert "ambiguous forwarding" in decision["reason"]


@pytest.mark.skipif(os.name != "nt", reason="actual Windows distlib console launcher")
@pytest.mark.parametrize("runtime", ["python", "node"])
def test_windows_python_console_launcher_honors_captured_startup(tmp_path, runtime):
    import shutil
    from distlib.scripts import ScriptMaker
    from molt.python_environment_location import environment_bytecode_prefix
    from tools.proof_queue_pkg import process_image_capture

    node = shutil.which("node") if runtime == "node" else None
    if runtime == "node" and node is None:
        pytest.skip("node is unavailable")
    prefix = str(environment_bytecode_prefix(tmp_path))
    payload_args = ["--help", "-I", "-B", "-X", "pycache_prefix=payload"]
    maker = ScriptMaker(None, str(tmp_path / "Scripts"))
    maker.executable = sys.executable
    maker.variants = {""}
    maker.script_template = "import json,sys\nprint(json.dumps([sys.pycache_prefix,sys.dont_write_bytecode,sys.argv[1:]]))\n"
    launchers = {}
    for name, options in (
        ("ordinary", []),
        ("isolated", ["-I"]),
        ("explicit", ["-I", "-B", "-X", f"pycache_prefix={prefix}"]),
    ):
        files = maker.make(
            f"{name} = unused:main", options={"interpreter_args": options}
        )
        assert len(files) == 1
        launchers[name] = files[0]
    foreign = str(tmp_path / "foreign")
    unsafe_environment = {
        key: value
        for key, value in os.environ.items()
        if key.upper() not in {"PYTHONPYCACHEPREFIX", "PYTHONDONTWRITEBYTECODE"}
    }
    unsafe_environment["PYTHONPYCACHEPREFIX"] = foreign
    ordinary = run_custody_subject_process(
        [launchers["ordinary"], *payload_args],
        env=unsafe_environment,
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert ordinary.returncode == 0, ordinary.stderr
    assert json.loads(ordinary.stdout) == [foreign, False, payload_args]
    retained = {
        path: path.read_bytes() for path in Path(foreign).rglob("*") if path.is_file()
    }
    images = [
        process_image_capture.capture_image(
            "selected-interpreter", Path(sys.executable), preserve_path=True
        ),
        *_python_base_process_images(),
    ]
    images.extend(
        process_image_capture.capture_image(
            f"console-script:{name}", Path(path), preserve_path=True
        )
        for name, path in launchers.items()
    )
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {"python": {"location": {"prefix": str(tmp_path)}, "process_images": images}},
        environment_executables={},
    )
    policy["allowed"][:0] = [
        {"toolchain": "generic", "path": row["path"], "sha256": row["sha256"]}
        for row in policy["allowed"]
    ]
    safe = {"PYTHONPYCACHEPREFIX": prefix, "PYTHONDONTWRITEBYTECODE": "1"}
    cases = [
        {
            "name": "ordinary",
            "argv": [launchers["ordinary"], *payload_args],
            "env": safe,
            "allow": True,
        },
        {
            "name": "foreign",
            "argv": [launchers["ordinary"], *payload_args],
            "env": {**safe, "PYTHONPYCACHEPREFIX": foreign},
            "allow": False,
        },
        {
            "name": "writes",
            "argv": [launchers["ordinary"], *payload_args],
            "env": {**safe, "PYTHONDONTWRITEBYTECODE": None},
            "allow": False,
        },
        {
            "name": "ignored",
            "argv": [launchers["isolated"], *payload_args],
            "env": safe,
            "allow": False,
        },
        {
            "name": "explicit",
            "argv": [launchers["explicit"], *payload_args],
            "env": {"PYTHONPYCACHEPREFIX": foreign, "PYTHONDONTWRITEBYTECODE": None},
            "allow": True,
        },
    ]
    server = execution_custody.ChildCustodyEventServer(runtime, policy)
    environment = {
        **os.environ,
        **safe,
        execution_custody.CHILD_POLICY_ENV: json.dumps(policy),
        **server.environment(),
    }
    if runtime == "python":
        payload = (
            "import json,os,subprocess\nrows=[]\n"
            f"for case in {cases!r}:\n"
            " env=dict(os.environ)\n"
            " for key,value in case['env'].items():\n"
            "  if value is None: env.pop(key,None)\n"
            "  else: env[key]=value\n"
            " try:\n"
            "  result=subprocess.run(case['argv'],env=env,capture_output=True,text=True,timeout=15)\n"
            "  rows.append([case['name'],True,result.returncode,result.stdout])\n"
            " except PermissionError: rows.append([case['name'],False])\n"
            "print(json.dumps(rows))"
        )
        bootstrap = Path(execution_custody.__file__).with_name(
            "python_custody_bootstrap.py"
        )
        argv = [sys.executable, bootstrap, "command", "0", payload]
    else:
        environment["NODE_OPTIONS"] = execution_custody.node_bootstrap_options()
        payload = (
            "const cp=require('child_process'),rows=[];"
            f"for(const c of {json.dumps(cases)}){{const env={{...process.env}};"
            "for(const [k,v] of Object.entries(c.env)){if(v===null)delete env[k];else env[k]=v;}"
            "try{const r=cp.spawnSync(c.argv[0],c.argv.slice(1),{env,encoding:'utf8',timeout:15000});"
            "rows.push([c.name,true,r.status,r.stdout]);}catch(e){if(!String(e).includes('python-bytecode-custody:'))throw e;rows.push([c.name,false]);}}"
            "console.log(JSON.stringify(rows));"
        )
        argv = [node, "-e", payload]
    with server:
        completed = run_custody_subject_process(
            argv,
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
    assert completed.returncode == 0, completed.stderr
    rows = json.loads(completed.stdout)
    assert [(row[0], row[1]) for row in rows] == [
        (case["name"], case["allow"]) for case in cases
    ]
    for row in rows:
        if row[1]:
            assert row[2] == 0, row
            assert json.loads(row[3]) == [prefix, True, payload_args]
    assert server.receipt()["broker_complete"] is True
    assert all(path.read_bytes() == data for path, data in retained.items())


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
    from molt.python_environment_location import environment_bytecode_prefix
    from tools.proof_command_argv import python_environment_command

    environment_root = tmp_path / "selected-image"
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
    child_argv = python_environment_command(
        [name, "-I", "-S", "-c", "print('selected-native-child')"],
        environment_root=environment_root,
    )
    payload = (
        "import json, subprocess\n"
        "try:\n"
        f" result = subprocess.run({child_argv!r}, "
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
                "python_launch": "interpreter",
                "path": execution_custody._norm(directory / name),
                "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
                "python_bytecode": {
                    "environment_root": str(environment_root),
                    "pycache_prefix": str(
                        environment_bytecode_prefix(environment_root)
                    ),
                },
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
    from molt.python_environment_location import environment_bytecode_prefix

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
            # These children are suspended for the image oracle; terminal
            # argv also preserves no-payload semantics if ever resumed.
            "args": [args[0], "--version", *args[1:]]
            if isinstance(args, list)
            else args,
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
        case("string-command", "hf137j --version", image("caller-path", "hf137j.exe")),
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
                "python_launch": "interpreter",
                "path": execution_custody._norm(path),
                "python_windows_transport": "unicode",
                "sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest(),
                "python_bytecode": {
                    "environment_root": str(tmp_path),
                    "pycache_prefix": str(environment_bytecode_prefix(tmp_path)),
                },
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
    identity = {"runtime": runtime, "pid": 123, "connection_id": 0}
    if runtime == "node":
        identity["thread_id"] = 0
    receipt = {
        "schema": execution_custody.CHILD_CUSTODY_RECEIPT_SCHEMA,
        "broker_complete": True,
        "errors": [],
        "violations": [],
        "events": [
            {"event": "hook-start", **identity},
            {"event": "child-process", "admitted": True, "connection_id": 0},
            {"event": "hook-end", **identity},
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
        ("schema", "molt.proof-child-custody-receipt.v3"),
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


def _install_linux_inotify(monkeypatch, *, denied=(), read_next=None):
    """Observe real path selection at the existing native inotify boundary."""
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
            path = Path(os.fsdecode(raw_path))
            add_calls.append(path)
            if path in denied:
                return -1
            return len(add_calls)

        inotify_add_watch = FakeFunction(_add)

    def read(_fd, _length):
        if read_next is not None:
            return read_next()
        raise BlockingIOError()

    install_module_view(
        monkeypatch,
        "ctypes",
        execution_custody.ctypes,
        execution_custody,
        CDLL=lambda *_a, **_k: FakeLibc(),
        get_errno=lambda: errno.EACCES,
    )
    install_module_view(
        monkeypatch,
        "os",
        os,
        execution_custody,
        O_NONBLOCK=0x800,
        O_CLOEXEC=0x80000,
        read=read,
    )
    return add_calls


def test_linux_root_watch_is_installed_before_recursive_enumeration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "root"
    root.mkdir()
    monitor = execution_custody.LiveCustodyMonitor(
        [execution_custody.WatchSpec(root, None)]
    )
    add_calls = _install_linux_inotify(monkeypatch)
    original_rglob = Path.rglob

    def asserting_rglob(path: Path, pattern: str):
        assert path == root
        assert add_calls == [root]
        return original_rglob(path, pattern)

    monkeypatch.setattr(Path, "rglob", asserting_rglob)
    monitor._stop.set()

    monitor._run_linux()

    assert add_calls == [root]
    assert monitor._ready.is_set()
    assert monitor._errors == []


@pytest.mark.parametrize("nested", [False, True])
def test_linux_finite_watch_visits_only_selected_ancestors(
    tmp_path: Path, nested: bool
) -> None:
    root = tmp_path / "system-lib"
    parent = root / "selected" / "nested" if nested else root
    parent.mkdir(parents=True)
    selected = parent / "input.py"
    selected.write_bytes(b"selected input")
    unrelated = root / "ssl"
    private = unrelated / "private"
    private.mkdir(parents=True)
    (private / "unrelated-secret").write_bytes(b"not a proof input")
    # Observe traversal through every filesystem API, including pathlib and
    # os.walk, in a disposable process. A module view would miss those APIs'
    # internal scandir calls; a process-wide patch would affect pytest custody.
    script = """
import json
import os
import sys
from pathlib import Path
import pytest
from tests.tools.test_proof_queue_execution_custody_p0 import _install_linux_inotify
from tools.proof_queue_pkg import execution_custody

root, selected, private = map(Path, sys.argv[1:])
monitor = execution_custody.LiveCustodyMonitor([
    execution_custody.WatchSpec(
        root, frozenset({execution_custody._event_key(selected)})
    )
])
scanned = []
real_scandir = os.scandir
def observe_scandir(path):
    scanned.append(str(path))
    return real_scandir(path)

with pytest.MonkeyPatch.context() as patch:
    add_calls = _install_linux_inotify(patch, denied=(private,))
    os.scandir = observe_scandir
    try:
        # Independent controls prove indirect traversal reaches the observer.
        for traversal in (lambda: private.rglob("*"), lambda: os.walk(private)):
            list(traversal())
            assert str(private) in scanned
            scanned.clear()
        monitor._stop.set()
        monitor._run_linux()
    finally:
        os.scandir = real_scandir
print(json.dumps({
    "errors": monitor._errors,
    "added": [str(path) for path in add_calls],
    "scanned": scanned,
}))
"""
    result = run_custody_subject_process(
        [sys.executable, "-B", "-c", script, str(root), str(selected), str(private)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=20,
    )
    observed = json.loads(result.stdout)
    assert observed["errors"] == []
    expected = {root, root / "selected", parent} if nested else {root}
    assert set(map(Path, observed["added"])) == expected
    assert len(observed["added"]) == len(expected)
    assert not any(Path(path).is_relative_to(unrelated) for path in observed["scanned"])


@pytest.mark.parametrize("action", [0x00000100, 0x00000080], ids=["create", "move-in"])
def test_linux_finite_watch_extends_only_for_selected_ancestor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, action: int
) -> None:
    root = tmp_path / "root"
    root.mkdir()
    selected = root / "owned" / "input.py"
    monitor = execution_custody.LiveCustodyMonitor(
        [
            execution_custody.WatchSpec(
                root, frozenset({execution_custody._event_key(selected)})
            )
        ]
    )
    delivered = False

    def read_next():
        nonlocal delivered
        if delivered:
            raise BlockingIOError()
        delivered = True
        payload = []
        for name in ("unowned", "owned"):
            (root / name).mkdir()
            encoded = os.fsencode(name) + b"\0"
            payload.append(
                struct.pack("iIII", 1, 0x40000000 | action, 0, len(encoded)) + encoded
            )
        return b"".join(payload)

    add_calls = _install_linux_inotify(
        monkeypatch, denied=(root / "unowned",), read_next=read_next
    )
    monitor._stop.set()

    monitor._run_linux()

    assert monitor._errors == []
    assert add_calls == [root, root / "owned"]
    assert [Path(event["path"]) for event in monitor._events] == [root / "owned"]


@pytest.mark.parametrize("finite", [False, True], ids=["broad-root", "finite-ancestor"])
def test_linux_required_watch_permission_error_remains_fatal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, finite: bool
) -> None:
    root = tmp_path / "root"
    required = root / "required" if finite else root
    required.mkdir(parents=True)
    selected = required / "input.py"
    selected.write_bytes(b"required input")
    paths = frozenset({execution_custody._event_key(selected)}) if finite else None
    monitor = execution_custody.LiveCustodyMonitor(
        [execution_custody.WatchSpec(root, paths)]
    )
    add_calls = _install_linux_inotify(monkeypatch, denied=(required,))
    monitor._stop.set()

    monitor._run_linux()

    assert required in add_calls
    assert monitor._ready.is_set()
    assert len(monitor._errors) == 1
    assert "PermissionError" in monitor._errors[0]
    assert str(required) in monitor._errors[0]


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
    environment = environment.updated(
        {execution_custody.CHILD_POLICY_ENV: json.dumps(policy), **server.environment()}
    )
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


def test_vanished_watch_entries_keep_their_own_key(tmp_path, monkeypatch):
    # HF-169: Windows custody raises for an absent entry, and the watcher
    # reports the whole subtree, so a staging file renamed away under a watched
    # directory once counted as an input mutation. Emulate that lookup here.
    real_norm = execution_custody._norm

    def windows_norm(path):
        if not Path(path).exists():
            raise FileNotFoundError(path)
        return real_norm(path)

    monkeypatch.setattr(execution_custody, "_norm", windows_norm)
    watched = tmp_path / "pyproject.toml"
    watched.write_text("[project]\n", encoding="utf-8")
    spec = execution_custody.WatchSpec(tmp_path, frozenset({real_norm(watched)}))
    staged = tmp_path / "proof-receipts" / ".molt-stage-0.tmp"
    staged.parent.mkdir()
    staged.write_text("receipt", encoding="utf-8")
    staged.rename(staged.with_name("receipt.json"))
    assert not spec.owns(staged)
    watched.unlink()
    assert spec.owns(watched), "a removed watched file must remain an input event"
    # Windows may report a deleted entry by its 8.3 short name; stay owned.
    assert spec.owns(tmp_path / "PYPROJ~1.TOM")


@pytest.mark.parametrize("identity_kind", ["image", "python-location"])
def test_watch_custody_retains_ancestor_alias_and_deleted_entry(
    tmp_path, identity_kind
):
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
    identity: dict[str, object] = {"path": str(selected)}
    if identity_kind == "python-location":
        from tests.python_environment_test_support import environment_location_manifest

        # Pre-arm locator custody must retain the lexical base selector even
        # before full capture has projected its interpreter-alias image row.
        identity = environment_location_manifest(
            prefix=target,
            selected_executable=tool,
            base_executable=tool,
            base_executable_selector=selected,
            roots=[target],
            external_roots=[],
            file_paths=[tool],
        )
    specs = execution_custody.watch_specs(
        source_root=source,
        tracked_paths=[],
        identities=[identity],
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


def test_popen_announcement_covers_only_its_own_createprocess() -> None:
    # HF-138: Popen decides its launch, then calls _winapi.CreateProcess with
    # the same application name; a direct call (multiprocessing spawn) is new.
    announced = python_child_custody.WindowsLaunchAnnouncements()
    assert not announced.consume("C:\\tools\\python.exe")
    announced.announce(None)
    assert not announced.consume("C:\\tools\\python.exe")
    assert announced.consume(None)
    assert not announced.consume(None)


def test_popen_announcements_belong_to_the_announcing_thread() -> None:
    announced = python_child_custody.WindowsLaunchAnnouncements()
    announced.announce("C:\\tools\\python.exe")
    other_thread: list[bool] = []
    worker = threading.Thread(
        target=lambda: other_thread.append(announced.consume("C:\\tools\\python.exe"))
    )
    worker.start()
    worker.join()
    assert other_thread == [False]
    assert announced.consume("C:\\tools\\python.exe")


def test_shell_and_startfile_launches_are_opaque_but_judged_launches_are_not() -> None:
    opaque = python_child_custody.OPAQUE_PROCESS_EVENTS
    assert {"os.system", "os.startfile", "os.startfile/2"} <= opaque
    assert not {"subprocess.Popen", "_winapi.CreateProcess"} & opaque
