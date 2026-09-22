"""Attested derived environments: the one way a locked build venv is admitted.

`molt extension produce-set` provisions a content-addressed uv environment
under the checkout custody root and re-launches itself from that
environment's Python. Its launcher is not a declared toolchain image, so the
child custody broker admits it only through the environment's attestation:
the manifest must validate, name its own directory, link to the admitted
base interpreter, provisioner and lock file, and attest the launcher bytes.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt.cli import source_build_environment as sbe  # noqa: E402
from tools import proof_plan  # noqa: E402
from tools.proof_queue_pkg import (  # noqa: E402
    execution_custody,
    process_image_capture,
    supervisor_custody,
)

KIND = execution_custody.DERIVED_ENVIRONMENT_UV_SOURCE_BUILD


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _address(lock_sha256: str, base_sha256: str, uv_sha256: str) -> dict[str, object]:
    return {
        "schema_version": sbe.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_group": "source-extension",
        "dependency_group_requirements": ["meson>=1.0", "ninja>=1.10"],
        "uv_lock_sha256": lock_sha256,
        "python": {
            "implementation": "cpython",
            "version": "3.12.13",
            "platform": "win-amd64",
            "base_executable": "python.exe",
            "base_executable_sha256": base_sha256,
        },
        "uv": {"executable": "uv.exe", "version": "0.9.0", "sha256": uv_sha256},
    }


def _environment_id(address: dict[str, object]) -> str:
    return _sha256(json.dumps(address, sort_keys=True, separators=(",", ":")).encode())


class _Fixture:
    """One derived root with one attested environment and its admission policy."""

    def __init__(self, tmp_path: Path) -> None:
        self.root = tmp_path / "build-environments" / "source-extension"
        self.root.mkdir(parents=True)
        self.lock_sha256 = _sha256(b"uv.lock bytes")
        self.base_bytes = b"base interpreter bytes"
        self.uv_bytes = b"uv provisioner bytes"
        self.launcher_bytes = b"venv launcher bytes"
        address = _address(
            self.lock_sha256, _sha256(self.base_bytes), _sha256(self.uv_bytes)
        )
        self.environment_id = _environment_id(address)
        self.environment_root = self.root / self.environment_id
        self.launcher = sbe.launcher_path(self.environment_root)
        self.launcher.parent.mkdir(parents=True)
        self.launcher.write_bytes(self.launcher_bytes)
        self.console_script = self.launcher.with_name("pkg-config.exe")
        self.console_script.write_bytes(b"pkg-config console script")
        self.manifest = {
            **address,
            "environment_id": self.environment_id,
            "installed_distributions": [{"name": "meson", "version": "1.12.0"}],
            "executable_images": sbe.environment_executable_images(
                self.environment_root
            ),
        }
        self.write_manifest(self.manifest)
        self.policy: dict[str, object] = {
            "schema": execution_custody.CHILD_POLICY_SCHEMA,
            "descendants": "declared-toolchains",
            "allowed": [
                {
                    "toolchain": "python",
                    "path": str(tmp_path / "venv" / "python.exe"),
                    "sha256": _sha256(b"proof venv launcher"),
                },
                {
                    "toolchain": "python",
                    "path": str(tmp_path / "base" / "python.exe"),
                    "sha256": _sha256(self.base_bytes),
                },
                {
                    "toolchain": "uv",
                    "path": str(tmp_path / "uv.exe"),
                    "sha256": _sha256(self.uv_bytes),
                },
            ],
            "derived_environments": [
                {
                    "kind": KIND,
                    "root": execution_custody._norm(self.root),
                    "uv_lock_sha256": self.lock_sha256,
                    "base_executable_sha256s": [_sha256(self.base_bytes)],
                }
            ],
        }

    def write_manifest(self, manifest: object) -> None:
        (self.environment_root / sbe.SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text(
            json.dumps(manifest, sort_keys=True), encoding="utf-8"
        )

    def decide(self, path: Path) -> dict[str, object]:
        server = execution_custody.ChildCustodyEventServer("python", self.policy)
        return server._decide_child(
            {"requested": str(path), "path": os.environ.get("PATH", ""), "cwd": None}
        )


def test_attested_launcher_is_admitted_with_its_environment_named(
    tmp_path: Path,
) -> None:
    fixture = _Fixture(tmp_path)
    decision = fixture.decide(fixture.launcher)
    assert decision["admitted"] is True
    assert decision["toolchain"] == "python"
    assert decision["derived_environment"] == {
        "kind": KIND,
        "root": execution_custody._norm(fixture.root),
        "environment_id": fixture.environment_id,
        "manifest_sha256": _sha256(
            (
                fixture.environment_root / sbe.SOURCE_BUILD_ENVIRONMENT_MANIFEST
            ).read_bytes()
        ),
    }


def test_attested_console_scripts_are_admitted_too(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    decision = fixture.decide(fixture.console_script)
    assert decision["admitted"] is True
    assert decision["derived_environment"]["environment_id"] == (  # type: ignore[index]
        fixture.environment_id
    )


def test_images_the_provisioner_did_not_attest_are_refused(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    other = fixture.launcher.with_name("meson.exe")
    other.write_bytes(b"dropped in after provisioning")
    decision = fixture.decide(other)
    assert decision["admitted"] is False
    assert decision["reason"] == "derived-environment-image-unattested"


def test_image_bytes_must_match_the_attestation(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    fixture.launcher.write_bytes(b"replaced launcher")
    decision = fixture.decide(fixture.launcher)
    assert decision["admitted"] is False
    assert decision["reason"] == "derived-environment-image-drift"


def test_environment_from_another_lock_file_is_refused(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    fixture.policy["derived_environments"][0]["uv_lock_sha256"] = _sha256(b"newer")  # type: ignore[index]
    decision = fixture.decide(fixture.launcher)
    assert decision["admitted"] is False
    assert decision["reason"] == "derived-environment-lock-drift"


def test_environment_must_link_to_an_admitted_base_interpreter(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    fixture.policy["derived_environments"][0]["base_executable_sha256s"] = [  # type: ignore[index]
        _sha256(b"another interpreter")
    ]
    decision = fixture.decide(fixture.launcher)
    assert decision["admitted"] is False
    assert decision["reason"] == "derived-environment-base-interpreter-unadmitted"


def test_environment_must_link_to_the_admitted_provisioner(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    fixture.policy["allowed"] = [
        row
        for row in fixture.policy["allowed"]
        if row["toolchain"] != "uv"  # type: ignore[index,union-attr]
    ]
    decision = fixture.decide(fixture.launcher)
    assert decision["admitted"] is False
    assert decision["reason"] == "derived-environment-provisioner-unadmitted"


def test_attestation_must_name_its_own_directory(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    fixture.write_manifest({**fixture.manifest, "environment_id": "f" * 64})
    decision = fixture.decide(fixture.launcher)
    assert decision["admitted"] is False
    assert decision["reason"] == "derived-environment-attestation-invalid"


def test_missing_or_malformed_attestation_is_refused(tmp_path: Path) -> None:
    fixture = _Fixture(tmp_path)
    manifest = fixture.environment_root / sbe.SOURCE_BUILD_ENVIRONMENT_MANIFEST
    manifest.write_text("{not json", encoding="utf-8")
    assert fixture.decide(fixture.launcher)["reason"] == (
        "derived-environment-attestation-unreadable"
    )
    manifest.unlink()
    assert fixture.decide(fixture.launcher)["reason"] == (
        "derived-environment-attestation-unreadable"
    )


def test_images_outside_every_derived_root_keep_the_closure_reason(
    tmp_path: Path,
) -> None:
    fixture = _Fixture(tmp_path)
    stray = tmp_path / "elsewhere" / "python.exe"
    stray.parent.mkdir()
    stray.write_bytes(b"stray")
    decision = fixture.decide(stray)
    assert decision["admitted"] is False
    assert decision["reason"] == "outside-declared-toolchain-closure"


def test_locked_environment_manifest_validator_names_every_defect() -> None:
    address = _address("a" * 64, "b" * 64, "c" * 64)
    environment_id = _environment_id(address)
    launcher = sbe.launcher_path(Path()).as_posix()
    manifest = {
        **address,
        "environment_id": environment_id,
        "installed_distributions": [{"name": "meson", "version": "1.12.0"}],
        "executable_images": {launcher: "d" * 64},
    }
    assert (
        sbe.locked_environment_manifest_problems(
            manifest, environment_id=environment_id
        )
        == []
    )
    assert sbe.locked_environment_manifest_problems(
        {**manifest, "extra": 1}, environment_id=environment_id
    ) == ["locked source-build environment attestation shape is invalid"]
    problems = sbe.locked_environment_manifest_problems(
        {
            **manifest,
            "installed_distributions": [{"name": "meson"}],
            "executable_images": {launcher: "short"},
        },
        environment_id="e" * 64,
    )
    assert problems == [
        "locked source-build environment attestation names another environment",
        "locked source-build environment installed distributions are invalid",
        "locked source-build environment executable images are invalid",
    ]
    assert sbe.locked_environment_manifest_problems(
        {**manifest, "executable_images": {launcher + ".bak": "d" * 64}},
        environment_id=environment_id,
    ) == [
        "locked source-build environment executable images do not attest the launcher"
    ]


def test_policy_rows_come_from_the_envelope_and_the_admitted_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "uv.lock").write_bytes(b"lock")
    home = tmp_path / "custody" / "build-environments" / "source-extension"
    monkeypatch.setattr(
        sbe, "source_build_environments_root", lambda root, environ=None: home
    )
    envelope = {"process_closure": {"derived_environments": [KIND]}}
    rows = execution_custody.derived_environment_policy_rows(envelope, cwd=repo)
    assert rows == [
        {
            "kind": KIND,
            "root": execution_custody._norm(home.resolve()),
            "uv_lock_sha256": _sha256(b"lock"),
        }
    ]
    assert execution_custody.derived_environment_policy_rows({}, cwd=repo) == []
    with pytest.raises(ValueError, match="unknown derived environment kind"):
        execution_custody.derived_environment_policy_rows(
            {"process_closure": {"derived_environments": ["conda"]}}, cwd=repo
        )
    (repo / "uv.lock").unlink()
    with pytest.raises(ValueError, match="uv.lock"):
        execution_custody.derived_environment_policy_rows(envelope, cwd=repo)


def test_child_policy_carries_the_derived_environment_rows() -> None:
    envelope = {"process_closure": {"descendants": "declared-toolchains"}}
    rows = [{"kind": KIND, "root": "C:\\custody\\envs", "uv_lock_sha256": "a" * 64}]
    python = {
        "process_images": [
            process_image_capture.capture_image(
                "python", Path(sys.executable), preserve_path=True
            )
        ],
        "runtime": {
            "explicit_authority_files": [
                {"authority": "base-executable", "sha256": "b" * 64},
                {"authority": "venv-executable", "sha256": "c" * 64},
            ]
        },
    }
    policy = execution_custody.child_policy(
        envelope, {"python": python}, derived_environments=rows
    )
    assert policy["schema"] == execution_custody.CHILD_POLICY_SCHEMA
    assert policy["derived_environments"] == [
        {**rows[0], "base_executable_sha256s": ["b" * 64]}
    ]
    located = {**python, "base_executable_sha256": "b" * 64}
    located.pop("runtime")
    assert execution_custody.child_policy(
        envelope, {"python": located}, derived_environments=rows
    )["derived_environments"] == [{**rows[0], "base_executable_sha256s": ["b" * 64]}]
    assert execution_custody.child_policy(envelope, {})["derived_environments"] == []
    with pytest.raises(ValueError, match="base interpreter"):
        execution_custody.child_policy(envelope, {}, derived_environments=rows)


def test_supervisor_treats_the_environment_home_as_a_shared_derived_root(
    tmp_path: Path,
) -> None:
    home = tmp_path / "envs"
    home.mkdir()
    (home / "existing-environment").mkdir()
    rows = [{"kind": KIND, "root": str(home), "uv_lock_sha256": "a" * 64}]
    derived = supervisor_custody._supervisor_derived_roots(
        descendants="declared-toolchains", env={}, derived_environments=rows
    )
    assert derived == [
        {
            "role": supervisor_custody.ATTESTED_ENVIRONMENT_ROLE,
            "path": str(home.resolve()),
        }
    ]
    with pytest.raises(ValueError, match="leaf closure"):
        supervisor_custody._supervisor_derived_roots(
            descendants="forbidden", env={}, derived_environments=rows
        )

    source = tmp_path / "source"
    source.mkdir()
    result = tmp_path / "result" / "result.json"
    result.parent.mkdir()
    [row] = supervisor_custody._derived_root_provenance(
        descendants="declared-toolchains",
        env={},
        source_root=source,
        result_path=result,
        derived_environments=rows,
    )
    assert row["role"] == supervisor_custody.ATTESTED_ENVIRONMENT_ROLE
    assert row["run_owned"] is False
    assert row["initial_entries"] == ["existing-environment"]
    assert row["initial_entry_count"] == 1
    assert supervisor_custody.derived_root_row_consistent(row)
    assert not supervisor_custody.derived_root_row_consistent(
        {**row, "initial_entries": []}
    )
    build_output = {
        "role": supervisor_custody.BUILD_OUTPUT_ROLE,
        "path": str(home),
        "initial_entry_count": 0,
        "initial_manifest_sha256": supervisor_custody._canonical_payload_sha256([]),
        "run_owned": True,
    }
    assert supervisor_custody.derived_root_row_consistent(build_output)
    assert not supervisor_custody.derived_root_row_consistent(
        {**build_output, "initial_entry_count": 1}
    )


def test_plan_refuses_unknown_derived_environment_kinds() -> None:
    plan = proof_plan.ProofPlan.load()
    lane = next(
        lane for lane in plan.named_lanes if lane.id == "pact.seal.numpy.produce"
    )
    assert lane.derived_environments == (KIND,)
    assert plan.validate() == []
    lane.data["derived_environments"] = ["conda-environment"]
    assert any("unknown derived environment kind" in error for error in plan.validate())
    lane.data["derived_environments"] = [KIND, KIND]
    assert any("distinct kinds" in error for error in plan.validate())
