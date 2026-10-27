from __future__ import annotations

from copy import deepcopy
import os
from pathlib import Path

import pytest

from molt.cli import cargo_capabilities

from tests import runtime_descendant_test_support as support
from tools import run_runtime_test_gate as gate

# Cargo runs in this file are fakes; build capacity is not under test.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")

# The actual runtime remainder contains the trap and cold-denial owners (and
# the trap's child test); the process-exit owner is a mandatory isolated case.
REMAINDER = ["ordinary::parallel_case", support.TRAP, support.TRAP_CHILD, support.COLD]


@pytest.fixture
def selected_cargo(monkeypatch, tmp_path):
    from molt.cli.runtime_tool_selection import runtime_tool_path
    from molt.rust_toolchain import rustup_proxy_selection

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
    assert (
        runtime_tool_path("cargo", root=tmp_path, env=os.environ, role="cargo") == proxy
    )
    selection = rustup_proxy_selection(proxy)
    assert selection is not None and selection.entrypoint == rustup
    return str(selected)


def fixture_ledger(tmp_path):
    binary = support.make_image(tmp_path)
    inventory = [*gate.isolated_for("linux"), *gate.probes_for("linux"), *REMAINDER]
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
        **{name: None for name in gate.isolated_for("linux")},
        support.EXIT: 2,
    }
    assert "--test-threads=4" in selections["parallel"][0]
    for name in gate.isolated_for("linux"):
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
    inventory = [*gate.isolated_for("linux"), *gate.probes_for("linux"), "ordinary"]
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
    darwin_inventory = [
        *gate.isolated_for("darwin"),
        *gate.probes_for("darwin"),
        "ordinary",
    ]
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


