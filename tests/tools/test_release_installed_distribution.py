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
from molt.process_environment import NameRule, ProcessEnvironment
from tools.release import verify_consumer
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
from tests.tools.test_release_native_build import llvm_input_fixture

_TAG = "manylinux_2_39_x86_64"


@pytest.mark.parametrize("component", [False, True])
@pytest.mark.parametrize("outcome", ["success", "failed-admission", "rival"])
def test_runtime_inventory_publication_is_one_exclusive_commit(
    tmp_path, monkeypatch, outcome, component
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
    requests = (runtime_cells.declared_runtime_cells()[0],) if component else None

    def populate(_repo, stage, **_kwargs):
        assert _kwargs["requests"] == requests
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
            tmp_path,
            output,
            source_sha="a" * 40,
            platform="linux",
            arch="x86_64",
            requests=requests,
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


@pytest.mark.parametrize(
    "selection",
    [
        "all",
        "one",
        "empty",
        "duplicate",
        "unknown",
        "field-type",
        "snapshot-policy",
        "wrong-output",
        "producer-failure",
    ],
)
def test_runtime_component_selection_uses_pinned_policy_before_production(
    tmp_path, monkeypatch, selection
):
    """Exercise the real selection/production/publication flow, without Cargo.

    Artifact receipt validation is a separate boundary; this test observes
    exactly which producer requests and source checks the transaction executes.
    """
    from types import SimpleNamespace
    from molt import compiler_distribution, verified_subset
    from molt.cli import installed_runtime, runtime_build_identity
    from tools.release import compiler_payload, git_source_snapshot, runtime_cells

    for name in runtime_cells.POLICY_OVERRIDE_ENV:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(
        verified_subset, "current_host_coordinate", lambda: ("linux", "x86_64")
    )
    declared = runtime_cells.declared_runtime_cells()
    assert len(declared) == 70
    chosen = next(
        row
        for row in declared
        if row.kind == compiler_distribution.NATIVE_RUNTIME_CELL
        and row.runtime_profile == "dev-fast"
        and row.stdlib_profile == "full"
        and not row.extra_runtime_features
    )
    requests = {
        "all": None,
        "one": (chosen,),
        "empty": (),
        "duplicate": (chosen, chosen),
        "unknown": (replace(chosen, freestanding=True),),
        "field-type": (replace(chosen, freestanding=0),),
        "snapshot-policy": (chosen,),
        "wrong-output": (chosen,),
        "producer-failure": (chosen,),
    }[selection]
    for name, value in {
        "MOLT_USE_SCCACHE": "1",
        "RUSTC_WRAPPER": "sccache",
        "RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
        "CARGO_BUILD_RUSTC_WRAPPER": "sccache",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
        "CARGO_INCREMENTAL": "1",
        "SOURCE_DATE_EPOCH": "17",
        "LC_ALL": "ambient-locale",
        "TZ": "Pacific/Honolulu",
    }.items():
        monkeypatch.setenv(name, value)
    pinned = tmp_path / "pinned-source"
    pinned.mkdir()
    snapshot = SimpleNamespace(
        files=(), object_format="sha1", source_sha="c" * 40, tree_sha="b" * 40
    )
    monkeypatch.setattr(compiler_payload, "source_snapshot", lambda *_: snapshot)
    epoch_queries = []

    def epoch(repo, commit):
        epoch_queries.append((repo, commit))
        return 1_700_000_000

    monkeypatch.setattr(compiler_payload, "source_commit_epoch", epoch)
    monkeypatch.setattr(
        git_source_snapshot, "materialize_git_source_snapshot", lambda *_a, **_k: pinned
    )
    policy_reads = []

    def policy(root):
        policy_reads.append(root)
        assert root == pinned
        return (
            tuple(row for row in declared if row != chosen)
            if selection == "snapshot-policy"
            else declared
        )

    monkeypatch.setattr(runtime_cells, "declared_runtime_cells", policy)
    produced = []
    environment_before = dict(os.environ)
    cwd_before = Path.cwd()

    def producer(request, source_root, output, timeout):
        assert source_root == pinned and timeout == 17
        assert os.environ["CARGO_INCREMENTAL"] == "0"
        assert os.environ["SOURCE_DATE_EPOCH"] == "1700000000"
        assert os.environ["LC_ALL"] == "C"
        assert os.environ["TZ"] == "UTC"
        assert os.environ["MOLT_USE_SCCACHE"] == "0"
        assert all(
            os.environ[name] == ""
            for name in (
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "CARGO_BUILD_RUSTC_WRAPPER",
                "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            )
        )
        # The common Cargo plan owns remapping after flag selection; the
        # release wrapper must not overwrite Cargo's ambient selection first.
        assert os.environ.get("CARGO_ENCODED_RUSTFLAGS") == environment_before.get(
            "CARGO_ENCODED_RUSTFLAGS"
        )
        assert os.environ.get("RUSTFLAGS") == environment_before.get("RUSTFLAGS")
        assert Path(os.environ["CARGO_TARGET_DIR"]) == Path.cwd() / "target"
        assert Path(os.environ["MOLT_BUILD_STATE_DIR"]) == Path.cwd() / "build-state"
        produced.append(request)
        if selection == "producer-failure":
            raise ValueError("injected producer failure")
        actual = declared[0] if selection == "wrong-output" else request
        return {
            "id": str(len(produced)),
            "kind": actual.kind,
            "key": runtime_cells.runtime_cell_key(actual),
        }

    monkeypatch.setattr(runtime_cells, "_produce_native", producer)
    monkeypatch.setattr(runtime_cells, "_produce_wasm", producer)
    monkeypatch.setattr(
        installed_runtime,
        "admit_runtime_cell_receipts",
        lambda cell, _path: [cell["id"]],
    )
    verified = []
    monkeypatch.setattr(
        runtime_build_identity,
        "verify_runtime_source_identities",
        lambda root, identities: verified.append((root, identities)),
    )
    monkeypatch.setattr(
        compiler_distribution,
        "verify_source_inventory",
        lambda root, records: verified.append((root, records)),
    )
    monkeypatch.setattr(
        compiler_distribution,
        "validate_runtime_inventory",
        lambda value, **_kwargs: value,
    )
    monkeypatch.setattr(compiler_distribution, "verify_runtime_tree", lambda *_: None)
    output = tmp_path / "runtime"

    def run():
        return runtime_cells.produce_runtime_cells(
            tmp_path,
            output,
            source_sha="a" * 40,
            platform="linux",
            arch="x86_64",
            cargo_timeout=17,
            requests=requests,
        )

    errors = {
        "empty": "nonempty tuple",
        "duplicate": "duplicate component",
        "unknown": "outside the pinned release policy",
        "field-type": "invalid field types",
        "snapshot-policy": "outside the pinned release policy",
        "wrong-output": "produced runtime cells differ",
        "producer-failure": "injected producer failure",
    }
    if selection in errors:
        with pytest.raises(ValueError, match=errors[selection]):
            run()
        assert not output.exists()
        assert produced == (
            [chosen] if selection in {"wrong-output", "producer-failure"} else []
        )
    else:
        inventory = run()
        assert produced == (list(declared) if selection == "all" else [chosen])
        assert len(inventory["cells"]) == (70 if selection == "all" else 1)
        assert verified == [
            (pinned, [str(i) for i in range(1, len(produced) + 1)]),
            (pinned, []),
        ]
        assert (
            json.loads(
                (output / runtime_cells.INVENTORY_NAME).read_text(encoding="utf-8")
            )
            == inventory
        )
    assert policy_reads == [pinned]
    assert epoch_queries == [(tmp_path, snapshot.source_sha)]
    assert dict(os.environ) == environment_before
    assert Path.cwd() == cwd_before
    assert not list(tmp_path.glob(".runtime-cells-*"))


@pytest.mark.parametrize(
    "mode",
    [
        "default",
        "component",
        "unknown-field",
        "missing-field",
        "empty",
        "duplicate-key",
    ],
)
def test_runtime_component_cli_reads_exact_request_records(tmp_path, monkeypatch, mode):
    from dataclasses import asdict
    from tools.release import runtime_cells

    for name in runtime_cells.POLICY_OVERRIDE_ENV:
        monkeypatch.delenv(name, raising=False)
    request = runtime_cells.declared_runtime_cells()[0]
    record = asdict(request)
    if mode == "unknown-field":
        record["extra"] = True
    elif mode == "missing-field":
        del record["freestanding"]
    path = tmp_path / "requests.json"
    content = json.dumps([] if mode == "empty" else [record])
    if mode == "duplicate-key":
        content = content.replace('"kind":', '"kind": "discarded", "kind":')
    path.write_text(content, encoding="utf-8")
    calls = []
    monkeypatch.setattr(
        runtime_cells,
        "produce_runtime_cells",
        lambda *_a, **kwargs: calls.append(kwargs) or {"cells": []},
    )
    args = [
        "--output",
        str(tmp_path / "runtime"),
        "--source-sha",
        "a" * 40,
        "--platform",
        "linux",
        "--arch",
        "x86_64",
    ]
    if mode != "default":
        args += ["--component-requests", str(path)]
    if mode in {"default", "component"}:
        assert runtime_cells.main(args) == 0
        assert calls[0]["requests"] == (None if mode == "default" else (request,))
    else:
        with pytest.raises(ValueError):
            runtime_cells.main(args)
        assert calls == []


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
        archive.writestr("molt/cli/__main__.py", "def run():\n    pass\n")
        archive.writestr(
            "molt-0.0.1.dist-info/METADATA",
            "Metadata-Version: 2.4\nName: molt\nVersion: 0.0.1\n",
        )
        archive.writestr(
            "molt-0.0.1.dist-info/entry_points.txt",
            "[console_scripts]\nmolt = molt.cli.__main__:run\n",
        )
        archive.writestr("molt-0.0.1.dist-info/WHEEL", "Root-Is-Purelib: true\n")
        archive.writestr("molt-0.0.1.dist-info/RECORD", "")
    return path


def _bundle_tree(root: Path) -> Path:
    for relative, data in {
        "bin/molt-backend": b"compiler",
        "bin/molt": b"launcher",
        "bin/molt-wasm-facts": b"facts",
        "runtime/cell/libmolt_runtime.stdlib_micro.a": b"runtime",
        "source/tools/script.py": b"#!/usr/bin/env python3\n",
        "source/src/molt/__init__.py": b"",
        "source/release-compiler-source.json": b"{}",
        "source/data/comma,name.txt": b"legal portable wheel member name",
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
            "bin/molt-wasm-facts",
            "runtime/cell/libmolt_runtime.stdlib_micro.a",
            "source/release-compiler-source.json",
        ):
            assert data + member in files
        # pip keeps only the executable bit: bundle and Git modes survive install.
        assert files[data + "bin/molt-backend"].external_attr >> 16 & 0o111
        assert archive.read(data + "bin/molt-wasm-facts") == b"facts"
        assert files[data + "bin/molt-wasm-facts"].external_attr >> 16 & 0o111
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
        assert len(rows) == len(recorded)
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


@pytest.mark.parametrize("rule", list(NameRule))
def test_rust_absent_environment_uses_child_name_rule(tmp_path, monkeypatch, rule):
    rust, tools = tmp_path / "rust", tmp_path / "tools"
    original = ProcessEnvironment(
        {
            "Path": "unrelated POSIX variable",
            "PATH": os.pathsep.join(map(str, (rust, tools))),
            "cargo": "ambient selector",
            "Rustup_Home": "ambient home",
        },
        rule=rule,
    )

    def candidates(command, *, environment):
        directories = environment.get("PATH", "").split(os.pathsep)
        directory = tools if command == "uv" else rust
        return (
            iter((directory / command,)) if str(directory) in directories else iter(())
        )

    monkeypatch.setattr(verify_consumer, "executable_candidates", candidates)
    result = rust_toolchain_absent_environment(original, root=tmp_path / "consumer")
    assert result.rule is rule
    assert result["Path"] == (
        str(tools) if rule is NameRule.WINDOWS else "unrelated POSIX variable"
    )
    if rule is NameRule.WINDOWS:
        assert "PATH" not in dict(result) and "cargo" not in result
        assert result["Rustup_Home"] == str(
            tmp_path / "consumer/rust-toolchain-absent/rustup"
        )
    else:
        assert result["PATH"] == str(tools)
        assert result["cargo"] == "ambient selector"
        assert result["Rustup_Home"] == "ambient home"
    assert original["PATH"] == os.pathsep.join(map(str, (rust, tools)))


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
    "mutate, expected_error",
    [
        (_set(("wheel", "sha256"), "f" * 64), "release consumer"),
        (_set(("compiler_sha256",), "f" * 64), "release consumer"),
        (_set(("wasm_facts_sha256",), "f" * 64), "release consumer"),
        (
            _set(("commands", 2, "argv", 0), "/consumer/bundle/molt-0.0.001/bin/molt"),
            "release consumer",
        ),
        (_set(("commands", 2, "argv", 5), "dev"), "release consumer"),
        pytest.param(
            _set(("commands", 3, "stdout_sha256"), "0" * 64),
            "^consumer archive differs from admitted candidate/proof$",
            id="raw-command-stdout",
        ),
        (
            _set(
                ("commands", 1, "argv", 6),
                "/consumer/candidate/molt-0.0.1-py3-none-any.whl",
            ),
            "release consumer",
        ),
        (lambda proof: proof["commands"].pop(), "release consumer"),
        (
            _set(("cells", 0, "artifact", "path"), "/elsewhere/release_consumer"),
            "release consumer",
        ),
    ],
)
def test_pip_consumer_admission_binds_one_plain_pip_install(
    release_inputs, mutate, expected_error
):
    assert _admit(release_inputs, lambda proof: None)
    with pytest.raises(ValueError, match=expected_error):
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


@pytest.mark.parametrize("mismatch", ["runtime-source", "gnu-x32"])
def test_bundle_rejects_runtime_cells_from_another_source(
    tmp_path, release_source, monkeypatch, mismatch
):
    other = (
        replace(release_source, source_sha="d" * 40)
        if mismatch == "runtime-source"
        else release_source
    )
    cells = _runtime_cells(
        tmp_path / "cells", platform="linux", arch="x86_64", snapshot=other
    )
    worker = tmp_path / "molt-worker"
    worker.write_bytes(b"worker")
    llvm_inputs = llvm_input_fixture(release_source, platform="linux", arch="x86_64")
    if mismatch == "gnu-x32":
        llvm_inputs["link_plan"]["host_target"] = "x86_64-unknown-linux-gnux32"
        llvm_inputs["link_plan"]["host_abi"].update(pointer_bits=32, header_bits=32)

    def unexpected_export(*_args, **_kwargs):
        pytest.fail("mismatched source or ABI must refuse before source export")

    monkeypatch.setattr(build_bundle, "materialize_sources", unexpected_export)
    match = (
        "not produced from the bundle source" if mismatch == "runtime-source" else "ABI"
    )
    with pytest.raises(ValueError, match=match):
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
            wasm_facts=worker,
            snapshot=release_source,
            runtime_cells=cells,
            llvm_inputs=llvm_inputs,
        )
    assert not (tmp_path / "bundle.tar.gz").exists()


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


