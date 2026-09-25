from __future__ import annotations

import hashlib
import copy
import json
from pathlib import Path
import shutil
import subprocess
import sys
from types import SimpleNamespace
import tomllib
import zipfile

import pytest

from tools.release import build_bundle
from tools.release import fetch_pinned_tool
from tools.release import release_authority
from tools.release import release_model
from tools.release import release_evidence
from tools import release_exit_gate
from tools.git_identity import clean_checkout_status_arguments
from tools.release import update_manifests
from tools.release import verify_consumer


ROOT = Path(__file__).resolve().parents[2]


def _wheel(path: Path, version: str = "0.0.1") -> Path:
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr(
            f"molt-{version}.dist-info/METADATA",
            "Metadata-Version: 2.4\n"
            "Name: molt\n"
            f"Version: {version}\n"
            "Requires-Dist: click>=8.3.1\n",
        )
    return path


def test_release_target_and_download_authority_is_complete_and_exact() -> None:
    targets = release_model.release_targets()
    assert {(target.platform, target.arch) for target in targets} == {
        ("macos", "arm64"),
        ("macos", "x86_64"),
        ("linux", "x86_64"),
        ("linux", "aarch64"),
        ("windows", "x86_64"),
        ("windows", "arm64"),
    }
    assert all("latest" not in target.runner for target in targets)
    assert all("self-hosted" not in target.runner for target in targets)

    config = release_model.load_config()
    elan = config["downloads"]["elan"]
    assert elan["version"] == "4.2.3"
    linux = elan["targets"]["x86_64-unknown-linux-gnu"]
    assert linux == {
        "url": "https://github.com/leanprover/elan/releases/download/v4.2.3/elan-x86_64-unknown-linux-gnu.tar.gz",
        "sha256": "df0b2b3a439961ffcbb3985214365ffe40f49bc871df04dff268c7d8e21ca8b2",
        "size": 4984019,
        "archive": "tar.gz",
        "member": "elan-init",
    }


def test_release_and_deployment_python_tools_are_exact_hash_locked() -> None:
    pyproject = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    assert pyproject["build-system"]["requires"] == [
        "setuptools==83.0.0",
        "wheel==0.47.0",
    ]
    assert pyproject["dependency-groups"]["release"] == [
        "build==1.5.0",
        "setuptools==83.0.0",
        "wheel==0.47.0",
    ]
    assert pyproject["dependency-groups"]["deployment"] == ["modal==1.5.2"]
    lock = tomllib.loads((ROOT / "uv.lock").read_text(encoding="utf-8"))
    packages = {package["name"]: package for package in lock["package"]}
    for name, version in {
        "build": "1.5.0",
        "modal": "1.5.2",
        "setuptools": "83.0.0",
        "wheel": "0.47.0",
    }.items():
        package = packages[name]
        assert package["version"] == version
        distributions = [package.get("sdist"), *package.get("wheels", [])]
        assert distributions
        assert all(
            distribution
            and str(distribution.get("hash", "")).startswith("sha256:")
            and distribution.get("size", 0) > 0
            for distribution in distributions
        )


class _Response:
    def __init__(self, payload: bytes) -> None:
        self.payload = payload
        self.offset = 0

    def __enter__(self) -> _Response:
        return self

    def __exit__(self, *_args: object) -> None:
        return None

    def read(self, size: int) -> bytes:
        chunk = self.payload[self.offset : self.offset + size]
        self.offset += len(chunk)
        return chunk


