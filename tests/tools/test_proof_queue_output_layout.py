"""Portable output placement keeps immutable proof authority on its own root."""

from __future__ import annotations

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import shlex
import shutil
import sqlite3
import sys
from types import SimpleNamespace

import pytest

from molt import disk_capacity
from tools import proof_command_argv
from tools.proof_queue_pkg import supervisor_generation
from tools.proof_queue_pkg import (
    cargo_cache_custody as cache,
    cargo_output_environment,
    cargo_output_layout as layout,
    cli,
    command_admission as admission,
    commands,
    custody_cas,
    execution_environment,
    execution_custody,
    guarded_execution,
    pact,
    policy,
    presentation,
    runner,
    scheduling,
    supervisor_custody,
    state,
)
from tests.process_guard_common import install_module_view, run_guarded_test_process
from tests.test_custody_layout import git_checkout as git_checkout
from tests.test_dx_run_context import _git
from tools.git_identity import repository_source_paths


pytestmark = pytest.mark.usefixtures("cargo_output_implementation_source")


@pytest.fixture
def linux_input_custody(monkeypatch):
    """Model admitted input custody for controls that stop before a watcher."""
    install_module_view(
        monkeypatch, "sys", execution_custody.sys, execution_custody, platform="linux"
    )

    def forbidden(*_args, **_kwargs):
        pytest.fail("output-layout controls must stop before a custody monitor starts")

    monkeypatch.setattr(execution_custody.LiveCustodyMonitor, "__enter__", forbidden)


def _root(tmp_path):
    root = tmp_path / "selected-volume"
    root.mkdir()
    return root


@pytest.mark.parametrize("external", [False, True])
def test_compact_target_binds_full_identity_without_repeating_metadata(
    tmp_path, external
):
    metadata = tmp_path / "receipts"
    declaration = layout.declare_root(str(_root(tmp_path))) if external else None
    selected = layout.CargoOutputLayout.create(
        result_root=metadata, declaration=declaration
    )
    root = Path(declaration["path"]) if declaration else metadata
    target = selected.target("a" * 64, "b" * 16)
    assert target.parent == root / "cargo-target"
    assert len(target.name) == 64
    assert target == selected.target("a" * 64, "b" * 16)
    assert target != selected.target("a" * 63 + "c", "b" * 16)
    assert target != selected.target("a" * 64, "b" * 15 + "c")
    other = layout.CargoOutputLayout.create(
        result_root=tmp_path / "other-receipts", declaration=declaration
    )
    assert target != other.target("a" * 64, "b" * 16)
    legacy = selected.target(
        "a" * 64, "b" * 16, version=layout.recorded_target_layout({})
    )
    assert (
        legacy
        == selected.payload_root / "cargo-cache" / ("a" * 64) / ("b" * 16) / "target"
    )
    assert target != legacy
    assert selected.targets_root in selected.capacity_paths()
    assert selected.payload_root / "cargo-cache" in selected.capacity_paths()
    assert not target.exists() and not legacy.exists()


@pytest.mark.parametrize("version", [None, True, 2, "v2", "unknown", []])
def test_target_layout_rejects_unknown_or_substituted_versions(version):
    with pytest.raises(ValueError, match="layout version"):
        layout.recorded_target_layout({"cargo_target_layout": version})


def test_historical_projection_keeps_absent_discriminator():
    assert layout.target_layout_fields({}) == {}
    current = {"cargo_target_layout": layout.TARGET_LAYOUT}
    assert layout.target_layout_fields(current) == current
    with pytest.raises(ValueError, match="layout binding"):
        layout.require_same_target_layout({}, current)


def test_windows_target_budget_counts_utf16_before_creating_outputs(tmp_path):
    selected = layout.CargoOutputLayout(
        tmp_path / ("\U0001f600" * 35) / ("\U0001f600" * 35)
    )
    selected.admit_target_path(platform="linux")
    with pytest.raises(
        ValueError, match="target_utf16_units=.*shorter --cargo-output-root"
    ):
        selected.admit_target_path(platform="win32")
    assert not selected.result_root.exists()


def test_windows_supervisor_budget_is_independent_of_payload_toolchains(tmp_path):
    root = tmp_path / ("\U0001f600" * 35) / ("\U0001f600" * 35)
    root.mkdir(parents=True)
    selected = layout.CargoOutputLayout(
        tmp_path / "receipts", layout.declare_root(str(root))
    )
    selected.admit_supervisor_target_path(platform="linux")
    with pytest.raises(ValueError, match="reserved_descendant_units=128; limit=259"):
        selected.admit_supervisor_target_path(platform="win32")
    assert not selected.result_root.exists()


@pytest.mark.parametrize("units,accepted", [(131, True), (132, False)])
def test_windows_cargo_family_shares_exact_tool_budget(units, accepted):
    # Use a syntactic path only: admission does not create or select a root.
    target = Path("x" * units)
    if accepted:
        layout.CargoOutputLayout.admit_cargo_path(target, platform="win32")
    else:
        with pytest.raises(ValueError, match="no fallback is permitted"):
            layout.CargoOutputLayout.admit_cargo_path(target, platform="win32")
    assert not target.exists()


def test_actual_supervisor_provisioning_checks_target_before_launch(
    tmp_path, monkeypatch
):
    target = tmp_path / ("long-supervisor-output-" * 10)
    calls = []
    install_module_view(monkeypatch, "sys", sys, layout, platform="win32")
    monkeypatch.setattr(
        supervisor_custody.command_identity,
        "_run_captured",
        lambda *args, **kwargs: calls.append(args),
    )
    with pytest.raises(ValueError, match="shorter --cargo-output-root"):
        supervisor_generation.provision(
            cwd=tmp_path, env={"CARGO_TARGET_DIR": str(target)}
        )
    assert calls == []
    assert not target.exists()


def test_actual_supervisor_provisioning_requires_absolute_explicit_target(tmp_path):
    with pytest.raises(ValueError, match="explicit absolute Cargo target"):
        supervisor_generation.provision(
            cwd=tmp_path, env={"CARGO_TARGET_DIR": "relative-target"}
        )


def test_python_payload_rejects_supervisor_path_before_preflight_or_command(
    tmp_path, monkeypatch, linux_input_custody
):
    source = tmp_path / "source"
    source.mkdir()
    receipts = tmp_path / "receipts"
    receipts.mkdir()
    result = receipts / "result.json"
    request = receipts / "request.json"
    command = [sys.executable, "-c", "raise AssertionError('must not run')"]
    output = tmp_path / ("long-control-plane-path-" * 5)
    output.mkdir()
    envelope = admission.envelope_for_command(command, cargo_output_root=str(output))
    assert "cargo" not in envelope["toolchains"]
    request.write_text(
        json.dumps(
            {
                "supervisor_artifact_root": layout.CargoOutputLayout.for_envelope(
                    envelope, result_root=Path(str(result)).parent
                ).supervisor_artifact_root,
                "schema": admission.EXECUTION_SCHEMA,
                "run_id": "path-admission",
                "execution_nonce": "a" * 64,
                "command": command,
                "envelope": envelope,
                "cwd": str(source),
                "resource_family": "python-tests",
                "result_path": str(result),
                "timeout_seconds": 30,
            }
        ),
        encoding="utf-8",
    )
    admit = layout.CargoOutputLayout.admit_supervisor_target_path
    monkeypatch.setattr(
        layout.CargoOutputLayout,
        "admit_supervisor_target_path",
        lambda self: admit(self, platform="win32"),
    )
    calls = []
    monkeypatch.setattr(
        policy,
        "_ensure_run_toolchain_preflight",
        lambda **kwargs: calls.append("preflight"),
    )
    monkeypatch.setattr(
        supervisor_generation,
        "provision",
        lambda **kwargs: calls.append("provision"),
    )
    assert guarded_execution.execute_guarded_request(request) == 2
    observation = json.loads(result.read_text(encoding="utf-8"))
    assert observation["command_started"] is False
    assert "Windows tool path budget" in observation["error"]
    assert calls == []
    assert not (receipts / "proof-supervisor-target").exists()