@pytest.mark.parametrize("portable_bundle", [False, True])
@pytest.mark.parametrize(
    "defect",
    [None, "compile-failure", "missing-cell", "compiler-substitution", "wasm-stdout"],
)
def test_installed_guest_owner_uses_only_sealed_compilation_products(
    tmp_path, monkeypatch, portable_bundle, defect
):
    """Actual installed caller; tool/capture/namespace boundaries are explicit fakes.

    Preparatory products contain different bytes. Neither their timings nor
    their identities may become the installed replay products. No kernel or
    executable success is simulated as product admission by this test.
    """
    from types import SimpleNamespace
    from tools.release import consumer_replay
    from tests.tools.test_release_sealed_replay import _compilation_export_fixture
    from molt import toolchain_identity, wasm_optimizer_identity

    lanes = tuple(
        next(
            lane for lane in release_authority.consumer_lanes() if lane.target == target
        )
        for target in ("native", "wasm")
    )
    root, project = tmp_path / "installation", tmp_path / "project"
    root.mkdir()
    project.mkdir()
    source = project / "release_consumer.py"
    source.write_text("preparation", encoding="utf-8")
    compiler = SimpleNamespace(
        binary=root / "backend",
        source_root=root / "source",
        record={"sha256": "c" * 64},
        verify_binary=lambda *args: None,
    )
    preparations = {}
    for target in ("native", "wasm"):
        prepared = tmp_path / ("preflight-" + target)
        prepared.mkdir()
        _compilation_export_fixture(prepared, target)
        files = prepared / "export"
        observation_path = next(files.glob("*.molt-link-inputs.json"))
        outputs = {"input_reproducer": next(files.glob("*.tar"))}
        if target == "native":
            outputs.update(
                driver_preparation=files / "program.molt-native-inputs.json",
                driver_dependencies=files / "program.molt-native-inputs.d",
            )
        preparations[target] = (
            json.loads(observation_path.read_text(encoding="utf-8")),
            outputs,
        )
    monkeypatch.setattr(
        verify_consumer,
        "_preparation_link_inputs",
        lambda output, *, target: preparations[target],
    )
    monkeypatch.setattr(
        consumer_replay,
        "compilation_runtime_selection",
        lambda path, *, compiler, lane: (lane.target, {"tier": "selected"}),
    )
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda name, **kwargs: Path("/tools") / name,
    )
    monkeypatch.setattr(
        wasm_optimizer_identity, "find_wasm_opt", lambda **kwargs: "/tools/wasm-opt"
    )
    python = root / "python"
    frontend = root / "frontend"
    frontend_calls = []

    def frontend_selection(**kwargs):
        frontend_calls.append(kwargs)
        return frontend

    monkeypatch.setattr(
        verify_consumer, "_installed_frontend_python", frontend_selection
    )
    capture_calls = []

    def capture(**kwargs):
        capture_calls.append(kwargs)
        assert kwargs["interpreters"] == (python, frontend)
        return [{"captured": "frontend"}], {"captured": "tools"}

    monkeypatch.setattr(verify_consumer, "_compilation_captures", capture)
    plans = []

    def prepare(**kwargs):
        plans.append(kwargs)
        assert [lane for lane, _, _ in kwargs["cells"]] == list(lanes)
        assert set(kwargs["native_link_inputs"]) == {lanes[0].id}
        assert kwargs["python_executable"] == python
        return {"prepared": True}

    monkeypatch.setattr(consumer_replay, "prepare_compilation", prepare)
    evidence = (
        tmp_path
        / "compilation-evidence"
        / (("bundle-" if portable_bundle else "pip-") + "3.12")
    )

    def diagnostics(path, lane, digest="c" * 64):
        path.write_text(
            json.dumps(
                {
                    "compiler": {
                        "sha256": digest,
                        "path": str(compiler.binary),
                        "cargo_profile": lane.compiler_profile,
                        "fingerprint": "d" * 64,
                    },
                    "program": lane.as_record(),
                }
            ),
            encoding="utf-8",
        )

    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs["role"]))
        if kwargs["role"].startswith("prepare_"):
            lane = next(
                lane for lane in lanes if kwargs["role"] == "prepare_" + lane.id
            )
            folder = project / lane.id
            diagnostics(folder / "diagnostics.json", lane)
            (folder / "program").write_bytes(b"unadmitted preparatory product")
        return {
            "role": kwargs["role"],
            "argv": command,
            "returncode": 0,
            "duration_seconds": 0.1,
            "stdout_sha256": hashlib.sha256(
                release_authority.CONSUMER_EXPECTED_STDOUT.encode()
            ).hexdigest(),
            "stderr_sha256": hashlib.sha256(b"").hexdigest(),
        }

    monkeypatch.setattr(verify_consumer, "_run", run)
    monkeypatch.setattr(verify_consumer, "_audit_guest", lambda *args, **kwargs: None)

    def execute(**kwargs):
        assert kwargs["lanes"] == lanes and kwargs["compiler"] is compiler
        assert kwargs["compilation"] == {"prepared": True}
        destination = kwargs["evidence"]
        cold = destination.name.endswith("-cold")
        product = b"cold sealed product" if cold else b"actual sealed product"
        if defect == "compile-failure" and not cold:
            raise ValueError("actual compilation refused")
        rows = []
        for lane in lanes:
            folder = destination / "outputs" / lane.id
            folder.mkdir(parents=True)
            name = "program.wasm" if lane.target == "wasm" else "program"
            (folder / name).write_bytes(product)
            diagnostics(
                folder / "diagnostics.json",
                lane,
                "e" * 64
                if defect == "compiler-substitution" and not cold
                else "c" * 64,
            )
            stdout = (
                release_authority.CONSUMER_EXPECTED_STDOUT.encode()
                if lane.target == "wasm"
                else b""
            )
            (folder / "command.stdout").write_bytes(
                b"wrong"
                if defect == "wasm-stdout" and lane.target == "wasm" and not cold
                else stdout
            )
            (folder / "command.stderr").write_bytes(b"")
            (folder / "manifest.json").write_bytes(b"manifest")
            command = release_authority.consumer_guest_command(
                ["/installed/molt"],
                lane=lane,
                python_minor="3.12",
                diagnostics="/work/output/diagnostics.json",
                source="/app/source.py",
                output="/work/output/" + name,
                capture_link_inputs=True,
            )
            rows.append(
                {
                    "outputs": {
                        "artifact": {
                            "path": name,
                            "filename": name,
                            "size": len(product),
                            "sha256": hashlib.sha256(product).hexdigest(),
                        },
                        "command": {
                            "argv": command,
                            "returncode": 0,
                            "duration_sec": 0.75 if cold else 0.5,
                        },
                    }
                }
            )
        if defect == "missing-cell" and not cold:
            rows.pop()
        result = {"cells": rows}
        (destination / "compilation.json").write_text(
            json.dumps(result), encoding="utf-8"
        )
        return result

    monkeypatch.setattr(consumer_replay, "execute_compilation", execute)
    commands = []
    kwargs = dict(
        root=root,
        project=project,
        source=source,
        launcher=["/installed/molt"],
        env=ProcessEnvironment({"MOLT_HOME": str(root / "home")}),
        compiler=compiler,
        minor="3.12",
        target={"platform": "linux", "arch": "x86_64"},
        wheel_tag="fixture",
        evidence_dir=tmp_path / "audit",
        commands=commands,
        role_prefix="" if portable_bundle else "pip_",
        python=python,
        portable_bundle=portable_bundle,
        selected_lanes=lanes,
        compilation_evidence=evidence,
        archive_cache=tmp_path / "archives",
        supervisor=tmp_path / "supervisor",
        generation={},
        projection=verify_consumer.SealedExecutionProjection.compilation(
            memory_max_bytes=2**30, workspace_max_bytes=2**28, runtime_max_seconds=60
        ),
    )
    if defect:
        with pytest.raises(ValueError):
            verify_consumer._verify_guest_cells(**kwargs)
    else:
        cells, reference = verify_consumer._verify_guest_cells(**kwargs)
        assert len(cells) == 2 and reference["path"].endswith("/compilation.json")
        assert all(
            Path(cell["artifact"]["path"]).read_bytes() == b"actual sealed product"
            for cell in cells
        )
        assert len(commands) == 5
        assert all(not row["role"].startswith("prepare_") for row in commands)
        assert [row["duration_seconds"] for row in commands] == [
            0.75,
            0.75,
            0.5,
            0.1,
            0.5,
        ]
        prefix = "" if portable_bundle else "pip_"
        assert [row["role"] for row in commands[:2]] == [
            prefix + "cold_build_" + lanes[0].id,
            prefix + "cold_run_" + lanes[1].id,
        ]
        assert len(frontend_calls) == len(capture_calls) == 1
        cold_evidence = evidence.with_name(evidence.name + "-cold")
        assert [(plan["evidence"], plan["reuse_compilation"]) for plan in plans] == [
            (cold_evidence, None),
            (evidence, cold_evidence),
        ]
        assert all(
            (
                cold_evidence
                / "outputs"
                / lane.id
                / ("program.wasm" if lane.target == "wasm" else "program")
            ).read_bytes()
            == b"cold sealed product"
            for lane in lanes
        )
        assert frontend_calls[0]["portable_bundle"] is portable_bundle


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "candidate-runtime",
        "candidate-source",
        "product",
        "command",
        "diagnostics",
        "manifest",
        "reference",
        "missing-cell",
        "python-image",
        "cold-cache-seed",
        "warm-cache-seed",
        "warm-origin",
        "cold-receipt",
        "cold-command",
        "cold-raw-stdout",
        "wrong-cold-tier",
        "wrong-warm-tier",
        "invalid-tier",
    ],
)
def test_release_compilation_receiver_binds_candidate_product_and_raw_command(
    tmp_path, monkeypatch, defect
):
    """Release join uses real retained bytes; native replay is a named seam."""
    from types import SimpleNamespace
    from molt import compiler_distribution
    from molt.exact_json import canonical_json_sha256
    from tools.release import consumer_replay
    from tests.tools.test_release_sealed_replay import _compilation_export_fixture

    lane = next(
        lane for lane in release_authority.consumer_lanes() if lane.target == "wasm"
    )
    root = tmp_path / "compilation-evidence/bundle-3.12"
    retained = root / "outputs" / lane.id
    retained.mkdir(parents=True)
    record = {"path": "bin/backend", "sha256": "a" * 64}
    files = [{"path": "compiler-source", "size": 1, "sha256": "a" * 64}]
    installed = SimpleNamespace(
        source_sha="a" * 40,
        source_root=release_authority.ROOT,
        record=record,
        launcher={"path": "bin/molt"},
        wasm_facts={"path": "bin/facts"},
        runtime={"cells": ["runtime"]},
        files=files,
        verify_launcher=lambda: None,
        verify_wasm_facts=lambda: None,
    )
    monkeypatch.setattr(
        compiler_distribution, "installed_compiler", lambda path: installed
    )
    candidate = {
        "source_sha": "a" * 40,
        "compiler": record,
        "launcher": installed.launcher,
        "wasm_facts": installed.wasm_facts,
        "runtime": installed.runtime,
        "native_build": {"source": {"files_sha256": canonical_json_sha256(files)}},
    }
    command = {"argv": ["/installed/molt", "run"], "returncode": 0, "duration_sec": 0.2}
    (retained / "command.stdout").write_bytes(b"independent stdout")
    (retained / "command.stderr").write_bytes(b"independent stderr")
    (retained / "manifest.json").write_bytes(b"independent manifest")
    (retained / "diagnostics.json").write_text(
        json.dumps(
            {
                "program": lane.as_record(),
                "compiler": {
                    "path": "/installed/bin/backend",
                    "sha256": "a" * 64,
                    "fingerprint": "b" * 64,
                    "cargo_profile": lane.compiler_profile,
                },
            }
        ),
        encoding="utf-8",
    )
    compilation = {
        "compiler_source_root": "/installed/source",
        "cells": [
            {
                "lane": lane.as_record(),
                "runtime_cell": {"key": {"stdlib_profile": "micro"}},
                "outputs": {
                    "artifact": {"path": "program.wasm", "size": 7, "sha256": "c" * 64},
                    "command": command,
                },
            }
        ],
    }
    # Native process replay remains the named seam below; the installed join
    # itself reads real independent cold policy/receipt, exported cache bytes,
    # and separate cold command streams. A current warm invocation cannot
    # substitute its identity for the original emitting scope.
    cold_root = root.with_name(root.name + "-cold")
    cold_output = cold_root / "outputs" / lane.id
    cold_output.mkdir(parents=True)
    (cold_output / "command.stdout").write_bytes(b"independent cold stdout")
    (cold_output / "command.stderr").write_bytes(b"independent cold stderr")
    cache = cold_output / "cache-export/backend/artifact.o"
    cache.parent.mkdir(parents=True)
    cache.write_bytes(b"original cached object")
    cache_digest = hashlib.sha256(b"original cached object").hexdigest()
    exported = {
        "path": "cache-export/backend/artifact.o",
        "size": len(b"original cached object"),
        "sha256": cache_digest,
    }
    seed = {
        "path": "app/cache/" + lane.id + "/backend/artifact.o",
        "size": len(b"original cached object"),
        "sha256": cache_digest,
        "mode": 0o444,
    }
    cold_policy = cold_root / "rootfs/policy.json"
    cold_policy.parent.mkdir(parents=True)
    cold_policy.write_text(json.dumps({"nonce": "a" * 64}), encoding="utf-8")
    cold_receipt = cold_root / "runs" / lane.id / "receipt.json"
    cold_receipt.parent.mkdir(parents=True)
    cold_receipt.write_text(json.dumps({"identity_sha256": "d" * 64}), encoding="utf-8")
    _compilation_export_fixture(tmp_path, "wasm")
    observation = json.loads(
        (tmp_path / "export/program.wasm.molt-link-inputs.json").read_text(
            encoding="utf-8"
        )
    )
    assert observation["compiler"]["producer"]["scope"] == "a" * 64
    cold_command = {
        "argv": ["/installed/molt", "run", "--cold-fixture"],
        "returncode": 0,
        "duration_sec": 0.3,
    }
    cold_cell = copy.deepcopy(compilation["cells"][0])
    cold_cell.update(id=lane.id, policy="policy.json")
    cold_cell["outputs"].update(
        command=cold_command, files=[exported], observation=observation
    )
    cold = {"cells": [cold_cell], "cache_seed": [], "origins": []}
    cold_path = cold_root / "compilation.json"
    compilation.update(
        cache_seed=[seed],
        origins=[{"scope": "a" * 64, "cell": lane.id, "identity_sha256": "d" * 64}],
    )
    if defect == "cold-cache-seed":
        cold["cache_seed"] = [seed]
    elif defect == "warm-cache-seed":
        compilation["cache_seed"][0]["sha256"] = "e" * 64
    elif defect == "warm-origin":
        compilation["origins"][0]["scope"] = "b" * 64
    elif defect == "cold-receipt":
        cold_receipt.write_text(
            json.dumps({"identity_sha256": "e" * 64}), encoding="utf-8"
        )
    if defect == "wrong-cold-tier":
        cold_cell["runtime_cell"]["key"]["stdlib_profile"] = "full"
    elif defect == "wrong-warm-tier":
        compilation["cells"][0]["runtime_cell"]["key"]["stdlib_profile"] = "full"
    cold_path.write_text(json.dumps(cold), encoding="utf-8")
    path = root / "compilation.json"
    path.write_text(json.dumps(compilation), encoding="utf-8")
    reference = {
        "path": path.relative_to(tmp_path).as_posix(),
        **consumer_replay.execution_root.file_identity(path),
    }
    (root / "rootfs").mkdir()
    (root / "rootfs/python").write_bytes(b"selected Python")
    proof = {
        "python": "3.12",
        "execution": {
            "python": {
                "executable": "/python",
                "size": len(b"selected Python"),
                "sha256": hashlib.sha256(b"selected Python").hexdigest(),
            }
        },
        "cells": [
            {
                "lane": lane.as_record(),
                "artifact": {
                    "path": "/export/program.wasm",
                    "size": 7,
                    "sha256": "c" * 64,
                },
                "compiler_sha256": "a" * 64,
                "compiler_fingerprint": "b" * 64,
                "manifest": {
                    "path": "/export/manifest.json",
                    **consumer_replay.execution_root.file_identity(
                        retained / "manifest.json"
                    ),
                },
            }
        ],
        "commands": [
            {
                "role": "cold_run_" + lane.id,
                "argv": cold_command["argv"],
                "returncode": 0,
                "duration_seconds": 0.3,
                "stdout_sha256": hashlib.sha256(b"independent cold stdout").hexdigest(),
                "stderr_sha256": hashlib.sha256(b"independent cold stderr").hexdigest(),
            },
            {
                "role": "run_" + lane.id,
                "argv": command["argv"],
                "returncode": 0,
                "duration_seconds": 0.2,
                "stdout_sha256": hashlib.sha256(b"independent stdout").hexdigest(),
                "stderr_sha256": hashlib.sha256(b"independent stderr").hexdigest(),
            },
        ],
    }
    replays = []

    def replay(**kwargs):
        assert kwargs["compiler"] is installed and kwargs["lanes"] == (lane,)
        assert kwargs["launcher"] == ["/installed/bin/molt"] and kwargs[
            "controller_python"
        ] == Path("/python")
        replays.append(kwargs)

    monkeypatch.setattr(consumer_replay, "validate_compilation", replay)
    if defect == "python-image":
        (root / "rootfs/python").write_bytes(b"replaced Python")
    elif defect == "candidate-runtime":
        candidate["runtime"] = {"cells": ["other"]}
    elif defect == "candidate-source":
        candidate["native_build"]["source"]["files_sha256"] = "e" * 64
    elif defect == "product":
        proof["cells"][0]["artifact"]["sha256"] = "e" * 64
    elif defect == "command":
        proof["commands"][1]["stdout_sha256"] = "e" * 64
    elif defect == "cold-command":
        proof["commands"][0]["argv"] = command["argv"]
    elif defect == "cold-raw-stdout":
        (cold_output / "command.stdout").write_bytes(b"substituted cold stdout")
    elif defect == "diagnostics":
        proof["cells"][0]["compiler_fingerprint"] = "e" * 64
    elif defect == "manifest":
        proof["cells"][0]["manifest"]["sha256"] = "e" * 64
    elif defect == "reference":
        reference["path"] = "compilation-evidence/pip-3.12/compilation.json"
    elif defect == "missing-cell":
        proof["cells"].clear()
    kwargs = dict(
        evidence_root=tmp_path,
        candidate=candidate,
        proof=proof,
        launcher=["/installed/bin/molt"],
        controller_python=Path("/python"),
        owner="bundle",
        selected_lanes=(lane,),
        selected_stdlib_profile="micro" if defect != "invalid-tier" else ["micro"],
        supervisor=tmp_path / "supervisor",
    )
    if defect:
        expected_errors = {
            "wrong-cold-tier": "installed compilation used another runtime stdlib profile",
            "wrong-warm-tier": "installed compilation used another runtime stdlib profile",
            "invalid-tier": "installed compilation runtime stdlib profile is invalid",
            "candidate-runtime": "retained compilation installed inputs differ from the release candidate",
            "candidate-source": "retained compilation installed inputs differ from the release candidate",
            "python-image": "emitted artifact changed before its standalone replay",
            "product": "standalone product differs from its installed compilation",
            "command": "installed command summary differs from its raw compilation export",
            "cold-command": "installed command summary differs from its raw compilation export",
            "cold-raw-stdout": "installed command summary differs from its raw compilation export",
            "diagnostics": "installed product diagnostics differ from admitted compiler or lane",
            "manifest": "emitted artifact changed before its standalone replay",
            "reference": "installed compilation reference is invalid",
            "missing-cell": "release proof omits an installed compilation product",
            "cold-cache-seed": "installed cold/warm compilation phases have incorrect reuse inputs",
            "warm-cache-seed": "warm compilation did not import the selected cold cache generation",
            "warm-origin": "warm compilation did not import the selected cold cache generation",
            "cold-receipt": "warm compilation did not import the selected cold cache generation",
        }
        with pytest.raises(ValueError, match="^" + expected_errors[defect] + "$"):
            consumer_replay.validate_installed_compilation(reference, **kwargs)
        if defect not in {
            "candidate-runtime",
            "candidate-source",
            "python-image",
            "reference",
            "invalid-tier",
        }:
            assert [row["evidence"] for row in replays] == [root, cold_root]
        else:
            assert replays == []
    else:
        consumer_replay.validate_installed_compilation(reference, **kwargs)
        assert [row["evidence"] for row in replays] == [root, cold_root]
        assert replays[0]["compilation"]["origins"] == [
            {"scope": "a" * 64, "cell": lane.id, "identity_sha256": "d" * 64}
        ]
        assert replays[1]["compilation"]["origins"] == []
        assert cache.read_bytes() == b"original cached object"