def test_pinned_tool_fetch_checks_size_and_digest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    payload = b"exact-tool-payload"
    manifest = tmp_path / "release.toml"
    manifest.write_text(
        'schema = "molt.release-supply-chain.v1"\n'
        "[downloads.test]\n"
        'version = "1"\n'
        "[downloads.test.targets.host]\n"
        'url = "https://github.com/example/tool/releases/download/v1/tool"\n'
        f'sha256 = "{hashlib.sha256(payload).hexdigest()}"\n'
        f"size = {len(payload)}\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(fetch_pinned_tool, "MANIFEST", manifest)
    monkeypatch.setattr(
        fetch_pinned_tool.urllib.request,
        "urlopen",
        lambda *_args, **_kwargs: _Response(payload),
    )
    output = tmp_path / "download" / "tool"
    fetch_pinned_tool.fetch("test", "host", output)
    assert output.read_bytes() == payload

    manifest.write_text(
        manifest.read_text().replace(f"size = {len(payload)}", "size = 1")
    )
    with pytest.raises(ValueError, match="size mismatch"):
        fetch_pinned_tool.fetch("test", "host", output)


def test_release_plan_requires_exact_project_tag_and_source(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(release_authority, "_project_version", lambda: "0.0.001")

    def exact_git(*args: str) -> str:
        if args == ("rev-parse", "HEAD"):
            return "a" * 40
        if args == ("tag", "--points-at", "HEAD", "--list", "v0.0.001"):
            return "v0.0.001"
        if args == ("show", "-s", "--format=%ct", "HEAD"):
            return "1700000000"
        if args == clean_checkout_status_arguments() or args == (
            "merge-base",
            "--is-ancestor",
            "a" * 40,
            "origin/main",
        ):
            return ""
        raise AssertionError(args)

    monkeypatch.setattr(release_authority, "_git", exact_git)
    plan = release_authority.resolve_source("v0.0.001", "a" * 40)
    assert plan["version"] == "0.0.001"
    assert plan["release_exit_archive"] == release_model.release_exit_archive_filename(
        "a" * 40
    )
    assert "matrix" not in plan  # Source resolution alone never admits release builds.

    monkeypatch.setattr(
        release_authority,
        "_git",
        lambda *args: "a" * 40 if args == ("rev-parse", "HEAD") else "",
    )
    with pytest.raises(ValueError, match="not the exact v0.0.001 tag"):
        release_authority.resolve_source("0.0.001", "a" * 40)


def _stable_git(*args: str) -> str:
    if args == ("rev-parse", "HEAD"):
        return "a" * 40
    if args == ("tag", "--points-at", "HEAD", "--list", "v1.0.0"):
        return "v1.0.0"
    if args == ("show", "-s", "--format=%ct", "HEAD"):
        return "1700000000"
    if args == clean_checkout_status_arguments() or args == (
        "merge-base",
        "--is-ancestor",
        "a" * 40,
        "origin/main",
    ):
        return ""
    raise AssertionError(args)


def test_stable_release_requires_green_h0_phase_exit(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from tools import phase_exit_manifest as pem

    monkeypatch.setattr(
        release_evidence, "verify_release_exit_manifest", lambda *_a, **_k: None
    )
    bundle = tmp_path / "release-exit.json"
    bundle.touch()
    with pytest.raises(ValueError, match="requires a green H0 phase-exit manifest"):
        release_evidence.verify_evidence(bundle, version="1.0.0", source_sha="a" * 40)

    phase = tmp_path / "H0.json"
    phase.write_text(
        json.dumps(
            {
                "signed_attestation": {
                    "path": release_model.phase_exit_attestation_filename("a" * 40)
                }
            }
        )
    )
    monkeypatch.setattr(
        pem,
        "verify_phase_manifest",
        lambda *_a, **_k: pem.PhaseReport(
            "H0", "a" * 40, False, ("legacy: legacy_count is 5, not 0",)
        ),
    )
    with pytest.raises(
        ValueError, match="H0 phase exit is not green.*legacy_count is 5"
    ):
        release_evidence.verify_evidence(
            bundle, version="1.0.0", source_sha="a" * 40, phase_manifest=phase
        )

    monkeypatch.setattr(
        pem,
        "verify_phase_manifest",
        lambda *_a, **_k: pem.PhaseReport("H0", "a" * 40, True, ()),
    )
    release_evidence.verify_evidence(
        bundle, version="1.0.0", source_sha="a" * 40, phase_manifest=phase
    )

    # Pre-1.0 releases are not stable contracts and need no phase exit.
    release_evidence.verify_evidence(bundle, version="0.0.001", source_sha="a" * 40)


def test_release_input_selection_requires_exact_cardinality(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="exactly one"):
        release_authority.select_one(tmp_path, "*.whl")
    only = tmp_path / "only.whl"
    only.touch()
    assert release_authority.select_one(tmp_path, "*.whl") == only
    (tmp_path / "duplicate.whl").touch()
    with pytest.raises(ValueError, match="only.whl"):
        release_authority.select_one(tmp_path, "*.whl")


@pytest.mark.parametrize("platform", ["linux", "windows"])
def test_bundle_archives_are_byte_reproducible(tmp_path: Path, platform: str) -> None:
    wheel = _wheel(tmp_path / "molt-0.0.001-py3-none-any.whl")
    worker = tmp_path / ("molt-worker.exe" if platform == "windows" else "molt-worker")
    worker.write_bytes(b"worker-binary")
    suffix = "zip" if platform == "windows" else "tar.gz"
    first = tmp_path / f"first.{suffix}"
    second = tmp_path / f"second.{suffix}"
    for output in (first, second):
        build_bundle.build_bundle(
            version="0.0.001",
            platform=platform,
            wheel=wheel,
            worker=worker,
            kind="molt",
            output=output,
            source_date_epoch=1_700_000_000,
        )
    assert first.read_bytes() == second.read_bytes()


def test_consumer_extraction_rejects_archive_escape(tmp_path: Path) -> None:
    archive = tmp_path / "poison.zip"
    with zipfile.ZipFile(archive, "w") as handle:
        handle.writestr("../escape", "poison")
    with pytest.raises(ValueError, match="escapes extraction root"):
        verify_consumer._extract(archive, tmp_path / "extract")
    assert not (tmp_path / "escape").exists()


@pytest.fixture
def release_inputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> dict[str, object]:
    # Transport/admission unit fixture: semantic producers are tested by their
    # owning gate suite. No fixture is release acceptance evidence.
    monkeypatch.setattr(release_authority, "_project_version", lambda: "0.0.001")
    monkeypatch.setattr(
        release_authority,
        "_git",
        lambda *args: {
            ("rev-parse", "HEAD"): "a" * 40,
            ("tag", "--points-at", "HEAD", "--list", "v0.0.001"): "v0.0.001",
            ("show", "-s", "--format=%ct", "HEAD"): "1700000000",
            clean_checkout_status_arguments(): "",
            ("merge-base", "--is-ancestor", "a" * 40, "origin/main"): "",
        }[args],
    )
    monkeypatch.setattr(
        release_exit_gate,
        "verify_release_bundle",
        lambda manifest, **kwargs: release_exit_gate.ReleaseGateReport(
            json.loads(manifest.read_text())["source_sha"], "PASS", True, ()
        ),
    )
    monkeypatch.setattr(
        release_evidence, "verify_e3_provenance", lambda *_a, **_k: None
    )
    monkeypatch.setattr(release_evidence, "verify_provenance", lambda *_a, **_k: None)
    monkeypatch.setattr(release_authority, "verify_remote_tag", lambda *_a, **_k: None)
    evidence_root = tmp_path / "evidence"
    evidence_root.mkdir()
    evidence = evidence_root / "release-exit.json"
    evidence.write_text(json.dumps({"source_sha": "a" * 40}))
    archive = tmp_path / release_model.release_exit_archive_filename("a" * 40)
    release_evidence.archive_release_exit(
        manifest=evidence,
        source_sha="a" * 40,
        source_date_epoch=1_700_000_000,
        output=archive,
    )
    wheel = _wheel(tmp_path / "molt-0.0.001-py3-none-any.whl")
    candidate_root = tmp_path / "candidates"
    candidate_root.mkdir()
    for target in release_model.release_targets():
        primary = tmp_path / target.id / "primary" / target.worker_filename
        secondary = tmp_path / target.id / "secondary" / target.worker_filename
        primary.parent.mkdir(parents=True)
        secondary.parent.mkdir(parents=True)
        primary.write_bytes(b"reproducible-worker")
        secondary.write_bytes(b"reproducible-worker")
        output = candidate_root / target.id
        candidate = release_authority.assemble_candidate(
            target_id=target.id,
            version="0.0.001",
            source_sha="a" * 40,
            source_date_epoch=1_700_000_000,
            wheel=wheel,
            primary_worker=primary,
            secondary_worker=secondary,
            output=output,
        )
        release_model.write_json(
            output / "consumer-verification.json",
            {
                "schema": "molt.release-consumer-proof.v1",
                "target": candidate["target"],
                "source_sha": "a" * 40,
                "selected": 1,
                "executed": 1,
                "passed": 1,
                "failed": 0,
                "errors": 0,
                "uninstall_verified": True,
            },
        )

    return dict(
        candidate_root=candidate_root,
        wheel=wheel,
        version="0.0.001",
        source_sha="a" * 40,
        source_date_epoch=1_700_000_000,
        release_exit_archive=archive,
        release_exit_sha256=release_model.sha256_file(archive),
    )


def test_candidate_matrix_builds_one_collision_free_signed_index(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    release_inputs: dict[str, object],
) -> None:
    publish = tmp_path / "publish"
    manifest = release_authority.assemble_index(**release_inputs, output=publish)
    artifacts = manifest["artifacts"]
    assert len(artifacts) == 13
    assert len({artifact["filename"] for artifact in artifacts}) == 13
    assert all((publish / artifact["filename"]).is_file() for artifact in artifacts)
    assert len((publish / "SHA256SUMS").read_text().splitlines()) == 14
    sbom = json.loads((publish / "release.spdx.json").read_text())
    assert sbom["spdxVersion"] == "SPDX-2.3"
    assert len(sbom["files"]) == 14
    assert (
        manifest["evidence_archive"]["sha256"] == release_inputs["release_exit_sha256"]
    )
    assert manifest["phase_exit"] is None
    assert manifest["attestation"]["signature"] == "Sigstore keyless OIDC certificate"
    assert (
        update_manifests._load_manifest(publish / "release_manifest.json") == manifest
    )
    projections = tmp_path / "projections"
    monkeypatch.setattr(update_manifests, "OUTPUT", projections)
    update_manifests._render_homebrew(artifacts, "0.0.001")
    update_manifests._render_scoop(artifacts, "0.0.001")
    update_manifests._render_winget(artifacts, "0.0.001")
    assert len([path for path in projections.rglob("*") if path.is_file()]) == 10

    receipt = (
        release_inputs["candidate_root"] / "linux-x86_64" / "consumer-verification.json"
    )
    invalid = json.loads(receipt.read_text())
    invalid["passed"] = 0
    release_model.write_json(receipt, invalid)
    with pytest.raises(ValueError, match="clean-consumer proof is invalid"):
        release_authority.assemble_index(
            **release_inputs,
            output=tmp_path / "rejected-publish",
        )


def test_index_rejects_incomplete_target_matrix(
    tmp_path: Path, release_inputs: dict[str, object]
) -> None:
    candidates = tmp_path / "empty-candidates"
    candidates.mkdir()
    with pytest.raises(ValueError, match="matrix mismatch"):
        release_authority.assemble_index(
            **{**release_inputs, "candidate_root": candidates},
            output=tmp_path / "publish",
        )


def test_signed_release_uses_generated_spdx_predicate(
    tmp_path, monkeypatch, release_inputs
):
    publish = tmp_path / "publish"
    manifest = release_authority.assemble_index(**release_inputs, output=publish)
    for name in release_authority._RELEASE_SIDECARS:
        (publish / name).write_text("{}")
    calls = []
    monkeypatch.setattr(
        release_evidence,
        "verify_provenance",
        lambda subject, **kwargs: calls.append((subject, kwargs)),
    )
    release_authority.verify_signed_release(publish, source_sha="a" * 40)
    sbom_calls = [
        (subject, kwargs)
        for subject, kwargs in calls
        if kwargs["bundle"].name == "release.sbom.sigstore.json"
    ]
    assert {subject.name for subject, _ in sbom_calls} == {
        record["filename"] for record in release_model.release_subjects(manifest)
    }
    assert all(
        kwargs["predicate_type"] == "https://spdx.dev/Document/v2.3"
        for _, kwargs in sbom_calls
    )


def test_promotion_verifier_rejects_missing_or_changed_assets(
    tmp_path: Path, release_inputs: dict[str, object]
) -> None:
    local = tmp_path / "local"
    remote = tmp_path / "remote"
    release_authority.assemble_index(**release_inputs, output=local)
    for name in release_authority._RELEASE_SIDECARS:
        (local / name).write_text("{}")
    shutil.copytree(local, remote)
    release_authority.verify_promotion(local, remote, source_sha="a" * 40)
    (remote / "RELEASE_NOTES.md").write_bytes(b"different")
    with pytest.raises(ValueError, match="digest mismatch"):
        release_authority.verify_promotion(local, remote, source_sha="a" * 40)
    (local / "stray").write_text("unexpected")
    (remote / "stray").write_text("unexpected")
    with pytest.raises(ValueError, match="asset set mismatch"):
        release_authority.verify_promotion(local, remote, source_sha="a" * 40)


def test_release_topology_has_one_atomic_promotion_and_separate_deployments() -> None:
    release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    cloudflare = (ROOT / ".github/workflows/deploy-cloudflare.yml").read_text(
        encoding="utf-8"
    )
    modal = (ROOT / ".github/workflows/deploy-modal.yml").read_text(encoding="utf-8")
    lean = (ROOT / ".github/actions/setup-lean/action.yml").read_text(encoding="utf-8")
    pyproject = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))

    assert "softprops/action-gh-release" not in release
    assert "deploy-worker:" not in release
    assert "deploy-modal:" not in release
    assert "gh release create" not in release
    assert release.count("release_authority promote-release") == 1
    assert "gh api --method PATCH" not in release
    assert "environment: release-production" in release
    assert "python -m build --wheel --no-isolation" in release
    assert "release_authority verify-wheel" in release
    assert "verify_consumer" in release
    consumer = (ROOT / "tools/release/verify_consumer.py").read_text(encoding="utf-8")
    assert '_run([str(worker), "--help"]' in consumer
    attestation_action = "uses: actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6"
    assert release.count(attestation_action) == 3
    subset = (ROOT / ".github/workflows/verified-subset.yml").read_text(
        encoding="utf-8"
    )
    assert subset.count(attestation_action) == 1
    assert "release.provenance.sigstore.json" in release
    assert "release.sbom.sigstore.json" in release
    assert (
        "release:" in cloudflare and "environment: cloudflare-production" in cloudflare
    )
    assert 'wranglerVersion: "4.112.0"' in cloudflare
    assert "release:" in modal and "environment: modal-production" in modal
    assert "pip install modal" not in modal
    assert pyproject["dependency-groups"]["deployment"] == ["modal==1.5.2"]
    assert "elan/master" not in lean
    assert "release_supply_chain.toml" in lean or "fetch_pinned_tool" in lean


def test_installers_verify_exact_release_digest_and_replace_atomically() -> None:
    shell = (ROOT / "packaging/install.sh").read_text(encoding="utf-8")
    powershell = (ROOT / "packaging/install.ps1").read_text(encoding="utf-8")

    assert "SHA256SUMS" in shell
    assert 'if [ "$checksum_count" -ne 1 ]' in shell
    assert "sha256sum" in shell and "shasum -a 256" in shell
    assert 'archive_root="molt-${VERSION}"' in shell
    assert 'stage="${MOLT_HOME}.new.$$"' in shell
    assert 'backup="${MOLT_HOME}.old.$$"' in shell
    assert 'rm -rf -- "$MOLT_HOME"' not in shell

    assert "RuntimeInformation]::OSArchitecture" in powershell
    assert '"Arm64" { "arm64" }' in powershell
    assert 'Join-Path $workdir "SHA256SUMS"' in powershell
    assert "$checksumLines.Count -ne 1" in powershell
    assert "Get-FileHash -LiteralPath $zipPath -Algorithm SHA256" in powershell
    assert '$staged = "$Prefix.new-$PID"' in powershell
    assert '$backup = "$Prefix.old-$PID"' in powershell
    assert 'Join-Path $binPath "molt.cmd"' in powershell


def test_windows_package_projections_cover_x64_and_arm64() -> None:
    for relative in (
        "packaging/templates/scoop/molt.json",
        "packaging/templates/scoop/molt-worker.json",
    ):
        template = json.loads((ROOT / relative).read_text(encoding="utf-8"))
        assert set(template["architecture"]) == {"64bit", "arm64"}
        assert set(template["autoupdate"]["architecture"]) == {"64bit", "arm64"}
    for relative in (
        "packaging/templates/winget/molt.installer.yaml",
        "packaging/templates/winget/molt-worker.installer.yaml",
    ):
        template = (ROOT / relative).read_text(encoding="utf-8")
        assert template.count("Architecture: x64") == 1
        assert template.count("Architecture: arm64") == 1


def test_release_workflow_uses_exact_input_cardinality_without_shell_listing() -> None:
    release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    assert release.count("release_authority select-one") == 4
    assert "ls " not in release
    assert "find dist/wheel" not in release


@pytest.mark.parametrize(
    "report",
    [
        release_exit_gate.ReleaseGateReport("b" * 40, "PASS", True, ()),
        release_exit_gate.ReleaseGateReport("a" * 40, "FAIL", False, ()),
        release_exit_gate.ReleaseGateReport(
            "a" * 40, "PASS", True, ("tampered receipt",)
        ),
    ],
)
def test_evidence_binding_rejects_other_source_failure_or_problems(
    tmp_path, monkeypatch, report
):
    monkeypatch.setattr(
        release_exit_gate, "verify_release_bundle", lambda *_a, **_k: report
    )
    with pytest.raises(ValueError):
        release_evidence.verify_release_exit_manifest(
            tmp_path / "release-exit.json", source_sha="a" * 40
        )


def test_no_release_version_can_admit_a_missing_semantic_bundle(tmp_path):
    with pytest.raises(ValueError):
        release_evidence.verify_evidence(
            tmp_path / "missing.json", source_sha="a" * 40, version="0.0.1"
        )


@pytest.mark.parametrize(
    "command,input_flag",
    [("archive-exit", "--manifest"), ("extract-exit", "--archive")],
)
def test_evidence_cli_rejects_epoch_different_from_commit(
    tmp_path, monkeypatch, command, input_flag
):
    monkeypatch.setattr(
        release_authority,
        "_git",
        lambda *args: "a" * 40 if args == ("rev-parse", "HEAD") else "1700000000",
    )
    output = tmp_path / "output"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "release_authority",
            command,
            "--source-sha",
            "a" * 40,
            "--source-date-epoch",
            "1700000001",
            input_flag,
            str(tmp_path / "input"),
            "--output",
            str(output),
        ],
    )
    with pytest.raises(ValueError, match="epoch differs from source commit"):
        release_authority.main()
    assert not output.exists()