def test_supervisor_boundary_rejects_root_replaced_during_preflight(
    tmp_path, monkeypatch, linux_input_custody
):
    source = tmp_path / "source"
    source.mkdir()
    receipts = tmp_path / "receipts"
    receipts.mkdir()
    selected_root = tmp_path / "owned-output"
    selected_root.mkdir()
    declaration = layout.declare_root(str(selected_root))
    result = receipts / "result.json"
    request = receipts / "request.json"
    command = [sys.executable, "-c", "raise AssertionError('must not run')"]
    envelope = admission.envelope_for_command(
        command, cargo_output_root=str(selected_root)
    )
    request.write_text(
        json.dumps(
            {
                "supervisor_artifact_root": layout.CargoOutputLayout.for_envelope(
                    envelope, result_root=Path(str(result)).parent
                ).supervisor_artifact_root,
                "schema": admission.EXECUTION_SCHEMA,
                "run_id": "root-replacement",
                "execution_nonce": "a" * 64,
                "command": command,
                "envelope": envelope,
                "cwd": str(source),
                "resource_family": "python-tests",
                "result_path": str(result),
                "timeout_seconds": 30,
            }
        ),
        encoding="utf-8",
    )
    # This model selects the unrestricted POSIX path-budget coordinate on the
    # host. It tests real directory identity replacement, not native POSIX use.
    admit = layout.CargoOutputLayout.admit_supervisor_target_path
    monkeypatch.setattr(
        layout.CargoOutputLayout,
        "admit_supervisor_target_path",
        lambda self: admit(self, platform="linux"),
    )
    admit_image = custody_cas.admit_executable_path
    monkeypatch.setattr(
        custody_cas,
        "admit_executable_path",
        lambda root, name: admit_image(root, name, platform="linux"),
    )
    calls = []

    def replace_root(**kwargs):
        selected_root.rename(tmp_path / "retained-original-output")
        selected_root.mkdir()
        calls.append("preflight")
        return []

    monkeypatch.setattr(policy, "_ensure_run_toolchain_preflight", replace_root)
    monkeypatch.setattr(
        supervisor_generation,
        "provision",
        lambda **kwargs: calls.append("provision"),
    )
    assert guarded_execution.execute_guarded_request(request) == 2
    observation = json.loads(result.read_text(encoding="utf-8"))
    assert observation["command_started"] is False
    assert "replaced or remounted" in observation["error"]
    assert calls == ["preflight"]
    assert list(selected_root.iterdir()) == []
    with pytest.raises(ValueError, match="replaced or remounted"):
        layout.validate_root(declaration)


def test_current_target_namespace_cannot_be_redirected(tmp_path):
    selected = layout.CargoOutputLayout.create(result_root=tmp_path / "receipts")
    selected.result_root.mkdir()
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    try:
        (selected.result_root / "cargo-target").symlink_to(
            elsewhere, target_is_directory=True
        )
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    with pytest.raises(ValueError, match="link|junction"):
        selected.target("a" * 64, "b" * 16)


@pytest.mark.parametrize(
    "command",
    [
        ["cargo", "check"],
        ["cargo", "test"],
        ["cargo", "test", "--no-run"],
        ["cargo", "build"],
    ],
)
def test_placement_is_independent_of_retention_and_preserves_historical_default(
    tmp_path, command
):
    root = _root(tmp_path)
    old = admission.envelope_for_command(command)
    assert "cargo_output_root" not in old
    envelope = admission.envelope_for_command(command, cargo_output_root=str(root))
    admission.validate_envelope(envelope, command)
    assert admission.validated_cargo_output_lifetime(envelope) == "retain"
    assert envelope["cargo_output_root"] == layout.declare_root(str(root))


@pytest.mark.parametrize("delegated", [False, True])
def test_registered_workspace_clippy_places_outputs_without_disposal_permission(
    tmp_path, delegated
):
    command = list(
        next(
            row.argv
            for row in admission.proof_plan.ProofPlan.load().commands
            if row.id == "portability.rust.linux-aarch64.clippy-workspace"
        )
    )
    if delegated:
        command = policy._canonical_cargo_proof_command(command[1:])
    original = admission.envelope_for_command(command)
    root = _root(tmp_path)
    envelope = admission.envelope_for_command(command, cargo_output_root=str(root))
    assert envelope == {**original, "cargo_output_root": layout.declare_root(str(root))}
    assert (
        admission.admission_envelope(command, cargo_output_root=str(root)) == envelope
    )
    admission.validate_envelope(envelope, command)
    assert admission.validated_cargo_output_lifetime(envelope) == "retain"
    assert list(root.iterdir()) == []

    outputs = cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope)
    target = root / "leased-target"
    target.mkdir()
    bound = outputs.bind({"CARGO_TARGET_DIR": str(root / "old-target")}, target=target)
    assert bound["CARGO_TARGET_DIR"] == str(target)
    outputs.validate(bound, target=target)
    with pytest.raises(ValueError, match="differs"):
        outputs.validate({**bound, "CARGO_TARGET_DIR": str(root)}, target=target)
    with pytest.raises(ValueError, match="terminal-success"):
        admission.envelope_for_command(
            command,
            cargo_output_root=str(root),
            cargo_output_lifetime="terminal-success",
        )
    with pytest.raises(ValueError, match="terminal-success"):
        admission.validate_envelope(
            {**envelope, "cargo_output_lifetime": "terminal-success"}, command
        )


@pytest.mark.parametrize(
    "command",
    [
        ["cargo", "--version"],
        ["cargo", "check", "--help"],
        ["cargo", "clippy", "--target-dir", "elsewhere"],
        ["cargo", "clippy", "--target-dir=elsewhere"],
        ["cargo", "clippy", "--artifact-dir", "elsewhere"],
        ["cargo", "clippy", "--artifact-dir=elsewhere"],
        ["cargo", "check", "--target-dir", "elsewhere"],
        ["cargo", "check", "--target-dir=elsewhere"],
        ["cargo", "test", "--artifact-dir", "elsewhere"],
        ["cargo", "test", "--artifact-dir=elsewhere"],
    ],
)
@pytest.mark.parametrize("delegated", [False, True])
def test_cargo_query_and_secondary_output_options_cannot_select_root(
    tmp_path, command, delegated
):
    if delegated:
        command = [
            "uv",
            "run",
            "--no-sync",
            "python",
            "tools/guarded_exec.py",
            "--",
            *command,
        ]
    with pytest.raises(ValueError):
        admission.envelope_for_command(command, cargo_output_root=str(_root(tmp_path)))