def observed_fixture(*, checked=False):
    import json

    artifact = {"executable": str(Path("runtime.exe").resolve())}
    payload = {
        "reason": "compiler-artifact",
        "executable": artifact["executable"],
        "profile": {
            "test": True,
            "debug_assertions": checked,
            "opt_level": "3",
            "debuginfo": 0,
            "overflow_checks": checked,
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
    # A correctness profile carries its observed checks, not just its label.
    checked_artifact, _, checked_output = observed_fixture(checked=True)
    assert (
        gate.observed_build(checked_output, checked_artifact, "dev-fast", {})[
            "requested_profile"
        ]
        == "dev-fast"
    )


@pytest.mark.parametrize("profile", ["dev-fast", "release-fast", "release-output"])
@pytest.mark.parametrize("field", ["debug_assertions", "overflow_checks"])
@pytest.mark.parametrize("value", [None, "true", 1, "opposite"])
def test_named_profile_requires_observed_check_evidence(profile, field, value):
    import json

    artifact, payload, _ = observed_fixture(checked=profile == "dev-fast")
    if value is None:
        del payload["profile"][field]
    else:
        payload["profile"][field] = (
            profile != "dev-fast" if value == "opposite" else value
        )
    with pytest.raises(RuntimeError):
        gate.observed_build(json.dumps(payload), artifact, profile, {})


@pytest.mark.parametrize("profile", ["dev-fast", "release-fast", "release-output"])
@pytest.mark.parametrize("field", ["debug-assertions", "overflow-checks"])
@pytest.mark.parametrize(
    "variable,template",
    [
        ("RUSTFLAGS", "-C{field}={value}"),
        ("CARGO_ENCODED_RUSTFLAGS", "-C\x1f{field}={value}"),
        ("CARGO_BUILD_RUSTFLAGS", "--codegen={field}={value}"),
        ("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS", "--codegen {field}={value}"),
    ],
)
def test_named_profile_rejects_contradictory_and_unknown_check_flags(
    profile, field, variable, template
):
    checked = profile == "dev-fast"
    artifact, _, output = observed_fixture(checked=checked)
    for value in ("no" if checked else "yes", "unknown", ""):
        flags = template.format(field=field.replace("-", "_"), value=value)
        environment = gate.build_environment({variable: flags})
        assert environment[variable] == flags
        with pytest.raises(RuntimeError):
            gate.observed_build(output, artifact, profile, environment)
    flags = template.format(field=field, value="yes" if checked else "no")
    assert gate.observed_build(output, artifact, profile, {variable: flags})


@pytest.mark.parametrize("profile", ["dev-fast", "release-fast", "release-output"])
@pytest.mark.parametrize("field", ["DEBUG_ASSERTIONS", "OVERFLOW_CHECKS"])
@pytest.mark.parametrize("inherited", [False, True])
def test_named_profile_checks_cargo_selectors_and_retains_them(
    profile, field, inherited
):
    checked = profile == "dev-fast"
    selector = (
        ("DEV" if checked else "RELEASE")
        if inherited
        else profile.upper().replace("-", "_")
    )
    variable = f"CARGO_PROFILE_{selector}_{field}"
    artifact, _, output = observed_fixture(checked=checked)
    for value in ("false" if checked else "true", "unknown", ""):
        environment = gate.build_environment({variable: value})
        assert environment[variable] == value
        with pytest.raises(RuntimeError):
            gate.observed_build(output, artifact, profile, environment)
    environment = gate.build_environment({variable: "true" if checked else "false"})
    assert gate.observed_build(output, artifact, profile, environment)


def abi_compiler_fixture(
    source_root: Path, features: list[str], *, artifact_root: Path | None = None
):
    # Cargo metadata and compiler-artifact are independent structured inputs.
    # The library carries no executable, so executable-only discovery misses it.
    # Identity names the source root; the library files live in the test's own
    # directory, never in a checkout's target/.
    library_dir = (artifact_root or source_root) / "target" / "deps"
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
            str(library_dir / "libmolt_cpython_abi-fixture.rlib"),
            str(library_dir / "libmolt_cpython_abi-fixture.dylib"),
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


def test_shipping_iteration_and_correctness_roles_are_explicit():
    import json

    artifact, _, output = observed_fixture()
    assert (
        gate.observed_build(output, artifact, "release-output", {})["profile_role"]
        == "shipping"
    )
    assert (
        gate.observed_build(output, artifact, "release-fast", {})["profile_role"]
        == "iteration"
    )
    checked_artifact, _, checked_output = observed_fixture(checked=True)
    assert (
        gate.observed_build(checked_output, checked_artifact, "dev-fast", {})[
            "profile_role"
        ]
        == "correctness"
    )
    custom = gate.observed_build(output, artifact, "custom-profile", {})
    assert custom["profile_role"] == "custom"
    assert custom["requested_profile"] == "custom-profile"
    _, payload, _ = observed_fixture()
    del payload["profile"]["overflow_checks"]
    assert (
        gate.observed_build(
            json.dumps(payload), artifact, "custom-profile", {"RUSTFLAGS": "-C"}
        )["profile_role"]
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


@pytest.mark.parametrize("returncode", [0, 101])
def test_cargo_build_streams_evidence_without_truncation_or_terminal_rewrite(
    monkeypatch, tmp_path, returncode
):
    import json
    import subprocess
    from molt.process_environment import ProcessEnvironment

    _, compiler_artifact, artifact_output = observed_fixture()
    stdout = (
        artifact_output
        + "\n"
        + json.dumps(
            {
                "reason": "build-finished",
                "success": returncode == 0,
                "padding": "x" * 100_000,
            }
        )
        + "\n"
    )
    stderr = "compiler-diagnostic\n"
    command = ["cargo", "test", "--no-run", "--message-format=json"]
    environment = ProcessEnvironment({"RUSTC": "selected-rustc"})
    result = subprocess.CompletedProcess(command, returncode, stdout, stderr)
    paths = (tmp_path / "build.stdout.jsonl", tmp_path / "build.stderr.log")
    captured_times = []

    class Commands:
        def run(self, argv, **kwargs):
            assert argv is command
            assert kwargs["cwd"] == gate.ROOT
            assert kwargs["env"] is environment
            assert kwargs["capture_output"] is True
            assert kwargs["capture_tail_bytes"] is None
            assert kwargs["timeout"] == 123
            assert kwargs["check"] is False
            assert kwargs["encoding"] == "utf-8"
            assert (
                kwargs["stdout_capture_path"],
                kwargs["stderr_capture_path"],
            ) == paths
            for path, text in zip(paths, (stdout, stderr), strict=True):
                with path.open("x", encoding="utf-8") as handle:
                    handle.write(text)
                # The real capture owner creates evidence before returning.
                assert path.read_text(encoding="utf-8") == text
                # A terminal rewrite of identical text is still a new write.
                # Give it a distinct, portable timestamp so the oracle catches
                # that duplicate without timing sleeps or process-wide patches.
                os.utime(path, (946684800, 946684800))
                captured_times.append(path.stat().st_mtime_ns)
            return result

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    assert gate.cargo_build(command, tmp_path, 123, environment) is result
    assert result.stdout == stdout and result.stderr == stderr
    assert list(
        gate.truth.compiler_artifacts(result.stdout, source_context="compiler-only")
    ) == [compiler_artifact]
    assert [path.stat().st_mtime_ns for path in paths] == captured_times


def test_cargo_build_timeout_keeps_real_partial_streams(monkeypatch, tmp_path):
    import json
    import subprocess
    import sys

    from tests.process_guard_common import (
        HUNG_CHILD_SOURCE,
        hung_child_marker,
        stop_test_processes_carrying,
    )

    _, compiler_artifact, artifact_output = observed_fixture()
    stdout = (
        artifact_output + "\r\n" + json.dumps({"padding": "x" * 100_000}) + "\r\n"
    ).encode("utf-8")
    stderr = "compiler diagnostic: café\r\n".encode("utf-8")
    marker = hung_child_marker()
    command = [
        sys.executable,
        "-c",
        "import json, sys\n"
        f"sys.stdout.buffer.write({artifact_output.encode('utf-8')!r} + bytes((13, 10)))\n"
        "sys.stdout.buffer.write(json.dumps({'padding': 'x' * 100_000}).encode('utf-8') + bytes((13, 10)))\n"
        "sys.stdout.buffer.flush()\n"
        f"sys.stderr.buffer.write({stderr!r})\n"
        "sys.stderr.buffer.flush()\n" + HUNG_CHILD_SOURCE,
        marker,
    ]
    # Exercise the real receiver and process owner with a tiny Python child;
    # Cargo policy/capacity and compilation are separate proof obligations.
    monkeypatch.setattr(gate, "admit_cargo_build", lambda *args, **kwargs: None)
    try:
        with pytest.raises(subprocess.TimeoutExpired) as caught:
            gate.cargo_build(command, tmp_path, 5)
        result = caught.value.guarded_result
        assert result.timed_out and result.descendants_closed
        assert (tmp_path / "build.stdout.jsonl").read_bytes() == stdout
        assert (tmp_path / "build.stderr.log").read_bytes() == stderr
        assert caught.value.output == stdout.decode("utf-8").replace("\r\n", "\n")
        assert list(
            gate.truth.compiler_artifacts(
                caught.value.output, source_context="compiler-only"
            )
        ) == [compiler_artifact]
        assert caught.value.stderr.startswith(
            stderr.decode("utf-8").replace("\r\n", "\n")
        )
        assert "memory_guard: timeout after" in caught.value.stderr
    finally:
        stop_test_processes_carrying(marker)


@pytest.mark.parametrize("policy", ["semantic", "allocation-observer"])
@pytest.mark.parametrize(
    "output_owner", ["explicit", "queue", "default", "external", "relative-external"]
)
def test_default_profile_is_shipping_before_build(
    monkeypatch, tmp_path, output_owner, selected_cargo, policy
):
    # Observe typed build argv without compiling or publishing a synthetic pass.
    seen = []
    source = tmp_path / "molt-src"
    source.mkdir()
    scratch = tmp_path / "queue-scratch"
    external = tmp_path / "external artifacts"
    monkeypatch.setattr(gate, "ROOT", source)
    for name in (
        "MOLT_PROOF_SCRATCH_ROOT",
        "MOLT_EXT_ROOT",
        "MOLT_CI_EPHEMERAL_CUSTODY_ROOT",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOLT_SCRATCH_STORAGE", "disk")
    argv = ["--policy", policy]
    if output_owner == "explicit":
        monkeypatch.setenv("MOLT_EXT_ROOT", str(external))
        monkeypatch.setenv("MOLT_PROOF_SCRATCH_ROOT", str(scratch))
        expected_root = tmp_path / "explicit-receipts"
        argv += ["--receipt-root", str(expected_root)]
    elif output_owner == "queue":
        monkeypatch.setenv("MOLT_EXT_ROOT", str(external))
        monkeypatch.setenv("MOLT_PROOF_SCRATCH_ROOT", str(scratch))
        expected_root = scratch / "runtime-gate-runs"
    elif output_owner in {"external", "relative-external"}:
        monkeypatch.setenv(
            "MOLT_EXT_ROOT",
            str(external) if output_owner == "external" else "../external artifacts",
        )
        expected_root = external / "tmp/runtime-gate-runs"
    else:
        # The existing family layout owns tmp beside molt-src, never inside it.
        expected_root = tmp_path / "tmp/runtime-gate-runs"
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
            result = SimpleNamespace(
                stdout="", stderr="intentional stopped build", returncode=1
            )
            assert kwargs["capture_tail_bytes"] is None
            Path(kwargs["stdout_capture_path"]).write_text(
                result.stdout, encoding="utf-8"
            )
            Path(kwargs["stderr_capture_path"]).write_text(
                result.stderr, encoding="utf-8"
            )
            return result

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
    assert list(source.iterdir()) == []
    if output_owner == "explicit":
        assert not scratch.exists()
    if output_owner in {"explicit", "queue", "default"}:
        assert not external.exists()


@pytest.mark.parametrize(
    "policy,dispatcher",
    [
        ("wasi-clippy", "run_compile_only_lint"),
        ("zlib-wasi", "run_zlib_wasi"),
        ("wasi-lifecycle", "run_wasi_lifecycle"),
    ],
)
def test_policy_receipts_use_external_scratch_without_checkout_outputs(
    monkeypatch, tmp_path, policy, dispatcher
):
    source = tmp_path / "molt-src"
    source.mkdir()
    external = tmp_path / "selected artifacts"
    monkeypatch.setattr(gate, "ROOT", source)
    monkeypatch.delenv("MOLT_PROOF_SCRATCH_ROOT", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(external))
    monkeypatch.setenv("MOLT_SCRATCH_STORAGE", "disk")
    observed = []

    def stop_before_build(args, run, aggregate):
        observed.append(run)
        assert args.policy == policy
        (run / "partial-build.log").write_bytes(b"retained failure witness\x00\xff")
        raise RuntimeError("independent stopped policy build")

    monkeypatch.setattr(gate, dispatcher, stop_before_build)
    assert gate.main(["--policy", policy]) == 1
    (run,) = observed
    assert run.parent == external / "tmp/runtime-gate-runs"
    import json

    receipt = json.loads((run / "aggregate.json").read_text(encoding="utf-8"))
    assert receipt["policy"] == policy and receipt["status"] == "failed"
    assert receipt["error"] == "RuntimeError: independent stopped policy build"
    assert (
        run / "partial-build.log"
    ).read_bytes() == b"retained failure witness\x00\xff"
    assert list(source.iterdir()) == []


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
    "cpython_abi_hooks::buffer_semantics_tests::builtin_buffer_names_share_cold_runtime_lookup_and_native_readiness",
    "cpython_abi_hooks::pending_cleanup_tests::canonical_cache_allocation_failure_reentry_and_ascii_no_retry",
    "cpython_abi_hooks::immortal_ownership_tests::immortal_c_views_preserve_runtime_roots_and_gc",
    "state::lifecycle::shutdown_tests::embedding_shutdown_collects_native_type_cycles_across_module_retirement",
    "concurrency::isolates::bootstrap_failure_tests::completed_isolate_preserves_primary_resource_factory_for_new_workers",
    "state::runtime_state::tests::runtime_policy_freezes_before_first_consumers_and_keeps_absent_vfs_absent",
    "state::runtime_state::tests::runtime_vfs_freezes_route_and_quota_without_loading_payload",
    "state::runtime_state::tests::runtime_relative_vfs_route_keeps_startup_cwd_until_first_use",
    "state::runtime_state::tests::runtime_output_sinks_keep_startup_cwd_until_first_write",
)
UNIX_FRESH_OWNERS = (
    "state::runtime_state::tests::relative_output_sink_capture_failure_disables_without_fallback",
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
    fresh_owners = FRESH_OWNERS + (() if platform == "win32" else UNIX_FRESH_OWNERS)
    inventory = sorted(
        set(
            gate.isolated_for(platform)
            + gate.probes_for(platform)
            + fresh_owners
            + DEVELOPMENT_PROBES
            + ("ordinary",)
        )
    )
    selected = gate.selections(inventory, 4, platform)
    assert selected["parallel"][1] == {"ordinary"}
    for name in fresh_owners:
        assert selected[name] == (
            ["--exact", name, "--ignored", "--test-threads=1", "--nocapture"],
            {name},
        )
    for name in DEVELOPMENT_PROBES:
        assert name in gate.probes_for(platform)
        assert name not in selected
        assert all(name not in tests for _argv, tests in selected.values())


@pytest.mark.parametrize("name", FRESH_OWNERS + UNIX_FRESH_OWNERS)
def test_new_fresh_semantic_owner_cannot_remain_ignored(tmp_path, name):
    children, selected, binding = fixture_ledger(tmp_path)
    children[name][0]["test_results"][0]["status"] = "ignored"
    with pytest.raises(RuntimeError):
        gate.validate_children(children, selected, **binding)


@pytest.mark.parametrize("platform", ["linux", "darwin", "freebsd14", "win32"])
def test_lost_cwd_semantic_owner_follows_the_native_source_cfg(platform):
    name = UNIX_FRESH_OWNERS[0]
    inventory = [*gate.ISOLATED, *gate.probes_for(platform), "ordinary"]
    if platform != "win32":
        # An actual Unix image must contain the ignored lost-CWD oracle, and
        # its absence must fail before any semantic child can be credited.
        with pytest.raises(RuntimeError, match=name):
            gate.selections(inventory, 4, platform)
        inventory.append(name)
    selected = gate.selections(inventory, 4, platform)
    assert selected["parallel"][1] == {"ordinary"}
    if platform == "win32":
        assert name not in selected
        assert name not in selected["parallel"][0]
    else:
        assert selected[name] == (
            ["--exact", name, "--ignored", "--test-threads=1", "--nocapture"],
            {name},
        )
        assert name in selected["parallel"][0]


@pytest.mark.parametrize("name", FRESH_OWNERS + UNIX_FRESH_OWNERS)
def test_missing_fresh_semantic_owner_is_refused_before_execution(name):
    inventory = [*gate.isolated_for("linux"), *gate.probes_for("linux"), "ordinary"]
    inventory.remove(name)
    with pytest.raises(RuntimeError, match=name):
        gate.selections(inventory, 4, "linux")


# Independent required controls from their Rust owners, not a test-count quota.
OBSERVER_WITNESSES = {
    "molt_runtime": {
        "builtins::frames::namespace_tests::task_growth_refusal::both_task_producers_refuse_actual_late_map_growth_and_restore_owners",
        "call::bind::inline_cache::binding_tests::direct_call_ic_hot_path_is_allocation_free_with_slow_path_control",
        "cpython_abi_hooks::pending_cleanup_tests::writable_buffer_lease_preparation_failure_precedes_exporter_dispatch",
        "cpython_abi_hooks::tests::cpython_abi_gil_custody_is_allocation_free_after_warmup",
        "object::gc::tests::repeated_reachable_collection_workspace_allocations",
        "cpython_abi_hooks::exception_rendering_tests::decimal_output_limits::rejected_decimal_magnitude_does_not_allocate_decimal_storage",
        "object::ops::unpack_transaction_tests::exact_builtin_unpack_hot_path_performs_zero_allocator_calls",
        "object::ops_slice::tests::owned_range_parts_constructor_and_subscription_transfer_and_retire",
        "regex_bridge::tests::regex_aggregates_retain_each_created_child_once",
        "regex_bridge::tests::regex_capture_aggregate_owns_children_and_rolls_back_every_failure",
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
        gate.ROOT,
        ["allocation-observer", "l7-attestation-core"],
        artifact_root=tmp_path,
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
            "profile": {
                "test": True,
                "debug_assertions": False,
                "overflow_checks": False,
                "opt_level": "3",
            },
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
                assert kwargs["capture_tail_bytes"] is None
                Path(kwargs["stdout_capture_path"]).write_text(
                    result.stdout, encoding="utf-8"
                )
                Path(kwargs["stderr_capture_path"]).write_text(
                    result.stderr, encoding="utf-8"
                )
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
        "platform_missing",
        "platform_duplicate",
        "platform_foreign",
        "platform_not_wasm",
        "platform_changed_before",
        "platform_changed_after",
        "consumer_changed_before",
        "consumer_changed_after",
        "swapped_handoff",
        "second_child_failure",
        "duplicate_invocation",
    ],
)
def test_wasi_policy_hands_exact_compiler_image_to_existing_consumer(
    monkeypatch, tmp_path, mutation, selected_cargo
):
    import json
    from types import SimpleNamespace
    from molt.cli import runtime_cargo_plan, runtime_tool_selection

    runtime_consumer = "actual_wasi_runtime_lifecycle_cases_through_production_host"
    platform_consumer = "actual_wasi_platform_errno_cases_through_production_host"
    # This policy executes the native production host, not Node. Preserve an
    # unrelated Node setting rather than imposing the counter policy on it.
    inherited_node_options = "--require=/unused-by-native-host.cjs"
    monkeypatch.setenv("NODE_OPTIONS", inherited_node_options)

    wasm = tmp_path / "molt_runtime.wasm"
    wasm.write_bytes(b"\x00asm\x01\x00\x00\x00fixture")
    platform = tmp_path / "molt_runtime_platform.wasm"
    platform.write_bytes(b"\x00asm\x01\x00\x00\x00platform")
    native = support.make_image(tmp_path / "host")
    production_host = support.make_image(tmp_path / "molt-wasm-host")
    host = "aarch64-unknown-linux-gnu"
    metadata, abi_artifact = abi_compiler_fixture(
        gate.ROOT, ["l7-attestation-core"], artifact_root=tmp_path
    )
    metadata["packages"].extend(
        [
            {
                "id": name,
                "name": name,
                "version": "0.1.0",
                "source": None,
                "manifest_path": str(gate.ROOT / "runtime" / name / "Cargo.toml"),
            }
            for name in ("molt-runtime", "molt-wasm-host", "molt-runtime-platform")
        ]
    )
    records = {}
    for package, name, kind, image in (
        ("molt-runtime", "molt_runtime", "rlib", wasm),
        ("molt-wasm-host", "wasi_runtime_lifecycle", "test", native),
        ("molt-runtime-platform", "molt_runtime_platform", "lib", platform),
    ):
        records[package] = [
            {
                "reason": "compiler-artifact",
                "package_id": package,
                "target": {"name": name, "kind": [kind]},
                "profile": {
                    "test": True,
                    "debug_assertions": True,
                    "overflow_checks": True,
                    "opt_level": "0",
                },
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
    provider = records["molt-runtime-platform"]
    if mutation == "platform_missing":
        provider.clear()
    elif mutation == "platform_duplicate":
        provider.append(deepcopy(provider[0]))
    elif mutation == "platform_foreign":
        provider[0]["package_id"] = "molt-runtime"
    elif mutation == "platform_not_wasm":
        platform.write_bytes(b"native platform image")
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
    platform_sha = support.sha256(platform.read_bytes())
    original_host_sha = support.sha256(production_host.read_bytes())
    monkeypatch.setattr(gate.truth, "host_target", lambda: host)
    source_reads = 0

    def source_identity():
        nonlocal source_reads
        source_reads += 1
        if mutation == "host_changed_before" and source_reads == 2:
            production_host.write_bytes(b"changed")
        if mutation == "platform_changed_before" and source_reads == 2:
            platform.write_bytes(b"changed platform")
        if mutation == "consumer_changed_before" and source_reads == 2:
            native.write_bytes(b"changed integration consumer")
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
        root,
        *,
        env,
        cargo_command,
        requested_target,
        host_target,
        rustflags_transform,
        required_cargo_capabilities,
    ):
        assert required_cargo_capabilities == (
            cargo_capabilities.CargoCapability.BUILD_DIRECTORY,
        )
        assert env["NODE_OPTIONS"] == inherited_node_options
        # Exercise the real planner's executable-selection boundary, while its
        # SDK/configuration/compiler work remains outside this host unit test.
        selected = runtime_tool_selection.runtime_tool_path(
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
            assert (
                kwargs.get("env", os.environ)["NODE_OPTIONS"] == inherited_node_options
            )
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
                assert kwargs["capture_tail_bytes"] is None
                Path(kwargs["stdout_capture_path"]).write_text(
                    result.stdout, encoding="utf-8"
                )
                Path(kwargs["stderr_capture_path"]).write_text(
                    result.stderr, encoding="utf-8"
                )
            else:
                children.append(argv)
                consumer = runtime_consumer if len(children) == 1 else platform_consumer
                selected_image = kwargs["env"]["MOLT_WASI_LIBTEST_ARTIFACT"]
                selected_sha, selected_case = {
                    str(wasm): (original_sha, runtime_consumer),
                    str(platform): (platform_sha, platform_consumer),
                }[selected_image]
                assert kwargs["env"]["MOLT_WASI_LIBTEST_SHA256"] == selected_sha
                split = argv.index("--")
                assert argv[split + 1] == str(native)
                args = argv[split + 2 :]
                assert args == [
                    "--exact",
                    consumer,
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ]
                assert argv[argv.index("--require-passed-test") + 1] == consumer
                directory = Path(argv[argv.index("--receipt-dir") + 1])
                names = (
                    []
                    if mutation == "zero"
                    else ["different"]
                    if mutation == "wrong_case"
                    else [selected_case]
                )
                receipt = support.binary_receipt(
                    directory,
                    native,
                    names,
                    [],
                    args=args,
                    retained_root=Path(argv[argv.index("--retained-root") + 1]),
                    run_id=argv[argv.index("--run-id") + 1],
                    invocation="wasi-consumer-1"
                    if mutation == "duplicate_invocation"
                    else f"wasi-consumer-{len(children)}",
                    statuses={consumer: "ignored"} if mutation == "ignored" else None,
                )
                receipt["required_passed_tests"] = (
                    [] if mutation == "wrong_required" else [consumer]
                )
                gate.runner.write_receipt(directory / "receipt.json", receipt)
                if mutation == "platform_changed_after" and len(children) == 2:
                    platform.write_bytes(b"changed platform")
                if mutation == "consumer_changed_after" and len(children) == 2:
                    native.write_bytes(b"changed integration consumer")
                if mutation == "changed_after":
                    wasm.write_bytes(b"changed")
                if mutation == "host_changed_after":
                    production_host.write_bytes(b"changed")
                if mutation == "abi_changed_after":
                    Path(abi_artifact["filenames"][-1]).write_bytes(b"replacement ABI")
                if mutation == "child_failure" or (
                    mutation == "second_child_failure" and len(children) == 2
                ):
                    result.returncode = 1
            return result

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    if mutation == "swapped_handoff":
        execute = gate.execute_child

        def swap_platform_handoff(*args, environment, **kwargs):
            if environment["MOLT_WASI_LIBTEST_ARTIFACT"] == str(platform):
                environment = dict(
                    environment,
                    MOLT_WASI_LIBTEST_ARTIFACT=str(wasm),
                    MOLT_WASI_LIBTEST_SHA256=original_sha,
                )
            return execute(*args, environment=environment, **kwargs)

        monkeypatch.setattr(gate, "execute_child", swap_platform_handoff)
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
    if len(builds) >= 2:
        assert builds[1][0][builds[1][0].index("--target") + 1] == host
        assert "link-arg=--export-table" not in builds[1][1].get(
            "CARGO_ENCODED_RUSTFLAGS", ""
        )
    if len(builds) == 3:
        assert builds[2][0][builds[2][0].index("-p") + 1] == "molt-runtime-platform"
        assert "--features" not in builds[2][0]
        assert builds[2][1].get("CARGO_ENCODED_RUSTFLAGS") == os.environ.get(
            "CARGO_ENCODED_RUSTFLAGS"
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
            "platform_missing",
            "platform_duplicate",
            "platform_foreign",
            "platform_not_wasm",
            "platform_changed_before",
            "consumer_changed_before",
        }:
            assert not children
    else:
        assert code == 0 and aggregate["status"] == "success"
        assert len(builds) == 3 and len(children) == 2
        assert aggregate["passed_test_count"] == 2
        assert aggregate["wasi_handoffs"] == [
            {
                "consumer": runtime_consumer,
                "target": "molt_runtime",
                "environment": {
                    "MOLT_WASI_LIBTEST_ARTIFACT": str(wasm),
                    "MOLT_WASI_LIBTEST_SHA256": original_sha,
                },
            },
            {
                "consumer": platform_consumer,
                "target": "molt_runtime_platform",
                "environment": {
                    "MOLT_WASI_LIBTEST_ARTIFACT": str(platform),
                    "MOLT_WASI_LIBTEST_SHA256": platform_sha,
                },
            },
        ]
        assert (
            aggregate["images"]["molt_runtime_platform"]["build"]["executable_sha256"]
            == platform_sha
        )
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

    if mutation in {"second_child_failure", "duplicate_invocation", "swapped_handoff"}:
        assert len(children) == 2
        assert (
            aggregate["images"]["wasi_runtime_lifecycle"]["children"][runtime_consumer][
                "returncode"
            ]
            == 0
        )

    if mutation in {"platform_missing", "platform_duplicate"}:
        assert (
            "requires exactly one molt_runtime_platform artifact" in aggregate["error"]
        )
    elif mutation == "platform_foreign":
        assert "foreign package authority" in aggregate["error"]
    elif mutation == "platform_changed_before":
        assert "source/image changed before handoff" in aggregate["error"]
    elif mutation == "platform_changed_after":
        assert "source/image changed during execution" in aggregate["error"]
    elif mutation in {
        "second_child_failure",
        "duplicate_invocation",
        "swapped_handoff",
    }:
        assert platform_consumer in aggregate["error"]


# Independent names from the provider's actual normal-width test modules;
# singular deflate::test is deliberate. Debug-only tests are a native policy.
ZLIB_WASI_WITNESSES = {
    "inflate::tests::counter_wrap_checksum_completion_and_gzip_length",
    "inflate::tests::counter_wrap_sync_preserves_recovered_totals",
    "deflate::test::counter_wrap_stored_and_compressed_streams",
    "stable::tests::counter_wrap_stable_totals_preserve_u64_width",
}


def test_queue_node_bootstrap_options_match_the_existing_wire_contract():
    from tools.proof_queue_pkg import execution_custody

    hook = gate.ROOT / "tools/proof_queue_pkg/node_child_custody.cjs"
    assert execution_custody.node_bootstrap_options() == (
        f"--no-global-search-paths --require={hook}"
    )


@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "custody_options",
        "empty_node_options",
        "node_engine_options",
        "node_foreign_preload",
        "node_extra_preload",
        "configured_node_engine_options",
        "configured_node_options_removed",
        "configured_node_options_added",
        "registry",
        "version",
        "source_path",
        "extra_image",
        "wrong_target",
        "not_wasm",
        "optimized",
        "unchecked",
        "no_debug",
        "features",
        "incomplete",
        "missing_case",
        "extra_case",
        "duplicate_case",
        "discovery_failure",
        "ignored",
        "changed_wasm",
        "changed_node",
        "changed_source",
    ],
)
def test_provider_wasi_requires_actual_checked_image_and_four_passes(
    monkeypatch,
    tmp_path,
    selected_cargo,
    mutation,
):
    import json
    from types import SimpleNamespace

    # Independent wire observation of the existing Node custody producer. Do
    # not derive expected options by calling the authority under test.
    hook = gate.ROOT / "tools/proof_queue_pkg/node_child_custody.cjs"
    custody_options = f"--no-global-search-paths --require={hook}"
    inherited_options = {
        "custody_options": custody_options,
        "empty_node_options": "",
        "node_engine_options": "--jitless",
        "node_foreign_preload": "--require=/outside-observer.cjs",
        "node_extra_preload": custody_options + " --require=/outside-observer.cjs",
        "configured_node_options_removed": custody_options,
    }.get(mutation)
    monkeypatch.delenv("NODE_OPTIONS", raising=False)
    if inherited_options is not None:
        monkeypatch.setenv("NODE_OPTIONS", inherited_options)

    node = support.make_image(tmp_path / "node")
    wasm = tmp_path / "target/wasm32-wasip1/debug/deps/zlib_rs.wasm"
    wasm.parent.mkdir(parents=True)
    wasm.write_bytes(b"\0asm\x01\0\0\0provider test")
    package_id = "path+file:///vendor/zlib-rs#0.6.8"
    metadata = {
        "target_directory": str(tmp_path / "target"),
        "packages": [
            {
                "id": package_id,
                "name": "zlib-rs",
                "version": "0.6.8",
                "source": None,
                "manifest_path": str(gate.ROOT / "vendor/rust/zlib-rs/Cargo.toml"),
            }
        ],
    }
    artifact = {
        "reason": "compiler-artifact",
        "package_id": package_id,
        "target": {
            "name": "zlib_rs",
            "kind": ["lib"],
            "src_path": str(gate.ROOT / "vendor/rust/zlib-rs/src/lib.rs"),
        },
        "profile": {
            "test": True,
            "opt_level": "0",
            "debug_assertions": True,
            "overflow_checks": True,
        },
        "features": ["rust-allocator"],
        "executable": str(wasm),
    }
    if mutation == "registry":
        metadata["packages"][0]["source"] = "registry+https://example.invalid/index"
    elif mutation == "version":
        metadata["packages"][0]["version"] = "0.6.9"
    elif mutation == "source_path":
        artifact["target"]["src_path"] = str(tmp_path / "foreign.rs")
    elif mutation == "wrong_target":
        other = tmp_path / "target/debug/zlib_rs.wasm"
        other.parent.mkdir(parents=True)
        other.write_bytes(wasm.read_bytes())
        artifact["executable"] = str(other)
    elif mutation == "not_wasm":
        wasm.write_bytes(b"native executable")
    elif mutation == "optimized":
        artifact["profile"]["opt_level"] = "1"
    elif mutation == "unchecked":
        artifact["profile"]["overflow_checks"] = False
    elif mutation == "no_debug":
        artifact["profile"]["debug_assertions"] = False
    elif mutation == "features":
        artifact["features"].append("ZLIB_DEBUG")
    artifacts = [artifact]
    if mutation == "extra_image":
        artifacts.append(deepcopy(artifact))
    inventory = sorted(ZLIB_WASI_WITNESSES)
    if mutation == "missing_case":
        inventory.pop()
    elif mutation == "extra_case":
        inventory.append("foreign::counter_wrap")
    elif mutation == "duplicate_case":
        inventory.append(inventory[0])
    reads = 0

    def source():
        nonlocal reads
        reads += 1
        return (
            dict(support.SOURCE, head="changed")
            if mutation == "changed_source" and reads > 2
            else support.SOURCE
        )

    monkeypatch.setattr(gate.truth, "git_source_identity", source)
    resolutions, configurations = [], []

    def resolve(**kwargs):
        assert kwargs["environment"].get("NODE_OPTIONS") == inherited_options
        resolutions.append(kwargs)
        return SimpleNamespace(path=node, version="26.11.1")

    def configure(command, env):
        assert env.get("NODE_OPTIONS") == inherited_options
        configurations.append(command)
        configured = dict(env)
        if mutation == "configured_node_engine_options":
            configured["NODE_OPTIONS"] = "--jitless"
        elif mutation == "configured_node_options_removed":
            configured.pop("NODE_OPTIONS")
        elif mutation == "configured_node_options_added":
            configured["NODE_OPTIONS"] = custody_options
        return {"inputs": {"sources": []}, "probes": []}, configured

    monkeypatch.setattr(gate, "resolve_node_runtime", resolve)
    monkeypatch.setattr(gate, "zlib_wasi_flags", configure)
    discovered = []

    def discover(binary, inherited_args, **kwargs):
        assert os.environ.get("NODE_OPTIONS") == inherited_options
        assert binary == str(node)
        assert inherited_args[-3:] == ["--", "counter_wrap", "--test-threads=1"]
        assert str(wasm) in inherited_args[1] and "node:wasi" in inherited_args[1]
        discovered.append(binary)
        return inventory, SimpleNamespace(
            succeeded=mutation != "discovery_failure", receipt=lambda: {}
        )

    monkeypatch.setattr(gate.runner, "listed_tests", discover)
    builds, children = [], []

    class Commands:
        def run(self, argv, **kwargs):
            assert kwargs["env"].get("NODE_OPTIONS") == inherited_options
            result = SimpleNamespace(returncode=0, stdout="", stderr="")
            if argv[:2] == [selected_cargo, "metadata"]:
                result.stdout = json.dumps(metadata)
            elif argv[:2] == [selected_cargo, "test"]:
                builds.append(argv)
                rows = artifacts + (
                    []
                    if mutation == "incomplete"
                    else [{"reason": "build-finished", "success": True}]
                )
                result.stdout = "\n".join(json.dumps(row) for row in rows)
                assert kwargs["capture_tail_bytes"] is None
                Path(kwargs["stdout_capture_path"]).write_text(
                    result.stdout, encoding="utf-8"
                )
                Path(kwargs["stderr_capture_path"]).write_text(
                    result.stderr, encoding="utf-8"
                )
            else:
                assert discovered
                split = argv.index("--")
                assert argv[split + 1] == str(node)
                args = argv[split + 2 :]
                directory = Path(argv[argv.index("--receipt-dir") + 1])
                required = {
                    argv[i + 1]
                    for i, word in enumerate(argv[:-1])
                    if word == "--require-passed-test"
                }
                assert required == ZLIB_WASI_WITNESSES
                receipt = support.binary_receipt(
                    directory,
                    node,
                    sorted(ZLIB_WASI_WITNESSES),
                    [],
                    args=args,
                    retained_root=Path(argv[argv.index("--retained-root") + 1]),
                    run_id=argv[argv.index("--run-id") + 1],
                    invocation="provider-child",
                    statuses={next(iter(ZLIB_WASI_WITNESSES)): "ignored"}
                    if mutation == "ignored"
                    else None,
                )
                receipt["required_passed_tests"] = sorted(required)
                gate.runner.write_receipt(directory / "receipt.json", receipt)
                children.append(argv)
                if mutation == "changed_wasm":
                    wasm.write_bytes(b"replacement")
                if mutation == "changed_node":
                    node.write_bytes(b"replacement")
            return result

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    root = tmp_path / "receipts"
    code = gate.main(
        ["--policy", "zlib-wasi", "--profile", "dev-fast", "--receipt-root", str(root)]
    )
    aggregate = json.loads(
        next(root.glob("*/aggregate.json")).read_text(encoding="utf-8")
    )
    if mutation in {
        "node_engine_options",
        "node_foreign_preload",
        "node_extra_preload",
        "configured_node_engine_options",
        "configured_node_options_removed",
        "configured_node_options_added",
    }:
        assert code == 1 and aggregate["images"] == {}
        assert not builds and not discovered and not children
        configured = mutation.startswith("configured_")
        assert len(resolutions) == len(configurations) == int(configured)
        expected = (
            "Cargo configuration changed Node bootstrap options"
            if configured
            else "permits only queue-owned Node bootstrap options"
        )
        assert expected in aggregate["error"]
        return
    assert len(builds) == 1
    argv = builds[0]
    assert argv[argv.index("-p") + 1] == "zlib-rs@0.6.8"
    assert argv[argv.index("--target") + 1] == "wasm32-wasip1"
    assert argv[argv.index("--features") + 1] == "rust-allocator"
    assert "--lib" in argv and "--no-default-features" in argv and "--no-run" in argv
    assert "--test" not in argv
    if mutation in {None, "custody_options", "empty_node_options"}:
        assert code == 0 and aggregate["passed_test_count"] == 4
        assert len(children) == 1
        assert (
            aggregate["images"]["zlib_rs"]["retained"]["sha256"]
            != aggregate["node"]["retained"]["sha256"]
        )
    else:
        assert code == 1 and aggregate["status"] == "failed"
        if mutation not in {
            "ignored",
            "changed_wasm",
            "changed_node",
            "changed_source",
        }:
            assert not children


@pytest.mark.parametrize(
    "flags,refused",
    [
        (["-Dwarnings", "-Clink-arg=--strip-debug"], False),
        (["-C", "overflow_checks=yes", "--codegen=opt-level=0"], False),
        (["-Coverflow-checks=no"], True),
        (["--codegen", "debug_assertions=false"], True),
        (["-C", "opt-level=3"], True),
        (["-O"], True),
    ],
)
def test_provider_keeps_effective_cargo_flags_and_refuses_weakened_coordinate(
    monkeypatch,
    tmp_path,
    flags,
    refused,
):
    from molt.cli import runtime_cargo_plan as plan, runtime_tool_selection
    from molt import rust_toolchain
    from tools.proof_queue_pkg import toolchain_capture

    environment = {"RUSTFLAGS": "-Dwarnings", "UNRELATED": "preserved"}
    captured = (tmp_path, environment, [], [], {}, {}, None, {}, [], {})
    monkeypatch.setattr(
        plan, "capture_cargo_configuration_inputs", lambda *a, **kw: captured
    )
    monkeypatch.setattr(
        runtime_tool_selection, "runtime_tool_path", lambda *a, **kw: tmp_path / "rustc"
    )
    monkeypatch.setattr(rust_toolchain, "resolve_rustup_proxy", lambda path, **kw: path)

    def capture(**kw):
        assert kw["env"] == environment and kw["target"] == "wasm32-wasip1"
        return {"selected_flags": flags, "sources": []}, dict(environment)

    monkeypatch.setattr(toolchain_capture, "_cargo_flag_context", capture)
    if refused:
        with pytest.raises(RuntimeError, match="flags contradict"):
            gate.zlib_wasi_flags(["cargo", "test"], environment)
    else:
        context, actual = gate.zlib_wasi_flags(["cargo", "test"], environment)
        assert actual == environment
        assert context["inputs"]["selected_flags"] == flags
        assert "CARGO_ENCODED_RUSTFLAGS" not in actual


@pytest.mark.parametrize(
    ("policy", "expected"),
    [
        (
            "semantic",
            [
                (
                    "runtime",
                    "cargo",
                    "native",
                    [
                        "-p",
                        "molt-runtime",
                        "-p",
                        "molt-runtime-core",
                        "-p",
                        "molt-runtime-math",
                        "--lib",
                        "--no-run",
                        "--message-format=json",
                    ],
                )
            ],
        ),
        (
            "allocation-observer",
            [
                (
                    "runtime",
                    "cargo",
                    "native",
                    [
                        "-p",
                        "molt-runtime",
                        "--lib",
                        "--no-run",
                        "--message-format=json",
                        "--test",
                        "l7_numeric_runtime_perf_attestation",
                        "--features",
                        "l7-attestation-probe",
                    ],
                )
            ],
        ),
        (
            "zlib-wasi",
            [
                (
                    "provider",
                    "zlib-wasi",
                    "none",
                    [
                        "-p",
                        "zlib-rs@0.6.8",
                        "--lib",
                        "--target",
                        "wasm32-wasip1",
                        "--no-default-features",
                        "--features",
                        "rust-allocator",
                        "--no-run",
                        "--message-format=json",
                    ],
                )
            ],
        ),
        (
            "wasi-lifecycle",
            [
                (
                    "runtime",
                    "runtime-wasi",
                    "managed-wasi",
                    [
                        "-p",
                        "molt-runtime",
                        "--target",
                        "wasm32-wasip1",
                        "--no-run",
                        "--message-format=json",
                        "--lib",
                        "--no-default-features",
                        "--features",
                        "stdlib_micro",
                    ],
                ),
                (
                    "host",
                    "cargo",
                    "native",
                    [
                        "-p",
                        "molt-wasm-host",
                        "--target",
                        "aarch64-unknown-linux-gnu",
                        "--no-run",
                        "--message-format=json",
                        "--test",
                        "wasi_runtime_lifecycle",
                    ],
                ),
                (
                    "provider",
                    "cargo",
                    "none",
                    [
                        "-p",
                        "molt-runtime-platform",
                        "--lib",
                        "--target",
                        "wasm32-wasip1",
                        "--no-default-features",
                        "--no-run",
                        "--message-format=json",
                    ],
                ),
            ],
        ),
    ],
)
def test_runtime_cargo_operation_contract_has_exact_commands_and_c_ownership(
    policy, expected
):
    from tools.runtime_test_gate_plan import cargo_operations

    operations = cargo_operations(
        policy,
        "dev-fast",
        cargo="/selected/cargo",
        compiler_host="aarch64-unknown-linux-gnu",
    )
    assert [
        (item.role, item.environment, item.c_units, list(item.command))
        for item in operations
    ] == [
        (
            role,
            environment,
            c_units,
            ["/selected/cargo", "test", "--locked", "--profile", "dev-fast", *tail],
        )
        for role, environment, c_units, tail in expected
    ]
    assert [item.native_requirements for item in operations] == [
        {} if c_units == "none" else {"target": ["c"]} for _, _, c_units, _ in expected
    ]


def test_wasi_clippy_uses_the_existing_compile_only_cargo_operation():
    from tools import proof_plan
    from tools.runtime_test_gate_plan import cargo_operations

    (operation,) = cargo_operations("wasi-clippy", "dev", cargo="/selected/cargo")
    assert (operation.role, operation.environment, operation.c_units) == (
        "runtime",
        "runtime-wasi",
        "managed-wasi",
    )
    assert list(operation.command) == [
        "/selected/cargo",
        "clippy",
        "--config",
        'build.warnings="deny"',
        "--locked",
        "--target",
        "wasm32-wasip1",
        "--lib",
        "-p",
        "molt-runtime",
        "-p",
        "molt-tir",
    ]
    assert operation.native_requirements == {"target": ["c"]}
    assert (
        proof_plan.clippy_diagnostic_format_error(("cargo", *operation.command[1:]))
        is None
    )
    with pytest.raises(ValueError):
        cargo_operations("wasi-clippy", "dev-fast", cargo="/selected/cargo")


@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "cargo-failure",
        "fake-json-success",
        "empty-output",
        "human-words",
        "plan-changed",
    ],
)
def test_wasi_clippy_executes_the_resolved_plan_without_claiming_runtime_tests(
    monkeypatch, tmp_path, selected_cargo, mutation
):
    import json
    from types import SimpleNamespace
    from molt.cli import runtime_cargo_plan

    source = tmp_path / "source"
    source.mkdir()
    monkeypatch.setattr(gate, "ROOT", source)
    monkeypatch.setenv("RUSTFLAGS", "--cfg explicit_user_flags")
    monkeypatch.setattr(gate.truth, "git_source_identity", lambda: support.SOURCE)
    monkeypatch.setattr(gate.truth, "host_target", lambda: "aarch64-apple-darwin")
    submitted = [
        selected_cargo,
        "clippy",
        "--config",
        'build.warnings="deny"',
        "--locked",
        "--target",
        "wasm32-wasip1",
        "--lib",
        "-p",
        "molt-runtime",
        "-p",
        "molt-tir",
    ]
    actual = [
        *submitted,
        "--config",
        'build.build-dir="/owned/selected-build"',
    ]
    flags = ("--cfg", "explicit_user_flags", "-C", "link-self-contained=no")
    checked, builds = [], []

    def verify():
        checked.append("verified")
        if mutation == "plan-changed" and builds:
            raise RuntimeError("selected Cargo input changed during execution")

    def plan(
        root,
        *,
        env,
        cargo_command,
        requested_target,
        host_target,
        rustflags_transform,
        required_cargo_capabilities,
    ):
        assert required_cargo_capabilities == (
            cargo_capabilities.CargoCapability.BUILD_DIRECTORY,
            cargo_capabilities.CargoCapability.WARNINGS_DENY,
        )
        assert root == source and list(cargo_command) == submitted
        assert requested_target == "wasm32-wasip1"
        assert host_target is None
        assert rustflags_transform is None
        assert env["RUSTFLAGS"] == "--cfg explicit_user_flags"
        return SimpleNamespace(
            command=tuple(actual),
            environment=dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags)),
            rustflags=flags,
            verify=verify,
        )

    class Commands:
        def run(self, argv, **kwargs):
            assert list(argv) == actual, (
                "execute the admitted plan, not its source argv"
            )
            assert checked and not builds
            assert kwargs["cwd"] == source
            assert kwargs["timeout"] == 1200
            assert kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"] == "\x1f".join(flags)
            assert "link-arg=--export-table" not in flags
            builds.append(list(argv))
            assert Path(kwargs["stdout_capture_path"]).name == "build.stdout.log"
            assert Path(kwargs["stderr_capture_path"]).name == "build.stderr.log"
            assert kwargs["capture_tail_bytes"] is None
            output = "ordinary Cargo output, not JSON\n"
            if mutation == "fake-json-success":
                output = '{"reason":"build-finished","success":true}\n'
            elif mutation == "empty-output":
                output = ""
            elif mutation == "human-words":
                output = "error appears in a package path, not an exit status\n"
            Path(kwargs["stdout_capture_path"]).write_text(output, encoding="utf-8")
            Path(kwargs["stderr_capture_path"]).write_text(
                "lint diagnostic\n", encoding="utf-8"
            )
            return SimpleNamespace(
                stdout=output,
                stderr="lint diagnostic\n",
                returncode=101
                if mutation in {"cargo-failure", "fake-json-success"}
                else 0,
            )

    def forbidden_runtime(*_args, **_kwargs):
        pytest.fail("compile-only Clippy cannot claim libtest or guest execution")

    monkeypatch.setattr(runtime_cargo_plan, "resolve_runtime_cargo_plan", plan)
    monkeypatch.setattr(gate, "COMMANDS", Commands())
    monkeypatch.setattr(
        gate, "loads_exact", lambda *_a, **_k: pytest.fail("lint verdict parsed output")
    )
    monkeypatch.setattr(gate, "execute_child", forbidden_runtime)
    monkeypatch.setattr(gate.runner, "listed_tests", forbidden_runtime)
    receipt_root = tmp_path / "receipts"
    result = gate.main(
        [
            "--policy",
            "wasi-clippy",
            "--profile",
            "dev",
            "--build-timeout-seconds",
            "1200",
            "--receipt-root",
            str(receipt_root),
        ]
    )
    aggregate = json.loads(
        next(receipt_root.glob("*/aggregate.json")).read_text(encoding="utf-8")
    )
    assert builds == [actual]
    assert aggregate["source_command"] == submitted
    assert aggregate["build_command"] == actual
    assert aggregate["resolved_rustflags"] == list(flags)
    assert aggregate["build_returncode"] == (
        101 if mutation in {"cargo-failure", "fake-json-success"} else 0
    )
    assert (result, aggregate["status"]) == (
        (0, "success")
        if mutation in {None, "empty-output", "human-words"}
        else (1, "failed")
    )
    assert aggregate["images"] == {}
    assert "passed_test_count" not in aggregate
    assert "build_finished" not in aggregate
    if mutation in {None, "empty-output", "human-words"}:
        assert len(checked) >= 2, "the source plan must close after compilation"


