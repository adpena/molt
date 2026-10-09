from __future__ import annotations
from tests.process_guard_common import install_module_view

import functools
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from tests.process_guard_common import run_custody_subject_process
from tests import proof_queue_owned_roots
from tools.proof_queue_pkg import (
    execution_custody,
    process_image_capture,
    supervisor_custody,
    supervisor_generation,
)


def test_supervisor_sources_follow_local_dependencies_and_workspace_inheritance(
    tmp_path: Path,
) -> None:
    source = tmp_path / "tools" / "proof_supervisor"
    shared = tmp_path / "runtime" / "shared"
    for crate in (source, shared):
        (crate / "src").mkdir(parents=True)
        (crate / "src" / "lib.rs").write_text("// authority\n", encoding="utf-8")
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers=["runtime/shared"]\n'
        '[workspace.package]\nedition="2024"\n',
        encoding="utf-8",
    )
    (source / "Cargo.toml").write_text(
        '[package]\nname="supervisor"\nversion="0.1.0"\n'
        "[workspace]\nmembers=[]\n"
        "[target.'cfg(windows)'.dependencies]\n"
        'shared={path="../../runtime/shared"}\n',
        encoding="utf-8",
    )
    (shared / "Cargo.toml").write_text(
        '[package]\nname="shared"\nversion="0.1.0"\nedition.workspace=true\n',
        encoding="utf-8",
    )
    for name in ("build.py", "Cargo.lock", "protocol.json"):
        (source / name).write_text("", encoding="utf-8")
    shared_asset = shared / "src" / "schema.json"
    shared_asset.write_text("{}", encoding="utf-8")
    unrelated = tmp_path / "src" / "unrelated.rs"
    unrelated.parent.mkdir()
    unrelated.write_text("// not a supervisor crate\n", encoding="utf-8")

    paths = supervisor_custody.source_authority_paths(tmp_path)

    assert paths == tuple(sorted(set(paths)))
    assert (tmp_path / "Cargo.toml").resolve() in paths
    assert (shared / "Cargo.toml").resolve() in paths
    assert (shared / "src" / "lib.rs").resolve() in paths
    assert shared_asset.resolve() in paths
    assert (source / "protocol.json").resolve() in paths
    assert unrelated.resolve() not in paths
    (shared / "Cargo.toml").unlink()
    with pytest.raises(ValueError, match="Cargo manifest"):
        supervisor_custody.source_authority_paths(tmp_path)


@pytest.mark.parametrize(
    "payload",
    [
        '{"root_exit_code":1,"root_exit_code":0}',
        '{"value":NaN}',
        '{"value":1e999}',
        "{",
    ],
)
def test_verified_supervisor_receipt_still_requires_exact_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    payload: str,
) -> None:
    receipt = tmp_path / "receipt.json"
    receipt.write_text(payload, encoding="utf-8")
    monkeypatch.setattr(
        supervisor_custody.command_identity,
        "_run_captured",
        lambda *a, **kw: subprocess.CompletedProcess([], 0, "", ""),
    )
    with pytest.raises(ValueError, match="no readable receipt"):
        supervisor_custody._validated_supervisor_receipt(
            binary=tmp_path / "never-executed",
            policy_path=tmp_path / "policy.json",
            receipt_path=receipt,
            cwd=tmp_path,
            env={},
        )


def test_supervisor_policy_cannot_publish_nonfinite_json(tmp_path: Path) -> None:
    policy = tmp_path / "policy.json"
    with pytest.raises(ValueError):
        supervisor_custody._atomic_json(policy, {"timeout": float("inf")})
    assert not policy.exists()


