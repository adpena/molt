from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tomllib
import zipfile

import pytest

from tools.release import build_bundle
from tools.release import fetch_pinned_tool
from tools.release import release_authority
from tools.release import release_model
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
        raise AssertionError(args)

    monkeypatch.setattr(release_authority, "_git", exact_git)
    plan = release_authority.plan_release("v0.0.001", "a" * 40)
    assert plan["version"] == "0.0.001"
    assert len(json.loads(plan["matrix"])["include"]) == 6

    monkeypatch.setattr(
        release_authority,
        "_git",
        lambda *args: "a" * 40 if args == ("rev-parse", "HEAD") else "",
    )
    with pytest.raises(ValueError, match="not the exact v0.0.001 tag"):
        release_authority.plan_release("0.0.001", "a" * 40)


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


def test_candidate_matrix_builds_one_collision_free_signed_index(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
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

    publish = tmp_path / "publish"
    manifest = release_authority.assemble_index(
        candidate_root=candidate_root,
        wheel=wheel,
        version="0.0.001",
        source_sha="a" * 40,
        source_date_epoch=1_700_000_000,
        output=publish,
    )
    artifacts = manifest["artifacts"]
    assert len(artifacts) == 13
    assert len({artifact["filename"] for artifact in artifacts}) == 13
    assert all((publish / artifact["filename"]).is_file() for artifact in artifacts)
    assert len((publish / "SHA256SUMS").read_text().splitlines()) == 13
    sbom = json.loads((publish / "release.spdx.json").read_text())
    assert sbom["spdxVersion"] == "SPDX-2.3"
    assert len(sbom["files"]) == 13
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

    receipt = candidate_root / "linux-x86_64" / "consumer-verification.json"
    invalid = json.loads(receipt.read_text())
    invalid["passed"] = 0
    release_model.write_json(receipt, invalid)
    with pytest.raises(ValueError, match="clean-consumer proof is invalid"):
        release_authority.assemble_index(
            candidate_root=candidate_root,
            wheel=wheel,
            version="0.0.001",
            source_sha="a" * 40,
            source_date_epoch=1_700_000_000,
            output=tmp_path / "rejected-publish",
        )


def test_index_rejects_incomplete_target_matrix(tmp_path: Path) -> None:
    wheel = _wheel(tmp_path / "molt-0.0.001-py3-none-any.whl")
    candidates = tmp_path / "candidates"
    candidates.mkdir()
    with pytest.raises(ValueError, match="matrix mismatch"):
        release_authority.assemble_index(
            candidate_root=candidates,
            wheel=wheel,
            version="0.0.001",
            source_sha="a" * 40,
            source_date_epoch=1_700_000_000,
            output=tmp_path / "publish",
        )


def test_promotion_verifier_rejects_missing_or_changed_assets(tmp_path: Path) -> None:
    local = tmp_path / "local"
    remote = tmp_path / "remote"
    local.mkdir()
    remote.mkdir()
    (local / "asset").write_bytes(b"same")
    (remote / "asset").write_bytes(b"same")
    release_authority.verify_promotion(local, remote)
    (remote / "asset").write_bytes(b"different")
    with pytest.raises(ValueError, match="digest mismatch"):
        release_authority.verify_promotion(local, remote)


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
    assert release.count("gh release create") == 1
    assert release.count("-F draft=false") == 1
    assert "environment: release-production" in release
    assert "python -m build --wheel --no-isolation" in release
    assert "release_authority verify-wheel" in release
    assert "verify_consumer" in release
    consumer = (ROOT / "tools/release/verify_consumer.py").read_text(encoding="utf-8")
    assert '_run([str(worker), "--help"]' in consumer
    assert (
        release.count("uses: actions/attest@36051bcae73b7c2a8a6945a48cbf80953c6baa35")
        == 2
    )
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
