"""Portable output placement keeps immutable proof authority on its own root."""

from __future__ import annotations

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import shlex
import sqlite3
from types import SimpleNamespace

import pytest

from molt import disk_capacity
from tools.proof_queue_pkg import (
    cargo_cache_custody as cache,
    cargo_output_environment,
    cargo_output_layout as layout,
    cli,
    command_admission as admission,
    commands,
    custody_cas,
    execution_environment,
    guarded_execution,
    pact,
    policy,
    presentation,
    runner,
    scheduling,
    state,
)


def _root(tmp_path):
    root = tmp_path / "selected-volume"
    root.mkdir()
    return root


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


@pytest.mark.parametrize(
    "command",
    [
        ["python", "-c", "print(1)"],
        ["cargo", "--version"],
        ["cargo", "check", "--target-dir", "elsewhere"],
        ["cargo", "test", "--artifact-dir", "elsewhere"],
    ],
)
def test_untyped_query_and_secondary_output_options_cannot_select_root(
    tmp_path, command
):
    with pytest.raises(ValueError):
        admission.envelope_for_command(command, cargo_output_root=str(_root(tmp_path)))


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
    forged = {**declaration, "inode": declaration["inode"] + 1}
    with pytest.raises(ValueError, match="replaced or remounted"):
        layout.validate_root(forged)
    with pytest.raises(ValueError, match="identity is malformed"):
        layout.declared_root({**declaration, "device": True})


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
    monkeypatch.setattr(
        layout.shutil,
        "which",
        lambda name, **kwargs: str(root / "bin" / name) if name == authority else None,
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
        "MOLT_DIFF_TMPDIR": "canonical-shared-locks",
    }
    assert default.caller_environment(env) == env
    target = root / "target"
    target.mkdir()
    bound = selected.bind(env, target=target)
    assert all(bound[name] == str(target) for name in selected.names)
    assert bound["MOLT_DIFF_TMPDIR"] == "canonical-shared-locks"
    selected.validate(bound, target=target)
    with pytest.raises(ValueError, match="differs"):
        selected.validate({**bound, "TMPDIR": "escaped"}, target=target)


@pytest.mark.parametrize("name", ["CARGO_BUILD_BUILD_DIR", "CARGO_BUILD_TARGET_DIR"])
def test_secondary_environment_output_escape_is_refused(tmp_path, name):
    outputs = cargo_output_environment.CargoOutputEnvironment(
        False, external_placement=True
    )
    with pytest.raises(ValueError, match="bypasses declared"):
        execution_environment._require_cargo_build_tool_environment_context(
            ["cargo", "check"], outputs=outputs, cwd=tmp_path, env={name: "elsewhere"}
        )


@pytest.mark.parametrize("field", ["build-dir", "target-dir", "artifact-dir"])
def test_config_output_escape_is_refused(tmp_path, monkeypatch, field):
    config = tmp_path / "config.toml"
    config.write_text(f'[build]\n{field} = "elsewhere"\n')
    monkeypatch.setattr(
        execution_environment.command_identity,
        "_tool_configuration_identities",
        lambda *args, **kwargs: [{"path": str(config)}],
    )
    with pytest.raises(ValueError, match="redirects output"):
        execution_environment._require_cargo_build_tool_environment_context(
            ["cargo", "check"],
            outputs=cargo_output_environment.CargoOutputEnvironment(
                False, external_placement=True
            ),
            cwd=tmp_path,
            env={},
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
        assert request_envelope == envelope == json.loads(path.read_text())["envelope"]
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
        },
        target=target,
        external_placement=True,
    )
    assert env["CARGO_TARGET_DIR"] == str(target)
    assert all(
        env[name] == str(selected.temporary) for name in ("TEMP", "TMP", "TMPDIR")
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
    with pytest.raises(ValueError, match="shorter --cargo-output-root"):
        guarded_execution._supervisor_build_environment(
            {"TMPDIR": "/" + "x" * 108, "RUSTC_WRAPPER": "/usr/bin/sccache"},
            target=tmp_path,
            external_placement=True,
        )


def test_cache_payload_capacity_and_roles_follow_root_but_identity_and_metadata_do_not(
    tmp_path, monkeypatch
):
    source = tmp_path / "source"
    source.mkdir()
    (source / "main.rs").write_text("fn main() {}")
    metadata = tmp_path / "receipts"
    monkeypatch.setattr(
        execution_environment,
        "_git_source_paths",
        lambda *args, **kwargs: [source / "main.rs"],
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
            pointer = json.loads((owner.parent.parent / "state.json").read_text())
            assert pointer["cargo_output_root"] == declaration
            assert Path(lease.provenance["inputs"]["path"]).is_relative_to(
                metadata / "custody-cas"
            )
            bound = outputs.bind({}, target=lease.target)
            assert all(bound[name] == str(lease.target) for name in outputs.names)
            identities.append(lease.provenance["input_sha256"])
            other_root = {**declaration, "inode": declaration["inode"] + 1}
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


def test_toml_submission_freezes_root_without_declaring_disposal(tmp_path, monkeypatch):
    root = _root(tmp_path)
    source = tmp_path / "source"
    source.mkdir()
    metadata = tmp_path / "receipts"
    command = policy._canonical_cargo_proof_command(["test", "--no-run"])
    dsl = tmp_path / "proof.toml"
    dsl.write_text(
        "[[proof]]\nid = 'retained'\nreason = 'retained producer'\ncommand = "
        + json.dumps(command)
        + "\ncargo_output_root = "
        + json.dumps(str(root))
        + "\n"
    )
    args = argparse.Namespace(
        dsl=str(dsl),
        db=str(tmp_path / "queue.sqlite3"),
        logs_root=str(metadata),
        repo_root=str(source),
    )
    monkeypatch.setattr(policy, "_proof_command_policy_error", lambda command: None)
    commands._cmd_submit(args)
    with closing(state._connect(Path(args.db))) as conn:
        row = conn.execute("SELECT command_envelope_json FROM proof_runs").fetchone()
        envelope = json.loads(row[0])
    assert envelope["cargo_output_root"] == layout.declare_root(str(root))
    assert "cargo_output_lifetime" not in envelope


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
