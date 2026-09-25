from __future__ import annotations

from dataclasses import replace
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import source_build_environment as build_environment
from molt.cli import source_extension_set_registry as registry_authority
from molt.cli.source_extension_invocation import SourceExtensionSetInvocation
from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import cli, command_identity, execution_environment, pact


@pytest.fixture
def producer_request(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    root = tmp_path / "build-environments" / "source-extension" / ("a" * 64)
    scripts = root / ("Scripts" if os.name == "nt" else "bin")
    scripts.mkdir(parents=True)
    python = scripts / ("python.exe" if os.name == "nt" else "python")
    python.write_bytes(b"fixture interpreter; never executed")
    manifest = root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST
    manifest.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        build_environment, "_source_build_custody_root", lambda _: root.parent
    )
    source = tmp_path / "upstream"
    source.mkdir()
    invocation = SourceExtensionSetInvocation(
        command="produce-set",
        package="numpy",
        package_version="2.5.1",
        module_set="pact-witness",
        python_version="3.12",
        source=str(source),
        build_root=str(tmp_path / "build"),
        target="wasm",
        abi_tier="cpython-abi",
        json_output=True,
        prepared=True,
    )
    environment = SimpleNamespace(
        root=root, python_executable=python, manifest_path=manifest
    )
    return invocation, environment


def test_registered_producer_has_declared_tool_family(producer_request) -> None:
    invocation, environment = producer_request
    command = invocation.module_argv(str(environment.python_executable))
    envelope = admission.envelope_for_command(command)
    assert envelope["kind"] == "typed-python-family"
    assert set(envelope["toolchains"]) == {"python", "source-extension", "git"}
    assert envelope["process_closure"]["descendants"] == "declared-toolchains"
    assert envelope["typed_command"]["target_triple"] == "wasm32-wasip1"
    assert envelope["typed_command"]["prepared"] is True
    admission.validate_envelope(envelope, command)
    assert (
        "source-extension"
        not in admission._proof_command_registry()["policy_executables"]
    )


@pytest.mark.parametrize("value", [0, 1, 1.0, None, "true"])
def test_persisted_prepared_precondition_retains_exact_json_type(
    producer_request, value
) -> None:
    invocation, environment = producer_request
    command = invocation.module_argv(str(environment.python_executable))
    envelope = admission.envelope_for_command(command)
    envelope["typed_command"]["prepared"] = value
    with pytest.raises(ValueError, match="does not match submitted argv"):
        admission.validate_envelope(envelope, command)


@pytest.mark.parametrize("version", ["3.12", "3.13", "3.14"])
def test_registered_coordinates_not_a_second_version_gate(
    producer_request,
    monkeypatch: pytest.MonkeyPatch,
    version: str,
) -> None:
    invocation, environment = producer_request
    observed = []

    def expected(extension_set, *, variant, registry):
        observed.append(variant.cpython)
        return "b" * 64

    monkeypatch.setattr(
        registry_authority, "source_extension_set_expected_identity", expected
    )
    command = replace(invocation, python_version=version).module_argv(
        str(environment.python_executable)
    )
    assert admission.envelope_for_command(command)["kind"] == "typed-python-family"
    assert observed == [version]


def test_registry_rejection_is_not_bypassed(
    producer_request, monkeypatch: pytest.MonkeyPatch
) -> None:
    invocation, environment = producer_request

    def outside(*args, **kwargs):
        raise ValueError("unregistered fixture matrix cell")

    monkeypatch.setattr(
        registry_authority, "source_extension_set_expected_identity", outside
    )
    with pytest.raises(ValueError, match="unregistered fixture matrix cell"):
        admission.envelope_for_command(
            invocation.module_argv(str(environment.python_executable))
        )


@pytest.mark.parametrize("prefix", [("-m",), ("-I", "-m"), ("-P", "-S", "-m")])
def test_producer_rejects_interpreter_semantic_drift(producer_request, prefix) -> None:
    invocation, environment = producer_request
    canonical = invocation.module_argv(str(environment.python_executable))
    with pytest.raises(ValueError, match="with -P"):
        admission.envelope_for_command([canonical[0], *prefix, *canonical[3:]])


def test_uv_producer_cannot_downgrade_to_leaf_custody(producer_request) -> None:
    invocation, environment = producer_request
    with pytest.raises(ValueError, match="direct locked interpreter"):
        admission.envelope_for_command(
            ["uv", "run", *invocation.module_argv(str(environment.python_executable))]
        )