def test_plan_admits_original_evidence_digest_and_exact_matrix(release_inputs):
    result = release_authority.plan_release(
        "v0.0.001",
        "a" * 40,
        release_exit_archive=release_inputs["release_exit_archive"],
    )
    assert len(json.loads(result["matrix"])["include"]) == 6
    assert result["release_exit_sha256"] == release_inputs["release_exit_sha256"]


def test_index_rejects_plan_archive_drift_without_publishing(tmp_path, release_inputs):
    output = tmp_path / "publish"
    with pytest.raises(ValueError, match="differs from admitted plan"):
        release_authority.assemble_index(
            **{**release_inputs, "release_exit_sha256": "f" * 64}, output=output
        )
    assert not output.exists()
    assert not list(tmp_path.glob(".release-index-*"))


def test_index_keeps_stable_h0_gate_before_artifact_publication(
    tmp_path, monkeypatch, release_inputs
):
    monkeypatch.setattr(release_authority, "_project_version", lambda: "1.0.0")
    monkeypatch.setattr(release_authority, "_git", _stable_git)
    with pytest.raises(ValueError, match="requires a green H0 phase-exit"):
        release_authority.assemble_index(
            **{**release_inputs, "version": "1.0.0"}, output=tmp_path / "publish"
        )
    assert not (tmp_path / "publish").exists()


