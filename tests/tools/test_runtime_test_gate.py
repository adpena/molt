from __future__ import annotations

from copy import deepcopy
import os
from pathlib import Path

import pytest

from tests import runtime_descendant_test_support as support
from tools import run_runtime_test_gate as gate

# Cargo runs in this file are fakes; build capacity is not under test.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")

# The actual runtime remainder contains the trap and cold-denial owners (and
# the trap's child test); the process-exit owner is a mandatory isolated case.
REMAINDER = ["ordinary::parallel_case", support.TRAP, support.TRAP_CHILD, support.COLD]


@pytest.fixture
def selected_cargo(monkeypatch, tmp_path):
    from molt.cli.runtime_cargo_plan import _tool_path
    from molt.rust_toolchain import rustup_proxy_entrypoint

    suffix = ".exe" if os.name == "nt" else ""
    selected = tmp_path / "selected toolchain with spaces" / f"cargo{suffix}"
    proxy = tmp_path / "path proxies" / f"cargo{suffix}"
    rustup = proxy.with_name(f"rustup{suffix}")
    for path, data in (
        (selected, b"selected physical Cargo"),
        (proxy, b"different rustup proxy"),
        (rustup, b"different rustup proxy"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o755)
    monkeypatch.setenv("CARGO", str(selected))
    monkeypatch.setenv("PATH", str(proxy.parent))
    # Prove that a bare selector would choose a real proxy identity, not the
    # queue-published physical Cargo. No compiler or proxy process is executed.
    assert _tool_path("cargo", root=tmp_path, env=os.environ, role="cargo") == proxy
    with rustup_proxy_entrypoint(proxy) as entrypoint:
        assert entrypoint == rustup
    return str(selected)


def fixture_ledger(tmp_path):
    binary = support.make_image(tmp_path)
    inventory = [*gate.ISOLATED, *gate.PROBES, *REMAINDER]
    selections = gate.selections(inventory, 4, "linux")
    data = binary.read_bytes()
    identity = (len(data), support.sha256(data))
    children = {}
    for index, (name, (args, tests)) in enumerate(selections.items()):
        owners = [test for test in tests if test in support.MODES]
        children[name] = [
            support.binary_receipt(
                tmp_path / f"child-{index}",
                binary,
                sorted(tests),
                support.family_records(binary, owners),
                args=args,
                invocation=str(index),
            )
        ]
    return (
        children,
        selections,
        dict(source=support.SOURCE, run_id="run", binary=binary, identity=identity),
    )


def test_complete_parallel_and_fresh_process_ledger_passes(tmp_path):
    children, selections, binding = fixture_ledger(tmp_path)
    verified = gate.validate_children(children, selections, **binding)
    assert {name: value and value["children"] for name, value in verified.items()} == {
        "parallel": 2,
        **{name: None for name in gate.ISOLATED},
        support.EXIT: 2,
    }
    assert "--test-threads=4" in selections["parallel"][0]
    for name in gate.ISOLATED:
        assert selections[name][0] == [
            "--exact",
            name,
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ]


@pytest.mark.parametrize(
    "mutation",
    [
        "missing",
        "extra",
        "duplicate_receipt",
        "same_process",
        "source",
        "binary",
        "selection",
        "ignored",
        "missing_test",
        "extra_test",
        "incomplete",
        "failed",
        "diagnostic",
        "execution_selection",
        "wrong_count",
        "run",
    ],
)
def test_aggregate_fails_closed_for_child_evidence_mutations(mutation, tmp_path):
    children, selections, binding = fixture_ledger(tmp_path)
    name = gate.ISOLATED[0]
    child = children[name][0]
    if mutation == "missing":
        del children[name]
    elif mutation == "extra":
        children["unexpected"] = []
    elif mutation == "duplicate_receipt":
        children[name].append(deepcopy(child))
    elif mutation == "same_process":
        child["invocation_id"] = children["parallel"][0]["invocation_id"]
    elif mutation == "source":
        child["source_identity"] = {"head": "other"}
    elif mutation == "binary":
        child["executable_sha256"] = "b" * 64
    elif mutation == "selection":
        child["inherited_args"] = ["--ignored"]
    elif mutation == "ignored":
        child["test_results"][0]["status"] = "ignored"
    elif mutation == "missing_test":
        child["test_results"] = []
    elif mutation == "extra_test":
        child["test_results"].append({"identity": "unexpected", "status": "pass"})
    elif mutation == "incomplete":
        child["result_accounting"]["complete"] = False
    elif mutation == "failed":
        child["status"] = "failed"
        child["returncode"] = 1
    elif mutation == "diagnostic":
        child["executions"].append(deepcopy(child["executions"][0]))
    elif mutation == "execution_selection":
        child["executions"][0]["argv"] = [str(binding["binary"])]
    elif mutation == "wrong_count":
        child["result_accounting"]["declared_results"] = 2
    elif mutation == "run":
        child["run_id"] = "old"
    with pytest.raises(RuntimeError):
        gate.validate_children(children, selections, **binding)


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("exit_lease_child_missing", "missing mandatory"),
        ("parallel_children_removed", "missing mandatory"),
        ("descendant_stream_changed", "custody file"),
        ("saved_rows_hide_owner", "disagree with the saved ledger"),
        ("parent_capture_changed", "capture changed"),
        ("saved_summary_forged", "missing mandatory"),
    ],
)
def test_aggregate_rederives_descendants_from_raw_child_evidence(
    mutation, message, tmp_path
):
    children, selections, binding = fixture_ledger(tmp_path)
    exit_child = children[support.EXIT][0]
    parallel = children["parallel"][0]
    if mutation == "exit_lease_child_missing":
        stderr = Path(exit_child["executions"][0]["stderr_evidence"]).read_text(
            encoding="utf-8"
        )
        kept = [line for line in stderr.splitlines() if '"mode": "lease"' not in line]
        support.republish(exit_child, "stderr", "\n".join(kept) + "\n")
    elif mutation == "parallel_children_removed":
        support.republish(parallel, "stderr", "")
    elif mutation == "descendant_stream_changed":
        source = next(
            source
            for source in parallel["retained_files"]
            if source.endswith("/stdout.log")
        )
        support.corrupt_retained_file(parallel, Path(source))
    elif mutation == "saved_rows_hide_owner":
        parallel["test_results"] = [
            row for row in parallel["test_results"] if row["identity"] != support.COLD
        ]
        accounting = parallel["result_accounting"]
        accounting["observed_results"] = accounting["declared_results"] = len(
            parallel["test_results"]
        )
        selections["parallel"][1].discard(support.COLD)
    elif mutation == "parent_capture_changed":
        path = Path(parallel["executions"][0]["stdout_evidence"])
        path.write_bytes(path.read_bytes() + b"\n")
    elif mutation == "saved_summary_forged":
        support.republish(exit_child, "stderr", "")
        exit_child["runtime_descendants"] = {"status": "verified", "children": 2}
    with pytest.raises(RuntimeError, match=message):
        gate.validate_children(children, selections, **binding)


