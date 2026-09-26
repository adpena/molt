from __future__ import annotations

import json
import os
from pathlib import Path
import py_compile
import subprocess
import sys

from packaging.requirements import Requirement
import pytest

from molt.cli import source_build_requirements as requirements
from molt.cli import source_extension_cython_tool as cython
from molt.cli.source_build_environment import (
    active_source_build_requirements,
    canonical_source_marker_environment,
    source_build_environment_problems,
)
from molt.cli.source_build_inventory import SourceBuildInventory
from molt.exact_json import canonical_json_sha256
from molt.python_file_node_custody import VerifiedTreeFile
from molt.toolchain_identity import stable_regular_file_identity
from tests.python_environment_test_support import build_environment_manifest


@pytest.mark.parametrize(
    "raw,version,admitted",
    [
        ("Cython>=3.0.6,<3.2", "3.2.8", False),
        ("Cython>=3.2.1", "3.2.0", False),
        ("Cython==3.2.*", "3.2.8", True),
        ("Cython!=3.2.8", "3.2.8", False),
        ("Cython~=3.2.0", "3.3.0", False),
        ("Cython==3.3.0a1", "3.3.0a1", True),
        ("Cython", "0.29.37", True),
    ],
)
def test_installed_requirement_obeys_complete_specifier(raw, version, admitted):
    resolved = requirements.realized_build_requirement(
        raw, Requirement(raw), {"name": "cython", "version": version}
    )
    assert (resolved is not None) is admitted


@pytest.mark.parametrize(
    "minor,os_name,machine",
    [(12, "nt", "AMD64"), (13, "posix", "aarch64"), (14, "posix", "x86_64")],
)
def test_requirement_markers_select_all_applicable_constraints(minor, os_name, machine):
    markers = {
        **canonical_source_marker_environment(),
        "python_version": f"3.{minor}",
        "os_name": os_name,
        "platform_machine": machine,
    }
    raw = [
        'Cython<3.2; python_version < "3.13"',
        'Cython>=3.2; python_version >= "3.13"',
        f'Cython!=3.2.7; os_name == "{os_name}" and platform_machine == "{machine}"',
    ]
    active = active_source_build_requirements(raw, markers)
    assert [entry for entry, _ in active] == [raw[0 if minor == 12 else 1], raw[2]]


def _locked_inventory(*, extra: bool = False):
    payload = build_environment_manifest(["cython==3.2.8"])
    if extra:
        closure = payload["custody"]["lock_closure"]
        closure["requirements"] = ["cython[fast-parser]==3.2.8"]
        closure["packages"][0]["extras"] = ["fast-parser"]
        closure["closure_sha256"] = canonical_json_sha256(
            {key: value for key, value in closure.items() if key != "closure_sha256"}
        )
    return SourceBuildInventory(payload["custody"], Path(sys.prefix))


@pytest.mark.parametrize("extra", [False, True])
def test_locked_requirement_uses_existing_extra_activation(extra):
    inventory = _locked_inventory(extra=extra)
    raw = "Cython[Fast_Parser]>=3.2"
    assert (inventory.requirement(raw, Requirement(raw)) is not None) is extra


def test_publication_rejects_upstream_extra_not_activated_in_lock():
    payload = build_environment_manifest(["cython==3.2.8"])
    raw = "Cython[fast-parser]==3.2.8"
    payload["requirements"] = [raw]
    payload["active_requirements"] = [raw]
    payload["resolved"][0]["requirement"] = raw
    problems = source_build_environment_problems(payload)
    assert any("extras does not satisfy" in problem for problem in problems)


@pytest.mark.parametrize(
    "raw,expected",
    [
        (["Cython>=3", "Cython<3.2"], False),
        (["Cython>=3", 'Cython<3.2; python_version < "3"'], True),
        (["Cython[fast-parser]>=3"], False),
    ],
)
def test_cython_locked_admission_checks_all_constraints_without_process(
    raw, expected, monkeypatch
):
    monkeypatch.setattr(
        cython.process_guard,
        "run_completed_command",
        lambda *a, **kw: pytest.fail("locked admission must not probe or install"),
    )
    version, error = cython.require_cython(
        pyproject={"build-system": {"requires": raw}}, inventory=_locked_inventory()
    )
    assert (version == "3.2.8" and error is None) is expected
    if not expected:
        assert error is not None and "No packages were installed" in error


def _distribution(root: Path, name: str, version: str, *fields: str) -> None:
    directory = root / f"{name}-{version}.dist-info"
    directory.mkdir()
    (directory / "METADATA").write_text(
        "\n".join(
            (
                "Metadata-Version: 2.3",
                f"Name: {name}",
                f"Version: {version}",
                *fields,
                "",
            )
        ),
        encoding="utf-8",
    )


