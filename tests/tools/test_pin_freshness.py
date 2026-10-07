from __future__ import annotations

import hashlib
import io
import json
import shutil
import tarfile
from pathlib import Path

import pytest

from molt import tool_releases
from tools import pin_freshness
from tools.pin_freshness import Pin, PinFreshnessError

ROOT = Path(__file__).resolve().parents[2]
PIN_AUTHORITIES = (
    "config/tool_releases.toml",
    "config/llvm_toolchain_releases.toml",
    "config/binaryen_releases.toml",
    "config/release_supply_chain.toml",
    "formal/lean/lean-toolchain",
    "rust-toolchain.toml",
    "tools/proof_plan.toml",
)


def _pin(name: str, current: str, latest: str) -> Pin:
    return Pin(name, "authority", current, lambda: latest)


def _check(pins, open_rows=frozenset(), holds=None) -> tuple[int, str]:
    out = io.StringIO()
    status = pin_freshness.check(
        pins, set(open_rows), holds={} if holds is None else holds, out=out
    )
    return status, out.getvalue()


def test_versions_compare_numerically() -> None:
    key = pin_freshness.version_key
    assert key("1.261.0") > key("1.259.0")
    assert key("26.10.0") > key("24.16.0")
    assert key("34") > key("33.0")
    assert key("133") > key("130")
    assert key("3.14") > key("3.12")
    with pytest.raises(PinFreshnessError):
        key("latest")


def test_check_passes_when_every_pin_is_current() -> None:
    status, out = _check([_pin("uv", "0.12.23", "0.12.23")])
    assert status == 0
    assert "current uv: 0.12.23" in out


def test_check_fails_on_a_stale_pin() -> None:
    status, out = _check([_pin("uv", "0.11.24", "0.12.23")])
    assert status == 1
    assert "STALE   uv: 0.11.24 -> 0.12.23 (authority)" in out


def test_a_hold_on_an_open_row_reports_without_failing() -> None:
    status, out = _check(
        [_pin("llvm", "22.1.8", "23.1.3")], {"HF-64"}, {"llvm": "HF-64"}
    )
    assert status == 0
    assert "HELD    llvm: 22.1.8 -> 23.1.3 (HF-64)" in out


def test_a_hold_must_cite_an_open_row() -> None:
    status, out = _check([_pin("llvm", "22.1.8", "23.1.3")], set(), {"llvm": "HF-64"})
    assert status == 1
    assert "hold cites HF-64, not an open ledger row" in out


def test_a_hold_on_a_current_pin_must_be_dropped() -> None:
    status, out = _check(
        [_pin("llvm", "23.1.3", "23.1.3")], {"HF-64"}, {"llvm": "HF-64"}
    )
    assert status == 1
    assert "current at 23.1.3; drop its hold" in out


def test_a_hold_must_name_a_known_pin() -> None:
    status, out = _check([], {"HF-64"}, {"llvm": "HF-64"})
    assert status == 1
    assert "hold names unknown pin 'llvm'" in out


def test_repository_holds_cite_open_ledger_rows() -> None:
    open_rows = pin_freshness.open_ledger_rows(ROOT)
    for name, row in pin_freshness.HOLDS.items():
        assert row in open_rows, (name, row)


def _no_network(url: str) -> object:
    raise AssertionError(f"unexpected fetch {url}")


def test_every_pin_reads_its_authority_loader() -> None:
    pins = {
        pin.name: pin
        for pin in pin_freshness.collect_pins(
            ROOT, fetch=_no_network, fetch_text=_no_network
        )
    }
    tools = tool_releases.load_tool_releases(ROOT)
    for name, release in tools.items():
        assert pins[name].current == release.version
        assert pins[name].authority == tool_releases.TOOL_RELEASES_PATH
    assert set(pin_freshness.HOLDS) <= set(pins)
    assert {"rust", "python", "uv", "llvm", "wasi-sdk", "binaryen", "lean"} <= set(pins)


def _tarball(member: str, payload: bytes = b"binary") -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        info = tarfile.TarInfo(member)
        info.size = len(payload)
        archive.addfile(info, io.BytesIO(payload))
    return buffer.getvalue()


class _FakeGithubRelease:
    """Upstream for one GitHub-released tool at a new version."""

    def __init__(self, release: tool_releases.ToolRelease, new: str) -> None:
        self.release = release
        self.new = new
        self.blobs: dict[str, bytes] = {}
        assets = []
        for asset in release.assets.values():
            url = asset.url.replace(release.version, new)
            member = asset.archive_member.replace(release.version, new)
            data = _zip(member) if url.endswith(".zip") else _tarball(member)
            self.blobs[url] = data
            assets.append(
                {
                    "name": url.rsplit("/", 1)[-1],
                    "size": len(data),
                    "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
                }
            )
        self.record = {"id": 424242, "tag_name": f"v{new}", "assets": assets}

    def fetch(self, url: str) -> object:
        if url.endswith("/releases/latest"):
            return {"tag_name": f"v{self.new}"}
        if url == self.release.provenance.url.replace(self.release.version, self.new):
            return self.record
        return {"tag_name": "v0.0.1", "crate": {"max_stable_version": "0.0.1"}}

    def fetch_text(self, url: str) -> bytes:
        return self.blobs[url]