def test_unprepared_producer_cannot_enter_proof_custody(producer_request) -> None:
    invocation, environment = producer_request
    with pytest.raises(ValueError, match="requires --prepared"):
        admission.envelope_for_command(
            replace(invocation, prepared=False).module_argv(
                str(environment.python_executable)
            )
        )


def test_unowned_environment_path_rejected(
    producer_request, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    invocation, environment = producer_request
    monkeypatch.setattr(
        build_environment, "_source_build_custody_root", lambda _: tmp_path / "other"
    )
    with pytest.raises(ValueError, match="content-addressed"):
        admission.envelope_for_command(
            invocation.module_argv(str(environment.python_executable))
        )


@pytest.mark.parametrize("field", ["source", "build_root"])
def test_typed_path_alias_is_rejected_before_watching(
    producer_request, tmp_path: Path, field: str
) -> None:
    invocation, environment = producer_request
    alias = tmp_path / "alias"
    try:
        alias.symlink_to(tmp_path, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink unavailable: {exc}")
    aliased = alias / ("upstream" if field == "source" else "build")
    command = replace(invocation, **{field: str(aliased)}).module_argv(
        str(environment.python_executable)
    )
    with pytest.raises(ValueError, match="canonical path"):
        admission.envelope_for_command(command)


def test_typed_git_root_must_match_bootstrap_checkout(tmp_path: Path) -> None:
    envelope = {"typed_command": {"family": "source-extension-producer"}}
    with pytest.raises(ValueError, match="bootstrap checkout"):
        execution_environment.validate_typed_source_root(
            envelope, {"root": str(tmp_path)}
        )
    execution_environment.validate_typed_source_root(
        envelope, {"root": str(admission._REPO_ROOT.resolve())}
    )
    execution_environment.validate_typed_source_root(
        {"typed_command": None}, {"root": str(tmp_path)}
    )


def test_named_spec_round_trips_shared_invocation_without_provisioning(
    producer_request,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    invocation, environment = producer_request
    calls = []
    monkeypatch.setattr(
        pact,
        "source_build_environment",
        lambda root, group: calls.append(group) or environment,
    )
    spec = pact._source_extension_producer_spec(
        package=invocation.package,
        package_version=invocation.package_version,
        module_set=invocation.module_set,
        python_version=invocation.python_version,
        source=invocation.source,
        build_root=invocation.build_root,
        target=invocation.target,
        abi_tier=invocation.abi_tier,
        expected_identity_sha256=None,
        expected_candidate_identity_sha256=None,
        timeout=None,
        repo_root=tmp_path,
    )
    assert spec["command"][:6] == [
        str(environment.python_executable),
        "-P",
        "-m",
        "molt.cli",
        "extension",
        "produce-set",
    ]
    actual = SourceExtensionSetInvocation.from_arguments(spec["command"][4:])
    assert actual == replace(invocation, target="wasm32-wasip1")
    assert (
        admission.envelope_for_command(spec["command"])["kind"] == "typed-python-family"
    )
    assert len(calls) == 1
    assert spec["env_overrides"]["PATH"].split(os.pathsep)[0] == str(
        environment.python_executable.parent
    )


def test_queue_cli_uses_shared_options_without_version_choices(
    producer_request,
) -> None:
    invocation, environment = producer_request
    arguments = list(
        replace(invocation, python_version="3.14", prepared=False).module_argv(
            str(environment.python_executable)
        )
    )[6:]
    parsed = cli._build_parser().parse_args(
        ["source-extension-produce", *arguments, "--print-spec"]
    )
    assert parsed.python_version == "3.14"
    assert parsed.pact_handler == "_cmd_source_extension_produce"
    assert parsed.print_spec


def test_named_native_intent_reaches_host_compiler_selection(
    producer_request, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt.cli import llvm_wasi_tools, source_extension_toolchain
    from molt.cli.source_extension_target import resolve_source_extension_target_plan

    invocation, environment = producer_request
    admitted_targets = []

    def expected(_extension_set, *, variant, **_kwargs):
        admitted_targets.append(variant.target_triple)
        return "b" * 64

    monkeypatch.setattr(pact, "source_extension_set_expected_identity", expected)
    monkeypatch.setattr(
        registry_authority, "source_extension_set_expected_identity", expected
    )
    monkeypatch.setattr(
        pact, "source_build_environment", lambda root, group: environment
    )
    spec = pact._source_extension_producer_spec(
        package=invocation.package,
        package_version=invocation.package_version,
        module_set=invocation.module_set,
        python_version=invocation.python_version,
        source=invocation.source,
        build_root=invocation.build_root,
        target="native",
        abi_tier=invocation.abi_tier,
        expected_identity_sha256=None,
        expected_candidate_identity_sha256=None,
        timeout=None,
        repo_root=tmp_path,
    )
    actual = SourceExtensionSetInvocation.from_arguments(spec["command"][4:])
    assert actual.target == "native"
    typed = admission.envelope_for_command(spec["command"])["typed_command"]
    target = resolve_source_extension_target_plan(actual.target)
    assert typed["target"] == "native"
    assert typed["target_triple"] == target.target_triple
    assert target.compiler_target_triple is None
    assert admitted_targets and set(admitted_targets) == {target.target_triple}

    compiler = tmp_path / "host-cc"
    cxx = tmp_path / "host-cxx"
    compiler.write_bytes(b"fixture; never executed")
    cxx.write_bytes(b"fixture; never executed")
    selected = {
        "CC": str(compiler),
        "CXX": str(cxx),
        "MOLT_CROSS_CC": str(tmp_path / "must-not-select-cross-cc"),
        "MOLT_CROSS_CXX": str(tmp_path / "must-not-select-cross-cxx"),
        "PATH": "",
    }
    seen = []

    def family(*, target_family, explicit_commands, sibling_directories, environment):
        assert target_family == "native"
        assert environment == selected
        seen.append(explicit_commands)

        def tool(role, command):
            return llvm_wasi_tools.ResolvedLlvmTool(
                role, command, Path(command[0]), "fixture", "a" * 64
            )

        return llvm_wasi_tools.LlvmWasiToolFamily(
            cc=tool("cc", explicit_commands["cc"]),
            cxx=tool("cxx", explicit_commands["cxx"]),
            ar=tool("ar", (str(tmp_path / "llvm-ar"),)),
            nm=tool("nm", (str(tmp_path / "llvm-nm"),)),
            wasm_ld=None,
            ranlib=None,
            strip=None,
        )

    monkeypatch.setattr(
        source_extension_toolchain, "resolve_llvm_wasi_tool_family", family
    )
    resolved = source_extension_toolchain._resolve_source_extension_toolchain(
        target, environment=selected
    )
    assert resolved.compiler_kind == "host"
    assert resolved.commands["c"] == (str(compiler),)
    assert resolved.commands["cpp"] == (str(cxx),)
    assert seen == [{"cc": (str(compiler),), "cxx": (str(cxx),)}]


def test_invalid_publication_identity_is_rejected_before_setup(
    producer_request,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    invocation, environment = producer_request
    monkeypatch.setattr(
        pact, "source_build_environment", lambda root, group: environment
    )
    monkeypatch.setattr(
        pact,
        "_prepare_source_extension_producer",
        lambda _: pytest.fail("setup ran before validation"),
    )
    arguments = list(
        replace(invocation, prepared=False).module_argv(
            str(environment.python_executable)
        )
    )[6:]
    parsed = cli._build_parser().parse_args(
        [
            "--repo-root",
            str(tmp_path),
            "source-extension-produce",
            *arguments,
            "--expected-identity-sha256",
            "not-a-digest",
        ]
    )
    with pytest.raises(ValueError, match="SHA-256"):
        pact._cmd_source_extension_produce(parsed)


def test_setup_verifies_source_before_mutation_and_environment_address(
    producer_request,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    from molt.cli import source_extension_producer

    invocation, environment = producer_request
    events = []
    monkeypatch.setattr(
        pact,
        "source_build_environment",
        lambda root, group: environment,
    )
    monkeypatch.setattr(
        source_extension_producer,
        "source_build_environment",
        lambda root, group, **kw: events.append(("environment", kw)) or environment,
    )
    plan = pact._source_extension_producer_plan(
        package=invocation.package,
        package_version=invocation.package_version,
        module_set=invocation.module_set,
        python_version=invocation.python_version,
        source=invocation.source,
        build_root=invocation.build_root,
        target=invocation.target,
        abi_tier=invocation.abi_tier,
        repo_root=tmp_path,
    )
    events.clear()
    monkeypatch.setattr(
        source_extension_producer,
        "verify_source_extension_checkout",
        lambda *a, **kw: events.append("source"),
    )
    monkeypatch.setattr(
        source_extension_producer,
        "_provision_recursive_submodules",
        lambda *a: events.append("provision-submodules"),
    )
    monkeypatch.setattr(
        source_extension_producer,
        "_verify_recursive_submodules",
        lambda *a: events.append("verify-submodules"),
    )
    pact._prepare_source_extension_producer(plan)
    assert events == [
        "source",
        "provision-submodules",
        "verify-submodules",
        ("environment", {"provision": True}),
    ]


def test_source_root_environment_is_owner_published_only() -> None:
    assert execution_environment.environment_override_policy_error(
        {"MOLT_PROOF_SOURCE_ROOT": "/shadow"}
    )
    environment, contract = execution_environment._deterministic_execution_environment(
        {"MOLT_PROOF_SOURCE_ROOT": "/shadow"},
        override_names=[],
    )
    assert "MOLT_PROOF_SOURCE_ROOT" not in environment
    assert "MOLT_PROOF_SOURCE_ROOT" in contract["omitted_names"]


@pytest.mark.parametrize(
    "invalid", ["file", "nonempty", "parent-file", "in-source", "contains-source"]
)
def test_queue_plan_rejects_invalid_build_topology_before_environment(
    producer_request,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    invalid: str,
) -> None:
    invocation, _ = producer_request
    build = tmp_path / "build"
    if invalid == "file":
        build.write_bytes(b"retain")
    elif invalid == "nonempty":
        build.mkdir()
        (build / "retain").write_bytes(b"retain")
    elif invalid == "parent-file":
        build.write_bytes(b"retain")
        build = build / "child"
    elif invalid == "in-source":
        build = Path(invocation.source) / "build"
    else:
        build = tmp_path
    monkeypatch.setattr(
        pact,
        "source_build_environment",
        lambda *a, **kw: pytest.fail(
            "invalid queue plan reached environment selection"
        ),
    )
    with pytest.raises(
        ValueError, match="disjoint|not a directory|prior configuration"
    ):
        pact._source_extension_producer_plan(
            package=invocation.package,
            package_version=invocation.package_version,
            module_set=invocation.module_set,
            python_version=invocation.python_version,
            source=invocation.source,
            build_root=str(build),
            target=invocation.target,
            abi_tier=invocation.abi_tier,
            repo_root=tmp_path,
        )


def test_queue_preparation_revalidates_build_topology_before_mutation(
    producer_request,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    from molt.cli import source_extension_producer

    invocation, environment = producer_request
    monkeypatch.setattr(pact, "source_build_environment", lambda *a: environment)
    plan = pact._source_extension_producer_plan(
        package=invocation.package,
        package_version=invocation.package_version,
        module_set=invocation.module_set,
        python_version=invocation.python_version,
        source=invocation.source,
        build_root=invocation.build_root,
        target=invocation.target,
        abi_tier=invocation.abi_tier,
        repo_root=tmp_path,
    )
    plan.build_root.mkdir()
    retained = plan.build_root / "retained"
    retained.write_bytes(b"retain")
    monkeypatch.setattr(
        source_extension_producer,
        "prepare_source_extension_prerequisites",
        lambda *a, **kw: pytest.fail("stale queue plan mutated prerequisites"),
    )
    with pytest.raises(ValueError, match="prior configuration"):
        pact._prepare_source_extension_producer(plan)
    assert retained.read_bytes() == b"retain"


def test_target_derived_capture_receives_selected_environment(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from tools import proof_plan
    from tools.proof_queue_pkg import target_derived_toolchains

    selected = {"CC": str(tmp_path / "clang")}
    observed = []
    monkeypatch.setattr(
        target_derived_toolchains,
        "capture_identity",
        lambda policy, envelope, *, environment: (
            observed.append(environment) or {"fixture": True}
        ),
    )
    result = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "source-extension",
        {"typed_command": {}},
        ["python"],
        cwd=tmp_path,
        env=selected,
    )
    assert result == {"fixture": True}
    assert observed == [selected]


def test_sysroot_directory_is_in_live_custody(tmp_path: Path) -> None:
    assert execution_environment._broad_toolchain_roots(
        {"source-extension": {"sysroot_custody": {"root": str(tmp_path)}}}
    ) == [tmp_path.resolve()]