@pytest.mark.parametrize("rooted", [False, True])
@pytest.mark.parametrize("changed", [None, "receipt", "policy"])
def test_verified_result_binds_exact_receipt_and_policy_generations(
    tmp_path, monkeypatch, rooted, changed
):
    # Protocol consumer test: this fixture does not attest native verification.
    receipt_path, policy_path = tmp_path / "receipt.json", tmp_path / "policy.json"
    original = {
        "schema": supervisor_custody.SUPERVISOR_RECEIPT_SCHEMA,
        "root_exit_code": 7,
    }
    receipt_bytes = json.dumps(original).encode()
    policy_bytes = b'{"fixture":"original policy"}'
    receipt_path.write_bytes(receipt_bytes)
    policy_path.write_bytes(policy_bytes)
    binding = {
        "receipt_sha256": hashlib.sha256(receipt_bytes).hexdigest(),
        "receipt_bytes": len(receipt_bytes),
        "policy_input_sha256": hashlib.sha256(policy_bytes).hexdigest(),
        "policy_input_bytes": len(policy_bytes),
    }
    commands = []

    def verifier_result(command, **kwargs):
        commands.append(command)
        if changed == "receipt":
            receipt_path.write_text(
                json.dumps({**original, "root_exit_code": 0}), encoding="utf-8"
            )
        elif changed == "policy":
            policy_path.write_bytes(b'{"fixture":"replaced policy"}')
        return subprocess.CompletedProcess(command, 0, json.dumps(binding), "")

    monkeypatch.setattr(
        supervisor_custody.command_identity, "_run_captured", verifier_result
    )
    kwargs = dict(
        binary=tmp_path / "fixture-verifier",
        policy_path=policy_path,
        receipt_path=receipt_path,
        cwd=tmp_path,
        env={},
        rootfs=tmp_path if rooted else None,
    )
    if changed:
        with pytest.raises(
            ValueError, match="verified different receipt or policy bytes"
        ):
            supervisor_custody._validated_supervisor_receipt(**kwargs)
    else:
        assert supervisor_custody._validated_supervisor_receipt(**kwargs) == original
    assert len(commands) == 1
    assert commands[0][1] == ("verify-rooted" if rooted else "verify")


@pytest.mark.parametrize(
    "result", ["{}", "", '{"receipt_bytes":true,"policy_input_bytes":true}']
)
def test_verification_success_without_exact_input_binding_is_not_admitted(
    tmp_path, monkeypatch, result
):
    receipt, policy = tmp_path / "receipt.json", tmp_path / "policy.json"
    receipt.write_text(
        json.dumps({"schema": supervisor_custody.SUPERVISOR_RECEIPT_SCHEMA}),
        encoding="utf-8",
    )
    policy.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        supervisor_custody.command_identity,
        "_run_captured",
        lambda *a, **k: subprocess.CompletedProcess([], 0, result, ""),
    )
    with pytest.raises(ValueError, match="verification binding|verified different"):
        supervisor_custody._validated_supervisor_receipt(
            binary=tmp_path / "fixture-verifier",
            policy_path=policy,
            receipt_path=receipt,
            cwd=tmp_path,
            env={},
        )


@functools.lru_cache(maxsize=1)
def _test_proof_supervisor_binary() -> Path:
    binary, _receipt = supervisor_generation.provision(
        cwd=Path(supervisor_custody.__file__).resolve().parents[2],
        env=proof_queue_owned_roots.native_build_environment(source=Path(__file__)),
    )
    return binary


def test_supervisor_admits_exact_platform_image_without_directory_authority(
    tmp_path: Path,
) -> None:
    root = Path(sys.executable).resolve(strict=True)
    broker = tmp_path / "conhost.exe"
    shutil.copy2(root, broker)
    platform_image = process_image_capture.capture_image(
        "windows-console-broker", broker, root_exit_disposition="terminate"
    )

    root_role, images = supervisor_custody._supervisor_fixed_images(
        {}, {}, [str(root)], [platform_image]
    )
    derived = supervisor_custody._supervisor_derived_roots(
        descendants="declared-toolchains", env={}
    )

    assert root_role == "root-command"
    assert [row for row in images if row["role"] == "windows-console-broker"] == [
        {
            "role": "windows-console-broker",
            "path": platform_image["path"],
            "sha256": platform_image["sha256"],
            "root_exit_disposition": "terminate",
        }
    ]
    assert derived == []


