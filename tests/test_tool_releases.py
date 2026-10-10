"""Pinned tool releases: one manifest, digest-verified provisioning, attested discovery."""

from __future__ import annotations

import hashlib
import io
import json
import os
import tarfile
import zipfile
from pathlib import Path

import pytest

from molt import dx, tool_releases
from molt.dx import TOOLCHAINS_DIRNAME

ROOT = Path(__file__).resolve().parents[1]


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def test_repository_manifest_pins_every_host_asset_of_wasm_tools() -> None:
    releases = tool_releases.load_tool_releases(ROOT)
    release = releases["wasm-tools"]
    assert release.executable == "wasm-tools"
    assert set(release.assets) >= {
        "x86_64-windows",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-macos",
        "aarch64-macos",
    }
    for asset in release.assets.values():
        assert asset.url.startswith(
            "https://github.com/bytecodealliance/wasm-tools/releases/download/"
            f"v{release.version}/"
        )
        assert asset.archive_member.endswith(("wasm-tools", "wasm-tools.exe"))
    assert release.prefix_name == f"wasm-tools-{release.version}"
    assert release.provenance.kind == tool_releases.PROVENANCE_GITHUB_RELEASE


def test_repository_manifest_pins_lune_for_every_ci_host() -> None:
    release = tool_releases.tool_release("lune", ROOT)
    assert release.version == "0.10.5"
    assert release.executable == "lune"
    assert release.provenance.kind == tool_releases.PROVENANCE_GITHUB_RELEASE
    assert release.provenance.release_id == 348167792
    assert release.provenance.url == (
        "https://api.github.com/repos/lune-org/lune/releases/tags/v0.10.5"
    )
    assert set(release.assets) == {
        f"{architecture}-{system}"
        for architecture in ("x86_64", "aarch64")
        for system in ("windows", "linux", "macos")
    }
    for key, asset in release.assets.items():
        architecture, system = key.split("-", 1)
        assert asset.url == (
            "https://github.com/lune-org/lune/releases/download/v0.10.5/"
            f"lune-0.10.5-{system}-{architecture}.zip"
        )
        assert asset.archive_member == ("lune.exe" if system == "windows" else "lune")
        assert asset.size > 0
        assert len(asset.sha256) == 64


def test_repository_manifest_pins_node_from_the_official_distribution() -> None:
    release = tool_releases.load_tool_releases(ROOT)["node"]
    assert release.provenance.kind == tool_releases.PROVENANCE_CHECKSUM_MANIFEST
    assert release.provenance.url == (
        f"https://nodejs.org/dist/v{release.version}/SHASUMS256.txt"
    )
    assert release.provenance.release_id is None
    assert set(release.assets) == {
        "x86_64-windows",
        "aarch64-windows",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-macos",
        "aarch64-macos",
    }
    for asset in release.assets.values():
        assert asset.url.startswith(f"https://nodejs.org/dist/v{release.version}/")
        assert asset.archive_member.endswith(("/bin/node", "/node.exe"))


def test_checksum_manifest_assets_must_live_in_the_manifest_directory(
    tmp_path: Path,
) -> None:
    (tmp_path / "config").mkdir(parents=True)
    manifest = tmp_path / "config" / "tool_releases.toml"

    def write(asset_url: str, provenance_url: str) -> None:
        manifest.write_text(
            "\n".join(
                [
                    "schema_version = 2",
                    "[tools.demo]",
                    'version = "1.2.3"',
                    'executable = "demo"',
                    "[tools.demo.provenance]",
                    'kind = "checksum-manifest"',
                    f'url = "{provenance_url}"',
                    f"[tools.demo.assets.{tool_releases.host_asset_key()}]",
                    f'url = "{asset_url}"',
                    "size = 1",
                    f'sha256 = "{"a" * 64}"',
                    'archive_member = "demo-1.2.3/demo"',
                ]
            )
            + "\n",
            encoding="utf-8",
        )

    write(
        "https://dist.example/v1.2.3/demo-1.2.3.tar.gz",
        "https://dist.example/v1.2.3/SHASUMS256.txt",
    )
    assert tool_releases.load_tool_releases(tmp_path)["demo"].provenance.kind == (
        tool_releases.PROVENANCE_CHECKSUM_MANIFEST
    )
    write(
        "https://dist.example/v1.2.4/demo-1.2.3.tar.gz",
        "https://dist.example/v1.2.3/SHASUMS256.txt",
    )
    with pytest.raises(
        tool_releases.ToolReleaseError, match=r"own\s+version directory"
    ):
        tool_releases.load_tool_releases(tmp_path)
    write(
        "https://dist.example/v1.2.3/demo-1.2.3.tar.gz",
        "https://dist.example/v1.2.3/checksums.txt",
    )
    with pytest.raises(tool_releases.ToolReleaseError, match="SHASUMS256"):
        tool_releases.load_tool_releases(tmp_path)


