"""Model Cargo production without replacing filesystem or receipt custody."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import nullcontext
import json
import os
from types import SimpleNamespace
from pathlib import Path
import subprocess
import sys
import threading
import time

import pytest

from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import (
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from tools.proof_queue_pkg import (
    cargo_output_layout,
    command_identity,
    custody_cas,
    supervisor_generation as generation,
)


pytestmark = pytest.mark.usefixtures("cargo_output_implementation_source")


def _model(tmp_path, monkeypatch, *, during_build=None):
    target = tmp_path / "store" / "target"
    target.mkdir(parents=True)
    source = tmp_path / "source.rs"
    source.write_bytes(b"supervisor source")
    name = (
        "molt-proof-supervisor.exe"
        if sys.platform == "win32"
        else "molt-proof-supervisor"
    )
    binary = target / "release" / name
    binary.parent.mkdir()
    calls = []

    def inputs(env):
        identity = stable_regular_file_identity(source, label="model supervisor input")
        return {
            "profile": "release",
            "source_root": str(tmp_path),
            "command": ["cargo", "build", "--locked", "--release"],
            "cargo_inputs": {
                "MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR": canonical_json_sha256(
                    {
                        "source": identity.sha256,
                        "flags": env.get("RUSTFLAGS"),
                    }
                )
            },
            "source_sha256": identity.sha256,
            "environment_sha256": canonical_json_sha256(dict(env)),
        }, [identity]

    def verify(inputs, identities, env):
        for identity in identities:
            verify_stable_regular_file_identity(
                identity, label="model supervisor input"
            )

    def build(command, *, cwd, env, timeout):
        # This boundary models Cargo only. Real locks, input fences, immutable
        # file publication and receipt consumers run below it.
        calls.append((tuple(command), cwd, dict(env)))
        if during_build is not None:
            during_build(source)
        binary.write_bytes(b"immutable supervisor image")
        message = {"reason": "compiler-artifact", "fresh": len(calls) > 1}
        return subprocess.CompletedProcess(
            command, 0, json.dumps(message) + "\n" + str(binary) + "\n", ""
        )

    # Model tests do not sample the host process table. The scope's real
    # ownership/reuse/exception contract is exercised independently below.
    monkeypatch.setattr(generation, "_provision_guard_scope", lambda env: nullcontext())
    monkeypatch.setattr(generation, "_build_inputs", inputs)
    monkeypatch.setattr(generation, "_verify_inputs", verify)
    monkeypatch.setattr(command_identity, "_run_captured", build)
    # No fixture image is launched; native path-budget models live with layout.
    monkeypatch.setattr(
        cargo_output_layout.CargoOutputLayout,
        "admit_cargo_path",
        staticmethod(lambda path: None),
    )
    monkeypatch.setattr(custody_cas, "admit_executable_path", lambda root, name: None)
    return {"CARGO_TARGET_DIR": str(target)}, source, binary, calls


def test_distinct_results_share_cargo_freshness_and_immutable_image(
    tmp_path, monkeypatch
):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    first, cold = generation.provision(cwd=tmp_path, env=env)
    second, warm = generation.provision(cwd=tmp_path, env=env)
    assert len(calls) == 2  # A retained receipt never bypasses Cargo.
    assert first == second and first != mutable
    assert cold["cargo_compiled_artifact_count"] == 1
    assert warm["cargo_fresh_artifact_count"] == 1
    assert warm["cargo_compiled_artifact_count"] == 0
    assert cold["generation_artifact"] == warm["generation_artifact"]
    records = []
    for index, telemetry in enumerate((cold, warm)):
        cas_root = tmp_path / f"result-{index}" / "custody-cas"
        copied = custody_cas.put_file(cas_root, first, executable=True).as_dict()
        receipt = generation.publish_receipt(telemetry, cas_root=cas_root)
        generation.validate_receipt(
            receipt,
            binary=copied,
            cas_root=cas_root,
            expected_target=Path(env["CARGO_TARGET_DIR"]),
        )
        records.append((cas_root, receipt, copied))
    # Historical receipts do not depend on the mutable target or shared store.
    (tmp_path / "store").rename(tmp_path / "retained-store")
    for cas_root, receipt, copied in records:
        generation.validate_receipt(
            receipt,
            binary=copied,
            cas_root=cas_root,
            expected_target=Path(env["CARGO_TARGET_DIR"]),
        )


def test_source_and_environment_bind_generation_without_selecting_a_new_target(
    tmp_path, monkeypatch
):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    image, first = generation.provision(cwd=tmp_path, env=env)
    source.write_bytes(b"changed supervisor source")
    same_image, second = generation.provision(cwd=tmp_path, env=env)
    _, third = generation.provision(
        cwd=tmp_path, env={**env, "RUSTFLAGS": "-C opt-level=2"}
    )
    assert image == same_image
    assert (
        len(
            {
                item["generation_artifact"]["semantic_sha256"]
                for item in (first, second, third)
            }
        )
        == 3
    )
    assert {call[2]["CARGO_TARGET_DIR"] for call in calls} == {env["CARGO_TARGET_DIR"]}
    assert (
        len({call[2]["MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR"] for call in calls}) == 3
    )


def test_mutating_input_cannot_publish_a_generation(tmp_path, monkeypatch):
    env, source, mutable, calls = _model(
        tmp_path,
        monkeypatch,
        during_build=lambda path: path.write_bytes(b"changed during Cargo"),
    )
    with pytest.raises(ValueError, match="changed since identity capture"):
        generation.provision(cwd=tmp_path, env=env)
    assert not (mutable.parents[2] / "custody-cas").exists()


def test_failed_cargo_cannot_reuse_a_prior_generation(tmp_path, monkeypatch):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    image, receipt = generation.provision(cwd=tmp_path, env=env)
    monkeypatch.setattr(
        command_identity,
        "_run_captured",
        lambda command, **kwargs: subprocess.CompletedProcess(
            command, 1, "", "Cargo rejected freshness"
        ),
    )
    with pytest.raises(ValueError, match="Cargo rejected freshness"):
        generation.provision(cwd=tmp_path, env=env)
    assert image.read_bytes() == b"immutable supervisor image"


def test_substituted_generation_cannot_bind_another_executable(tmp_path, monkeypatch):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    image, telemetry = generation.provision(cwd=tmp_path, env=env)
    cas_root = tmp_path / "result" / "custody-cas"
    copied = custody_cas.put_file(cas_root, image, executable=True).as_dict()
    receipt = generation.publish_receipt(telemetry, cas_root=cas_root)
    payload = custody_cas.read_ref(
        receipt["generation_artifact"], expected_root=cas_root
    )
    payload["binary"]["sha256"] = "0" * 64
    receipt["generation_artifact"] = custody_cas.put_json(cas_root, payload).as_dict()
    with pytest.raises(ValueError, match="differs from admitted build or executable"):
        generation.validate_receipt(
            receipt,
            binary=copied,
            cas_root=cas_root,
            expected_target=Path(env["CARGO_TARGET_DIR"]),
        )


def test_shared_cargo_target_has_one_provision_owner(tmp_path, monkeypatch):
    lock = threading.Lock()
    active = maximum = 0

    def overlap(source):
        nonlocal active, maximum
        with lock:
            active += 1
            maximum = max(maximum, active)
        time.sleep(0.05)
        with lock:
            active -= 1

    env, source, mutable, calls = _model(tmp_path, monkeypatch, during_build=overlap)
    with ThreadPoolExecutor(max_workers=2) as workers:
        futures = [
            workers.submit(generation.provision, cwd=tmp_path, env=env)
            for _ in range(2)
        ]
        outputs = [future.result() for future in futures]
    assert maximum == 1
    assert outputs[0][0] == outputs[1][0]
    assert len(calls) == 2


def test_result_roots_do_not_select_supervisor_build_directories(tmp_path):
    output = tmp_path / "output"
    output.mkdir()
    declaration = cargo_output_layout.declare_root(str(output))
    first = cargo_output_layout.CargoOutputLayout.create(
        result_root=tmp_path / "result-one", declaration=declaration
    )
    second = cargo_output_layout.CargoOutputLayout.create(
        result_root=tmp_path / "result-two", declaration=declaration
    )
    assert first.payload_root != second.payload_root
    assert first.supervisor_target == second.supervisor_target
    assert first.supervisor_store.is_relative_to(output)
    assert not first.supervisor_store.exists()


@pytest.mark.parametrize("inherited_scope", [False, True])
@pytest.mark.parametrize("fail_setup", [False, True])
def test_provision_scope_reuses_one_observer_and_keeps_each_child_guard(
    tmp_path, monkeypatch, inherited_scope, fail_setup
):
    from tools import harness_memory_guard as harness

    limits = harness.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
    )
    environment = {"MOLT_FIXTURE_INPUT": "unchanged"}
    context = harness.HarnessExecutionContext(
        prefix="MOLT_TEST",
        repo_root=tmp_path,
        env=environment,
        limits=limits,
        artifact_root=tmp_path,
    )
    observers = []
    exits = []
    guarded = []
    monkeypatch.setattr(harness, "_AUTO_SENTINEL_SUPPRESSORS", int(inherited_scope))

    class Observer:
        def __enter__(self):
            harness._note_auto_sentinel_suppressor_entered()
            return self

        def __exit__(self, *exception):
            exits.append(exception[0])
            harness._note_auto_sentinel_suppressor_exited()

    def observer(**kwargs):
        observers.append(kwargs)
        return Observer()

    def run_guarded(command, **kwargs):
        guarded.append((list(command), kwargs))
        return harness.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
        )

    monkeypatch.setattr(
        harness.HarnessExecutionContext, "from_env", lambda *args, **kwargs: context
    )
    monkeypatch.setattr(harness, "repo_process_sentinel", observer)
    monkeypatch.setattr(harness.memory_guard, "run_guarded", run_guarded)

    def setup():
        with generation._provision_guard_scope(environment):
            # Exercise the real nested context selection. Only the native
            # process producer is modeled; both calls retain per-child custody.
            for index in range(2):
                result = harness.guarded_completed_process(
                    [sys.executable, "-c", f"print({index})"],
                    prefix="MOLT_TEST",
                    env=environment,
                    limits=limits,
                )
                assert result.returncode == 0
            if fail_setup:
                raise RuntimeError("setup failed")

    if fail_setup:
        with pytest.raises(RuntimeError, match="setup failed"):
            setup()
    else:
        setup()
    assert len(guarded) == 2
    assert all(kwargs["cleanup_orphans"] is True for _, kwargs in guarded)
    assert all(
        kwargs["max_rss_kb"] == limits.max_process_rss_kb for _, kwargs in guarded
    )
    assert environment == {"MOLT_FIXTURE_INPUT": "unchanged"}
    assert harness._AUTO_SENTINEL_SUPPRESSORS == int(inherited_scope)
    if inherited_scope:
        assert observers == [] and exits == []
    else:
        assert len(observers) == 1
        assert observers[0]["drain_on_exit"] is False
        assert observers[0]["suppress_auto_guard"] is True
        assert exits == [RuntimeError if fail_setup else None]


@pytest.mark.parametrize("custody_kind", ["source", "nested", "external"])
def test_undeclared_supervisor_store_is_external_by_construction(
    tmp_path, monkeypatch, custody_kind
):
    source = tmp_path / "plain-clone"
    source.mkdir()
    custody = {
        "source": source,
        "nested": source / "tmp",
        "external": tmp_path / "canonical",
    }[custody_kind]
    monkeypatch.setattr(
        cargo_output_layout, "implementation_source_root", lambda: source
    )
    monkeypatch.setattr(
        cargo_output_layout,
        "checkout_custody",
        lambda root: SimpleNamespace(custody_root=custody),
    )
    first = cargo_output_layout.CargoOutputLayout(tmp_path / "result-one")
    second = cargo_output_layout.CargoOutputLayout(tmp_path / "result-two")
    expected_root = custody if custody_kind == "external" else source.parent
    assert first.supervisor_store.parent == expected_root / "proof-supervisor"
    assert first.supervisor_target == second.supervisor_target
    assert not first.supervisor_store.is_relative_to(source)
    assert not first.supervisor_store.exists()
    overlapping = cargo_output_layout.CargoOutputLayout(first.supervisor_store)
    with pytest.raises(ValueError, match="overlaps source or receipt"):
        _ = overlapping.supervisor_store


def _content_fixture(tmp_path):
    source = tmp_path / "tools" / "proof_supervisor"
    dependency = tmp_path / "runtime" / "shared"
    for crate, name in ((source, "molt-proof-supervisor"), (dependency, "shared")):
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            f'[package]\nname="{name}"\nversion="0.1.0"\n', encoding="utf-8"
        )
        (crate / "src" / "lib.rs").write_text(
            "pub const VALUE: u8 = 1;\n", encoding="utf-8"
        )
    with (source / "Cargo.toml").open("a", encoding="utf-8") as stream:
        stream.write(
            '[workspace]\nmembers=[]\n[dependencies]\nshared={path="../../runtime/shared"}\n'
        )
    (source / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    (source / "protocol.json").write_text("{}", encoding="utf-8")
    config = tmp_path / "config.toml"
    config.write_text('[env]\nCOMPILED_VALUE="default"\n', encoding="utf-8")

    def capture(environment):
        paths = sorted(p for p in tmp_path.rglob("*") if p.is_file())
        identities = [
            stable_regular_file_identity(p, label="fixture compile input")
            for p in paths
        ]
        inputs = {
            "source_root": str(tmp_path),
            "profile": "release",
            "target": None,
            "toolchains": {},
            "environment_executables": {},
            "configuration_paths": [str(config)],
            "files": [
                {"path": str(item.path), "size_bytes": item.size, "sha256": item.sha256}
                for item in identities
            ],
        }
        return generation._cargo_content_inputs(inputs, environment)

    return source, dependency, config, capture


def test_content_inputs_detect_restored_mtime_and_rebuild_only_owning_crate(tmp_path):
    source, dependency, config, capture = _content_fixture(tmp_path)
    first = capture({})
    rust = dependency / "src" / "lib.rs"
    prior = rust.stat()
    rust.write_text("pub const VALUE: u8 = 2;\n", encoding="utf-8")
    os.utime(rust, ns=(prior.st_atime_ns, prior.st_mtime_ns))
    second = capture({})
    assert rust.stat().st_mtime_ns == prior.st_mtime_ns
    assert first["MOLT_CARGO_INPUT_SHARED"] != second["MOLT_CARGO_INPUT_SHARED"]
    assert (
        first["MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR"]
        == second["MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR"]
    )
    (source / "protocol.json").write_text('{"changed":true}', encoding="utf-8")
    third = capture({})
    assert second["MOLT_CARGO_INPUT_SHARED"] == third["MOLT_CARGO_INPUT_SHARED"]
    assert (
        second["MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR"]
        != third["MOLT_CARGO_INPUT_MOLT_PROOF_SUPERVISOR"]
    )


def test_cargo_content_inputs_exclude_transport_but_bind_compile_environment(tmp_path):
    source, dependency, config, capture = _content_fixture(tmp_path)
    first = capture({"RUSTFLAGS": "-C opt-level=2", "COMPILED_VALUE": "one"})
    warm = capture(
        {
            "RUSTFLAGS": "-C opt-level=2",
            "COMPILED_VALUE": "one",
            "MOLT_PROOF_QUEUE_RUN_ID": "different-run",
            "MOLT_MEMORY_GUARD_TOKEN": "new-token",
            "PYTEST_CURRENT_TEST": "different-fixture",
            "CARGO_TARGET_DIR": "another-output",
            "CARGO_BUILD_JOBS": "1",
            "CARGO_BUILD_BUILD_DIR": "another-build",
            "TMP": "new-temp",
        }
    )
    assert first == warm
    for changed in (
        {"RUSTFLAGS": "-C opt-level=3", "COMPILED_VALUE": "one"},
        {"RUSTFLAGS": "-C opt-level=2", "COMPILED_VALUE": "two"},
    ):
        assert all(value != capture(changed)[key] for key, value in first.items())