@pytest.mark.parametrize(
    "command_id",
    [
        "wasm.build.shared-runtime",
        "wasm.build.split-runtime-release",
        "rust.test.default-truth",
    ],
)
def test_registered_python_cargo_driver_places_retained_outputs(tmp_path, command_id):
    command = list(
        next(
            row.argv
            for row in admission.proof_plan.ProofPlan.load().commands
            if row.id == command_id
        )
    )
    original = admission.envelope_for_command(command)
    assert "cargo" in original["toolchains"]
    assert admission.cargo_invocation_for_envelope(original) is None
    root = _root(tmp_path)
    envelope = admission.envelope_for_command(command, cargo_output_root=str(root))
    assert envelope == {**original, "cargo_output_root": layout.declare_root(str(root))}
    assert list(root.iterdir()) == []
    admission.validate_envelope(envelope, command)
    assert admission.validated_cargo_output_lifetime(envelope) == "retain"

    outputs = cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope)
    assert outputs.external_placement and outputs.documentation
    target = root / "leased-target"
    target.mkdir()
    scratch = root / "admitted-scratch"
    scratch.mkdir()
    bound = outputs.bind(
        {
            "CARGO_BUILD_TARGET": "wasm32-wasip1",
            "MOLT_PROOF_SCRATCH_ROOT": str(scratch),
        },
        target=target,
    )
    assert bound["CARGO_BUILD_TARGET"] == "wasm32-wasip1"
    assert set(bound) == {
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "MOLT_CARGO_BUILD_DIR_BASE",
        "TEMP",
        "TMP",
        "TMPDIR",
        "PYTHONPYCACHEPREFIX",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_DIFF_TMPDIR",
        "MOLT_COMPAT_SCRATCH_ROOT",
        "MOLT_PROOF_SCRATCH_ROOT",
    }
    assert all(bound[name] == str(target) for name in outputs.build_names)
    assert (
        bound["MOLT_DIFF_TMPDIR"] == bound["MOLT_COMPAT_SCRATCH_ROOT"] == str(scratch)
    )
    outputs.validate(bound, target=target)
    with pytest.raises(ValueError, match="differs"):
        outputs.validate({**bound, "CARGO_TARGET_DIR": str(root)}, target=target)
    with pytest.raises(ValueError, match="explicit Cargo"):
        admission.envelope_for_command(
            command,
            cargo_output_root=str(root),
            cargo_output_lifetime="terminal-success",
        )
    with pytest.raises(ValueError, match="explicit Cargo"):
        admission.validate_envelope(
            {**envelope, "cargo_output_lifetime": "terminal-success"}, command
        )


def test_typed_python_root_places_supervisor_without_granting_cargo_permissions(
    tmp_path,
):
    command = [sys.executable, "-c", "print(1)"]
    original = admission.envelope_for_command(command)
    root = _root(tmp_path)
    envelope = admission.envelope_for_command(command, cargo_output_root=str(root))
    assert envelope == {**original, "cargo_output_root": layout.declare_root(str(root))}
    assert "cargo" not in envelope["toolchains"]
    assert envelope["process_closure"]["descendants"] == "forbidden"
    assert admission.validated_cargo_output_lifetime(envelope) == "retain"
    admission.validate_envelope(envelope, command)
    with pytest.raises(ValueError, match="explicit Cargo"):
        admission.envelope_for_command(
            command,
            cargo_output_root=str(root),
            cargo_output_lifetime="terminal-success",
        )
    forged = {**envelope, "toolchains": [*envelope["toolchains"], "cargo"]}
    with pytest.raises(ValueError):
        admission.validate_envelope(forged, command)


@pytest.mark.parametrize("raw", [True, {}, [], "", "relative"])
def test_root_is_strict_and_never_provisioned_by_admission(raw):
    with pytest.raises(ValueError):
        layout.declare_root(raw)


def test_missing_root_has_no_fallback_and_creates_nothing(tmp_path):
    missing = tmp_path / "not-mounted" / "output"
    with pytest.raises(ValueError, match="unavailable"):
        layout.declare_root(str(missing))
    assert not missing.parent.exists()


def test_link_root_is_not_resolved_into_custody(tmp_path):
    root = _root(tmp_path)
    link = tmp_path / "linked-output"
    try:
        link.symlink_to(root, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    with pytest.raises(ValueError, match="link|junction"):
        layout.declare_root(str(link))


def test_historical_envelope_validation_is_offline_but_live_custody_is_not(
    tmp_path, monkeypatch
):
    command = ["cargo", "test", "--no-run"]
    envelope = admission.envelope_for_command(
        command, cargo_output_root=str(_root(tmp_path))
    )

    def absent(raw):
        raise OSError("selected volume disconnected")

    monkeypatch.setattr(layout, "declare_root", absent)
    admission.validate_envelope(envelope, command)
    with pytest.raises(ValueError, match="unavailable"):
        layout.validate_root(envelope["cargo_output_root"])


def test_remounted_or_forged_root_identity_is_not_current_custody(tmp_path):
    declaration = layout.declare_root(str(_root(tmp_path)))
    forged = {
        **declaration,
        "identity": {**declaration["identity"], "file_id": "f" * 32},
    }
    with pytest.raises(ValueError, match="replaced or remounted"):
        layout.validate_root(forged)
    with pytest.raises(ValueError, match="invalid durable"):
        layout.declared_root(
            {**declaration, "identity": {**declaration["identity"], "file_id": True}}
        )


@pytest.mark.parametrize("error", [PermissionError("denied"), OSError("I/O failure")])
def test_terminal_absence_does_not_suppress_filesystem_errors(error):
    def stat(**kwargs):
        raise error

    with pytest.raises(type(error)):
        cache._terminal_target_present(SimpleNamespace(stat=stat), {})


def test_terminal_absence_requires_selected_media_still_present(tmp_path, monkeypatch):
    declaration = layout.declare_root(str(_root(tmp_path)))

    def stat(**kwargs):
        raise FileNotFoundError("missing target")

    target = SimpleNamespace(stat=stat)
    assert not cache._terminal_target_present(
        target, {"cargo_output_root": declaration}
    )

    def absent(raw):
        raise OSError("volume disappeared during terminal validation")

    monkeypatch.setattr(layout, "declare_root", absent)
    with pytest.raises(ValueError, match="unavailable"):
        cache._terminal_target_present(target, {"cargo_output_root": declaration})


@pytest.mark.parametrize("protected", ["metadata", "source", "toolchain"])
def test_overlap_is_refused_before_payload_creation(tmp_path, protected):
    root = _root(tmp_path)
    declaration = layout.declare_root(str(root))
    with pytest.raises(ValueError, match="overlaps protected"):
        if protected == "metadata":
            layout.CargoOutputLayout.create(
                result_root=root / "receipts", declaration=declaration
            )
        elif protected == "source":
            layout.CargoOutputLayout.create(
                result_root=tmp_path / "receipts",
                source_root=root / "source",
                declaration=declaration,
            )
        else:
            selected = layout.CargoOutputLayout.create(
                result_root=tmp_path / "receipts", declaration=declaration
            )
            selected.validate(protected_roots=[root / "tools"])
    assert list(root.iterdir()) == []


def test_one_layout_places_all_payload_siblings_without_moving_metadata(tmp_path):
    root = _root(tmp_path)
    selected = layout.CargoOutputLayout.create(
        result_root=tmp_path / "receipts", declaration=layout.declare_root(str(root))
    )
    assert selected.result_root == tmp_path / "receipts"
    paths = [
        selected.target("a" * 64, "b" * 16),
        selected.selection,
        selected.supervisor_target,
        selected.temporary,
        selected.scratch("c" * 64),
        *selected.capacity_paths(),
    ]
    assert all(path.is_relative_to(root) for path in paths)
    assert not selected.payload_root.exists()
    other = layout.CargoOutputLayout.create(
        result_root=tmp_path / "other-receipts", declaration=selected.declaration
    )
    assert other.payload_root != selected.payload_root


@pytest.mark.parametrize("reverse", [False, True])
def test_overlap_uses_directory_identity_for_alias_ancestry(
    tmp_path, monkeypatch, reverse
):
    root = tmp_path / "selected"
    alias = tmp_path / "alias"
    descendant = alias / "not-yet-created" / "payload"
    identities = {root: (7, 11), alias: (7, 11), tmp_path: (7, 12)}
    monkeypatch.setattr(layout, "_directory_identity", identities.get)
    left, right = (descendant, root) if reverse else (root, descendant)
    assert layout._overlaps(left, right)
    assert not layout._overlaps(root, tmp_path / "unrelated")


@pytest.mark.parametrize(
    "authority",
    [
        "CARGO_HOME",
        "RUSTUP_HOME",
        "VIRTUAL_ENV",
        "SCCACHE_DIR",
        "cargo",
        "rustc",
        "rustup",
        "uv",
    ],
)
def test_shared_environment_boundary_refuses_toolchain_overlap_before_provision(
    tmp_path, monkeypatch, authority
):
    root = _root(tmp_path)
    selected = layout.CargoOutputLayout.create(
        result_root=tmp_path / "receipts", declaration=layout.declare_root(str(root))
    )
    env = {authority: str(root / "tools")} if authority.isupper() else {}
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        layout,
        which=lambda name, **kwargs: (
            str(root / "bin" / name) if name == authority else None
        ),
    )
    with pytest.raises(ValueError, match="overlaps protected"):
        selected.validate_environment(env)
    assert list(root.iterdir()) == []