def _write_manifest(
    root: Path, asset: dict[str, object], *, tool: str = "demo"
) -> None:
    (root / "config").mkdir(parents=True, exist_ok=True)
    lines = [
        "schema_version = 2",
        f"[tools.{tool}]",
        'version = "1.2.3"',
        f'executable = "{tool}"',
        f"[tools.{tool}.provenance]",
        'kind = "github-release"',
        'url = "https://api.github.com/repos/o/r/releases/tags/v1.2.3"',
        "release_id = 7",
        f"[tools.{tool}.assets.{tool_releases.host_asset_key()}]",
        *(f"{key} = {json.dumps(value)}" for key, value in asset.items()),
    ]
    (root / "config" / "tool_releases.toml").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )


def _zip_archive(path: Path, member: str, payload: bytes) -> None:
    with zipfile.ZipFile(path, "w") as bundle:
        bundle.writestr(member, payload)


def test_manifest_refuses_unpinned_or_escaping_assets(tmp_path: Path) -> None:
    good = {
        "url": "https://github.com/o/r/releases/download/v1.2.3/demo-1.2.3.zip",
        "size": 10,
        "sha256": "a" * 64,
        "archive_member": "demo-1.2.3/demo",
    }
    _write_manifest(tmp_path, good)
    assert "demo" in tool_releases.load_tool_releases(tmp_path)
    for defect, message in (
        ({"url": "https://example.com/demo.zip"}, "tag-addressed GitHub release"),
        ({"sha256": "A" * 64}, "64 lowercase hex"),
        ({"size": 0}, "positive integer"),
        ({"archive_member": "../demo"}, "escapes archive"),
    ):
        _write_manifest(tmp_path, {**good, **defect})
        with pytest.raises(tool_releases.ToolReleaseError, match=message):
            tool_releases.load_tool_releases(tmp_path)


def _tool_fixture_archive(path: Path, member: str, payload: bytes) -> None:
    if path.name.endswith(".zip"):
        _zip_archive(path, member, payload)
    else:
        with tarfile.open(path, "w:xz") as archive:
            entry = tarfile.TarInfo(member)
            entry.size = len(payload)
            archive.addfile(entry, io.BytesIO(payload))


def _pinned_release(
    tmp_path: Path, payload: bytes, *, archive_kind: str = "zip"
) -> tuple[tool_releases.ToolRelease, Path]:
    exe = "demo.exe" if os.name == "nt" else "demo"
    archive = tmp_path / f"demo-1.2.3.{archive_kind}"
    _tool_fixture_archive(archive, f"demo-1.2.3/{exe}", payload)
    data = archive.read_bytes()
    _write_manifest(
        tmp_path,
        {
            "url": f"https://github.com/o/r/releases/download/v1.2.3/{archive.name}",
            "size": len(data),
            "sha256": _sha256(data),
            "archive_member": f"demo-1.2.3/{exe}",
        },
    )
    return tool_releases.tool_release("demo", tmp_path), archive


