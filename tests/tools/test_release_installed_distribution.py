"""Release projections of the installed distribution: pip wheel, Rust-absent
consumer environment, pip consumer admission and runtime-cell provenance."""

from __future__ import annotations

# Shared pytest fixtures are imported by name and requested as parameters.
# ruff: noqa: F401, F811

import base64
import copy
import csv
from dataclasses import replace
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import stat
import zipfile

import pytest

from molt.toolchain_identity import find_executable
from tools.release import build_bundle, release_authority, release_model
from tools.release.archive import ArchivePolicy
from tools.release.binary_compatibility import wheel_platform_tag_matches
from tools.release.platform_wheel import write_platform_wheel
from tools.release.verify_consumer import rust_toolchain_absent_environment
from tests.tools.test_release_supply_chain import (
    _runtime_cells,
    consumer_transport_boundary,
    release_evidence_inputs,
    release_inputs,
    release_source,
    release_transport_files,
)

_TAG = "manylinux_2_39_x86_64"


@pytest.mark.parametrize("outcome", ["success", "failed-admission", "rival"])
def test_runtime_inventory_publication_is_one_exclusive_commit(
    tmp_path, monkeypatch, outcome
):
    from tools.release import runtime_cells
    from molt import verified_subset

    for name in runtime_cells.POLICY_OVERRIDE_ENV:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(
        verified_subset, "current_host_coordinate", lambda: ("linux", "x86_64")
    )
    output = tmp_path / "runtime"
    staged = []
    inventory = {"cells": ["fixture-generation"]}

    def populate(_repo, stage, **_kwargs):
        assert stage.is_absolute() and stage != output
        assert not output.exists()
        staged.append(stage)
        stage.mkdir()
        (stage / "member").write_bytes(b"complete runtime member")
        if outcome == "failed-admission":
            raise ValueError("runtime source identity mismatch")
        (stage / runtime_cells.INVENTORY_NAME).write_text(
            json.dumps(inventory), encoding="utf-8"
        )
        if outcome == "rival":
            output.mkdir()
            (output / "winner").write_bytes(b"preserve concurrent generation")
        return inventory

    # Runtime compilation and receipt admission are outside this transaction
    # test. The filesystem publication and competing destination are real.
    monkeypatch.setattr(runtime_cells, "_populate_runtime_cells", populate)

    def produce():
        return runtime_cells.produce_runtime_cells(
            tmp_path, output, source_sha="a" * 40, platform="linux", arch="x86_64"
        )

    if outcome == "failed-admission":
        with pytest.raises(ValueError, match="runtime source identity mismatch"):
            produce()
        assert not output.exists()
    elif outcome == "rival":
        with pytest.raises(FileExistsError):
            produce()
        assert list(output.iterdir()) == [output / "winner"]
        assert (output / "winner").read_bytes() == b"preserve concurrent generation"
    else:
        assert produce() == inventory
        assert (output / "member").read_bytes() == b"complete runtime member"
        assert (
            json.loads(
                (output / runtime_cells.INVENTORY_NAME).read_text(encoding="utf-8")
            )
            == inventory
        )
    assert not staged[0].parent.exists()


def test_runtime_cell_staging_never_removes_a_sibling_producers_tree(tmp_path):
    from tools.release import runtime_cells

    output = tmp_path / "runtime"
    output.mkdir()
    rival = output / ".staging"
    rival.mkdir()
    (rival / "owned").write_bytes(b"another producer")
    member = tmp_path / "member"
    member.write_bytes(b"runtime input")
    cell = runtime_cells._publish_cell(
        output, kind="fixture", key={}, members=[(member, "member", "fixture")]
    )
    assert (output / cell["id"] / "member").read_bytes() == b"runtime input"
    assert (rival / "owned").read_bytes() == b"another producer"
    assert sorted(path.name for path in output.iterdir()) == sorted(
        [".staging", cell["id"]]
    )


def _pure_wheel(path: Path) -> Path:
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("molt/__init__.py", "")
        archive.writestr("molt/cli/__init__.py", "def main():\n    pass\n")
        archive.writestr(
            "molt-0.0.1.dist-info/METADATA",
            "Metadata-Version: 2.4\nName: molt\nVersion: 0.0.1\n",
        )
        archive.writestr(
            "molt-0.0.1.dist-info/entry_points.txt",
            "[console_scripts]\nmolt = molt.cli:main\n",
        )
        archive.writestr("molt-0.0.1.dist-info/WHEEL", "Root-Is-Purelib: true\n")
        archive.writestr("molt-0.0.1.dist-info/RECORD", "")
    return path


def _bundle_tree(root: Path) -> Path:
    for relative, data in {
        "bin/molt-backend": b"compiler",
        "bin/molt": b"launcher",
        "runtime/cell/libmolt_runtime.stdlib_micro.a": b"runtime",
        "source/tools/script.py": b"#!/usr/bin/env python3\n",
        "source/src/molt/__init__.py": b"",
        "source/release-compiler-source.json": b"{}",
    }.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    return root