@pytest.mark.skipif(
    sys.platform not in {"win32", "linux"},
    reason="lossless process inventory is available on Windows and Linux",
)
def test_process_image_inventory_captures_distinct_runtime_and_projects_once(
    tmp_path: Path,
) -> None:
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path,
        source=Path(__file__),
        nodeid="native-process-inventory",
    )
    supervisor = _test_proof_supervisor_binary()
    runtime = tmp_path / ("runtime.exe" if sys.platform == "win32" else "runtime")
    shutil.copy2(supervisor, runtime)

    images, telemetry = supervisor_custody.capture_process_image_inventory(
        binary=supervisor,
        role="fixture",
        executable=supervisor,
        probe_args=["fixture-child", "spawn-and-wait", str(runtime)],
        cwd=tmp_path,
        env=os.environ,
    )

    assert telemetry["schema"] == "molt.proof-process-image-inventory.v1"
    assert telemetry["observed_image_count"] == 2
    assert {image["path"] for image in images} == {
        process_image_capture._image_path_key(supervisor.resolve()),
        process_image_capture._image_path_key(runtime.resolve()),
    }
    launcher = next(image for image in images if image["role"] == "fixture-launcher")
    identity = {
        "path": str(supervisor),
        "launcher_sha256": launcher["sha256"],
        "content_path": str(supervisor),
        "executable_sha256": launcher["sha256"],
        "process_images": images,
    }
    envelope = {
        "process_closure": {
            "kind": "registered-toolchain",
            "descendants": "declared-toolchains",
            "toolchains": ["fixture"],
        }
    }
    child = execution_custody.child_policy(
        envelope, {"fixture": identity}, environment_executables={}
    )
    _root_role, fixed = supervisor_custody._supervisor_fixed_images(
        {"fixture": identity}, {}, [str(supervisor)]
    )
    child_images = {
        (row["path"], row["sha256"])
        for row in child["allowed"]
        if row["toolchain"] == "fixture"
    }
    supervisor_images = {
        (execution_custody._norm(row["path"]), row["sha256"])
        for row in fixed
        if row["role"].startswith("fixture-")
    }
    assert child_images == supervisor_images