def test_provisioning_installs_and_attests_only_a_byte_exact_download(
    tmp_path: Path,
) -> None:
    release, archive = _pinned_release(tmp_path, b"demo binary")
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    (downloads / archive.name).write_bytes(archive.read_bytes())
    toolchain_root = tmp_path / "target-root"

    discovery = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )
    assert discovery.prefix == toolchain_root / TOOLCHAINS_DIRNAME / "demo-1.2.3"
    assert discovery.executable.read_bytes() == b"demo binary"
    assert discovery.executable_sha256 == _sha256(b"demo binary")
    attestation = json.loads(
        (discovery.prefix / tool_releases.TOOL_ATTESTATION_FILENAME).read_text(
            encoding="utf-8"
        )
    )
    assert attestation["schema"] == tool_releases.TOOL_ATTESTATION_SCHEMA
    assert attestation["executable_sha256"] == discovery.executable_sha256
    assert tool_releases.discover_tool(release, toolchain_root) == discovery
    assert (
        tool_releases.provision_tool(release, toolchain_root, downloads=downloads)
        == discovery
    )


def test_tampered_or_stale_installs_are_not_discoveries(tmp_path: Path) -> None:
    release, archive = _pinned_release(tmp_path, b"demo binary")
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    (downloads / archive.name).write_bytes(archive.read_bytes())
    toolchain_root = tmp_path / "target-root"
    discovery = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )

    discovery.executable.write_bytes(b"replaced")
    assert tool_releases.discover_tool(release, toolchain_root) is None
    reprovisioned = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )
    assert reprovisioned.executable.read_bytes() == b"demo binary"

    attestation = discovery.prefix / tool_releases.TOOL_ATTESTATION_FILENAME
    attestation.write_text("{}", encoding="utf-8")
    assert tool_releases.discover_tool(release, toolchain_root) is None
    attestation.unlink()
    assert tool_releases.discover_tool(release, toolchain_root) is None


def test_download_that_disagrees_with_the_pin_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    release, archive = _pinned_release(tmp_path, b"demo binary")
    forged = tmp_path / "forged.zip"
    _zip_archive(forged, "demo-1.2.3/demo", b"something else")

    class Response:
        def __init__(self) -> None:
            self._data = forged.read_bytes()

        def __enter__(self) -> Response:
            return self

        def __exit__(self, *_exc: object) -> None:
            return None

        status = 200
        headers = {}

        def geturl(self):
            return next(iter(release.assets.values())).url

        def read(self, size: int = -1) -> bytes:
            data, self._data = (
                self._data[:size] if size > 0 else self._data,
                (self._data[size:] if size > 0 else b""),
            )
            return data

    monkeypatch.setattr(
        tool_releases.urllib.request,
        "build_opener",
        lambda *_: type(
            "Opener", (), {"open": lambda self, request, timeout: Response()}
        )(),
    )
    downloads = tmp_path / "downloads"
    with pytest.raises(tool_releases.ToolReleaseError, match="pinned identity"):
        tool_releases.provision_tool(
            release, tmp_path / "target-root", downloads=downloads
        )
    assert not (downloads / archive.name).exists()
    assert tool_releases.discover_tool(release, tmp_path / "target-root") is None


def test_pinned_executable_prefers_a_provisioned_release(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # No release pinned for the name, or pinned but not provisioned: None, so
    # callers fall back to host discovery rather than a partial install.
    assert tool_releases.pinned_executable("no-such-tool", ROOT) is None
    release = tool_releases.tool_release("node", ROOT)
    toolchain_root = tmp_path / "toolchains"

    custody = dx.CheckoutCustody(
        source_root=ROOT,
        custody_root=tmp_path,
        toolchain_root=toolchain_root,
        kind="durable",
    )
    monkeypatch.setattr("molt.dx.checkout_custody", lambda root, *a, **k: custody)
    assert tool_releases.pinned_executable("node", ROOT) is None

    prefix = tool_releases.tool_prefix(toolchain_root, release)
    executable = tool_releases.tool_executable(prefix, release)
    executable.parent.mkdir(parents=True)
    executable.write_bytes(b"pinned")
    asset = tool_releases.host_asset(release)
    (prefix / tool_releases.TOOL_ATTESTATION_FILENAME).write_text(
        json.dumps(
            tool_releases._attestation_payload(
                release, asset, hashlib.sha256(b"pinned").hexdigest()
            )
        ),
        encoding="utf-8",
    )
    assert tool_releases.pinned_executable("node", ROOT) == executable


def _installed_demo(tmp_path, monkeypatch):
    release, archive = _pinned_release(tmp_path, b"managed executable generation")
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    (downloads / archive.name).write_bytes(archive.read_bytes())
    toolchain_root = tmp_path / "target-root"
    discovery = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )
    custody = dx.CheckoutCustody(
        source_root=ROOT,
        custody_root=tmp_path,
        toolchain_root=toolchain_root,
        kind="durable",
    )
    monkeypatch.setattr("molt.dx.checkout_custody", lambda *_a, **_k: custody)
    return discovery


