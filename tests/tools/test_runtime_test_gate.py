from __future__ import annotations

from copy import deepcopy
from pathlib import Path

import pytest

from tools import run_runtime_test_gate as gate


def fixture_ledger():
    binary = Path("runtime-test.exe").resolve()
    inventory = [*gate.ISOLATED, *gate.PROBES, "ordinary::parallel_case"]
    selections = gate.selections(inventory, 4)
    source = {"schema": "molt.git-source.v1", "head": "tested"}
    identity = (123, "a" * 64)
    children = {}
    for index, (name, (args, tests)) in enumerate(selections.items()):
        children[name] = [
            {
                "schema": "molt.cargo-test-binary.v2",
                "source_identity": source,
                "run_id": "run",
                "invocation_id": str(index),
                "executable_resolved": str(binary),
                "executable_size": 123,
                "executable_sha256": "a" * 64,
                "inherited_args": args,
                "status": "success",
                "returncode": 0,
                "failure_identities": [],
                "test_results": [
                    {"identity": test, "status": "pass"} for test in tests
                ],
                "result_accounting": {
                    "schema": "molt.libtest-accounting.v1",
                    "complete": True,
                    "observed_results": len(tests),
                    "declared_results": len(tests),
                    "issues": [],
                },
                "executions": [{"argv": [str(binary), *args]}],
            }
        ]
    return (
        children,
        selections,
        dict(source=source, run_id="run", binary=binary, identity=identity),
    )


def test_complete_parallel_and_fresh_process_ledger_passes():
    children, selections, binding = fixture_ledger()
    gate.validate_children(children, selections, **binding)
    assert len(children) == 8
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
def test_aggregate_fails_closed_for_child_evidence_mutations(mutation):
    children, selections, binding = fixture_ledger()
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
        gate.selections(inventory, threads)


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


def test_default_profile_is_shipping_before_build(monkeypatch, tmp_path):
    # Observe typed build argv without compiling or publishing a synthetic pass.
    seen = []
    monkeypatch.setattr(
        gate.truth, "git_source_identity", lambda: {"schema": "molt.git-source.v1"}
    )

    class Commands:
        def run(self, argv, **kwargs):
            from types import SimpleNamespace

            if argv[:2] == ["cargo", "metadata"]:
                return SimpleNamespace(
                    stdout='{"packages":[]}', stderr="", returncode=0
                )
            seen.append(argv)
            return SimpleNamespace(
                stdout="", stderr="intentional stopped build", returncode=1
            )

    monkeypatch.setattr(gate, "COMMANDS", Commands())
    assert gate.main(["--receipt-root", str(tmp_path)]) == 1
    command = seen[0]
    assert command[command.index("--profile") + 1] == "release-output"


def test_build_rustflags_cannot_override_shipping_assertions():
    artifact, _, output = observed_fixture()
    with pytest.raises(RuntimeError):
        gate.observed_build(
            output,
            artifact,
            "release-output",
            {"CARGO_BUILD_RUSTFLAGS": "-Cdebug-assertions=yes"},
        )