def _mode(root: Path):
    modes = {"tools/script.py": 0o755}

    def resolve(relative: PurePosixPath) -> int:
        if relative.parts[0] == "source":
            return modes.get(relative.as_posix().removeprefix("source/"), 0o644)
        path = root / relative
        return 0o755 if path.is_dir() or path.parent.name == "bin" else 0o644

    return resolve


def _write(tmp_path: Path, name: str, **overrides) -> Path:
    bundle = _bundle_tree(tmp_path / "bundle")
    arguments = dict(
        platform="linux",
        arch="x86_64",
        platform_tag=_TAG,
        source_date_epoch=1_700_000_000,
        bundle_mode=_mode(bundle),
        policy=ArchivePolicy(),
    )
    arguments.update(overrides)
    return write_platform_wheel(
        bundle,
        _pure_wheel(tmp_path / "molt-0.0.1-py3-none-any.whl"),
        tmp_path / name,
        **arguments,
    )


def test_platform_wheel_carries_the_bundle_for_pip(tmp_path: Path) -> None:
    first = _write(tmp_path, "first")
    second = _write(tmp_path, "second")
    assert first.name == f"molt-0.0.1-py3-none-{_TAG}.whl"
    assert first.read_bytes() == second.read_bytes()
    data = "molt-0.0.1.data/data/share/molt/distribution/"
    with zipfile.ZipFile(first) as archive:
        files = {
            info.filename: info for info in archive.infolist() if not info.is_dir()
        }
        wheel = archive.read("molt-0.0.1.dist-info/WHEEL").decode()
        assert (
            "Root-Is-Purelib: false\n" in wheel and f"Tag: py3-none-{_TAG}\n" in wheel
        )
        assert archive.read("molt/cli/__init__.py") == b"def main():\n    pass\n"
        for member in (
            "bin/molt-backend",
            "runtime/cell/libmolt_runtime.stdlib_micro.a",
            "source/release-compiler-source.json",
        ):
            assert data + member in files
        # pip keeps only the executable bit: bundle and Git modes survive install.
        assert files[data + "bin/molt-backend"].external_attr >> 16 & 0o111
        assert files[data + "source/tools/script.py"].external_attr >> 16 & 0o111
        assert (
            not files[data + "source/src/molt/__init__.py"].external_attr >> 16 & 0o111
        )
        rows = list(
            csv.reader(
                io.StringIO(archive.read("molt-0.0.1.dist-info/RECORD").decode())
            )
        )
        recorded = {row[0]: row[1:] for row in rows}
        assert set(recorded) == set(files)
        for name, (digest, size) in recorded.items():
            if name.endswith("/RECORD"):
                assert (digest, size) == ("", "")
                continue
            payload = archive.read(name)
            expected = base64.urlsafe_b64encode(hashlib.sha256(payload).digest())
            assert digest == "sha256=" + expected.rstrip(b"=").decode()
            assert int(size) == len(payload)


@pytest.mark.parametrize(
    "overrides",
    [
        {"platform_tag": "manylinux_2_39_aarch64"},
        {"platform_tag": "macosx_15_0_x86_64"},
        {"platform": "windows", "platform_tag": _TAG},
    ],
)
def test_platform_wheel_rejects_a_tag_for_another_coordinate(tmp_path, overrides):
    with pytest.raises(ValueError, match="does not name"):
        _write(tmp_path, "out", **overrides)


@pytest.mark.parametrize(
    ("platform", "arch", "tag", "expected"),
    [
        ("windows", "x86_64", "win_amd64", True),
        ("windows", "arm64", "win_amd64", False),
        ("linux", "aarch64", "manylinux_2_39_aarch64", True),
        ("linux", "x86_64", "linux_x86_64", False),
        ("macos", "arm64", "macosx_15_0_arm64", True),
        ("macos", "arm64", "macosx_15_0_universal2", False),
    ],
)
def test_wheel_tag_authority(platform, arch, tag, expected):
    assert wheel_platform_tag_matches(platform, arch, tag) is expected