def test_required_tool_ignores_ambient_path_and_never_provisions(tmp_path, monkeypatch):
    discovery = _installed_demo(tmp_path, monkeypatch)
    monkeypatch.setattr(
        tool_releases.shutil, "which", lambda *_a, **_k: pytest.fail("ambient PATH")
    )
    monkeypatch.setattr(
        tool_releases,
        "provision_tool",
        lambda *_a, **_k: pytest.fail("implicit install"),
    )
    assert tool_releases.require_pinned_tool("demo", tmp_path) == discovery
    discovery.executable.unlink()
    with pytest.raises(
        tool_releases.ToolReleaseError, match="attested toolchain custody"
    ):
        tool_releases.require_pinned_tool("demo", tmp_path)


def test_pinned_tool_runner_preserves_guard_options_and_exact_entrypoint(
    tmp_path, monkeypatch
):
    discovery = _installed_demo(tmp_path, monkeypatch)
    seen = []

    def run(argv, **kwargs):
        seen.append((argv, kwargs))
        return "validated"

    assert (
        tool_releases.run_pinned_tool(
            "demo",
            ["validate", "guest.wasm"],
            run=run,
            repo_root=tmp_path,
            memory_guard_prefix="MOLT_BUILD",
            timeout=60,
        )
        == "validated"
    )
    assert seen == [
        (
            [str(discovery.executable), "validate", "guest.wasm"],
            {"memory_guard_prefix": "MOLT_BUILD", "timeout": 60},
        )
    ]


def test_pinned_tool_runner_rejects_replacement_after_discovery(tmp_path, monkeypatch):
    discovery = _installed_demo(tmp_path, monkeypatch)

    def replaced(*_args, **_kwargs):
        discovery.executable.write_bytes(b"changed after discovery")
        return discovery

    monkeypatch.setattr(tool_releases, "require_pinned_tool", replaced)
    with pytest.raises(
        tool_releases.ToolReleaseError, match="changed after attested discovery"
    ):
        tool_releases.run_pinned_tool(
            "demo",
            ["validate"],
            repo_root=tmp_path,
            run=lambda *_a, **_k: pytest.fail("must not run replacement"),
        )


def test_pinned_tool_runner_rejects_mutation_during_execution(tmp_path, monkeypatch):
    discovery = _installed_demo(tmp_path, monkeypatch)

    def run(*_args, **_kwargs):
        discovery.executable.write_bytes(b"changed during execution")
        return "not accepted"

    with pytest.raises(
        tool_releases.ToolReleaseError, match="execution identity failed"
    ):
        tool_releases.run_pinned_tool(
            "demo",
            ["validate"],
            run=run,
            repo_root=tmp_path,
        )


def test_cli_exports_only_attested_tool_directory(tmp_path: Path, monkeypatch) -> None:
    discovery = _installed_demo(tmp_path, monkeypatch)
    output = tmp_path / "github-path"
    output.write_text("existing-directory\n", encoding="utf-8")
    assert (
        tool_releases.main(
            [
                "discover",
                "demo",
                "--repo-root",
                str(tmp_path),
                "--github-path",
                str(output),
            ]
        )
        == 0
    )
    assert output.read_text(encoding="utf-8").splitlines() == [
        "existing-directory",
        str(discovery.executable.parent),
    ]
    discovery.executable.write_bytes(b"tampered")
    before = output.read_bytes()
    assert (
        tool_releases.main(
            [
                "discover",
                "demo",
                "--repo-root",
                str(tmp_path),
                "--github-path",
                str(output),
            ]
        )
        == 1
    )
    assert output.read_bytes() == before