@pytest.mark.parametrize("overlap", ["physical", "lexical", "neither"])
def test_read_only_toolchain_aliases_protect_names_and_destinations(tmp_path, overlap):
    root = _root(tmp_path)
    toolchain = (root if overlap == "physical" else tmp_path) / "toolchain"
    toolchain.mkdir()
    alias = (root if overlap == "lexical" else tmp_path) / "toolchain-alias"
    try:
        alias.symlink_to(toolchain, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    selected = layout.CargoOutputLayout.create(
        result_root=tmp_path / "receipts", declaration=layout.declare_root(str(root))
    )
    env = {"CARGO_HOME": str(alias)}
    if overlap == "neither":
        selected.validate_environment(env)
    else:
        with pytest.raises(ValueError, match="overlaps protected"):
            selected.validate_environment(env)
    assert not selected.payload_root.exists()
    assert alias.resolve() == toolchain


def test_explicit_role_binding_owns_temps_without_changing_default_check_inputs(
    tmp_path,
):
    root = _root(tmp_path)
    command = ["cargo", "check"]
    default = cargo_output_environment.CargoOutputEnvironment.for_envelope(
        admission.envelope_for_command(command)
    )
    selected = cargo_output_environment.CargoOutputEnvironment.for_envelope(
        admission.envelope_for_command(command, cargo_output_root=str(root))
    )
    env = {
        "TEMP": "caller-temp",
        "TMP": "caller-tmp",
        "TMPDIR": "caller-tmpdir",
        "PYTHONPYCACHEPREFIX": "caller-cache",
        "MOLT_DIFF_TMPDIR": "caller-diff-payload",
    }
    assert default.caller_environment(env) == env
    target = root / "target"
    target.mkdir()
    bound = selected.bind(env, target=target)
    assert all(bound[name] == str(target) for name in selected.names)
    assert bound["MOLT_DIFF_TMPDIR"] == "caller-diff-payload"
    selected.validate(bound, target=target)
    with pytest.raises(ValueError, match="differs"):
        selected.validate({**bound, "TMPDIR": "escaped"}, target=target)


@pytest.mark.parametrize("name", ["CARGO_BUILD_TARGET_DIR"])
def test_secondary_environment_output_escape_is_refused(tmp_path, name):
    outputs = cargo_output_environment.CargoOutputEnvironment(
        False, external_placement=True
    )
    with pytest.raises(ValueError, match="bypasses declared"):
        execution_environment._require_cargo_build_tool_environment_context(
            admission.envelope_for_command(["cargo", "check"]),
            outputs=outputs,
            cwd=tmp_path,
            env={name: "elsewhere"},
        )


@pytest.mark.parametrize("field", ["build-dir", "target-dir"])
@pytest.mark.parametrize("form", ["inline", "file"])
@pytest.mark.parametrize("delegated", [False, True])
def test_config_output_escape_is_refused(tmp_path, field, form, delegated):
    config = tmp_path / "config.toml"
    config.write_text(f'[build]\n{field} = "elsewhere"\n', encoding="utf-8")
    command = [
        "cargo",
        "check",
        "--config",
        str(config) if form == "file" else f'build.{field}="elsewhere"',
    ]
    if delegated:
        command = ["python", "tools/guarded_exec.py", "--", *command]
    with pytest.raises(ValueError, match="bypasses proof output custody"):
        execution_environment._require_cargo_build_tool_environment_context(
            admission.envelope_for_command(command),
            outputs=cargo_output_environment.CargoOutputEnvironment(False),
            cwd=tmp_path,
            env={"CARGO_HOME": str(tmp_path / "cargo-home")},
        )


def test_file_output_defaults_remain_below_leased_environment(tmp_path):
    target = tmp_path / "leased"
    target.mkdir()
    config = tmp_path / ".cargo" / "config.toml"
    config.parent.mkdir()
    config.write_text(
        '[build]\ntarget-dir="outside-final"\nbuild-dir="outside-build"\n',
        encoding="utf-8",
    )
    outputs = cargo_output_environment.CargoOutputEnvironment(
        True, external_placement=True
    )
    bound = outputs.bind(
        {
            "CARGO_HOME": str(tmp_path / "cargo-home"),
            "CARGO_BUILD_BUILD_DIR": "inherited-escape",
        },
        target=target,
    )
    effective = execution_environment._require_cargo_build_tool_environment_context(
        admission.envelope_for_command(["cargo", "doc"]),
        outputs=outputs,
        cwd=tmp_path,
        env=bound,
    )
    outputs.validate(effective, target=target)
    assert effective["CARGO_BUILD_BUILD_DIR"] == str(target)
    config.write_text('[build]\nartifact-dir="outside-artifact"\n', encoding="utf-8")
    with pytest.raises(ValueError, match="redirects output"):
        execution_environment._require_cargo_build_tool_environment_context(
            admission.envelope_for_command(["cargo", "doc"]),
            outputs=outputs,
            cwd=tmp_path,
            env=bound,
        )


@pytest.mark.parametrize("insert", [scheduling._insert_run, scheduling._admit_run])
def test_root_identity_survives_immutable_queue_and_detached_request(tmp_path, insert):
    root = _root(tmp_path)
    source = tmp_path / "source"
    source.mkdir()
    metadata = tmp_path / "receipts"
    metadata.mkdir()
    command = policy._canonical_cargo_proof_command(["test", "--no-run"])
    with closing(state._connect(metadata / "queue.sqlite3")) as conn:
        insert(
            conn,
            run_id="placement",
            logical_id="placement",
            reason="retained producer",
            command=command,
            cwd=source,
            resource_family="rust",
            contention_key="rust",
            scopes=[],
            git_snapshot={},
            log_path=metadata / "unit.log",
            summary_json=metadata / "guard.json",
            cargo_output_root=str(root),
        )
        row = state._row_by_run_id(conn, "placement")
        envelope = json.loads(row["command_envelope_json"])
        path, _result, request_envelope, _nonce = runner._write_execution_request(
            output_layout=layout.CargoOutputLayout.for_envelope(
                envelope, result_root=(metadata / "unit.log").parent
            ),
            row=row,
            command=command,
            repo_root=source,
            resource_family="rust",
            run_id="placement",
            env_override_names=[],
            log_path=metadata / "unit.log",
            summary_path=metadata / "guard.json",
            timeout_seconds=10,
        )
        assert (
            request_envelope
            == envelope
            == json.loads(path.read_text(encoding="utf-8"))["envelope"]
        )
        assert envelope["cargo_output_root"] == layout.declare_root(str(root))
        assert "cargo_output_lifetime" not in envelope
        with pytest.raises(sqlite3.IntegrityError):
            conn.execute(
                "UPDATE proof_runs SET command_envelope_json='{}' WHERE run_id='placement'"
            )


@pytest.mark.parametrize("detach", [False, True])
def test_cli_propagates_root_without_opt_in_to_disposal(tmp_path, monkeypatch, detach):
    root = _root(tmp_path)
    captured = {}

    def submit(args, **kwargs):
        captured.update(kwargs)
        return (2, None) if detach else 0

    monkeypatch.setattr(runner, "_queue_one" if detach else "_run_one", submit)
    cli.main(
        [
            "cargo",
            "--id",
            "unit",
            "--reason",
            "retained producer",
            "--cargo-output-root",
            str(root),
            *(["--detach"] if detach else []),
            "--",
            "test",
            "--no-run",
        ]
    )
    assert captured["cargo_output_root"] == str(root)
    assert captured["cargo_output_lifetime"] == "retain"


@pytest.mark.parametrize("protected", ["source", "receipts"])
def test_supervisor_store_rejects_overlapping_custody(
    tmp_path, cargo_output_implementation_source, protected
):
    root = (
        cargo_output_implementation_source if protected == "source" else _root(tmp_path)
    )
    metadata = (
        root / "proof-supervisor" if protected == "receipts" else tmp_path / "receipts"
    )
    selected = layout.CargoOutputLayout(metadata, layout.declare_root(str(root)))
    with pytest.raises(ValueError, match="overlaps source or receipt custody"):
        _ = selected.supervisor_store
    assert not (root / "proof-supervisor").exists()


@pytest.mark.parametrize(
    "placement", ["durable", "scratch", "env", "external", "inside-other-source"]
)
def test_supervisor_storage_never_enters_an_enclosing_source_checkout(
    tmp_path, monkeypatch, git_checkout, placement
):
    from molt import dx

    source = git_checkout / ".claude" / "worktrees" / "agent"
    _git(git_checkout, "worktree", "add", "-q", "--detach", str(source))
    monkeypatch.setattr(layout, "implementation_source_root", lambda: source)
    for key in (
        "MOLT_EXT_ROOT",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
        "MOLT_EXTERNAL_ARTIFACT_ROOTS",
        "MOLT_EXTERNAL_MIN_FREE_GB",
    ):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(
        dx,
        "_host_scratch_roots",
        lambda: (tmp_path if placement == "scratch" else tmp_path / "host-temp",),
    )
    declaration = None
    if placement in {"durable", "env", "external", "inside-other-source"}:
        root = tmp_path / "operator-output"
        root.mkdir()
        if placement == "inside-other-source":
            _git(root, "init", "-q")
        if placement == "durable":
            monkeypatch.setattr(
                disk_capacity, "_default_measure_free_bytes", lambda _: 64 * 1024**3
            )
            dx.set_artifact_root_binding(source, root, {})
        elif placement == "env":
            monkeypatch.setenv("MOLT_EXT_ROOT", str(root))
        else:
            declaration = layout.declare_root(str(root))
    selected = layout.CargoOutputLayout(tmp_path / "receipts", declaration)

    def run_git(root, arguments):
        result = run_guarded_test_process(
            ["git", "-C", str(root), *arguments], text=False, check=True, timeout=30
        )
        assert isinstance(result.stdout, bytes)
        return result.stdout

    before = repository_source_paths(
        git_checkout, run_git=run_git, include_untracked=True
    )
    if placement in {"inside-other-source", "scratch"}:
        with pytest.raises(ValueError, match="overlaps source or receipt custody"):
            _ = selected.supervisor_store
        rejected_root = root if placement == "inside-other-source" else source
        assert not (rejected_root / "proof-supervisor").exists()
        return
    store = selected.supervisor_store
    assert store.parent == root / "proof-supervisor"
    assert not store.is_relative_to(git_checkout)
    assert not store.is_relative_to(source)
    store.mkdir(parents=True)
    (store / "identity.json").write_bytes(b"retained tool identity")
    assert (
        repository_source_paths(git_checkout, run_git=run_git, include_untracked=True)
        == before
    )


def test_supervisor_store_refuses_an_unbound_plain_clone(tmp_path, monkeypatch):
    source = tmp_path / "plain-clone"
    source.mkdir()
    _git(source, "init", "-q", "-b", "main")
    monkeypatch.setattr(layout, "implementation_source_root", lambda: source)
    for key in (
        "MOLT_EXT_ROOT",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
        "MOLT_EXTERNAL_ARTIFACT_ROOTS",
    ):
        monkeypatch.delenv(key, raising=False)
    selected = layout.CargoOutputLayout(tmp_path / "receipts")
    with pytest.raises(ValueError, match="artifact-root --bind"):
        _ = selected.supervisor_store
    with pytest.raises(ValueError, match="artifact-root --bind"):
        selected.bind_supervisor_artifact_root({})
    assert not (source / "proof-supervisor").exists()
    assert not (source.parent / "proof-supervisor").exists()


@pytest.mark.parametrize("root_source", ["env", "binding", "declared"])
@pytest.mark.parametrize("name_rule", ["exact", "windows"])
def test_request_carries_parent_supervisor_selection_without_child_environment(
    tmp_path, monkeypatch, git_checkout, root_source, name_rule, linux_input_custody
):
    import hashlib
    from molt import dx
    from molt.process_environment import NameRule, ProcessEnvironment

    source = git_checkout.resolve()
    root = tmp_path / "selected-artifacts"
    root.mkdir()
    receipts = tmp_path / "receipts"
    receipts.mkdir()
    monkeypatch.setattr(layout, "implementation_source_root", lambda: source)
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda _: 64 * 1024**3
    )
    if root_source == "binding":
        dx.set_artifact_root_binding(source, root, {})
    rule = NameRule.EXACT if name_rule == "exact" else NameRule.WINDOWS
    key = "MOLT_EXT_ROOT" if rule == NameRule.EXACT else "molt_ext_root"
    parent_environment = ProcessEnvironment(
        {key: str(root)} if root_source == "env" else {}, rule=rule
    )
    command = [sys.executable, "-c", "pass"]
    envelope = admission.envelope_for_command(
        command, cargo_output_root=str(root) if root_source == "declared" else None
    )
    parent_layout = layout.CargoOutputLayout.for_envelope(
        envelope, result_root=receipts, source_root=source
    ).bind_supervisor_artifact_root(parent_environment)
    request_path, result_path, persisted, _nonce = runner._write_execution_request(
        row={"command_envelope_json": json.dumps(envelope)},
        command=command,
        repo_root=source,
        resource_family="python-tests",
        run_id="storage-transport",
        env_override_names=[],
        log_path=receipts / "run.log",
        summary_path=receipts / "guard.json",
        timeout_seconds=30,
        output_layout=parent_layout,
    )
    request = json.loads(request_path.read_text(encoding="utf-8"))
    assert persisted == envelope == request["envelope"]
    assert request["env_override_names"] == []
    assert request["supervisor_artifact_root"] == layout.declare_root(str(root))
    # A scrubbed or differently configured child must consume the declaration,
    # including on CI where the parent had env configuration but no binding.
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "wrong-root"))

    def refuse_selection(*args, **kwargs):
        pytest.fail("execution child reselected the parent's supervisor root")

    monkeypatch.setattr(layout, "artifact_root", refuse_selection)
    namespace = hashlib.sha256(os.path.normcase(str(source)).encode()).hexdigest()[:24]
    expected = root / "proof-supervisor" / namespace
    observed = []

    def receive(selected):
        observed.append(selected.supervisor_store)
        raise ValueError("storage declaration reached receiver")

    monkeypatch.setattr(
        layout.CargoOutputLayout, "admit_supervisor_target_path", receive
    )
    assert guarded_execution.execute_guarded_request(request_path) == 2
    assert observed == [expected]
    result = json.loads(result_path.read_text(encoding="utf-8"))
    assert result["error"] == "ValueError: storage declaration reached receiver"
    assert result["command_started"] is False
    assert not expected.exists()
    captured_root = request.pop("supervisor_artifact_root")
    request_path.write_text(json.dumps(request), encoding="utf-8")
    observed.clear()
    assert guarded_execution.execute_guarded_request(request_path) == 2
    refused = json.loads(result_path.read_text(encoding="utf-8"))
    assert (
        refused["error"]
        == "ValueError: proof execution request has no supervisor artifact root"
    )
    assert observed == [], "missing transport authority must not select ambient storage"
    # A new generation of the directory cannot inherit the captured authority.
    root.rename(tmp_path / "retained-root")
    root.mkdir()
    with pytest.raises(ValueError, match="replaced or remounted"):
        layout.CargoOutputLayout.for_envelope(
            envelope,
            result_root=receipts,
            supervisor_artifact_root=captured_root,
        )