@pytest.mark.skipif(
    sys.platform not in {"win32", "linux"},
    reason="lossless process supervision is available on Windows and Linux",
)
def test_python_generated_child_matches_native_execution_identity(tmp_path: Path):
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path,
        source=Path(__file__),
        nodeid="native-python-child",
    )
    supervisor = _test_proof_supervisor_binary()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    source = tmp_path / "source"
    source.mkdir()
    receipt_path = tmp_path / "native-receipt.json"
    environment = dict(os.environ)
    environment.pop("CARGO_TARGET_DIR", None)
    environment[supervisor_custody.PROOF_SCRATCH_ROOT_ENV] = str(scratch)
    required = supervisor_custody.required_execution_environment(
        binary=supervisor,
        mode="declared-tree",
        cwd=tmp_path,
        env=environment,
    )
    environment = supervisor_custody.bind_required_environment(environment, required)
    roots = supervisor_custody._derived_root_provenance(
        descendants="declared-toolchains",
        env=environment,
        source_root=source,
        result_path=receipt_path,
    )
    envelope = {"process_closure": {"descendants": "declared-toolchains"}}
    child_policy = execution_custody.child_policy(
        envelope, {}, environment_executables={}, derived_roots=roots
    )
    generated = scratch / supervisor.name
    # Materialize the candidate after admission, then launch through the actual
    # Python hook. The OS supervisor must observe the same path and bytes.
    payload = (
        "import shutil,subprocess; "
        f"shutil.copy2({str(supervisor)!r},{str(generated)!r}); "
        f"subprocess.run([{str(generated)!r},'fixture-child','exit','0'],check=True)"
    )
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    command = [
        str(Path(sys._base_executable).resolve()),
        str(bootstrap),
        "command",
        "0",
        payload,
    ]
    server = execution_custody.ChildCustodyEventServer("python", child_policy)
    environment.update(server.environment())
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(child_policy)
    policy_path = tmp_path / "native-policy.json"
    native_policy = supervisor_custody._supervisor_policy(
        envelope=envelope,
        execution_command=command,
        execution_env=environment,
        cwd=tmp_path,
        nonce="a" * 64,
        toolchains={},
        environment_executables={},
        platform_process_images=process_image_capture.platform_auxiliary_images(
            "declared-toolchains"
        ),
    )
    assert native_policy["derived_roots"] == child_policy["derived_roots"]
    supervisor_custody._atomic_json(policy_path, native_policy)
    with server:
        completed = run_custody_subject_process(
            [
                str(supervisor),
                "run",
                "--policy",
                str(policy_path),
                "--receipt",
                str(receipt_path),
            ],
            cwd=tmp_path,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    assert completed.returncode == 0, completed.stderr
    native = supervisor_custody._validated_supervisor_receipt(
        binary=supervisor,
        policy_path=policy_path,
        receipt_path=receipt_path,
        cwd=tmp_path,
        env=environment,
    )
    assert native["complete"] is True
    assert native["root_exit_code"] == 0
    assert native["violations"] == []
    child = server.receipt()
    assert execution_custody.child_receipt_is_admitted(child)
    decisions = [
        event for event in child["events"] if event.get("event") == "child-process"
    ]
    assert len(decisions) == 1
    assert decisions[0]["derived_role"] == supervisor_custody.SCRATCH_OUTPUT_ROLE
    assert decisions[0]["resolved"] == str(generated.resolve())
    event_log = receipt_path.with_name(native["event_log"]["file"])
    execution_custody.require_derived_child_image_bindings(child, event_log)


def test_process_image_authority_rejects_mutation_and_conflicting_identity(
    tmp_path: Path,
) -> None:
    executable = tmp_path / "tool.exe"
    executable.write_bytes(b"before")
    captured = process_image_capture.capture_image("tool", executable)
    selection = process_image_capture.capture_image(
        "tool", executable, preserve_path=True
    )
    conflict = {**captured, "role": "tool-runtime", "sha256": "0" * 64}

    assert process_image_capture.canonical_images([captured, selection]) == [selection]
    with pytest.raises(ValueError, match="conflicting identities"):
        process_image_capture.canonical_images([captured, conflict])

    executable.write_bytes(b"after")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        process_image_capture.revalidate_images([captured])


@pytest.mark.skipif(os.name != "nt", reason="native Windows resolved image spelling")
@pytest.mark.parametrize("preserve_path", [False, True])
@pytest.mark.parametrize("disposition", ["require-exit", "terminate"])
def test_windows_process_image_roundtrip_keeps_canonical_coordinate(
    tmp_path, preserve_path, disposition
):
    executable = tmp_path / "ExactCompiler.EXE"
    payload = b"unchanged process image"
    executable.write_bytes(payload)
    physical = executable.resolve(strict=True)
    if len(physical.drive) != 2 or physical.drive[1] != ":":
        pytest.skip("fixture requires a DOS drive spelling")
    # Independent expected wire spelling: lower-case drive, actual entry names.
    expected_path = physical.drive.lower() + str(physical)[len(physical.drive) :]
    request = Path(physical.drive.upper() + str(physical)[len(physical.drive) :])
    expected = {
        "schema": "molt.proof-process-image-capture.v1",
        "role": "cargo-launcher",
        "path": expected_path,
        "sha256": hashlib.sha256(payload).hexdigest(),
        "size_bytes": len(payload),
    }
    if preserve_path:
        expected["path_kind"] = "selection"
    if disposition == "terminate":
        expected["root_exit_disposition"] = "terminate"
    captured = process_image_capture.capture_image(
        "cargo-launcher", request, disposition, preserve_path=preserve_path
    )
    assert captured == expected
    assert (
        process_image_capture.capture_image(
            "cargo-launcher",
            Path("\\\\?\\" + str(request)),
            disposition,
            preserve_path=preserve_path,
        )
        == expected
    )
    assert process_image_capture.canonical_images([captured]) == [expected]
    assert process_image_capture.revalidate_images([captured]) == [expected]
    assert process_image_capture.revalidate_images(
        process_image_capture.canonical_images([captured])
    ) == [expected]


@pytest.mark.parametrize("preserve_path", [False, True])
@pytest.mark.parametrize("mutation", ["content", "digest", "size", "extra-field"])
def test_process_image_coordinate_roundtrip_does_not_relax_identity(
    tmp_path, preserve_path, mutation
):
    executable = tmp_path / "tool.exe"
    executable.write_bytes(b"before")
    row = process_image_capture.canonical_images(
        [
            process_image_capture.capture_image(
                "tool", executable, preserve_path=preserve_path
            )
        ]
    )[0]
    if mutation == "content":
        executable.write_bytes(b"AFTER!")  # Same length: size alone is insufficient.
    elif mutation == "digest":
        row["sha256"] = "0" * 64
    elif mutation == "size":
        row["size_bytes"] += 1
    else:
        row["unexpected"] = True
    expected_field = {
        "content": "sha256",
        "digest": "sha256",
        "size": "size_bytes",
        "extra-field": "unexpected fields",
    }[mutation]
    with pytest.raises(ValueError, match="changed while live custody armed") as failure:
        process_image_capture.revalidate_images([row])
    assert f"(differing fields: {expected_field})" in str(failure.value)


def test_process_image_revalidation_reads_shared_role_content_once(
    tmp_path, monkeypatch
):
    executable = tmp_path / "tool.exe"
    executable.write_bytes(b"one shared image")
    captured = process_image_capture.capture_image("cargo-launcher", executable)
    rows = [captured, {**captured, "role": "rust-build-helper"}]
    actual_digest = hashlib.file_digest
    reads = []

    def counted_digest(stream, algorithm):
        reads.append((Path(stream.name), algorithm))
        return actual_digest(stream, algorithm)

    install_module_view(
        monkeypatch,
        "hashlib",
        hashlib,
        process_image_capture,
        file_digest=counted_digest,
    )
    assert process_image_capture.revalidate_images(rows) == rows
    assert len(reads) == 1
    assert reads[0][0].samefile(executable)
    assert reads[0][1] == "sha256"


def test_process_image_coordinate_keeps_samefile_selection_aliases_distinct(tmp_path):
    selected, alias = tmp_path / "selected.exe", tmp_path / "alias.exe"
    selected.write_bytes(b"same inode and bytes")
    os.link(selected, alias)
    assert selected.samefile(alias)
    rows = [
        process_image_capture.capture_image("cargo-launcher", path, preserve_path=True)
        for path in (selected, alias)
    ]
    assert rows[0]["path"] != rows[1]["path"]
    assert len(process_image_capture.canonical_images(rows)) == 2
    assert process_image_capture.revalidate_images(rows) == rows
    # Even same-inode, same-hash content cannot admit another launcher entry.
    identity = {
        "path": rows[0]["path"],
        "content_path": rows[0]["path"],
        "launcher_sha256": rows[0]["sha256"],
        "executable_sha256": rows[0]["sha256"],
        "process_images": [rows[1]],
    }
    with pytest.raises(
        ValueError, match="launcher image is outside its process closure"
    ):
        process_image_capture.toolchain_images("cargo", identity)


def test_resolved_image_cannot_borrow_selection_alias_identity(tmp_path):
    target, alias = tmp_path / "target.exe", tmp_path / "alias.exe"
    target.write_bytes(b"same content")
    try:
        alias.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"symlink capability unavailable: {exc}")
    selection = process_image_capture.capture_image("tool", alias, preserve_path=True)
    resolved = process_image_capture.capture_image("tool", alias)
    assert selection["path"] != resolved["path"]
    assert selection["sha256"] == resolved["sha256"]
    assert process_image_capture.revalidate_images([selection, resolved]) == [
        selection,
        resolved,
    ]
    forged = dict(selection)
    del forged["path_kind"]
    with pytest.raises(ValueError, match="changed while live custody armed"):
        process_image_capture.revalidate_images(
            process_image_capture.canonical_images([forged])
        )