class _ArchiveBody(io.BytesIO):
    status = 200

    def __init__(self, data, url, headers=None):
        super().__init__(data)
        self.url = url
        self.headers = {} if headers is None else headers
        self.read_sizes = []

    def geturl(self):
        return self.url

    def read(self, size=-1):
        assert size > 0, "archive transfer must use finite reads"
        self.read_sizes.append(size)
        return super().read(size)


def _archive_network(monkeypatch, body, *, before_open=None):
    calls = []

    class Opener:
        def open(self, request, timeout):
            calls.append(request.full_url)
            assert timeout == 180
            assert request.get_header("Authorization") is None
            if before_open:
                before_open()
            return body

    monkeypatch.setattr(
        tool_releases.urllib.request, "build_opener", lambda *_: Opener()
    )
    return calls


@pytest.mark.parametrize("archive_kind", ["zip", "tar.xz"])
def test_archive_provisioning_and_tool_install_share_one_verified_cache(
    tmp_path, monkeypatch, archive_kind
):
    release, original = _pinned_release(
        tmp_path, b"exact binary", archive_kind=archive_kind
    )
    asset = next(iter(release.assets.values()))
    body = _ArchiveBody(original.read_bytes(), asset.url)
    calls = _archive_network(monkeypatch, body)
    cache = tmp_path / "cache"
    result = tool_releases.provision_archive(
        url=asset.url, size=asset.size, sha256=asset.sha256, downloads=cache
    )
    assert result.read_bytes() == original.read_bytes()
    installed = tool_releases.provision_tool(
        release, tmp_path / "toolchains", downloads=cache
    )
    assert installed.executable.read_bytes() == b"exact binary"
    assert calls == [asset.url]
    assert all(n <= 1024 * 1024 for n in body.read_sizes)
    assert not any(child.is_dir() for child in cache.iterdir())


@pytest.mark.parametrize(
    "defect",
    [
        "short",
        "long",
        "digest",
        "length",
        "encoding",
        "status",
        "network",
        "publication",
    ],
)
def test_archive_failure_never_replaces_existing_generation(
    tmp_path, monkeypatch, defect
):
    expected = b"expected archive"
    data = {
        "short": expected[:-1],
        "long": expected + b"x" * 100000,
        "digest": b"X" * len(expected),
    }.get(defect, expected)
    url = "https://nodejs.org/dist/v1/archive.tar.xz"
    headers = (
        {"Content-Length": str(len(expected) + 1)}
        if defect == "length"
        else {"Content-Encoding": "gzip"}
        if defect == "encoding"
        else {}
    )
    body = _ArchiveBody(data, url, headers)
    if defect == "status":
        body.status = 206

    def fail_network():
        if defect == "network":
            raise TimeoutError("fixture transport interruption")

    _archive_network(monkeypatch, body, before_open=fail_network)
    if defect == "publication":

        def failed_publication(*_):
            raise OSError("independent publication interruption")

        monkeypatch.setattr(tool_releases, "durable_replace", failed_publication)
    cache = tmp_path / "cache"
    cache.mkdir()
    original = cache / "archive.tar.xz"
    original.write_bytes(b"previous generation")
    with pytest.raises((tool_releases.ToolReleaseError, OSError)):
        tool_releases.provision_archive(
            url=url, size=len(expected), sha256=_sha256(expected), downloads=cache
        )
    assert original.read_bytes() == b"previous generation"
    assert not any(child.is_dir() for child in cache.iterdir())
    if defect == "long":
        assert body.read_sizes == [len(expected) + 1]