def test_supervisor_store_uses_the_durable_artifact_binding(
    tmp_path, monkeypatch, git_checkout
):
    import hashlib
    from molt import dx

    source = git_checkout.resolve()
    artifacts = tmp_path / "bound-artifacts"
    artifacts.mkdir()
    monkeypatch.setattr(layout, "implementation_source_root", lambda: source)
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda _: 64 * 1024**3
    )
    dx.set_artifact_root_binding(source, artifacts, {})
    first = layout.CargoOutputLayout(tmp_path / "receipts-one")
    second = layout.CargoOutputLayout(tmp_path / "receipts-two")
    namespace = hashlib.sha256(os.path.normcase(str(source)).encode()).hexdigest()[:24]
    expected = artifacts / "proof-supervisor" / namespace
    assert first.supervisor_store == second.supervisor_store == expected
    assert first.supervisor_target == expected / "target"
    assert not expected.exists(), "selection must not create output"
    artifacts.rename(tmp_path / "offline-artifacts")
    with pytest.raises(dx.DxConfigError, match="bound artifact root"):
        _ = first.supervisor_store


def test_supervisor_intermediates_are_external_but_executable_cas_stays_canonical(
    tmp_path,
):
    root = _root(tmp_path)
    metadata = tmp_path / "receipts"
    selected = layout.CargoOutputLayout.create(
        result_root=metadata, declaration=layout.declare_root(str(root))
    )
    target = selected.supervisor_target
    target.mkdir(parents=True)
    env = guarded_execution._supervisor_build_environment(
        {
            "TEMP": str(selected.temporary),
            "TMP": str(selected.temporary),
            "TMPDIR": str(selected.temporary),
            "MOLT_CARGO_BUILD_DIR_BASE": str(tmp_path / "foreign-payload"),
        },
        target=target,
        external_placement=True,
    )
    assert env["CARGO_TARGET_DIR"] == str(target)
    assert env["MOLT_CARGO_BUILD_DIR_BASE"] == str(target)
    assert env["CARGO_BUILD_BUILD_DIR"] == str(target.parent / "build")
    assert all(
        env[name] == str(target.parent / "tmp") for name in ("TEMP", "TMP", "TMPDIR")
    )
    binary = target / "supervisor.exe"
    binary.write_bytes(b"sealed supervisor image")
    reference = custody_cas.put_file(
        metadata / "custody-cas", binary, logical_name=binary.name, executable=True
    ).as_dict()
    assert Path(reference["path"]).is_relative_to(metadata / "custody-cas")
    assert Path(reference["path"]).read_bytes() == binary.read_bytes()