@pytest.mark.parametrize(
    "policy,profile,host",
    [
        ("unknown", "dev-fast", "host"),
        ("zlib-wasi", "release", "host"),
        ("wasi-lifecycle", "dev-fast", None),
    ],
)
def test_runtime_cargo_operation_contract_refuses_unknown_or_incomplete_policy(
    policy, profile, host
):
    from tools.runtime_test_gate_plan import cargo_operations

    with pytest.raises(ValueError):
        cargo_operations(policy, profile, cargo="/selected/cargo", compiler_host=host)


def test_runtime_cargo_operation_c_declaration_preserves_supersets_and_roles():
    from tools.runtime_test_gate_plan import (
        cargo_operations,
        project_native_requirements,
    )

    native = cargo_operations("semantic", "release-output", cargo="cargo")
    assert project_native_requirements(native, {"target": ["c", "c++"]}) == (
        {"target": ["c", "c++"]},
    )
    for declaration in ({}, {"host": ["c"]}, {"target": ["c++"]}):
        with pytest.raises(ValueError, match="C source declaration"):
            project_native_requirements(native, declaration)
    provider = cargo_operations("zlib-wasi", "dev-fast", cargo="cargo")
    assert project_native_requirements(provider, {}) == ({},)
    with pytest.raises(ValueError, match="no operation owning"):
        project_native_requirements(provider, {"target": ["c"]})
    mixed = cargo_operations(
        "wasi-lifecycle", "dev-fast", cargo="cargo", compiler_host="host"
    )
    declaration = {"target": ["c", "c++"], "host": ["c"]}
    assert project_native_requirements(mixed, declaration) == (
        declaration,
        declaration,
        {},
    )