def test_archive_refuses_to_overwrite_concurrent_cache_replacement(
    tmp_path, monkeypatch
):
    cache = tmp_path / "cache"
    cache.mkdir()
    path = cache / "archive.tar.xz"
    path.write_bytes(b"old generation")
    expected = b"expected archive"
    url = "https://nodejs.org/dist/v1/archive.tar.xz"

    def replace():
        new = tmp_path / "replacement"
        new.write_bytes(b"someone else's generation")
        os.replace(new, path)

    _archive_network(monkeypatch, _ArchiveBody(expected, url), before_open=replace)
    with pytest.raises(tool_releases.ToolReleaseError, match="changed"):
        tool_releases.provision_archive(
            url=url, size=len(expected), sha256=_sha256(expected), downloads=cache
        )
    assert path.read_bytes() == b"someone else's generation"


def test_archive_rejects_indirect_cache_leaf_before_network(tmp_path, monkeypatch):
    outside = tmp_path / "outside"
    outside.write_bytes(b"keep")
    cache = tmp_path / "cache"
    cache.mkdir()
    (cache / "archive.tar.xz").symlink_to(outside)
    monkeypatch.setattr(
        tool_releases.urllib.request,
        "build_opener",
        lambda *_: pytest.fail("indirect cache must fail before network"),
    )
    with pytest.raises(tool_releases.ToolReleaseError, match="indirect"):
        tool_releases.provision_archive(
            url="https://nodejs.org/dist/v1/archive.tar.xz",
            size=4,
            sha256=_sha256(b"keep"),
            downloads=cache,
        )
    assert outside.read_bytes() == b"keep"


@pytest.mark.parametrize(
    "url",
    [
        "http://nodejs.org/a",
        "https://user:password@nodejs.org/a",
        "https://nodejs.org:8443/a",
        "https://nodejs.org/a#hidden",
        "https://nodejs.org/a\n",
        "https://nodejs.org/%2e%2e",
        "https://nodejs.org/a?selector=other",
    ],
)
def test_archive_rejects_unadmitted_source_urls_without_network(
    tmp_path, monkeypatch, url
):
    monkeypatch.setattr(
        tool_releases.urllib.request,
        "build_opener",
        lambda *_: pytest.fail("invalid URL must fail before network"),
    )
    with pytest.raises((ValueError, tool_releases.ToolReleaseError)):
        tool_releases.provision_archive(
            url=url, size=1, sha256=_sha256(b"x"), downloads=tmp_path / "cache"
        )


def test_archive_redirects_admit_only_same_origin_and_exact_github_asset_host():
    start = "https://github.com/o/r/releases/download/v1/archive.zip"
    policy = tool_releases._ArchiveRedirects(start)
    request = tool_releases.urllib.request.Request(start)
    for target in (
        start + "?download=1",
        "https://release-assets.githubusercontent.com/asset?sig=provider-signature",
    ):
        assert (
            policy.redirect_request(request, None, 302, "Found", {}, target).full_url
            == target
        )
    for target in (
        "http://github.com/asset",
        "https://objects.githubusercontent.com/asset",
        "https://release-assets.githubusercontent.com.evil.invalid/asset",
        "https://user@release-assets.githubusercontent.com/asset",
        "https://release-assets.githubusercontent.com:444/asset",
        "https://127.0.0.1/asset",
        "file:///host/file",
    ):
        with pytest.raises(tool_releases.ToolReleaseError):
            policy.redirect_request(request, None, 302, "Found", {}, target)
    for start in (
        "https://nodejs.org/dist/v1/node.tar.xz",
        "https://deb.debian.org/pool/libc.deb",
    ):
        policy = tool_releases._ArchiveRedirects(start)
        with pytest.raises(tool_releases.ToolReleaseError):
            policy.admit("https://release-assets.githubusercontent.com/asset")