def test_external_supervisor_rejects_known_impossible_sccache_socket_prefix(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(
        guarded_execution, "os", SimpleNamespace(name="posix", fsencode=os.fsencode)
    )
    target = tmp_path / ("long-supervisor-store-" * 7) / "target"
    target.mkdir(parents=True)
    with pytest.raises(ValueError, match="shorter --cargo-output-root"):
        guarded_execution._supervisor_build_environment(
            {"TMPDIR": "/" + "x" * 108, "RUSTC_WRAPPER": "/usr/bin/sccache"},
            target=target,
            external_placement=True,
        )


def test_cache_payload_capacity_and_roles_follow_root_but_identity_and_metadata_do_not(
    tmp_path, monkeypatch
):
    source = tmp_path / "source"
    source.mkdir()
    (source / "main.rs").write_text("fn main() {}", encoding="utf-8")
    metadata = tmp_path / "receipts"
    monkeypatch.setattr(
        execution_environment,
        "repository_source_paths",
        lambda root, *, run_git, include_untracked=False: [source / "main.rs"],
    )
    content, _, _ = execution_environment.capture_source_content(
        source_root=source,
        env={},
        overlays=(),
        cas_root=metadata / "custody-cas",
        hash_workers=1,
    )
    probes = []

    def capacity(path):
        probes.append(path)
        return 100 * 1024**3

    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", capacity)
    identities = []
    for index in range(2):
        root = tmp_path / f"volume-{index}"
        root.mkdir()
        declaration = layout.declare_root(str(root))
        outputs = cargo_output_environment.CargoOutputEnvironment.for_envelope(
            admission.envelope_for_command(
                ["cargo", "test", "--no-run"], cargo_output_root=str(root)
            )
        )
        kwargs = dict(
            result_root=metadata,
            source_root=source,
            toolchains={},
            command=["cargo", "test", "--no-run"],
            outputs=outputs,
            env={},
            requested_target=None,
            run_id=f"external-{index}",
            execution_nonce_sha256=str(index) * 64,
            timeout_s=0,
            source_snapshot={"root": str(source), "commit": "fixture"},
            source_content=content,
            cargo_output_root=declaration,
        )
        start = len(probes)
        lease = cache.acquire(**kwargs)
        try:
            assert probes[start:] and all(
                path.is_relative_to(root) for path in probes[start:]
            )
            assert lease.target.is_relative_to(root)
            owner = Path(lease.provenance["generation_owner"])
            assert owner.is_relative_to(metadata / "cargo-cache")
            assert (owner.parent.parent / "target.lock").exists()
            pointer = json.loads(
                (owner.parent.parent / "state.json").read_text(encoding="utf-8")
            )
            assert pointer["cargo_output_root"] == declaration
            assert Path(lease.provenance["inputs"]["path"]).is_relative_to(
                metadata / "custody-cas"
            )
            bound = outputs.bind({}, target=lease.target)
            assert all(bound[name] == str(lease.target) for name in outputs.names)
            identities.append(lease.provenance["input_sha256"])
            other_root = {
                **declaration,
                "identity": {**declaration["identity"], "file_id": "f" * 32},
            }
            with pytest.raises(ValueError, match="differs from admitted envelope"):
                cache.validate_prelaunch(
                    lease.provenance,
                    cas_root=metadata / "custody-cas",
                    command=kwargs["command"],
                    outputs=outputs,
                    env=bound,
                    toolchains={},
                    source_root=str(source),
                    source_snapshot=kwargs["source_snapshot"],
                    source_content=content,
                    cargo_output_root=other_root,
                )
        finally:
            lease.close()
    assert identities[0] == identities[1]


@pytest.mark.parametrize("mode", ["print", "queue", "run"])
def test_named_spec_propagates_root_independently_of_retention(
    tmp_path, monkeypatch, capsys, mode
):
    root = _root(tmp_path)
    spec = {
        "logical_id": "external",
        "reason": "retained producer",
        "command": ["cargo", "test", "--no-run"],
        "resource_family": "cargo",
        "contention_key": "cargo",
        "scopes": [],
        "env_overrides": {},
        "notes": [],
        "timeout": 10,
        "cargo_output_root": str(root),
    }
    args = argparse.Namespace(
        cargo_output_root=None,
        cargo_output_lifetime=None,
        env=[],
        print_spec=mode == "print",
        queue_only=mode == "queue",
        detach=False,
    )
    captured = {}

    def submit(args, **kwargs):
        captured.update(kwargs)
        return (0, "external") if mode == "queue" else 0

    monkeypatch.setattr(runner, "_queue_one" if mode == "queue" else "_run_one", submit)
    assert pact._run_named_spec(args, spec) == 0
    result = json.loads(capsys.readouterr().out) if mode == "print" else captured
    assert result["cargo_output_root"] == str(root)
    assert result["cargo_output_lifetime"] == "retain"


@pytest.mark.parametrize("submission", ["toml", "inline"])
@pytest.mark.parametrize("lifetime", ["retain", "terminal-success"])
def test_submission_log_projects_immutable_storage_contract(
    tmp_path, monkeypatch, submission, lifetime
):
    root = _root(tmp_path)
    source = tmp_path / "source"
    source.mkdir()
    metadata = tmp_path / "receipts"
    command = policy._canonical_cargo_proof_command(
        ["test", "--lib", *(["--no-run"] if lifetime == "retain" else [])]
    )
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "[[proof]]\nid = 'retained'\nreason = 'retained producer'\ncommand = "
        + json.dumps(command)
        + "\ncargo_output_root = "
        + json.dumps(str(root))
        + "\ncargo_output_lifetime = "
        + json.dumps(lifetime)
        + "\n",
        encoding="utf-8",
    )
    args = argparse.Namespace(
        dsl=str(dsl),
        db=str(tmp_path / "queue.sqlite3"),
        logs_root=str(metadata),
        repo_root=str(source),
    )
    monkeypatch.setattr(policy, "_proof_command_policy_error", lambda command: None)
    monkeypatch.setattr(
        commands.evidence, "_try_write_marimo_notebook", lambda *args, **kwargs: None
    )
    if submission == "toml":
        assert commands._cmd_submit(args) == 0
    else:
        rc, run_id = runner._queue_one(
            args,
            logical_id="storage-projection",
            reason="persisted storage declaration",
            command=command,
            resource_family="rust",
            contention_key="cargo:workspace",
            scopes=[],
            env_overrides={},
            initial_notes=["declared output placement and lifetime"],
            cargo_output_root=str(root),
            cargo_output_lifetime=lifetime,
        )
        assert rc == 0 and run_id is not None
    with closing(state._connect(Path(args.db))) as conn:
        row = conn.execute(
            "SELECT command_envelope_json, log_path FROM proof_runs"
        ).fetchone()
        envelope = json.loads(row[0])
    assert envelope["cargo_output_root"] == layout.declare_root(str(root))
    assert envelope.get("cargo_output_lifetime", "retain") == lifetime
    if lifetime == "retain":
        assert "cargo_output_lifetime" not in envelope
    logged = next(
        line.removeprefix("command_envelope=")
        for line in Path(row[1]).read_text(encoding="utf-8").splitlines()
        if line.startswith("command_envelope=")
    )
    assert json.loads(logged) == envelope