@pytest.mark.parametrize(
    "problem", ["serial", "duplicate", "missing_semantic", "missing_probe", "empty"]
)
def test_inventory_and_parallel_witness_are_mandatory(problem):
    inventory = [*gate.ISOLATED, *gate.PROBES, "ordinary"]
    threads = 4
    if problem == "serial":
        threads = 1
    elif problem == "duplicate":
        inventory.append("ordinary")
    elif problem == "missing_semantic":
        inventory.remove(gate.ISOLATED[0])
    elif problem == "missing_probe":
        inventory.remove(gate.PROBES[0])
    elif problem == "empty":
        inventory.remove("ordinary")
    with pytest.raises(RuntimeError):
        gate.selections(inventory, threads, "linux")


PINNED_BENCH = "cpython_abi_hooks::tests::single_thread_extension_call_preemption_bench"


PINNED_LIFECYCLE = "builtins::concurrent::tests::lifecycle_cost_probe"


def test_probe_is_required_only_where_the_runtime_compiles_it():
    # The bench pins a CPU; the runtime source compiles it only on Linux and
    # Windows, so a macOS inventory must not be asked for it and must not
    # declare it as an unrun probe.
    assert PINNED_BENCH in gate.probes_for("linux")
    assert PINNED_BENCH in gate.probes_for("win32")
    assert PINNED_BENCH not in gate.probes_for("darwin")
    assert PINNED_LIFECYCLE in gate.probes_for("linux")
    assert PINNED_LIFECYCLE in gate.probes_for("win32")
    assert PINNED_LIFECYCLE not in gate.probes_for("darwin")
    assert set(gate.probes_for("darwin")) == set(gate.PROBES) - {
        PINNED_BENCH,
        PINNED_LIFECYCLE,
    }
    darwin_inventory = [*gate.ISOLATED, *gate.probes_for("darwin"), "ordinary"]
    selection = gate.selections(darwin_inventory, 4, "darwin")
    assert PINNED_BENCH not in selection["parallel"][0]
    assert selection["parallel"][1] == {"ordinary"}
    with pytest.raises(RuntimeError, match="missing required runtime cases"):
        gate.selections(darwin_inventory, 4, "linux")


def test_actual_cargo_rlib_artifact_authority():
    metadata = {
        "packages": [
            {
                "id": "path+file:///C:/Molt/molt-src/runtime/molt-runtime#molt-runtime@0.1.0",
                "name": "molt-runtime",
                "version": "0.1.0",
            }
        ]
    }
    artifact = {
        "reason": "compiler-artifact",
        "package_id": metadata["packages"][0]["id"],
        "target": {"kind": ["rlib"], "crate_types": ["rlib"], "name": "molt_runtime"},
        "profile": {"test": True},
        "executable": str(Path("molt_runtime-shipping.exe").resolve()),
    }
    import json

    identities = gate.truth.package_identities_from_metadata(json.dumps(metadata))
    actual = next(
        iter(
            gate.truth.expected_test_binaries(json.dumps(artifact), identities).values()
        )
    )
    gate.validate_artifact(actual)
    for key, value in (
        ("target_kind", "bin"),
        ("target_name", "other"),
        ("package", "other@0.1.0"),
    ):
        changed = dict(actual, **{key: value})
        with pytest.raises(RuntimeError):
            gate.validate_artifact(changed)


@pytest.mark.parametrize("value", ["nan", "inf", "-inf", "0", "-1"])
@pytest.mark.parametrize(
    "option", ["--build-timeout-seconds", "--child-timeout-seconds"]
)
def test_nonfinite_or_nonpositive_timeouts_rejected_before_any_child(option, value):
    with pytest.raises(SystemExit) as error:
        gate.main([f"{option}={value}"])
    assert error.value.code == 2


def observed_fixture():
    import json

    artifact = {"executable": str(Path("runtime.exe").resolve())}
    payload = {
        "reason": "compiler-artifact",
        "executable": artifact["executable"],
        "profile": {
            "test": True,
            "debug_assertions": False,
            "opt_level": "3",
            "debuginfo": 0,
            "overflow_checks": False,
        },
        "features": ["default", "stdlib"],
        "target": {"name": "molt_runtime", "kind": ["rlib"]},
    }
    return artifact, payload, json.dumps(payload)


def test_resolved_compiler_profile_features_preserved():
    artifact, payload, output = observed_fixture()
    observed = gate.observed_build(output, artifact, "release-fast", {})
    assert observed["observed_profile"] == payload["profile"]
    assert observed["features"] == payload["features"]
    # A different inner-loop profile remains explicitly that coordinate.
    assert (
        gate.observed_build(output, artifact, "dev-fast", {})["requested_profile"]
        == "dev-fast"
    )