def test_direct_extra_closure_checks_transitive_constraints_and_cycles(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(requirements.sysconfig, "get_path", lambda _name: str(tmp_path))
    _distribution(
        tmp_path,
        "Cython",
        "3.2.8",
        "Provides-Extra: fast",
        'Requires-Dist: parser[speed]>=2; extra == "fast"',
    )
    _distribution(
        tmp_path,
        "parser",
        "2.1",
        "Provides-Extra: speed",
        'Requires-Dist: Cython>=3; extra == "speed"',
        'Requires-Dist: absent; python_version < "3"',
    )
    raw = "Cython[fast]>=3.2"
    active = ((raw, Requirement(raw)),)
    assert (
        requirements.current_build_requirements(
            active, canonical_source_marker_environment()
        )[0].version
        == "3.2.8"
    )
    metadata_path = tmp_path / "parser-2.1.dist-info/METADATA"
    metadata_path.write_text(
        metadata_path.read_text() + 'Requires-Dist: missing>=1; extra == "speed"\n'
    )
    with pytest.raises(ValueError, match="missing>=1"):
        requirements.current_build_requirements(
            active, canonical_source_marker_environment()
        )


@pytest.mark.parametrize(
    "fault",
    [
        "absent",
        "invalid-version",
        "requires-python",
        "duplicate",
    ],
)
def test_direct_cython_failure_never_installs(tmp_path, monkeypatch, fault):
    monkeypatch.setattr(requirements.sysconfig, "get_path", lambda _name: str(tmp_path))
    if fault != "absent":
        _distribution(
            tmp_path,
            "Cython",
            "invalid" if fault == "invalid-version" else "3.2.8",
            *(["Requires-Python: <3"] if fault == "requires-python" else []),
        )
    if fault == "duplicate":
        _distribution(tmp_path, "cython", "3.1.0")
    monkeypatch.setattr(
        cython.process_guard,
        "run_completed_command",
        lambda *a, **kw: pytest.fail("invalid requirements must fail before execution"),
    )
    version, error = cython.require_cython(pyproject={})
    assert (
        version is None and error is not None and "No packages were installed" in error
    )


@pytest.mark.parametrize("fault", ["none", "probe", "version", "origin", "mutation"])
def test_locked_cython_execution_binds_distribution_and_fences_generation(
    tmp_path, monkeypatch, fault
):
    inventory = _locked_inventory()
    module = tmp_path / "cython.py"
    compiler = tmp_path / "Compiler.py"
    module.write_bytes(b"module")
    compiler.write_bytes(b"compiler")
    files = tuple(
        VerifiedTreeFile(path, stable_regular_file_identity(path, label="test"))
        for path in (module, compiler)
    )
    monkeypatch.setattr(inventory, "files", lambda **kw: files)
    monkeypatch.setattr(
        inventory,
        "python_identity",
        lambda **kw: stable_regular_file_identity(
            Path(sys.executable).resolve(), label="test interpreter"
        ),
    )

    def probe(argv, **kw):
        assert argv[:3] == [sys.executable, "-B", "-I"]
        assert argv[3:6] == [
            "-X",
            f"pycache_prefix={tmp_path / '.molt-cython-no-bytecode'}",
            "-c",
        ]
        return subprocess.CompletedProcess(
            argv,
            1 if fault == "probe" else 0,
            json.dumps(
                {
                    "version": "0" if fault == "version" else "3.2.8",
                    "origins": {
                        "Cython": str(module),
                        "cython": str(
                            tmp_path / "unowned.py" if fault == "origin" else module
                        ),
                    },
                }
            ),
            "unowned Cython import" if fault == "probe" else "",
        )

    monkeypatch.setattr(cython.process_guard, "run_completed_command", probe)

    def generate():
        with cython.cython_execution(
            pyproject={}, inventory=inventory, working_directory=tmp_path
        ) as tool:
            assert tool.version == "3.2.8"
            if fault == "mutation":
                compiler.write_bytes(b"mutated!")

    if fault == "none":
        generate()
    else:
        with pytest.raises(
            ValueError,
            match={
                "probe": "unowned Cython",
                "version": "distribution version",
                "origin": "locked distribution files",
                "mutation": "changed",
            }[fault],
        ):
            generate()


def test_cython_execution_ignores_existing_bytecode_without_modifying_it(tmp_path):
    source = tmp_path / "cached_generator.py"
    source.write_bytes(b"value = 'bad!'\n")
    stamp = source.stat()
    cache = Path(py_compile.compile(str(source), doraise=True))
    poisoned = cache.read_bytes()
    source.write_bytes(b"value = 'good'\n")
    os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    tool = cython.CythonTool("test", sys.executable, tmp_path / "absent-cache")
    result = tool.run(
        [
            "-c",
            f"import sys; sys.path.insert(0, {str(tmp_path)!r}); import cached_generator; print(cached_generator.value)",
        ],
        capture_output=True,
        text=True,
        timeout=30,
        check=True,
    )
    assert result.stdout.strip() == "good"
    assert cache.read_bytes() == poisoned and not tool.bytecode_prefix.exists()
    tool.bytecode_prefix.mkdir()
    with pytest.raises(ValueError, match="must remain absent"):
        tool.run(["-c", "raise AssertionError('must not run')"], timeout=30)