@pytest.mark.parametrize(
    "template", [presentation._cmd_cargo_template, presentation._cmd_quickstart]
)
def test_consumed_cargo_templates_are_admitted_complete_crate_shards(template, capsys):
    template(argparse.Namespace())
    output = capsys.readouterr().out.replace("\\\n", " ")
    line = next(line for line in output.splitlines() if "proof_queue.py cargo " in line)
    argv = shlex.split(line)
    cargo_args = argv[argv.index("--") + 1 :]
    assert cargo_args == ["test", "-p", "molt-cpython-abi", "--lib"]
    assert policy._cold_single_lib_test_policy_error(cargo_args) is None
    admission.envelope_for_command(
        policy._canonical_cargo_proof_command(cargo_args),
        cargo_output_lifetime="terminal-success",
    )


def test_queue_help_prints_the_uv_contract_the_queue_admits(capsys) -> None:
    # The quickstart taught `uv run --active --project . --python 3.12 pytest`,
    # which queue policy refuses for lacking --no-sync and --no-config. The
    # oracle is that policy: it admits every `uv run` launch the help prints.
    for show in (
        presentation._cmd_quickstart,
        presentation._cmd_template,
        presentation._cmd_cargo_template,
    ):
        show(argparse.Namespace())
    output = capsys.readouterr().out.replace("\\\n", " ")
    prefixes = []
    for line in output.splitlines():
        if line.startswith("command = "):
            argv = json.loads(line.removeprefix("command = "))
        else:
            argv = shlex.split(line)
        for index, token in enumerate(argv):
            if token == "uv" and argv[index + 1 : index + 2] == ["run"]:
                end = next(
                    position
                    for position in range(index, len(argv))
                    if argv[position] in {"python", "pytest"}
                )
                prefixes.append(argv[index:end])
    assert len(prefixes) == 9
    for prefix in prefixes:
        assert (
            policy._proof_command_policy_error([*prefix, "python", "-c", "pass"])
            is None
        )


def test_queue_uv_contract_follows_the_project_interpreter_pin(tmp_path) -> None:
    # Review v4, finding 3. `uv run --python` selects the interpreter that
    # .python-version pins; the default compile target is another fact.
    (tmp_path / ".python-version").write_text("3.13.4\n", encoding="utf-8")
    contract = proof_command_argv.uv_project_contract(tmp_path)
    assert contract[contract.index("--python") + 1] == "3.13"
    current = list(proof_command_argv.uv_project_contract(state.ROOT))
    assert proof_command_argv.uv_project_contract_gaps(current, root=tmp_path) == [
        "--python 3.13"
    ]
    pin = (state.ROOT / ".python-version").read_text(encoding="utf-8").split()[0]
    assert current[current.index("--python") + 1] == ".".join(pin.split(".")[:2])
    (tmp_path / ".python-version").write_text("pypy3.10\n", encoding="utf-8")
    with pytest.raises(ValueError, match="must pin one CPython"):
        proof_command_argv.uv_project_contract(tmp_path)