def test_manifest_consumers_reject_unbound_evidence_and_inexact_matrix(
    tmp_path, release_inputs
):
    valid = release_authority.assemble_index(
        **release_inputs, output=tmp_path / "valid"
    )
    for field, invalid in (
        ("filename", "../escape.zip"),
        ("kind", "wheel"),
        ("sha256", "invalid"),
        ("size", True),
    ):
        payload = copy.deepcopy(valid)
        payload["evidence_archive"][field] = invalid
        with pytest.raises(ValueError):
            release_model.validate_release_manifest(payload)
    for key, invalid in (
        ("schema", "molt.release-manifest.v2"),
        ("phase_exit", valid["evidence_archive"]),
        ("artifacts", valid["artifacts"][:-1]),
    ):
        payload = copy.deepcopy(valid)
        payload[key] = invalid
        path = tmp_path / "invalid.json"
        path.write_text(json.dumps(payload))
        with pytest.raises(ValueError):
            update_manifests._load_manifest(path)


def test_candidate_admission_is_typed_and_never_publishes_malformed_proofs(
    tmp_path, release_inputs
):
    candidate_path = (
        release_inputs["candidate_root"] / "linux-x86_64" / "candidate.json"
    )
    valid = json.loads(candidate_path.read_text())
    variants = []
    for key, invalid in (("target", []), ("reproducibility", []), ("artifacts", {})):
        payload = copy.deepcopy(valid)
        payload[key] = invalid
        variants.append(payload)
    for count in (2.0, True):
        payload = copy.deepcopy(valid)
        payload["reproducibility"]["independent_worker_builds"] = count
        variants.append(payload)
    for i, payload in enumerate(variants):
        candidate_path.write_text(json.dumps(payload))
        output = tmp_path / f"invalid-{i}"
        with pytest.raises(ValueError):
            release_authority.assemble_index(**release_inputs, output=output)
        assert not output.exists()


