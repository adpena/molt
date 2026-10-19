"""Model Cargo production without replacing filesystem or receipt custody."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import nullcontext
import hashlib
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
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.process_environment import NameRule, ProcessEnvironment
from molt.toolchain_identity import (
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from tests.executable_test_support import custody_spelling
from tools.proof_queue_pkg import (
    cargo_output_layout,
    command_identity,
    custody_cas,
    supervisor_generation as generation,
)


pytestmark = pytest.mark.usefixtures("cargo_output_implementation_source")
_OBSERVED_ENV = "Molt_Supervisor_Unlisted_Input"


def _model(tmp_path, monkeypatch, *, during_build=None):
    target = tmp_path / "store" / "target"
    target.mkdir(parents=True)
    crate = tmp_path / "tools" / "proof_supervisor"
    source = crate / "src" / "main.rs"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"supervisor source")
    (crate / "Cargo.toml").write_text(
        '[package]\nname="molt_proof_supervisor"\nversion="0.1.0"\n[workspace]\nmembers=[]\n',
        encoding="utf-8",
    )
    name = (
        "molt-proof-supervisor.exe"
        if sys.platform == "win32"
        else "molt-proof-supervisor"
    )
    binary = target / "release" / name
    binary.parent.mkdir()
    calls = []

    def inputs(env, *, profile="release", target=None, reuse_telemetry=None):
        del profile, target, reuse_telemetry
        identity = stable_regular_file_identity(source, label="model supervisor input")
        captured = {
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
        }
        return (
            captured,
            [identity],
            generation.effective_cargo_environment(captured, env),
        )

    def verify(inputs, identities, env):
        for identity in identities:
            verify_stable_regular_file_identity(
                identity, label="model supervisor input"
            )

    def build(command, *, cwd, env, timeout):
        from tests.cli.test_cargo_build_reads import _script, _unit

        # This boundary models Cargo only. Real locks, input fences, immutable
        # file publication and receipt consumers run below it.
        calls.append((tuple(command), cwd, dict(env)))
        if during_build is not None:
            during_build(source)
        binary.write_bytes(b"immutable supervisor image")
        deps = binary.parent / "deps"
        message = json.loads(
            _unit(
                deps,
                crate,
                "molt_proof_supervisor",
                ["src/main.rs"],
                kind="bin",
                environment={
                    "SUPERVISOR_BUILD_SCRIPT_VALUE": "generated",
                    _OBSERVED_ENV: env.get(_OBSERVED_ENV),
                },
            )
        )
        # Cargo reports the uplifted executable and its deps image. The real
        # reader must bind that final output to the populated rustc rule.
        Path(message["filenames"][0]).write_bytes(binary.read_bytes())
        message["executable"] = str(binary)
        message["fresh"] = len(calls) > 1
        script = json.loads(
            _script(
                binary.parent / "build",
                crate,
                "molt_proof_supervisor",
                [
                    f"cargo:rerun-if-env-changed={_OBSERVED_ENV}",
                    "cargo:rustc-env=SUPERVISOR_BUILD_SCRIPT_VALUE=generated",
                ],
                generated={"SUPERVISOR_BUILD_SCRIPT_VALUE": "generated"},
            )
        )
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(message)
            + "\n"
            + json.dumps(script)
            + "\n"
            + json.dumps({"reason": "build-finished", "success": True})
            + "\n",
            "",
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


@pytest.mark.usefixtures("admitted_build_capacity")
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
    monkeypatch.setenv(_OBSERVED_ENV, "different historical verifier environment")
    for cas_root, receipt, copied in records:
        generation.validate_receipt(
            receipt,
            binary=copied,
            cas_root=cas_root,
            expected_target=Path(env["CARGO_TARGET_DIR"]),
        )


@pytest.mark.usefixtures("admitted_build_capacity")
@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize("value", [None, "", "private-build-value"])
def test_generation_records_declared_environment_from_frozen_cargo_boundary(
    tmp_path, monkeypatch, rule, value
):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    if value is not None:
        env[_OBSERVED_ENV] = value
    frozen = ProcessEnvironment(env, rule=rule)
    image, telemetry = generation.provision(cwd=tmp_path, env=frozen)
    record = generation.read_generation(telemetry)
    observation = record["inputs"]["cargo_environment"]
    expected_digest = (
        None if value is None else hashlib.sha256(value.encode()).hexdigest()
    )
    assert observation == {
        "schema": "molt.cargo-environment-inputs.v1",
        "name_rule": rule.value,
        "local_unit_count": 1,
        "variables": [{"name": rule.key(_OBSERVED_ENV), "sha256": expected_digest}],
    }
    assert image.read_bytes() == b"immutable supervisor image"
    assert len(calls) == 1
    assert calls[0][2].get(_OBSERVED_ENV) == value
    assert "private-build-value" not in json.dumps(record)
    observed = CargoEnvironmentInputs.from_dict(observation)
    observed.verify(frozen)
    with pytest.raises(ValueError, match="Cargo environment inputs changed"):
        observed.verify(frozen.updated({_OBSERVED_ENV: "changed after compilation"}))


@pytest.mark.usefixtures("admitted_build_capacity")
def test_cargo_observation_does_not_recapture_mutated_caller_or_global_environment(
    tmp_path, monkeypatch
):
    def mutate(_source):
        env[_OBSERVED_ENV] = "caller changed during Cargo"
        monkeypatch.setenv(_OBSERVED_ENV, "global changed during Cargo")

    env, source, mutable, calls = _model(tmp_path, monkeypatch, during_build=mutate)
    env[_OBSERVED_ENV] = "frozen execution value"
    image, telemetry = generation.provision(cwd=tmp_path, env=env)
    observation = generation.read_generation(telemetry)["inputs"]["cargo_environment"]
    assert observation["variables"] == [
        {
            "name": NameRule.host().key(_OBSERVED_ENV),
            "sha256": hashlib.sha256(b"frozen execution value").hexdigest(),
        }
    ]
    assert calls[0][2][_OBSERVED_ENV] == "frozen execution value"
    assert image.read_bytes() == b"immutable supervisor image"


@pytest.mark.usefixtures("admitted_build_capacity")
def test_equal_supervisor_bytes_keep_each_published_observation(tmp_path, monkeypatch):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    image, first = generation.provision(
        cwd=tmp_path, env={**env, _OBSERVED_ENV: "first build"}
    )
    same_image, second = generation.provision(
        cwd=tmp_path, env={**env, _OBSERVED_ENV: "second build"}
    )
    assert image == same_image
    assert first["generation_artifact"] != second["generation_artifact"]
    cas_root = tmp_path / "result" / "custody-cas"
    binary = custody_cas.put_file(cas_root, image, executable=True).as_dict()
    # Copy both receipts after the second provision changed the shared target.
    # Each exact generation reference must retain its own observation.
    for telemetry, value in ((first, b"first build"), (second, b"second build")):
        receipt = generation.publish_receipt(telemetry, cas_root=cas_root)
        record = custody_cas.read_ref(
            receipt["generation_artifact"], expected_root=cas_root
        )
        assert (
            receipt["generation_artifact"]["semantic_sha256"]
            == telemetry["generation_artifact"]["semantic_sha256"]
        )
        assert record["inputs"]["cargo_environment"]["variables"] == [
            {
                "name": NameRule.host().key(_OBSERVED_ENV),
                "sha256": hashlib.sha256(value).hexdigest(),
            }
        ]
        generation.validate_receipt(
            receipt,
            binary=binary,
            cas_root=cas_root,
            expected_target=Path(env["CARGO_TARGET_DIR"]),
        )
    assert len(calls) == 2


@pytest.mark.usefixtures("admitted_build_capacity")
@pytest.mark.parametrize("fault", ["missing", "extra-field", "bad-digest", "unbound"])
def test_historical_generation_requires_exact_bound_cargo_observation(
    tmp_path, monkeypatch, fault
):
    env, source, mutable, calls = _model(tmp_path, monkeypatch)
    image, telemetry = generation.provision(cwd=tmp_path, env=env)
    cas_root = tmp_path / "result" / "custody-cas"
    binary = custody_cas.put_file(cas_root, image, executable=True).as_dict()
    receipt = generation.publish_receipt(telemetry, cas_root=cas_root)
    record = custody_cas.read_ref(
        receipt["generation_artifact"], expected_root=cas_root
    )
    observation = record["inputs"]["cargo_environment"]
    if fault == "missing":
        del record["inputs"]["cargo_environment"]
    elif fault == "extra-field":
        observation["ambient_values"] = {}
    elif fault == "bad-digest":
        observation["variables"][0]["sha256"] = "not a digest"
    else:
        observation["variables"][0]["sha256"] = "0" * 64
    if fault != "unbound":
        record["input_sha256"] = canonical_json_sha256(record["inputs"])
    receipt["generation_artifact"] = custody_cas.put_json(cas_root, record).as_dict()
    with pytest.raises(ValueError, match="Cargo environment|supervisor generation"):
        generation.validate_receipt(
            receipt,
            binary=binary,
            cas_root=cas_root,
            expected_target=Path(env["CARGO_TARGET_DIR"]),
        )


@pytest.mark.usefixtures("admitted_build_capacity")
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


@pytest.mark.usefixtures("admitted_build_capacity")
def test_mutating_input_cannot_publish_a_generation(tmp_path, monkeypatch):
    env, source, mutable, calls = _model(
        tmp_path,
        monkeypatch,
        during_build=lambda path: path.write_bytes(b"changed during Cargo"),
    )
    with pytest.raises(ValueError, match="changed since identity capture"):
        generation.provision(cwd=tmp_path, env=env)
    assert not (mutable.parents[2] / "custody-cas").exists()


@pytest.mark.usefixtures("admitted_build_capacity")
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


@pytest.mark.usefixtures("admitted_build_capacity")
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


@pytest.mark.usefixtures("admitted_build_capacity")
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
            child_stderr="",
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
            "MOLT_CARGO_BUILD_DIR_BASE": "another-base",
            "TMP": "new-temp",
        }
    )
    assert first == warm
    for changed in (
        {"RUSTFLAGS": "-C opt-level=3", "COMPILED_VALUE": "one"},
        {"RUSTFLAGS": "-C opt-level=2", "COMPILED_VALUE": "two"},
        *(
            {"RUSTFLAGS": "-C opt-level=2", "COMPILED_VALUE": "one", name: "/new SDK"}
            for name in (
                "MOLT_LLVM_PREFIX",
                "LLVM_CONFIG_PATH",
                "LLVM_SYS_221_PREFIX",
                "MLIR_SYS_220_PREFIX",
                "TABLEGEN_220_PREFIX",
            )
        ),
    ):
        assert all(value != capture(changed)[key] for key, value in first.items())


def test_source_authority_includes_observed_cargo_input_implementation():
    from tools.proof_queue_pkg import supervisor_custody

    root = Path(__file__).resolve().parents[2]
    paths = set(supervisor_custody.source_authority_paths(root))
    assert {
        root / "src/molt/cli/cargo_build_reads.py",
        root / "src/molt/cli/cargo_environment_inputs.py",
        root / "src/molt/cargo_execution_policy.py",
        root / "src/molt/process_environment.py",
    } <= paths


def test_build_inputs_bind_dispatch_and_probes_to_selected_cargo(tmp_path, monkeypatch):
    from tools.proof_queue_pkg import toolchain_capture

    selected, decoy = tmp_path / "selected", tmp_path / "path"
    for directory in (selected, decoy):
        directory.mkdir()
        for role in ("cargo", "rustc"):
            image = directory / (role + (".exe" if os.name == "nt" else ""))
            image.write_bytes((directory.name + role).encode())
            image.chmod(0o755)
    suffix = ".exe" if os.name == "nt" else ""
    cargo = selected / ("cargo" + suffix)
    rustc = selected / ("rustc" + suffix)
    cargo_home = tmp_path / "cargo-home"
    cargo_home.mkdir()
    (cargo_home / "config.toml").write_text(
        '[env]\nMOLT_SUPERVISOR_CAPTURED = { value = "configured", force = true }\n',
        encoding="utf-8",
    )
    environment = {
        "CARGO": str(cargo),
        "RUSTC": str(rustc),
        "PATH": str(decoy),
        "CARGO_HOME": str(cargo_home),
        "MOLT_SUPERVISOR_CAPTURED": "caller value",
    }
    version_commands, link_commands = [], []

    def version(argv, **kwargs):
        version_commands.append(argv)
        return subprocess.CompletedProcess(argv, 0, "selected tool version", "")

    def link_probe(**kwargs):
        link_commands.append(kwargs)
        return [], {}

    monkeypatch.setattr(command_identity, "_run_captured", version)
    monkeypatch.setattr(
        toolchain_capture, "capture_rust_link_process_images", link_probe
    )
    inputs, identities, execution_environment = generation._build_inputs(
        environment, profile="debug"
    )
    assert inputs["command"][0] == str(cargo)
    # Toolchain identity records the custody spelling; argv keeps the selection.
    assert inputs["toolchains"]["cargo"]["path"] == custody_spelling(cargo)
    assert version_commands == [
        [str(cargo), "--version"],
        [str(rustc), "--version", "--verbose"],
    ]
    assert len(link_commands) == 1
    assert link_commands[0]["cargo"] == cargo
    assert link_commands[0]["command_argv"] == inputs["command"]
    assert identities
    assert isinstance(execution_environment, ProcessEnvironment)
    assert execution_environment["CARGO"] == str(cargo)
    assert execution_environment["MOLT_SUPERVISOR_CAPTURED"] == "configured"
    assert all(
        execution_environment[name] == value
        for name, value in inputs["cargo_inputs"].items()
    )