@pytest.mark.parametrize("units", [259, 260])
@pytest.mark.parametrize("non_bmp", [False, True])
def test_executable_cas_budget_uses_canonical_factory_and_utf16(
    tmp_path, units, non_bmp
):
    name = "molt-proof-supervisor.exe"
    base = Path(tmp_path.anchor) / "molt-cas-budget-probe"
    initial = custody_cas._file_path(base, "0" * 64, name, True)
    current = len(str(initial).encode("utf-16-le", "surrogatepass")) // 2
    extra = units - current
    assert extra > 4
    suffix = ("😀" if non_bmp else "a") + "b" * (extra - (2 if non_bmp else 1))
    root = Path(str(base) + suffix)
    expected = custody_cas._file_path(root, "0" * 64, name, True)
    assert len(str(expected).encode("utf-16-le", "surrogatepass")) // 2 == units
    if units == 259:
        custody_cas.admit_executable_path(root, name, platform="win32")
    else:
        with pytest.raises(
            ValueError, match="shorter execution result path or --logs-root"
        ):
            custody_cas.admit_executable_path(root, name, platform="win32")
    assert not root.exists()


def test_data_cas_and_posix_images_do_not_inherit_windows_launch_limit(tmp_path):
    root = tmp_path / ("long" * 50) / ("long" * 50)
    custody_cas.admit_executable_path(root, "supervisor", platform="linux")
    assert not root.exists()


def test_executable_cas_admission_rejects_noncomponent_logical_name(tmp_path):
    with pytest.raises(ValueError, match="one path component"):
        custody_cas.admit_executable_path(
            tmp_path, "nested/supervisor.exe", platform="win32"
        )


def test_oversized_metadata_image_rejects_before_preflight_or_provision(
    tmp_path, monkeypatch, linux_input_custody
):
    source = tmp_path / "source"
    source.mkdir()
    receipts = tmp_path / ("long-result-" * 12)
    receipts.mkdir()
    selected_root = tmp_path / "cargo-output"
    selected_root.mkdir()
    result = receipts / "result.json"
    request = receipts / "request.json"
    command = [sys.executable, "-c", "raise AssertionError('must not launch')"]
    envelope = admission.envelope_for_command(
        command, cargo_output_root=str(selected_root)
    )
    request.write_text(
        json.dumps(
            {
                "supervisor_artifact_root": layout.CargoOutputLayout.for_envelope(
                    envelope, result_root=Path(str(result)).parent
                ).supervisor_artifact_root,
                "schema": admission.EXECUTION_SCHEMA,
                "run_id": "image-preflight",
                "execution_nonce": "a" * 64,
                "command": command,
                "envelope": envelope,
                "cwd": str(source),
                "resource_family": "python-tests",
                "result_path": str(result),
                "timeout_seconds": 30,
            }
        ),
        encoding="utf-8",
    )
    # Isolate the executable CAS coordinate; the Cargo budget has its own
    # admission models. No command or native tool is provisioned by this test.
    monkeypatch.setattr(
        layout.CargoOutputLayout, "admit_supervisor_target_path", lambda self: None
    )
    admit_image = custody_cas.admit_executable_path
    monkeypatch.setattr(
        custody_cas,
        "admit_executable_path",
        lambda root, name: admit_image(root, name, platform="win32"),
    )
    calls = []
    monkeypatch.setattr(
        policy,
        "_ensure_run_toolchain_preflight",
        lambda **kwargs: calls.append("preflight"),
    )
    monkeypatch.setattr(
        supervisor_generation,
        "provision",
        lambda **kwargs: calls.append("provision"),
    )
    assert guarded_execution.execute_guarded_request(request) == 2
    record = json.loads(result.read_text(encoding="utf-8"))
    assert record["command_started"] is False
    assert (
        "Supervisor CAS executable exceeds Windows launch path budget"
        in record["error"]
    )
    assert calls == []
    assert not (receipts / "custody-cas").exists()


@pytest.mark.parametrize(
    "selector", ["MOLT_DIFF_GUEST_OUTPUT_ROOT", "MOLT_DIFF_GUEST_OUTPUT_IDENTITY"]
)
def test_independent_guest_root_refuses_before_queue_output_side_effects(
    tmp_path: Path, monkeypatch, selector: str, linux_input_custody
):
    from tools.proof_queue_pkg import supervisor_generation

    lane = admission.proof_plan.ProofPlan.load().named_lane(
        "runtime.public-consumers.native"
    )
    command = list(lane.argv)
    envelope = admission.envelope_for_command(command)
    receipts = tmp_path / "receipts"
    receipts.mkdir()
    result = receipts / "result.json"
    request = receipts / "request.json"
    request.write_text(
        json.dumps(
            {
                "supervisor_artifact_root": layout.CargoOutputLayout.for_envelope(
                    envelope, result_root=Path(str(result)).parent
                ).supervisor_artifact_root,
                "schema": admission.EXECUTION_SCHEMA,
                "run_id": "independent-guest-output",
                "execution_nonce": "a" * 64,
                "command": command,
                "envelope": envelope,
                "cwd": str(admission._REPO_ROOT),
                "resource_family": "python-tests",
                "result_path": str(result),
                "timeout_seconds": 30,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv(selector, str(tmp_path / "independent-output"))

    def forbidden(*_args, **_kwargs):
        pytest.fail("queue output side effect before independent-root refusal")

    monkeypatch.setattr(layout.CargoOutputLayout, "for_envelope", forbidden)
    monkeypatch.setattr(supervisor_generation, "provision", forbidden)
    assert guarded_execution.execute_guarded_request(request) == 2
    observed = json.loads(result.read_text(encoding="utf-8"))
    assert observed["command_started"] is False
    assert "independent differential guest output conflicts" in observed["error"]
    assert not (tmp_path / "independent-output").exists()


def test_runtime_cargo_intermediate_base_is_owned_by_the_payload_lease(tmp_path):
    target = tmp_path / "leased"
    target.mkdir()
    outputs = cargo_output_environment.CargoOutputEnvironment(False)
    inherited = {
        "MOLT_CARGO_BUILD_DIR_BASE": str(tmp_path / "escaped"),
        "CARGO_BUILD_BUILD_DIR": str(tmp_path / "escaped-direct"),
    }
    assert outputs.caller_environment(inherited) == {}
    bound = outputs.bind(inherited, target=target)
    assert bound["MOLT_CARGO_BUILD_DIR_BASE"] == str(target)
    assert bound["CARGO_BUILD_BUILD_DIR"] == str(target)
    outputs.validate(bound, target=target)
    with pytest.raises(ValueError, match="differs"):
        outputs.validate({**bound, **inherited}, target=target)


@pytest.mark.parametrize("renumbered", [False, True])
def test_legacy_cargo_root_rekeys_only_a_proven_mount_session(tmp_path, renumbered):
    root = _root(tmp_path)
    metadata = root.stat()
    legacy = {
        "schema": "molt.proof-cargo-output-root.v1",
        "path": str(root),
        "device": metadata.st_dev + int(renumbered),
        "inode": metadata.st_ino,
    }
    before = list(root.iterdir())
    if renumbered:
        with pytest.raises(ValueError, match="identity-unverifiable"):
            layout.validate_root(legacy)
        assert list(root.iterdir()) == before
    else:
        current = layout.validate_root(legacy)
        assert current == layout.declare_root(str(root))
        assert current["schema"] == "molt.proof-cargo-output-root.v2"
        assert set(current) == {"schema", "path", "identity"}


def test_current_cargo_root_survives_device_renumbering(tmp_path, monkeypatch):
    from tests.test_dx_run_context import _renumber_stat_devices

    root = _root(tmp_path)
    declaration = layout.declare_root(str(root))
    _renumber_stat_devices(monkeypatch)
    assert layout.validate_root(declaration) == declaration