def abi_compiler_fixture(source_root: Path, features: list[str]):
    # Cargo metadata and compiler-artifact are independent structured inputs.
    # The library carries no executable, so executable-only discovery misses it.
    manifest = source_root / "runtime/molt-cpython-abi/Cargo.toml"
    package_id = f"path+{manifest.parent.as_uri()}#molt-lang-cpython-abi@0.1.0"
    target = {
        "name": "molt_cpython_abi",
        "kind": ["rlib", "cdylib"],
        "crate_types": ["rlib", "cdylib"],
        "src_path": str(manifest.parent / "src/lib.rs"),
    }
    package = {
        "id": package_id,
        "name": "molt-lang-cpython-abi",
        "version": "0.1.0",
        "source": None,
        "manifest_path": str(manifest),
        "targets": [deepcopy(target)],
        "features": {
            "default": [],
            "runtime-test-support": [],
            "l7-attestation-core": [],
            "allocation-observer": ["l7-attestation-core"],
        },
    }
    metadata = {
        "packages": [package],
        "workspace_members": [package_id],
        "workspace_root": str(source_root),
    }
    artifact = {
        "reason": "compiler-artifact",
        "package_id": package_id,
        "manifest_path": str(manifest),
        "target": target,
        "profile": {"test": False, "debug_assertions": False, "opt_level": "3"},
        "features": features,
        "filenames": [
            str(source_root / "target/deps/libmolt_cpython_abi-fixture.rlib"),
            str(source_root / "target/deps/libmolt_cpython_abi-fixture.dylib"),
        ],
        "executable": None,
        "fresh": True,
    }
    for filename in artifact["filenames"]:
        path = Path(filename)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"ABI library fixture")
    return metadata, artifact


@pytest.mark.parametrize(
    "policy,features",
    [
        ("semantic", []),
        ("semantic", ["l7-attestation-core", "runtime-test-support"]),
        ("allocation-observer", ["allocation-observer", "l7-attestation-core"]),
    ],
)
def test_abi_features_bound_to_emitted_library_not_runtime_labels(
    tmp_path, policy, features
):
    import json

    metadata, artifact = abi_compiler_fixture(tmp_path, features)
    build_script = {
        "reason": "compiler-artifact",
        "package_id": artifact["package_id"],
        "target": {"kind": ["custom-build"], "name": "build-script-build"},
    }
    output = "\n".join(json.dumps(item) for item in [build_script, artifact])
    metadata_output = json.dumps(metadata)
    observed = gate.observed_abi_dependency(
        output, metadata_output, policy=policy, source_root=tmp_path
    )
    assert observed["package_id"] == artifact["package_id"]
    assert observed["package"] == "molt-lang-cpython-abi@0.1.0"
    for key in ("manifest_path", "target", "features", "filenames"):
        assert observed[key] == artifact[key]
    assert observed["compiler_stdout_sha256"] == support.sha256(output.encode())
    assert observed["metadata_sha256"] == support.sha256(metadata_output.encode())
    assert observed["allocation_feature_contract"] == {
        "allocation-observer": ["l7-attestation-core"],
        "l7-attestation-core": [],
    }


@pytest.mark.parametrize(
    "mutation",
    [
        "missing",
        "duplicate",
        "foreign_package",
        "foreign_manifest",
        "foreign_source",
        "foreign_target",
        "test_image",
        "missing_features",
        "malformed_features",
        "duplicate_features",
        "unknown_feature",
        "no_rlib",
        "relative_output",
        "missing_core",
        "missing_observer",
        "timing_observer",
        "missing_package",
        "duplicate_package",
        "nonworkspace",
        "foreign_workspace",
        "foreign_metadata_source",
        "malformed_target",
        "missing_split_contract",
        "malformed_json",
        "duplicate_json_key",
    ],
)
def test_abi_dependency_evidence_fails_closed(tmp_path, mutation):
    import json

    metadata, artifact = abi_compiler_fixture(
        tmp_path, ["allocation-observer", "l7-attestation-core"]
    )
    records = [artifact]
    package = metadata["packages"][0]
    policy = "allocation-observer"
    if mutation == "missing":
        records.clear()
    elif mutation == "duplicate":
        records.append(deepcopy(artifact))
    elif mutation == "foreign_package":
        artifact["package_id"] = "registry+foreign#molt-lang-cpython-abi@0.1.0"
    elif mutation == "foreign_manifest":
        artifact["manifest_path"] = str(tmp_path / "foreign/Cargo.toml")
    elif mutation == "foreign_source":
        artifact["target"]["src_path"] = str(tmp_path / "foreign/lib.rs")
    elif mutation == "foreign_target":
        artifact["target"]["kind"] = ["bin"]
    elif mutation == "test_image":
        artifact["profile"]["test"] = True
    elif mutation == "missing_features":
        del artifact["features"]
    elif mutation == "malformed_features":
        artifact["features"] = [False]
    elif mutation == "duplicate_features":
        artifact["features"].append("allocation-observer")
    elif mutation == "unknown_feature":
        artifact["features"].append("undeclared")
    elif mutation == "no_rlib":
        artifact["filenames"] = [str(tmp_path / "target/abi.rmeta")]
    elif mutation == "relative_output":
        artifact["filenames"] = ["target/abi.rlib"]
    elif mutation == "missing_core":
        artifact["features"] = ["allocation-observer"]
    elif mutation == "missing_observer":
        artifact["features"] = ["l7-attestation-core"]
    elif mutation == "timing_observer":
        policy = "semantic"
    elif mutation == "missing_package":
        metadata["packages"].clear()
    elif mutation == "duplicate_package":
        metadata["packages"].append(deepcopy(package))
    elif mutation == "nonworkspace":
        metadata["workspace_members"].clear()
    elif mutation == "foreign_workspace":
        metadata["workspace_root"] = str(tmp_path / "foreign")
    elif mutation == "foreign_metadata_source":
        package["source"] = "registry+foreign"
    elif mutation == "malformed_target":
        package["targets"][0]["crate_types"] = None
    elif mutation == "missing_split_contract":
        del package["features"]["allocation-observer"]
    output = "\n".join(json.dumps(item) for item in records)
    if mutation == "malformed_json":
        output += '\n{"reason":'
    elif mutation == "duplicate_json_key":
        output = output.replace('"features":', '"features": [], "features":')
    with pytest.raises(RuntimeError):
        gate.observed_abi_dependency(
            output, json.dumps(metadata), policy=policy, source_root=tmp_path
        )