@pytest.mark.skipif(
    sys.platform not in {"win32", "linux"} or shutil.which("git") is None,
    reason="real Git inventory requires a lossless native backend and Git",
)
def test_real_git_launcher_runtime_closure_is_kernel_observed(tmp_path: Path) -> None:
    git = Path(str(shutil.which("git"))).resolve(strict=True)

    images, telemetry = supervisor_custody.capture_process_image_inventory(
        binary=_test_proof_supervisor_binary(),
        role="git",
        executable=git,
        probe_args=["--version"],
        cwd=tmp_path,
        env=os.environ,
    )

    assert telemetry["observed_image_count"] == len(images)
    assert process_image_capture._image_path_key(git) in {
        image["path"] for image in images
    }
    assert process_image_capture.revalidate_images(images) == images


@pytest.mark.skipif(os.name != "nt", reason="native Windows directory-entry identity")
def test_image_membership_normalizes_windows_device_spelling(tmp_path):
    image = tmp_path / "Compiler.EXE"
    image.write_bytes(b"image")
    ordinary = str(image).swapcase()
    extended = "\\\\?\\" + str(image)
    assert Path(ordinary).samefile(image)
    assert process_image_capture._image_path_key(
        Path(extended)
    ) == process_image_capture._image_path_key(Path(ordinary))


@pytest.mark.skipif(os.name != "nt", reason="actual Windows extended path capture")
def test_extended_windows_environment_tool_reaches_both_consumers(tmp_path):
    from tools.proof_queue_pkg import execution_environment

    executable = tmp_path / "selected.exe"
    shutil.copyfile(sys.executable, executable)
    extended = "\\\\?\\" + str(executable)
    configured = execution_environment._execution_environment_executable_identities(
        {"CC": extended}, cwd=tmp_path
    )
    policy = execution_custody.child_policy(
        {"process_closure": {"descendants": "declared-toolchains"}},
        {},
        environment_executables=configured,
    )
    _, native = supervisor_custody._supervisor_fixed_images(
        {}, configured, [sys.executable]
    )
    expected = {
        (
            execution_custody._norm(executable),
            hashlib.sha256(executable.read_bytes()).hexdigest(),
        )
    }
    assert {(row["path"], row["sha256"]) for row in policy["allowed"]} == expected
    assert {
        (execution_custody._norm(row["path"]), row["sha256"])
        for row in native
        if row["role"] == "env:CC"
    } == expected