def test_index_rehashes_all_staged_destinations_before_publication(
    tmp_path, monkeypatch, release_inputs
):
    real_copy = release_authority._copy_verified_release_file
    count = 0

    def corrupt_prior_asset(source, destination, expected):
        nonlocal count
        real_copy(source, destination, expected)
        count += 1
        if count == 3:
            (
                destination.parent / release_inputs["release_exit_archive"].name
            ).write_bytes(b"changed after verification")

    monkeypatch.setattr(
        release_authority, "_copy_verified_release_file", corrupt_prior_asset
    )
    with pytest.raises(ValueError, match="digest mismatch"):
        release_authority.assemble_index(**release_inputs, output=tmp_path / "publish")
    assert not (tmp_path / "publish").exists()


def test_extract_rejects_noncanonical_zip_and_preserves_foreign_destination(
    tmp_path, release_inputs
):
    archive = tmp_path / "noncanonical" / release_inputs["release_exit_archive"].name
    archive.parent.mkdir()
    with zipfile.ZipFile(archive, "w") as handle:
        handle.writestr("release-exit.json", json.dumps({"source_sha": "a" * 40}))
    output = tmp_path / "extracted"
    output.mkdir()
    (output / "foreign").write_bytes(b"preserve")
    with pytest.raises(ValueError, match="canonical reproducible ZIP"):
        release_evidence.extract_release_exit(
            archive=archive,
            source_sha="a" * 40,
            source_date_epoch=1_700_000_000,
            output=output,
        )
    assert (output / "foreign").read_bytes() == b"preserve"