@pytest.mark.parametrize(
    "variable,value",
    [
        ("RUSTFLAGS", "-C debug-assertions=yes"),
        ("RUSTFLAGS", "-Cdebug-assertions"),
        ("RUSTFLAGS", "-Cdebug-assertions=on"),
        ("CARGO_ENCODED_RUSTFLAGS", "-C\x1fdebug-assertions=true"),
        ("CARGO_PROFILE_RELEASE_FAST_DEBUG_ASSERTIONS", "true"),
        ("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "true"),
    ],
)
def test_flags_cannot_override_disabled_assertions_claim(variable, value):
    artifact, _, output = observed_fixture()
    with pytest.raises(RuntimeError):
        gate.observed_build(output, artifact, "release-fast", {variable: value})


@pytest.mark.parametrize("nonobject", ["[]", "null", "123", '"status"'])
def test_nonobject_compiler_evidence_fails_with_diagnostic(nonobject):
    artifact, _, output = observed_fixture()
    with pytest.raises(RuntimeError, match="must contain JSON objects"):
        gate.observed_build(output + "\n" + nonobject, artifact, "release-fast", {})


def test_false_observed_profile_cannot_be_replaced_by_profile_label():
    import json

    artifact, payload, _ = observed_fixture()
    payload["profile"]["debug_assertions"] = True
    with pytest.raises(RuntimeError):
        gate.observed_build(json.dumps(payload), artifact, "release-fast", {})


@pytest.mark.parametrize(
    "mutation", ["missing_assertions", "missing_features", "duplicate_artifact"]
)
def test_missing_or_ambiguous_compiler_evidence_fails_closed(mutation):
    import json

    artifact, payload, output = observed_fixture()
    if mutation == "missing_assertions":
        del payload["profile"]["debug_assertions"]
        output = json.dumps(payload)
    elif mutation == "missing_features":
        del payload["features"]
        output = json.dumps(payload)
    else:
        output += "\n" + output
    with pytest.raises(RuntimeError):
        gate.observed_build(output, artifact, "release-fast", {})


@pytest.mark.parametrize(
    "variable,value",
    [
        ("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS", "-Cdebug-assertions=yes"),
        ("RUSTFLAGS", "--cfg debug_assertions"),
    ],
)
def test_target_flags_and_explicit_cfg_do_not_escape_profile_witness(variable, value):
    artifact, _, output = observed_fixture()
    with pytest.raises(RuntimeError):
        gate.observed_build(output, artifact, "release-fast", {variable: value})


def test_shipping_and_iteration_roles_are_explicit():
    artifact, _, output = observed_fixture()
    assert (
        gate.observed_build(output, artifact, "release-output", {})["profile_role"]
        == "shipping"
    )
    assert (
        gate.observed_build(output, artifact, "release-fast", {})["profile_role"]
        == "iteration"
    )
    assert (
        gate.observed_build(output, artifact, "dev-fast", {})["profile_role"]
        == "custom"
    )


@pytest.mark.parametrize(
    "environment",
    [
        {"RUSTFLAGS": "-Cdebug-assertions=yes"},
        {"CARGO_PROFILE_RELEASE_OUTPUT_DEBUG_ASSERTIONS": "true"},
        {"CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS": "true"},
    ],
)
def test_shipping_profile_rejects_assertion_overrides(environment):
    artifact, _, output = observed_fixture()
    with pytest.raises(RuntimeError):
        gate.observed_build(output, artifact, "release-output", environment)


@pytest.mark.parametrize("output_owner", ["explicit", "queue", "proof-plan"])
def test_default_profile_is_shipping_before_build(
    monkeypatch, tmp_path, output_owner, selected_cargo
):
    # Observe typed build argv without compiling or publishing a synthetic pass.
    seen = []
    source = tmp_path / "source"
    source.mkdir()
    scratch = tmp_path / "queue-scratch"
    monkeypatch.setattr(gate, "ROOT", source)
    monkeypatch.delenv("MOLT_PROOF_SCRATCH_ROOT", raising=False)
    argv = []
    if output_owner == "explicit":
        monkeypatch.setenv("MOLT_PROOF_SCRATCH_ROOT", str(scratch))
        expected_root = tmp_path / "explicit-receipts"
        argv = ["--receipt-root", str(expected_root)]
    elif output_owner == "queue":
        monkeypatch.setenv("MOLT_PROOF_SCRATCH_ROOT", str(scratch))
        expected_root = scratch / "runtime-gate-runs"
    else:
        expected_root = source / "proof-receipts/evidence/runtime-gate-runs"
    monkeypatch.setattr(
        gate.truth, "git_source_identity", lambda: {"schema": "molt.git-source.v1"}
    )

    class Commands:
        def run(self, argv, **kwargs):
            from types import SimpleNamespace

            assert argv[0] == selected_cargo
            if argv[:2] == [selected_cargo, "metadata"]:
                return SimpleNamespace(
                    stdout='{"packages":[]}', stderr="", returncode=0
                )
            seen.append(argv)
            return SimpleNamespace(
                stdout="", stderr="intentional stopped build", returncode=1
            )

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    assert gate.main(argv) == 1
    command = seen[0]
    assert command[command.index("--profile") + 1] == "release-output"
    runs = list(expected_root.iterdir())
    assert len(runs) == 1
    assert {path.name for path in runs[0].iterdir()} == {
        "aggregate.json",
        "build.stdout.jsonl",
        "build.stderr.log",
        "cargo.metadata.json",
    }
    if output_owner != "proof-plan":
        assert list(source.iterdir()) == []
    if output_owner == "explicit":
        assert not scratch.exists()


def test_build_rustflags_cannot_override_shipping_assertions():
    artifact, _, output = observed_fixture()
    with pytest.raises(RuntimeError):
        gate.observed_build(
            output,
            artifact,
            "release-output",
            {"CARGO_BUILD_RUSTFLAGS": "-Cdebug-assertions=yes"},
        )


def test_child_failure_summary_names_failing_tests_and_abnormal_exits():
    receipts = [
        {
            "failure_identities": ["object::tests::b", "attr::tests::a"],
            "baseline_termination": {"kind": "exit", "returncode": 101},
        },
        {
            "reported_failures": ["attr::tests::a"],
            "baseline_termination": {
                "kind": "signal",
                "name": "SIGABRT",
                "returncode": -6,
                "signal": 6,
            },
        },
    ]
    stderr = "\n".join(f"line {index}" for index in range(60))

    summary = gate.child_failure_summary("parallel", 101, receipts, stderr)

    lines = summary.splitlines()
    assert lines[0] == "runtime-gate: child parallel failed with exit code 101"
    assert lines[1:4] == [
        "  2 failing test(s):",
        "    attr::tests::a",
        "    object::tests::b",
    ]
    assert '"name": "SIGABRT"' in lines[4]
    assert lines[5] == "  driver stderr (last 40 lines):"
    assert lines[6] == "    line 20" and lines[-1] == "    line 59"


def test_child_failure_summary_bounds_long_failure_lists():
    receipts = [{"failure_identities": [f"t::{index:03}" for index in range(45)]}]

    lines = gate.child_failure_summary("fresh", 1, receipts, "").splitlines()

    assert lines[1] == "  45 failing test(s):"
    assert lines[2] == "    t::000" and lines[41] == "    t::039"
    assert lines[42] == "    ... 5 more in the receipts"
    assert len(lines) == 43


FRESH_OWNERS = (
    "async_rt::sockets::state::socket_runtime_state_tests::cold_socket_and_websocket_retirement_never_starts_services",
    "cpython_abi_hooks::pending_cleanup_tests::canonical_cache_allocation_failure_reentry_and_ascii_no_retry",
    "cpython_abi_hooks::immortal_ownership_tests::immortal_c_views_preserve_runtime_roots_and_gc",
    "state::lifecycle::shutdown_tests::embedding_shutdown_collects_native_type_cycles_across_module_retirement",
)
DEVELOPMENT_PROBES = (
    "builtins::functools::tests::lru_cache_cost_probe",
    "c_api::tests::dictionary_snapshot_materialization_components",
    "cpython_abi_hooks::generic_attributes_tests::mutation_consumers::managed_mutation_projection_profile",
)


@pytest.mark.parametrize("platform", ["linux", "darwin", "win32"])
def test_ignored_runtime_inventory_has_explicit_semantic_and_profile_owners(platform):
    # Names are independent observations of Rust #[ignore] contracts, not a
    # table copied from the selection under test. Missing declarations fail.
    inventory = sorted(
        set(
            gate.ISOLATED
            + gate.probes_for(platform)
            + FRESH_OWNERS
            + DEVELOPMENT_PROBES
            + ("ordinary",)
        )
    )
    selected = gate.selections(inventory, 4, platform)
    assert selected["parallel"][1] == {"ordinary"}
    for name in FRESH_OWNERS:
        assert selected[name] == (
            ["--exact", name, "--ignored", "--test-threads=1", "--nocapture"],
            {name},
        )
    for name in DEVELOPMENT_PROBES:
        assert name in gate.probes_for(platform)
        assert name not in selected
        assert all(name not in tests for _argv, tests in selected.values())


@pytest.mark.parametrize("name", FRESH_OWNERS)
def test_new_fresh_semantic_owner_cannot_remain_ignored(tmp_path, name):
    children, selected, binding = fixture_ledger(tmp_path)
    children[name][0]["test_results"][0]["status"] = "ignored"
    with pytest.raises(RuntimeError):
        gate.validate_children(children, selected, **binding)


# Independent required controls from their Rust owners, not a test-count quota.
OBSERVER_WITNESSES = {
    "molt_runtime": {
        "builtins::frames::namespace_tests::task_growth_refusal::both_task_producers_refuse_actual_late_map_growth_and_restore_owners",
        "call::bind::inline_cache::binding_tests::direct_call_ic_hot_path_is_allocation_free_with_slow_path_control",
        "cpython_abi_hooks::tests::cpython_abi_gil_custody_is_allocation_free_after_warmup",
        "object::gc::tests::repeated_reachable_collection_workspace_allocations",
        "cpython_abi_hooks::exception_rendering_tests::decimal_output_limits::rejected_decimal_magnitude_does_not_allocate_decimal_storage",
        "object::ops::unpack_transaction_tests::exact_builtin_unpack_hot_path_performs_zero_allocator_calls",
        "state::runtime_state::tests::encoded_core_gil_entry_is_allocation_free_after_warmup",
    },
    "l7_numeric_runtime_perf_attestation": {
        "integer_byte_encoder_runs_with_allocation_denied_after_preparation",
        "prepared_memory_error_transport_and_reuse_attempt_no_rust_or_platform_c_allocation",
    },
}


@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "missing_case",
        "missing_image",
        "wrong_target",
        "missing_feature",
        "ignored",
        "required_pass",
        "same_process",
        "changed_image",
        "abi_changed_after",
        "changed_source",
        "first_image_failure",
        "missing_abi",
        "foreign_abi",
        "duplicate_abi",
        "uninstrumented_abi",
        "abi_missing_core",
        "malformed_abi_features",
    ],
)
def test_observer_builds_once_and_requires_isolated_source_bound_controls(
    monkeypatch, tmp_path, mutation, selected_cargo
):
    import json
    from types import SimpleNamespace

    # Real capture files and the existing receipt validators are used; Cargo
    # and libtest launch themselves are fake, so this test cannot build Rust.
    images = {name: support.make_image(tmp_path / name) for name in OBSERVER_WITNESSES}
    package_id = "path+file:///runtime#molt-runtime@0.1.0"
    metadata, abi_artifact = abi_compiler_fixture(
        gate.ROOT, ["allocation-observer", "l7-attestation-core"]
    )
    metadata["packages"].append(
        {"id": package_id, "name": "molt-runtime", "version": "0.1.0"}
    )
    artifacts = [
        {
            "reason": "compiler-artifact",
            "package_id": package_id,
            "target": {
                "name": name,
                "kind": ["rlib" if name == "molt_runtime" else "test"],
            },
            "profile": {"test": True, "debug_assertions": False, "opt_level": "3"},
            "features": ["l7-attestation-probe", "stdlib_full"],
            "executable": str(binary),
        }
        for name, binary in images.items()
    ]
    if mutation == "missing_image":
        artifacts.pop()
    elif mutation == "wrong_target":
        artifacts[-1]["target"]["kind"] = ["bin"]
    elif mutation == "missing_feature":
        artifacts[-1]["features"] = ["stdlib_full"]
    if mutation != "missing_abi":
        artifacts.append(abi_artifact)
    if mutation == "foreign_abi":
        abi_artifact["package_id"] = "registry+foreign#molt-lang-cpython-abi@0.1.0"
    elif mutation == "duplicate_abi":
        artifacts.append(deepcopy(abi_artifact))
    elif mutation == "uninstrumented_abi":
        abi_artifact["features"] = ["l7-attestation-core"]
    elif mutation == "abi_missing_core":
        abi_artifact["features"] = ["allocation-observer"]
    elif mutation == "malformed_abi_features":
        abi_artifact["features"] = "allocation-observer"
    inventories = {
        name: [*sorted(required), "ordinary_not_selected", "timed_probe_not_selected"]
        for name, required in OBSERVER_WITNESSES.items()
    }
    if mutation == "missing_case":
        inventories["l7_numeric_runtime_perf_attestation"].pop(0)
    source_reads = 0

    def source_identity():
        nonlocal source_reads
        source_reads += 1
        if mutation == "changed_source" and source_reads > 2:
            return dict(support.SOURCE, head="changed")
        return support.SOURCE

    monkeypatch.setattr(gate.truth, "git_source_identity", source_identity)
    discoveries = []

    def discover(binary, inherited_args, **kwargs):
        name = next(name for name, path in images.items() if str(path) == binary)
        discoveries.append(name)
        return inventories[name], SimpleNamespace(
            succeeded=True, receipt=lambda: {"argv": [binary, "--list"]}
        )

    monkeypatch.setattr(gate.runner, "listed_tests", discover)
    builds = []
    children = []

    class Commands:
        def run(self, argv, **kwargs):
            result = SimpleNamespace(stdout="", stderr="", returncode=0)
            if argv[:2] == [selected_cargo, "metadata"]:
                result.stdout = json.dumps(metadata)
            elif argv[:2] == [selected_cargo, "test"]:
                builds.append(argv)
                result.stdout = "\n".join(json.dumps(item) for item in artifacts)
            elif argv[0] in {selected_cargo, "rustc"}:
                result.stdout = "fixture-toolchain"
            else:
                # Both inventories must be admitted before any test starts.
                assert set(discoveries) == set(OBSERVER_WITNESSES)
                split = argv.index("--")
                binary = Path(argv[split + 1])
                args = argv[split + 2 :]
                name = args[args.index("--exact") + 1]
                assert args == [
                    "--exact",
                    name,
                    "--include-ignored",
                    "--test-threads=1",
                    "--nocapture",
                ]
                assert argv[argv.index("--require-passed-test") + 1] == name
                directory = Path(argv[argv.index("--receipt-dir") + 1])
                receipt = support.binary_receipt(
                    directory,
                    binary,
                    [name],
                    [],
                    args=args,
                    retained_root=Path(argv[argv.index("--retained-root") + 1]),
                    run_id=argv[argv.index("--run-id") + 1],
                    invocation=(
                        f"child-{len(children) - 1}"
                        if mutation == "same_process"
                        and binary == images["l7_numeric_runtime_perf_attestation"]
                        else f"child-{len(children)}"
                    ),
                    statuses=(
                        {name: "ignored"}
                        if mutation == "ignored"
                        else {name: "fail"}
                        if mutation == "first_image_failure"
                        and binary == images["molt_runtime"]
                        else None
                    ),
                )
                if (
                    mutation == "first_image_failure"
                    and binary == images["molt_runtime"]
                ):
                    result.returncode = 1
                receipt["required_passed_tests"] = (
                    [] if mutation == "required_pass" else [name]
                )
                gate.runner.write_receipt(directory / "receipt.json", receipt)
                children.append((binary, name))
                if mutation == "changed_image":
                    binary.write_bytes(b"replacement-image")
                if mutation == "abi_changed_after":
                    Path(abi_artifact["filenames"][-1]).write_bytes(b"replacement ABI")
            return result

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    root = tmp_path / "receipts"
    code = gate.main(["--policy", "allocation-observer", "--receipt-root", str(root)])
    receipt = json.loads(
        next(root.glob("*/aggregate.json")).read_text(encoding="utf-8")
    )
    assert len(builds) == 1
    command = builds[0]
    assert "--lib" in command and "--no-run" in command
    assert command[command.index("--test") + 1] == "l7_numeric_runtime_perf_attestation"
    assert command[command.index("--features") + 1] == "l7-attestation-probe"
    if mutation:
        assert code == 1 and receipt["status"] == "failed"
        if mutation in {
            "missing_case",
            "missing_image",
            "wrong_target",
            "missing_feature",
            "missing_abi",
            "foreign_abi",
            "duplicate_abi",
            "uninstrumented_abi",
            "abi_missing_core",
            "malformed_abi_features",
        }:
            assert children == []
        if mutation in {
            "missing_feature",
            "missing_abi",
            "foreign_abi",
            "duplicate_abi",
            "uninstrumented_abi",
            "abi_missing_core",
            "malformed_abi_features",
        }:
            assert discoveries == []
        if mutation == "first_image_failure":
            assert "verification_error" in receipt["images"]["molt_runtime"]
            assert "passed_test_count" not in receipt["images"]["molt_runtime"]
            assert {
                name
                for binary, name in children
                if binary == images["l7_numeric_runtime_perf_attestation"]
            } == OBSERVER_WITNESSES["l7_numeric_runtime_perf_attestation"]
            assert (
                "verification_error"
                not in receipt["images"]["l7_numeric_runtime_perf_attestation"]
            )
        return
    assert code == 0 and receipt["status"] == "success"
    assert receipt["schema"] == "molt.runtime-test-gate.v2"
    assert receipt["policy"] == "allocation-observer"
    assert receipt["abi_dependency"]["features"] == [
        "allocation-observer",
        "l7-attestation-core",
    ]
    run = next(root.iterdir())
    assert receipt["abi_dependency"]["metadata_sha256"] == support.sha256(
        (run / "cargo.metadata.json").read_bytes()
    )
    assert receipt["abi_dependency"]["compiler_stdout_sha256"] == support.sha256(
        (run / "build.stdout.jsonl").read_bytes()
    )
    assert set(receipt["images"]) == set(OBSERVER_WITNESSES)
    for target, required in OBSERVER_WITNESSES.items():
        image = receipt["images"][target]
        assert set(image["children"]) == required
        assert {
            name for binary, name in children if binary == images[target]
        } == required
        assert len(
            [name for binary, name in children if binary == images[target]]
        ) == len(required)
        assert {row["identity"] for row in image["unrun"]} == {
            "ordinary_not_selected",
            "timed_probe_not_selected",
        }
        assert all(row["status"] == "unrun" for row in image["unrun"])
        assert image["build"]["artifact"]["target_name"] == target
    assert receipt["passed_test_count"] == sum(map(len, OBSERVER_WITNESSES.values()))