@pytest.mark.parametrize(
    "mutation", [None, "missing_image", "foreign_package", "missing_case", "ignored"]
)
def test_semantic_build_owns_exact_satellite_controls_in_one_cargo_lifecycle(
    monkeypatch, tmp_path, mutation, selected_cargo
):
    import json
    from types import SimpleNamespace

    # Independent semantic names, not a copy of the policy's descriptor object.
    required = {
        "molt_runtime_core": {
            "numeric_ratio::tests::ratio_to_f64_matches_cpython_bit_exact",
            "numeric_ratio::tests::ratio_to_f64_overflows_past_f64_max",
            "numeric_ratio::tests::binary64_rounding_boundaries",
        },
        "molt_runtime_math": {
            "math::statistics_reduction_tests::exact_reduction_oracles_and_primitive_cost"
        },
    }
    inventories = {
        "molt_runtime": [
            *gate.isolated_for(gate.sys.platform),
            *gate.probes_for(gate.sys.platform),
            *REMAINDER,
        ],
        **{
            target: [*sorted(cases), "unselected::other_case"]
            for target, cases in required.items()
        },
    }
    images = {target: support.make_image(tmp_path / target) for target in inventories}
    metadata, abi = abi_compiler_fixture(gate.ROOT, [], artifact_root=tmp_path)
    artifacts = [abi]
    for package, target, kind in (
        ("molt-runtime", "molt_runtime", "rlib"),
        ("molt-runtime-core", "molt_runtime_core", "lib"),
        ("molt-runtime-math", "molt_runtime_math", "lib"),
    ):
        if mutation == "foreign_package" and target == "molt_runtime_core":
            package = "foreign-core"
        package_id = f"path+file:///fixture/{package}#{package}@0.1.0"
        metadata["packages"].append(
            {"id": package_id, "name": package, "version": "0.1.0"}
        )
        artifacts.append(
            {
                "reason": "compiler-artifact",
                "package_id": package_id,
                "target": {"name": target, "kind": [kind]},
                "profile": {
                    "test": True,
                    "debug_assertions": False,
                    "overflow_checks": False,
                    "opt_level": "z",
                },
                "features": [],
                "executable": str(images[target]),
            }
        )
    if mutation == "missing_image":
        artifacts.pop()
    if mutation == "missing_case":
        inventories["molt_runtime_core"].remove(
            sorted(required["molt_runtime_core"])[0]
        )
    monkeypatch.setattr(gate.truth, "git_source_identity", lambda: support.SOURCE)
    discoveries, builds, children = [], [], []

    def discover(binary, args, **kwargs):
        target = next(name for name, path in images.items() if str(path) == binary)
        discoveries.append(target)
        return inventories[target], SimpleNamespace(succeeded=True, receipt=lambda: {})

    monkeypatch.setattr(gate.runner, "listed_tests", discover)

    class Commands:
        def run(self, argv, **kwargs):
            result = SimpleNamespace(stdout="", stderr="", returncode=0)
            if argv[:2] == [selected_cargo, "metadata"]:
                result.stdout = json.dumps(metadata)
            elif argv[:2] == [selected_cargo, "test"]:
                builds.append(argv)
                result.stdout = "\n".join(json.dumps(row) for row in artifacts)
                assert kwargs["capture_tail_bytes"] is None
                Path(kwargs["stdout_capture_path"]).write_text(
                    result.stdout, encoding="utf-8"
                )
                Path(kwargs["stderr_capture_path"]).write_text(
                    result.stderr, encoding="utf-8"
                )
            elif argv[0] in {selected_cargo, "rustc"}:
                result.stdout = "fixture-toolchain"
            else:
                # All required images and inventories precede the first child.
                assert set(discoveries) == set(inventories)
                split = argv.index("--")
                binary, args = Path(argv[split + 1]), argv[split + 2 :]
                target = next(name for name, path in images.items() if path == binary)
                if target in required:
                    cases = required[target]
                    assert args[0] == "--exact"
                    assert set(args[1:-2]) == cases
                    assert args[-2:] == ["--test-threads=1", "--nocapture"]
                else:
                    cases = {args[1]} if args[0] == "--exact" else set(REMAINDER)
                required_passed = [
                    argv[i + 1]
                    for i, value in enumerate(argv[:-1])
                    if value == "--require-passed-test"
                ]
                assert set(required_passed) == (cases if target in required else set())
                directory = Path(argv[argv.index("--receipt-dir") + 1])
                owners = [name for name in cases if name in support.MODES]
                receipt = support.binary_receipt(
                    directory,
                    binary,
                    sorted(cases),
                    support.family_records(binary, owners),
                    args=args,
                    retained_root=Path(argv[argv.index("--retained-root") + 1]),
                    run_id=argv[argv.index("--run-id") + 1],
                    invocation=f"child-{len(children)}",
                    statuses=(
                        {sorted(cases)[0]: "ignored"}
                        if mutation == "ignored" and target == "molt_runtime_core"
                        else None
                    ),
                )
                receipt["required_passed_tests"] = required_passed
                gate.runner.write_receipt(directory / "receipt.json", receipt)
                children.append((target, cases))
            return result

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    root = tmp_path / "receipts"
    code = gate.main(["--receipt-root", str(root)])
    aggregate = json.loads(
        next(root.glob("*/aggregate.json")).read_text(encoding="utf-8")
    )
    assert len(builds) == 1
    assert builds[0][builds[0].index("--profile") + 1] == "release-output"
    assert [
        builds[0][i + 1] for i, arg in enumerate(builds[0][:-1]) if arg == "-p"
    ] == ["molt-runtime", "molt-runtime-core", "molt-runtime-math"]
    if mutation:
        assert code == 1 and aggregate["status"] == "failed"
        assert "passed_test_count" not in aggregate
        if mutation != "ignored":
            assert children == []
        else:
            assert "verification_error" in aggregate["images"]["molt_runtime_core"]
            assert "passed_test_count" not in aggregate["images"]["molt_runtime_core"]
        return
    assert code == 0 and aggregate["status"] == "success"
    for target, cases in required.items():
        image = aggregate["images"][target]
        assert image["passed_test_count"] == len(cases)
        assert {row["identity"] for row in image["unrun"]} == {"unselected::other_case"}
        assert (
            image["unrun"][0]["reason"] == "outside declared satellite numeric controls"
        )
    assert aggregate["passed_test_count"] == (
        len(gate.isolated_for(gate.sys.platform)) + len(REMAINDER) + 4
    )