@pytest.mark.parametrize(
    ("receiver", "outcome"),
    [
        (receiver, outcome)
        for receiver in ("execution", "absent", "capture", "frontend")
        for outcome in ("success", "closed-timeout", "unknown-timeout")
    ]
    + [("execution-first", "unknown-timeout"), ("execution", "wrong-coordinate")],
)
def test_release_metadata_receivers_own_guarded_namespace(
    tmp_path, monkeypatch, receiver, outcome
):
    """Keep release owners real; replace only child execution and CAS publication."""
    import gc
    import subprocess
    import weakref
    from types import SimpleNamespace

    from molt import input_report, process_guard, temporary_artifacts
    from molt import python_environment_identity, python_interpreter
    from tests.executable_test_support import (
        native_executable_name,
        write_mock_executable,
    )
    from tests.process_guard_common import guard_result_for_test
    from tests.python_environment_test_support import runtime_identity_manifest
    from tools.proof_queue_pkg import command_admission, toolchain_capture

    image = b"release Python content; never executed"
    python = write_mock_executable(tmp_path / native_executable_name("python"), image)
    prepared = write_mock_executable(
        tmp_path / native_executable_name("prepared"), image
    )
    source_root = tmp_path / "bundle" / "source"
    source_root.mkdir(parents=True)
    compiler = SimpleNamespace(source_root=source_root)
    env = ProcessEnvironment({"RELEASE_METADATA_SENTINEL": "preserve"})
    allocations = []
    original_allocation = temporary_artifacts.OwnedTemporaryDirectory

    def allocate(**kwargs):
        allocation = original_allocation(dir=tmp_path, **kwargs)
        path = Path(allocation.name).resolve(strict=True)
        allocations.append(
            (path, temporary_artifacts._identity(path), weakref.ref(allocation))
        )
        return allocation

    monkeypatch.setattr(temporary_artifacts, "OwnedTemporaryDirectory", allocate)
    # Reporting is outside this argv contract; prohibit an ambient logging flag.
    monkeypatch.setattr(input_report, "worker_log_dir", lambda: None)
    capture = {
        "schema": "molt.python-capture.v2",
        "identity": runtime_identity_manifest(python="3.12.15"),
        "file_custody": [{"path": str(python), "size": 1, "sha256": "a" * 64}],
        "node_custody": [{"node": "/file_nodes/0", "file_index": 0}],
        "inventory_profile": {
            "hash_workers": 1,
            "hashed_files": 1,
            "hashed_bytes": 1,
            "hash_seconds": 0.0,
        },
    }
    published = []
    reference = {"fixture": "toolchain-capture-reference"}

    def publish(root, identities):
        assert root == tmp_path / "cas" and identities == {}
        assert all(not path.exists() for path, _, _ in allocations)
        published.append(True)
        return {}, reference, None

    monkeypatch.setattr(toolchain_capture, "publish_capture", publish)
    calls = []
    failure = subprocess.TimeoutExpired([str(python)], 30)
    receipt = guard_result_for_test(
        [str(python)],
        124,
        "partial metadata",
        "timeout",
        descendants_closed=outcome == "closed-timeout",
    )
    failure.guarded_result = receipt

    def run(command, **kwargs):
        assert len(allocations) == 1
        prefix = allocations[0][0]
        assert prefix.is_dir() and list(prefix.iterdir()) == []
        assert command[:5] == [
            str(python),
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
        ]
        assert kwargs["cwd"] == tmp_path and kwargs["capture_output"] is True
        first_identity = receiver.startswith("execution") and not calls
        expected_env = dict(env)
        if first_identity:
            expected_env["PYTHONHASHSEED"] = "0"
            assert command[5:] == ["-S", "-c", python_interpreter._IDENTITY_PROBE]
            assert kwargs["memory_guard_prefix"] == "MOLT_PYTHON_IDENTITY"
            assert kwargs["timeout"] == 10.0 and kwargs["check"] is False
            output = json.dumps(
                {
                    "implementation": "CPython",
                    "version": "3.12.15",
                    "executable": str(python),
                }
            )
        else:
            executor = (
                command_admission._COMMANDS
                if receiver == "capture"
                else verify_consumer._COMMANDS
            )
            assert kwargs["memory_guard_prefix"] == executor.prefix
            assert kwargs["guard_source_root"] == executor.repo_root
            assert kwargs["timeout"] == (60 if receiver == "frontend" else 30)
            assert kwargs["check"] is (receiver != "capture")
            if receiver == "execution":
                assert command[5:] == ["-c", verify_consumer._HOST_PROBE]
                output = json.dumps(
                    {
                        "system": "Linux",
                        "machine": "aarch64"
                        if outcome == "wrong-coordinate"
                        else "x86_64",
                        "pointer_bits": 64,
                        "gil_disabled": False,
                    }
                )
            elif receiver == "absent":
                assert command[5:] == [
                    "-c",
                    "import importlib.util; assert importlib.util.find_spec('molt') is None",
                ]
                output = ""
            elif receiver == "capture":
                assert command[5:] == [
                    str(
                        Path(python_environment_identity.__file__).resolve(strict=True)
                    ),
                    "--capture-active-environment",
                    "--with-custody",
                    "--hash-workers",
                    "1",
                    "--admit-virtualenv-bootstrap",
                    "--admit-external-root",
                    str(source_root.resolve(strict=True)),
                ]
                output = json.dumps(capture)
            else:
                assert receiver == "frontend"
                assert command[5:] == [
                    "-c",
                    "import json,runpy,sys; from pathlib import Path; "
                    "root=Path(sys.argv[1]); "
                    "m=runpy.run_path(str(root/'source'/'packaging'/'bootstrap.py')); "
                    "home,identity=m['_release_inputs'](root); "
                    "print(json.dumps(str(home/'environments'/identity/'bin'/'python')))",
                    str(source_root.parent),
                ]
                output = json.dumps(str(prepared))
        assert dict(kwargs["env"]) == expected_env
        calls.append(command)
        if outcome.endswith("timeout") and (
            not first_identity or receiver == "execution-first"
        ):
            raise failure
        return guard_result_for_test(command, 0, output, "", descendants_closed=True)

    monkeypatch.setattr(process_guard, "run_completed_command", run)

    def unexpected_child(*_args, **_kwargs):
        pytest.fail("release metadata bypassed the mocked core guard")

    monkeypatch.setattr(subprocess, "Popen", unexpected_child)

    def invoke():
        if receiver.startswith("execution"):
            return verify_consumer._capture_python_execution(
                python,
                env=env,
                cwd=tmp_path,
                target={"platform": "linux", "arch": "x86_64"},
                reference_python="3.12.15",
            )
        if receiver == "absent":
            return verify_consumer._absent_probe(python, env=env, cwd=tmp_path)
        if receiver == "capture":
            return verify_consumer._compilation_captures(
                interpreters=(python, python),
                tools=(),
                compiler=compiler,
                cwd=tmp_path,
                env=env,
                supervisor=tmp_path / "unused-supervisor",
                cas_root=tmp_path / "cas",
            )
        return verify_consumer._installed_frontend_python(
            compiler=compiler,
            python=python,
            env=env,
            cwd=tmp_path,
            portable_bundle=True,
        )

    try:
        if outcome.endswith("timeout"):
            with pytest.raises(subprocess.TimeoutExpired) as raised:
                invoke()
            assert raised.value is failure and raised.value.guarded_result is receipt
            failure.__traceback__ = None
            del raised
        elif outcome == "wrong-coordinate":
            with pytest.raises(ValueError, match="host differs from candidate"):
                invoke()
        else:
            result = invoke()
            if receiver == "execution":
                assert result == {
                    "host": {"platform": "linux", "arch": "x86_64", "pointer_bits": 64},
                    "python": {
                        "implementation": "CPython",
                        "version": "3.12.15",
                        "executable": str(python),
                        "sha256": hashlib.sha256(image).hexdigest(),
                        "size": len(image),
                        "gil_disabled": False,
                    },
                }
            elif receiver == "capture":
                assert result == ([capture], reference)
            elif receiver == "frontend":
                assert result == prepared
            else:
                assert result is None
        assert len(calls) == (2 if receiver == "execution" else 1)
        assert published == (
            [True] if receiver == "capture" and outcome == "success" else []
        )
        assert len(allocations) == 1
        path, _, allocation_ref = allocations[0]
        gc.collect()
        assert allocation_ref() is None
        assert path.exists() is (outcome == "unknown-timeout")
        if outcome == "unknown-timeout":
            assert any(str(path) in note for note in failure.__notes__)
    finally:
        # The core seam never spawned a child. Dispose only these exact fixture owners.
        for path, identity, _ in allocations:
            temporary_artifacts._cleanup_temporary_directory(path, identity)