def _executable(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def _tool(directory: Path, name: str) -> Path:
    return _executable(directory / (name + (".exe" if os.name == "nt" else "")))


def _base_env(*directories: Path) -> dict[str, str]:
    env = {"PATH": os.pathsep.join(map(str, directories))}
    if os.name == "nt":
        env.update(PATHEXT=".EXE", NoDefaultCurrentDirectoryInExePath="1")
        env["SYSTEMROOT"] = os.environ.get("SYSTEMROOT", r"C:\Windows")
    return env


def test_consumer_environment_makes_rust_unavailable(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    rust = tmp_path / "rust-bin"
    tools = tmp_path / "tools-bin"
    for name in ("cargo", "rustc", "rustup"):
        _tool(rust, name)
    _tool(tools, "uv")
    env = _base_env(rust, tools)
    env.update(CARGO=str(rust / "cargo"), RUSTUP_TOOLCHAIN="stable", RUSTC_WRAPPER="x")
    result = rust_toolchain_absent_environment(env, root=tmp_path / "consumer")
    for name in ("cargo", "rustc", "rustup"):
        assert find_executable(name, environment=result) is None
    assert find_executable("uv", environment=result) is not None
    assert not {"CARGO", "RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER"} & set(result)
    for variable in ("CARGO_HOME", "RUSTUP_HOME"):
        home = Path(result[variable])
        assert home.is_dir() and not any(home.iterdir())
        assert home.is_relative_to(tmp_path / "consumer")


def test_consumer_environment_fails_closed_when_rust_shares_uv_directory(
    tmp_path, monkeypatch
):
    monkeypatch.chdir(tmp_path)
    shared = tmp_path / "shared-bin"
    _tool(shared, "cargo")
    _tool(shared, "uv")
    with pytest.raises(RuntimeError, match="shares a PATH directory with uv"):
        rust_toolchain_absent_environment(_base_env(shared), root=tmp_path / "consumer")


def _admit(release_inputs, mutate):
    candidate_dir = release_inputs["candidate_root"] / "linux-x86_64"
    candidate = release_authority._load_candidate(candidate_dir / "candidate.json")
    receipt = candidate_dir / "consumer-verification.json"
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    mutate(payload["pip_proof"])
    release_model.write_json(receipt, payload)
    return release_authority._admit_candidate(
        candidate,
        candidate_dir,
        version=release_inputs["version"],
        source_sha=release_inputs["source_sha"],
        source_date_epoch=release_inputs["source_date_epoch"],
        wheel_record=candidate["wheel"],
        supervisor=Path("/fixture-supervisor"),
    )


def _set(path, value):
    def mutate(proof):
        target = proof
        for key in path[:-1]:
            target = target[key]
        target[path[-1]] = value

    return mutate


@pytest.mark.parametrize(
    "mutate",
    [
        _set(("wheel", "sha256"), "f" * 64),
        _set(("compiler_sha256",), "f" * 64),
        _set(("commands", 2, "argv", 0), "/consumer/bundle/molt-0.0.001/bin/molt"),
        _set(("commands", 2, "argv", 5), "dev"),
        _set(("commands", 3, "stdout_sha256"), "0" * 64),
        _set(
            ("commands", 1, "argv", 6),
            "/consumer/candidate/molt-0.0.1-py3-none-any.whl",
        ),
        lambda proof: proof["commands"].pop(),
        _set(("artifact", "path"), "/elsewhere/release_consumer"),
    ],
)
def test_pip_consumer_admission_binds_one_plain_pip_install(release_inputs, mutate):
    assert _admit(release_inputs, lambda proof: None)
    with pytest.raises(ValueError, match="release consumer"):
        _admit(release_inputs, mutate)


@pytest.mark.parametrize(
    "mutate",
    [
        *[_set(("commands", index, "argv"), []) for index in range(5)],
        _set(("commands", 0, "duration_seconds"), True),
        _set(("commands", 2, "duration_seconds"), -1),
        _set(("commands", 3, "duration_seconds"), "0.125"),
        _set(("commands", 2, "argv", -1), "/consumer/source\x00.py"),
    ],
)
def test_pip_command_records_share_typed_admission(release_inputs, mutate):
    with pytest.raises(ValueError, match="release consumer"):
        _admit(release_inputs, mutate)


def test_bundle_rejects_runtime_cells_from_another_source(tmp_path, release_source):
    other = replace(release_source, source_sha="d" * 40)
    cells = _runtime_cells(
        tmp_path / "cells", platform="linux", arch="x86_64", snapshot=other
    )
    worker = tmp_path / "molt-worker"
    worker.write_bytes(b"worker")
    with pytest.raises(ValueError, match="not produced from the bundle source"):
        build_bundle.build_bundle(
            version="0.0.001",
            platform="linux",
            worker=None,
            kind="molt",
            output=tmp_path / "bundle.tar.gz",
            source_date_epoch=1_700_000_000,
            arch="x86_64",
            compiler=worker,
            launcher=worker,
            snapshot=release_source,
            runtime_cells=cells,
        )


def test_release_manifest_requires_one_platform_wheel_per_target(
    release_inputs, tmp_path
):
    manifest = release_authority.assemble_index(
        **release_inputs, output=tmp_path / "out"
    )
    wheels = [a for a in manifest["artifacts"] if a["name"] == "molt-wheel"]
    targets = {(t.platform, t.arch) for t in release_model.release_targets()}
    assert {(a["platform"], a["arch"]) for a in wheels} == targets | {("any", "any")}
    payload = copy.deepcopy(manifest)
    payload["artifacts"] = [
        a
        for a in payload["artifacts"]
        if not (a["name"] == "molt-wheel" and a["platform"] == "linux")
    ]
    with pytest.raises(ValueError):
        release_model.validate_release_manifest(payload)