@pytest.mark.parametrize(
    "boundary",
    [
        "capture",
        "canonical",
        "revalidate",
        "frozen",
        "supervisor",
        "hash",
        "probe",
        "inventory",
    ],
)
def test_proof_image_admission_refuses_parent_traversal_before_custody(
    tmp_path, monkeypatch, boundary
):
    from types import SimpleNamespace
    from tools.proof_queue_pkg import command_identity, toolchain_capture

    base = tmp_path / "bin"
    foreign = tmp_path / "foreign"
    base.mkdir()
    (foreign / "deep").mkdir(parents=True)
    selected = base / "tool"
    selected.write_bytes(b"same bytes")
    other = foreign / "tool"
    other.write_bytes(selected.read_bytes())
    hop = base / "hop"
    try:
        hop.symlink_to(foreign / "deep", target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink capability unavailable: {exc}")
    witness = hop / ".." / "tool"
    assert hop.samefile(foreign / "deep")
    # Win32 resolves the lexical parent component before following the link;
    # POSIX traverses the link first. Both forms must be refused before custody.
    expected, excluded = (
        (selected, other) if sys.platform == "win32" else (other, selected)
    )
    assert witness.samefile(expected) and not witness.samefile(excluded)
    row = process_image_capture.capture_image("tool", selected, preserve_path=True)
    changed = {**row, "path": str(witness)}
    monkeypatch.setattr(
        command_identity.admission,
        "_COMMANDS",
        SimpleNamespace(run=lambda *a, **k: pytest.fail("unadmitted probe launched")),
    )
    with pytest.raises(ValueError, match="parent traversal"):
        if boundary == "capture":
            process_image_capture.capture_image("tool", witness, preserve_path=True)
        elif boundary == "canonical":
            process_image_capture.canonical_images([changed])
        elif boundary == "revalidate":
            process_image_capture.revalidate_images([changed])
        elif boundary == "frozen":
            toolchain_capture.frozen_files(changed)
        elif boundary == "supervisor":
            supervisor_custody._supervisor_fixed_images({}, {}, [str(witness)])
        elif boundary == "hash":
            command_identity._hash_file(witness)
        elif boundary == "inventory":
            supervisor_custody.capture_process_image_inventory(
                binary=selected,
                role="tool",
                executable=witness,
                probe_args=["--version"],
                cwd=tmp_path,
                env={},
            )
        else:
            command_identity._run_captured([str(witness)], cwd=tmp_path, env={})


@pytest.mark.skipif(
    os.name != "nt", reason="native Windows case-sensitive directory capability"
)
@pytest.mark.parametrize("kind", ["file", "hardlink", "symlink"])
def test_windows_case_distinct_entries_keep_image_and_watch_identity(tmp_path, kind):
    from molt.llvm_linker_roles import lexical_executable_path
    from tools.proof_queue_pkg import toolchain_capture

    upper, lower = tmp_path / "Driver.EXE", tmp_path / "driver.exe"
    content = tmp_path / "content.exe"
    content.write_bytes(b"same bytes")
    try:
        if kind == "symlink":
            upper.symlink_to(content)
            lower.symlink_to(content)
        else:
            upper.write_bytes(content.read_bytes())
            if kind == "hardlink":
                os.link(upper, lower)
            else:
                with lower.open("xb") as stream:
                    stream.write(content.read_bytes())
    except (FileExistsError, PermissionError) as exc:
        pytest.skip(f"case-distinct entry capability unavailable: {exc}")
    except OSError as exc:
        if kind == "symlink":
            pytest.skip(f"symlink capability unavailable: {exc}")
        raise
    assert {entry.name for entry in tmp_path.iterdir()} >= {"Driver.EXE", "driver.exe"}
    if kind in {"hardlink", "symlink"}:
        assert upper.samefile(lower)
    assert str(lexical_executable_path(upper)) == str(upper)
    rows = [
        process_image_capture.capture_image("compiler", path, preserve_path=True)
        for path in (upper, lower)
    ]
    assert len(process_image_capture.canonical_images(rows)) == 2
    assert process_image_capture.revalidate_images(rows) == rows
    assert len(toolchain_capture.frozen_files(rows)) == 2
    assert execution_custody._norm(upper) != execution_custody._norm(lower)
    watch = execution_custody.WatchSpec(
        tmp_path, frozenset({execution_custody._norm(upper)})
    )
    assert watch.owns(upper) and not watch.owns(lower)
    _, native = supervisor_custody._supervisor_fixed_images({}, {}, [str(upper)], rows)
    assert len([row for row in native if row["role"] == "compiler"]) == 2


@pytest.mark.skipif(os.name != "nt", reason="native Windows directory-entry identity")
def test_windows_case_distinct_ancestor_aliases_and_missing_entries(tmp_path):
    target = tmp_path / "target"
    target.mkdir()
    (target / "tool.exe").write_bytes(b"same image")
    upper, lower = tmp_path / "Entry", tmp_path / "entry"
    try:
        upper.symlink_to(target, target_is_directory=True)
        lower.symlink_to(target, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"case-distinct directory symlink capability unavailable: {exc}")
    assert {p.name for p in tmp_path.iterdir()} >= {"Entry", "entry"}
    selected, other = upper / "tool.exe", lower / "tool.exe"
    assert selected.samefile(other)
    first = process_image_capture._image_path_key(selected)
    second = process_image_capture._image_path_key(other)
    assert first != second
    rows = [
        process_image_capture.capture_image("tool", p, preserve_path=True)
        for p in (selected, other)
    ]
    assert len(process_image_capture.canonical_images(rows)) == 2
    upper.unlink()
    with pytest.raises((OSError, ValueError)):
        process_image_capture._image_path_key(selected)
    with pytest.raises((OSError, ValueError)):
        process_image_capture.revalidate_images(rows)
    watch = execution_custody.WatchSpec(tmp_path, frozenset({first}))
    assert watch.owns(upper)


@pytest.mark.skipif(os.name != "nt", reason="native Windows path identity")
def test_windows_exact_name_image_custody_uses_real_file_identity(tmp_path):
    executable = tmp_path / "ExactCompiler.EXE"
    executable.write_bytes(b"exact image")
    selected = process_image_capture.capture_image(
        "compiler", executable, preserve_path=True
    )
    key = process_image_capture._image_path_key(executable)
    assert Path(key).samefile(executable)
    assert Path(key).name == executable.name
    assert process_image_capture.canonical_images([selected]) == [selected]
    assert process_image_capture.revalidate_images([selected]) == [selected]
    extended = Path("\\\\?\\" + str(executable))
    assert process_image_capture._image_path_key(extended) == key
    assert (
        process_image_capture.capture_image("compiler", extended, preserve_path=True)
        == selected
    )


@pytest.mark.skipif(os.name != "nt", reason="native Windows verbatim entry semantics")
@pytest.mark.parametrize("suffix", [".", " "])
@pytest.mark.parametrize("ancestor", [False, True])
@pytest.mark.parametrize("same_inode", [False, True])
def test_verbatim_sensitive_entry_cannot_borrow_ordinary_image(
    tmp_path, monkeypatch, suffix, ancestor, same_inode
):
    from types import SimpleNamespace
    from molt.llvm_linker_roles import lexical_executable_path
    from tools.proof_queue_pkg import command_identity, toolchain_capture

    directory = tmp_path / "tools"
    directory.mkdir()
    ordinary = directory / "CLANG.EXE"
    ordinary.write_bytes(b"identical bytes")
    if ancestor:
        selected_directory = Path("\\\\?\\" + str(directory) + suffix)
        selected_directory.mkdir()
        selected = selected_directory / ordinary.name
    else:
        selected = Path("\\\\?\\" + str(ordinary) + suffix)
    if same_inode:
        os.link(ordinary, selected)
    else:
        selected.write_bytes(ordinary.read_bytes())
    assert selected.read_bytes() == ordinary.read_bytes()
    assert selected.samefile(ordinary) is same_inode
    assert str(selected) != str(ordinary)
    assert str(lexical_executable_path(selected)) == str(selected)
    row = process_image_capture.capture_image("compiler", ordinary, preserve_path=True)
    changed = {**row, "path": str(selected)}
    refusal = "verbatim trailing-dot/space"
    monkeypatch.setattr(
        command_identity.admission,
        "_COMMANDS",
        SimpleNamespace(run=lambda *a, **k: pytest.fail("unsupported probe launched")),
    )
    monkeypatch.setattr(
        toolchain_capture, "_COMMANDS", command_identity.admission._COMMANDS
    )
    with pytest.raises(ValueError, match=refusal):
        command_identity._run_captured([str(selected)], cwd=tmp_path, env={})
    with pytest.raises(ValueError, match=refusal):
        toolchain_capture._run_rust_link_probe(
            [str(selected)],
            phase="selection",
            unit="host",
            cwd=tmp_path,
            compiler_cwd=tmp_path,
            env={},
            timeout=1,
            probes=[],
        )
    with pytest.raises(ValueError, match=refusal):
        process_image_capture.capture_image("compiler", selected, preserve_path=True)
    with pytest.raises(ValueError, match=refusal):
        process_image_capture.canonical_images([changed])
    with pytest.raises(ValueError, match=refusal):
        process_image_capture.revalidate_images([changed])
    with pytest.raises(ValueError, match=refusal):
        toolchain_capture.frozen_files(changed)
    with pytest.raises(ValueError, match=refusal):
        supervisor_custody._supervisor_fixed_images({}, {}, [str(selected)])


@pytest.mark.skipif(os.name != "nt", reason="native Windows namespace syntax")
@pytest.mark.parametrize(
    "coordinate",
    [
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\CLANG.EXE",
        r"\\?\GLOBALROOT\Device\MissingDevice\CLANG.EXE",
        r"\\?\UNC\server",
        r"\\?\unc\server",
    ],
)
def test_unsupported_windows_namespace_refuses_before_lookup_or_probe(
    tmp_path, monkeypatch, coordinate
):
    from types import SimpleNamespace
    from tools.proof_queue_pkg import command_identity

    monkeypatch.setattr(
        command_identity.admission,
        "_COMMANDS",
        SimpleNamespace(
            run=lambda *a, **k: pytest.fail("unsupported namespace launched")
        ),
    )
    with pytest.raises(ValueError, match="proof path custody requires"):
        command_identity._run_captured([coordinate], cwd=tmp_path, env={})


@pytest.mark.skipif(
    os.name != "nt", reason="native Windows per-drive working directory"
)
@pytest.mark.parametrize("cross_drive", [False, True])
@pytest.mark.parametrize("consumer", ["product", "capture"])
def test_windows_drive_relative_selection_preserves_absolute_entrypoint(
    tmp_path, cross_drive, consumer
):
    from molt.llvm_linker_roles import lexical_executable_path

    if len(tmp_path.drive) != 2 or tmp_path.drive[1] != ":":
        pytest.skip("fixture root has no DOS drive-relative coordinate")
    original_cwd = Path.cwd()
    fixture_drive_cwd = Path(os.path.abspath(tmp_path.drive))
    other_cwd = None
    if cross_drive:
        other = next(
            (
                Path(root)
                for root in os.listdrives()
                if Path(root).drive.casefold() != tmp_path.drive.casefold()
                and Path(root).is_dir()
            ),
            None,
        )
        if other is None:
            pytest.skip(
                "no second accessible drive for actual per-drive-CWD regression"
            )
        # Observe that drive's existing cwd; do not reset its remembered state
        # to its root merely to select a different current drive.
        other_cwd = Path(os.path.abspath(other.drive))
    executable = tmp_path / "clang++.exe"
    executable.write_bytes(b"this drive's selected compiler")
    try:
        os.chdir(tmp_path)
        requested = Path(tmp_path.drive + executable.name)
        assert not requested.is_absolute()
        assert requested.samefile(executable)
        if other_cwd is not None:
            try:
                os.chdir(other_cwd)
            except OSError as exc:
                pytest.skip(f"second drive cannot be selected as cwd: {exc}")
            assert Path.cwd().drive.casefold() != tmp_path.drive.casefold()
            assert requested.samefile(executable), (
                "OS per-drive coordinate must name fixture"
            )
        if consumer == "product":
            selected = lexical_executable_path(requested)
        else:
            row = process_image_capture.capture_image(
                "compiler", requested, preserve_path=True
            )
            selected = Path(row["path"])
            assert row["sha256"] == hashlib.sha256(executable.read_bytes()).hexdigest()
            assert process_image_capture.revalidate_images([row]) == [row]
        assert selected.is_absolute()
        assert selected.name == executable.name
        assert selected.samefile(executable)
    finally:
        try:
            os.chdir(fixture_drive_cwd)
        finally:
            os.chdir(original_cwd)