def test_observer_discovery_rejects_duplicate_case():
    names = sorted(OBSERVER_WITNESSES["molt_runtime"])
    with pytest.raises(RuntimeError, match="duplicate"):
        gate.observer_selections([*names, names[0]], "molt_runtime")


def test_feature_only_performance_probes_are_unrun_without_changing_default_policy():
    for name in (
        "builtins::module_table::tests::import_catalog_perf_attestation",
        "object::gc::tests::cycle_capable_registry_hot_path_bench",
    ):
        assert name not in gate.probes_for("linux")
        assert (
            gate.unrun_reason("allocation-observer", "molt_runtime", name, "linux")
            == "performance probe"
        )
    assert (
        gate.unrun_reason(
            "allocation-observer",
            "l7_numeric_runtime_perf_attestation",
            "l7_numeric_runtime_performance_attestation",
            "linux",
        )
        == "performance probe"
    )


@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "missing",
        "duplicate",
        "foreign",
        "wrong_target",
        "not_test",
        "not_wasm",
        "producer_failure",
        "changed_before",
        "changed_after",
        "source",
        "ignored",
        "zero",
        "wrong_case",
        "wrong_required",
        "child_failure",
        "host_missing",
        "host_duplicate",
        "host_foreign",
        "host_wrong_kind",
        "host_changed_before",
        "host_changed_after",
        "abi_changed_after",
        "instrumented_abi",
    ],
)
def test_wasi_policy_hands_exact_compiler_image_to_existing_consumer(
    monkeypatch, tmp_path, mutation, selected_cargo
):
    import json
    from types import SimpleNamespace
    from molt.cli import runtime_cargo_plan

    wasm = tmp_path / "molt_runtime.wasm"
    wasm.write_bytes(b"\x00asm\x01\x00\x00\x00fixture")
    native = support.make_image(tmp_path / "host")
    production_host = support.make_image(tmp_path / "molt-wasm-host")
    host = "aarch64-unknown-linux-gnu"
    metadata, abi_artifact = abi_compiler_fixture(gate.ROOT, ["l7-attestation-core"])
    metadata["packages"].extend(
        [
            {
                "id": name,
                "name": name,
                "version": "0.1.0",
                "source": None,
                "manifest_path": str(gate.ROOT / "runtime" / name / "Cargo.toml"),
            }
            for name in ("molt-runtime", "molt-wasm-host")
        ]
    )
    records = {}
    for package, name, kind, image in (
        ("molt-runtime", "molt_runtime", "rlib", wasm),
        ("molt-wasm-host", "wasi_runtime_lifecycle", "test", native),
    ):
        records[package] = [
            {
                "reason": "compiler-artifact",
                "package_id": package,
                "target": {"name": name, "kind": [kind]},
                "profile": {"test": True, "debug_assertions": True, "opt_level": "0"},
                "features": ["stdlib_micro"] if package == "molt-runtime" else [],
                "executable": str(image),
            }
        ]
    host_record = {
        "reason": "compiler-artifact",
        "package_id": "molt-wasm-host",
        "target": {"name": "molt-wasm-host", "kind": ["bin"]},
        "profile": {"test": False, "debug_assertions": True, "opt_level": "0"},
        "features": [],
        "executable": str(production_host),
    }
    if mutation != "host_missing":
        records["molt-wasm-host"].append(host_record)
    if mutation == "host_duplicate":
        records["molt-wasm-host"].append(deepcopy(host_record))
    elif mutation == "host_foreign":
        host_record["package_id"] = "molt-runtime"
    elif mutation == "host_wrong_kind":
        host_record["target"]["kind"] = ["rlib"]
    emitted = records["molt-runtime"]
    if mutation == "missing":
        emitted.clear()
    elif mutation == "duplicate":
        emitted.append(deepcopy(emitted[0]))
    elif mutation == "foreign":
        emitted[0]["package_id"] = "molt-wasm-host"
    elif mutation == "wrong_target":
        emitted[0]["target"]["kind"] = ["bin"]
    elif mutation == "not_test":
        emitted[0]["profile"]["test"] = False
    elif mutation == "not_wasm":
        wasm.write_bytes(b"native executable")
    if mutation == "instrumented_abi":
        abi_artifact["features"].append("allocation-observer")
    emitted.append(abi_artifact)
    original_sha = support.sha256(wasm.read_bytes())
    original_host_sha = support.sha256(production_host.read_bytes())
    monkeypatch.setattr(gate.truth, "host_target", lambda: host)
    source_reads = 0

    def source_identity():
        nonlocal source_reads
        source_reads += 1
        if mutation == "host_changed_before" and source_reads == 2:
            production_host.write_bytes(b"changed")
        return (
            dict(support.SOURCE, head="changed")
            if mutation == "source" and source_reads > 1
            else support.SOURCE
        )

    monkeypatch.setattr(gate.truth, "git_source_identity", source_identity)
    monkeypatch.setattr(
        gate.runner,
        "listed_tests",
        lambda *a, **kw: pytest.fail(
            "WASI policy must not execute native inventory on the module"
        ),
    )
    resolved = []

    def plan(
        root, *, env, cargo_command, requested_target, host_target, rustflags_transform
    ):
        # Exercise the real planner's executable-selection boundary, while its
        # SDK/configuration/compiler work remains outside this host unit test.
        selected = runtime_cargo_plan._tool_path(
            cargo_command[0], root=root, env=env, role="cargo"
        )
        assert selected == Path(selected_cargo)
        assert (
            runtime_cargo_plan.resolve_rustup_proxy(
                selected, role="cargo", root=root, env=env
            )
            == selected
        )
        assert (
            root == gate.ROOT
            and requested_target == "wasm32-wasip1"
            and host_target == host
        )
        # Real flag selection is separately owned; this boundary must preserve
        # already selected SDK/linker flags while adding table export only here.
        flags = (
            "-C",
            "linker=/selected/sdk/wasm-ld",
            "-C",
            "target-feature=-reference-types",
        )
        transformed = rustflags_transform(flags)
        assert transformed == (*flags, "-C", "link-arg=--export-table")
        resolved.append(cargo_command)
        return SimpleNamespace(
            command=cargo_command,
            environment=dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(transformed)),
            rustflags=transformed,
            verify=lambda: None,
        )

    monkeypatch.setattr(runtime_cargo_plan, "resolve_runtime_cargo_plan", plan)
    builds, children = [], []

    class Commands:
        def run(self, argv, **kwargs):
            result = SimpleNamespace(stdout="", stderr="", returncode=0)
            if argv[:2] == [selected_cargo, "metadata"]:
                result.stdout = json.dumps(metadata)
            elif argv[:2] == [selected_cargo, "test"]:
                package = argv[argv.index("-p") + 1]
                builds.append((argv, kwargs["env"]))
                result.stdout = "\n".join(json.dumps(item) for item in records[package])
                if mutation == "producer_failure" and package == "molt-runtime":
                    result.returncode = 9
                if mutation == "changed_before" and package == "molt-wasm-host":
                    wasm.write_bytes(b"changed")
            else:
                children.append(argv)
                assert kwargs["env"]["MOLT_WASI_LIBTEST_ARTIFACT"] == str(wasm)
                assert kwargs["env"]["MOLT_WASI_LIBTEST_SHA256"] == original_sha
                split = argv.index("--")
                assert argv[split + 1] == str(native)
                args = argv[split + 2 :]
                assert args == [
                    "--exact",
                    gate.WASI_CONSUMER,
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ]
                assert (
                    argv[argv.index("--require-passed-test") + 1] == gate.WASI_CONSUMER
                )
                directory = Path(argv[argv.index("--receipt-dir") + 1])
                names = (
                    []
                    if mutation == "zero"
                    else ["different"]
                    if mutation == "wrong_case"
                    else [gate.WASI_CONSUMER]
                )
                receipt = support.binary_receipt(
                    directory,
                    native,
                    names,
                    [],
                    args=args,
                    retained_root=Path(argv[argv.index("--retained-root") + 1]),
                    run_id=argv[argv.index("--run-id") + 1],
                    invocation="wasi-consumer",
                    statuses={gate.WASI_CONSUMER: "ignored"}
                    if mutation == "ignored"
                    else None,
                )
                receipt["required_passed_tests"] = (
                    [] if mutation == "wrong_required" else [gate.WASI_CONSUMER]
                )
                gate.runner.write_receipt(directory / "receipt.json", receipt)
                if mutation == "changed_after":
                    wasm.write_bytes(b"changed")
                if mutation == "host_changed_after":
                    production_host.write_bytes(b"changed")
                if mutation == "abi_changed_after":
                    Path(abi_artifact["filenames"][-1]).write_bytes(b"replacement ABI")
                if mutation == "child_failure":
                    result.returncode = 1
            return result

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    root = tmp_path / "receipts"
    code = gate.main(
        [
            "--policy",
            "wasi-lifecycle",
            "--profile",
            "dev-fast",
            "--receipt-root",
            str(root),
        ]
    )
    aggregate = json.loads(
        next(root.glob("*/aggregate.json")).read_text(encoding="utf-8")
    )
    assert len(resolved) == 1
    assert "--lib" in builds[0][0] and "--no-default-features" in builds[0][0]
    assert builds[0][0][builds[0][0].index("--features") + 1] == "stdlib_micro"
    if len(builds) == 2:
        assert builds[1][0][builds[1][0].index("--target") + 1] == host
        assert "link-arg=--export-table" not in builds[1][1].get(
            "CARGO_ENCODED_RUSTFLAGS", ""
        )
    if mutation:
        assert code == 1 and aggregate["status"] == "failed"
        assert "passed_test_count" not in aggregate
        if mutation in {
            "missing",
            "duplicate",
            "foreign",
            "wrong_target",
            "not_test",
            "not_wasm",
            "producer_failure",
            "changed_before",
            "source",
            "host_missing",
            "host_duplicate",
            "host_foreign",
            "host_wrong_kind",
            "host_changed_before",
            "instrumented_abi",
        }:
            assert not children
    else:
        assert code == 0 and aggregate["status"] == "success"
        assert len(builds) == 2 and len(children) == 1
        assert aggregate["passed_test_count"] == 1
        assert aggregate["wasi_handoff"]["MOLT_WASI_LIBTEST_SHA256"] == original_sha
        assert (
            aggregate["images"]["molt_runtime"]["build"]["executable_sha256"]
            == original_sha
        )
        captured_host = aggregate["images"]["wasi_runtime_lifecycle"]["production_host"]
        assert captured_host["executable_sha256"] == original_host_sha
        assert captured_host["artifact"]["executable"] == str(production_host)
        assert aggregate["abi_dependency"]["features"] == ["l7-attestation-core"]
    if mutation == "instrumented_abi":
        assert len(builds) == 1
        assert (
            "ABI allocation features contradict semantic policy" in aggregate["error"]
        )