def test_provenance_checks_repository_workflow_source_and_hosted_signer(
    tmp_path, monkeypatch
):
    subject = tmp_path / "receipt.json"
    subject.write_text("{}")
    calls = []
    monkeypatch.setattr(
        release_evidence,
        "_COMMANDS",
        SimpleNamespace(run=lambda command, **kwargs: calls.append(command)),
    )
    release_evidence.verify_provenance(
        subject, source_sha="a" * 40, workflow="verified-subset.yml"
    )
    command = calls[0]
    repository = release_model.load_config()["repository"]
    name = f"{repository['owner']}/{repository['name']}"
    assert command[command.index("--repo") + 1] == name
    assert (
        command[command.index("--signer-workflow") + 1]
        == f"{name}/.github/workflows/verified-subset.yml"
    )
    assert command[command.index("--source-digest") + 1] == "a" * 40
    assert "--deny-self-hosted-runners" in command
    assert "https://slsa.dev/provenance/v1" in command


def test_provenance_failure_is_not_a_digest_only_success(tmp_path, monkeypatch):
    subject = tmp_path / "subject.json"
    subject.write_text("{}")

    def reject(command, **kwargs):
        raise subprocess.CalledProcessError(1, command)

    monkeypatch.setattr(release_evidence, "_COMMANDS", SimpleNamespace(run=reject))
    with pytest.raises(subprocess.CalledProcessError):
        release_evidence.verify_provenance(
            subject, source_sha="a" * 40, workflow="release.yml"
        )