@pytest.mark.parametrize(
    "mutation", ["replace-before-open", "replace-after-pin", "overwrite-after-pin"]
)
@pytest.mark.parametrize("archive_kind", ["zip", "tar.xz"])
def test_tool_extraction_cannot_attest_an_archive_substitution(
    tmp_path, monkeypatch, mutation, archive_kind
):
    release, original = _pinned_release(
        tmp_path, b"expected tool executable", archive_kind=archive_kind
    )
    asset = next(iter(release.assets.values()))
    cache = tmp_path / "cache"
    cache.mkdir()
    path = cache / original.name
    path.write_bytes(original.read_bytes())
    forged = tmp_path / f"forged.{archive_kind}"
    _tool_fixture_archive(forged, asset.archive_member, b"unadmitted replacement tool")
    if mutation == "replace-before-open":
        actual_provision = tool_releases.provision_archive

        def replace_after_provision(**kwargs):
            admitted = actual_provision(**kwargs)
            os.replace(forged, path)
            return admitted

        monkeypatch.setattr(tool_releases, "provision_archive", replace_after_provision)
    else:
        real_identity = tool_releases.stable_regular_file_handle_identity
        # Corrupt-cache discovery is a separate read: mutate only extraction.
        count = 0

        def mutate_after_pin(opened, **kwargs):
            nonlocal count
            identity = real_identity(opened, **kwargs)
            if opened.path == path:
                count += 1
                if count == 2:
                    if mutation == "replace-after-pin":
                        os.replace(forged, path)
                    else:
                        path.write_bytes(forged.read_bytes())
            return identity

        monkeypatch.setattr(
            tool_releases, "stable_regular_file_handle_identity", mutate_after_pin
        )
    toolchain = tmp_path / "toolchains"
    with pytest.raises(
        (
            ValueError,
            OSError,
            zipfile.BadZipFile,
            tarfile.TarError,
            tool_releases.ToolReleaseError,
        )
    ):
        tool_releases.provision_tool(release, toolchain, downloads=cache)
    prefix = tool_releases.tool_prefix(toolchain, release)
    assert not tool_releases.tool_executable(prefix, release).exists()
    assert not (prefix / tool_releases.TOOL_ATTESTATION_FILENAME).exists()


def test_archive_lock_timeout_is_typed_but_programming_errors_propagate(
    tmp_path, monkeypatch
):
    def busy(_path, **kwargs):
        raise RuntimeError(kwargs["timeout_message"])

    monkeypatch.setattr(tool_releases, "_acquire_file_lock", busy)
    arguments = dict(
        url="https://nodejs.org/dist/v1/archive.zip",
        size=1,
        sha256=_sha256(b"x"),
        downloads=tmp_path,
    )
    with pytest.raises(
        tool_releases.ToolReleaseError, match="cache is busy"
    ) as refusal:
        tool_releases.provision_archive(**arguments)
    assert type(refusal.value.__cause__) is RuntimeError
    failure = RuntimeError("independent programming defect")

    def broken(*_, **__):
        raise failure

    monkeypatch.setattr(tool_releases, "_acquire_file_lock", broken)
    with pytest.raises(RuntimeError) as observed:
        tool_releases.provision_archive(**arguments)
    assert observed.value is failure


def test_dx_discovery_leaves_indirect_download_cache_untouched(tmp_path, monkeypatch):
    from molt import dx

    release, archive = _pinned_release(tmp_path, b"demo binary")
    original_archive = archive.read_bytes()
    root = tmp_path / "tools"
    cache = root / TOOLCHAINS_DIRNAME / tool_releases.DOWNLOADS_DIRNAME
    cache.mkdir(parents=True)
    (cache / archive.name).symlink_to(archive)
    monkeypatch.setattr(tool_releases, "tool_release", lambda _: release)
    monkeypatch.setattr(
        tool_releases,
        "provision_tool",
        lambda *_args, **_kwargs: pytest.fail("discovery must not provision"),
    )
    monkeypatch.setattr(
        tool_releases,
        "provision_archive",
        lambda *_args, **_kwargs: pytest.fail("discovery must not read download cache"),
    )
    env = {"MOLT_TARGET_ROOT": str(root)}
    assert dx.pinned_sccache(env) is None
    assert dx.pinned_sccache(env) is None
    assert (cache / archive.name).is_symlink()
    assert archive.read_bytes() == original_archive
    assert not tool_releases.tool_prefix(root, release).exists()