def _zip(member: str, payload: bytes = b"binary") -> bytes:
    import zipfile

    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr(member, payload)
    return buffer.getvalue()


def test_github_release_update_verifies_every_asset() -> None:
    release = tool_releases.load_tool_releases(ROOT)["wasm-tools"]
    upstream = _FakeGithubRelease(release, "9.9.9")
    update = pin_freshness.plan_tool_update(
        release, "9.9.9", fetch=upstream.fetch, fetch_text=upstream.fetch_text
    )
    assert update["version"] == "9.9.9"
    assert update["provenance"]["release_id"] == 424242
    assert set(update["assets"]) == set(release.assets)
    for key, fields in update["assets"].items():
        assert "9.9.9" in fields["url"] and "9.9.9" in fields["archive_member"]
        assert (
            fields["sha256"]
            == hashlib.sha256(upstream.blobs[fields["url"]]).hexdigest()
        ), key


@pytest.mark.parametrize("defect", ["digest", "size", "missing", "member"])
def test_github_release_update_fails_closed(defect: str) -> None:
    release = tool_releases.load_tool_releases(ROOT)["wasm-tools"]
    upstream = _FakeGithubRelease(release, "9.9.9")
    first = upstream.record["assets"][0]
    if defect == "digest":
        first["digest"] = "sha256:" + "0" * 64
    elif defect == "size":
        first["size"] += 1
    elif defect == "missing":
        del first["digest"]
    else:
        url = next(url for url in upstream.blobs if url.endswith(first["name"]))
        archive = _zip if first["name"].endswith(".zip") else _tarball
        data = archive("unexpected/member")
        upstream.blobs[url] = data
        first["size"] = len(data)
        first["digest"] = "sha256:" + hashlib.sha256(data).hexdigest()
    with pytest.raises(PinFreshnessError):
        pin_freshness.plan_tool_update(
            release, "9.9.9", fetch=upstream.fetch, fetch_text=upstream.fetch_text
        )


def test_checksum_manifest_update_verifies_against_shasums() -> None:
    release = tool_releases.load_tool_releases(ROOT)["node"]
    new = "99.0.0"
    blobs = {}
    lines = []
    for asset in release.assets.values():
        url = asset.url.replace(release.version, new)
        member = asset.archive_member.replace(release.version, new)
        data = _zip(member) if url.endswith(".zip") else _tarball(member)
        blobs[url] = data
        lines.append(f"{hashlib.sha256(data).hexdigest()}  {url.rsplit('/', 1)[-1]}")
    blobs[release.provenance.url.replace(release.version, new)] = "\n".join(
        lines
    ).encode()
    update = pin_freshness.plan_tool_update(
        release, new, fetch=_no_network, fetch_text=blobs.__getitem__
    )
    assert "release_id" not in update["provenance"]
    assert len(update["assets"]) == len(release.assets)


def _pin_root(tmp_path: Path) -> Path:
    for relative in PIN_AUTHORITIES:
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, target)
    return tmp_path


def test_update_moves_the_manifest_and_its_plan_policy(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["wasm-tools"]
    upstream = _FakeGithubRelease(release, "9.9.9")
    plan_before = (root / "tools/proof_plan.toml").read_text(encoding="utf-8")
    manifest_before = (root / tool_releases.TOOL_RELEASES_PATH).read_text(
        encoding="utf-8"
    )

    message = pin_freshness.update_tool(
        root, "wasm-tools", fetch=upstream.fetch, fetch_text=upstream.fetch_text
    )

    assert message.startswith(f"wasm-tools: {release.version} -> 9.9.9")
    moved = tool_releases.load_tool_releases(root)
    assert moved["wasm-tools"].version == "9.9.9"
    assert moved["wasm-tools"].provenance.release_id == 424242
    for name, other in tool_releases.load_tool_releases(ROOT).items():
        if name != "wasm-tools":
            assert moved[name] == other
    plan_after = (root / "tools/proof_plan.toml").read_text(encoding="utf-8")
    changed = [
        (old, new)
        for old, new in zip(
            plan_before.splitlines(), plan_after.splitlines(), strict=True
        )
        if old != new
    ]
    assert changed and all(
        old.replace(release.version, "9.9.9") == new for old, new in changed
    )
    manifest_after = (root / tool_releases.TOOL_RELEASES_PATH).read_text(
        encoding="utf-8"
    )
    assert manifest_after.count("\n") == manifest_before.count("\n")


def test_update_rejects_pins_outside_the_tool_manifest(tmp_path) -> None:
    root = _pin_root(tmp_path)
    with pytest.raises(PinFreshnessError, match="uv is not a tool_releases.toml tool"):
        pin_freshness.update_tool(root, "uv", fetch=_no_network, fetch_text=_no_network)


def test_rewrite_policy_requires_exactly_one_policy() -> None:
    with pytest.raises(PinFreshnessError, match="0 'nope' policies"):
        pin_freshness.rewrite_policy(
            '[[toolchain_policy]]\nname = "x"\n', "nope", "1", "2"
        )


def test_toml_values_are_quoted_exactly() -> None:
    assert pin_freshness._toml_value('a"b') == json.dumps('a"b')
    assert pin_freshness._toml_value(12) == "12"