def test_e3_provenance_cannot_succeed_with_an_empty_subject_glob(tmp_path):
    manifest = tmp_path / "release-exit.json"
    manifest.write_text('{"evidence":[]}')
    with pytest.raises(ValueError, match="every exact E3"):
        release_evidence.verify_e3_provenance(manifest, source_sha="a" * 40)


def test_release_workflow_binds_all_admission_and_publication_consumers():
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    assert "push:" not in workflow
    assert "gh release create" not in workflow
    assert "--release-exit-sha256" in workflow
    assert "--release-exit-archive" in workflow
    assert "--release-id" in workflow
    assert "phase_exit_manifest prepare" in workflow
    assert "phase_exit_manifest seal" in workflow
    assert "subject-path: dist/publish/*" in workflow
    assert "release_authority verify-signed" in workflow
    assert "release_authority download-evidence" in workflow
    assert "release_authority promote-release" in workflow
    assert "gh api --method PATCH" not in workflow
    assert "--evidence-asset-id" in workflow
    assert "--clobber" not in workflow
    assert "gh release upload" not in workflow
    assert "gh release download" not in workflow
    assert workflow.count("attestations: read") == 2
    assert "find dist/release-exit" not in workflow


def test_promotion_cli_binds_ids_and_shared_verifiers(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        release_authority,
        "promote_release",
        lambda *args, **kwargs: calls.append((args, kwargs)),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "release_authority",
            "promote-release",
            "--version",
            "0.0.1",
            "--source-sha",
            "a" * 40,
            "--release-id",
            "17",
            "--evidence-asset-id",
            "23",
            "--local",
            str(tmp_path),
        ],
    )
    release_authority.main()
    assert calls == [
        (
            ("0.0.1", "a" * 40),
            {
                "release_id": 17,
                "evidence_asset_id": 23,
                "local": tmp_path,
                "verify_local": release_authority.verify_signed_release,
                "verify_copy": release_authority._verify_release_copy,
            },
        )
    ]
