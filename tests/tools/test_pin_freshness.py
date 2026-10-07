from __future__ import annotations

import hashlib
import io
import re
import shutil
import tarfile
import tomllib
import zipfile
from pathlib import Path

import pytest

from molt import tool_releases
from molt.binaryen_toolchain import BinaryenConfigError, load_binaryen_manifest
from molt.llvm_toolchain import load_llvm_releases
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


def _leb128(value: int) -> bytes:
    out = bytearray()
    while True:
        byte, value = value & 0x7F, value >> 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def _wasm(*sections: tuple[int, bytes]) -> bytes:
    return b"\0asm\x01\0\0\0" + b"".join(
        bytes([section]) + _leb128(len(payload)) + payload
        for section, payload in sections
    )


def _custom(name: str, data: bytes) -> tuple[int, bytes]:
    return 0, _leb128(len(name)) + name.encode() + data


def _ar(members: dict[str, bytes]) -> bytes:
    out = b"!<arch>\n"
    for name, data in members.items():
        out += f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{644:<8}{len(data):<10}`\n".encode()
        out += data + (b"\n" if len(data) % 2 else b"")
    return out


def _wasm_archive(producer: bytes = b"clang") -> bytes:
    """One wasm32 object archive; `producer` varies like host build paths."""
    return _ar({"a.o": _wasm((1, b"\x00"), _custom("producers", producer))})


def _tarball(
    *members: str, payload: bytes = b"binary", builtin: bytes | None = None
) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for member in members:
            if member.endswith("/"):
                info = tarfile.TarInfo(member.rstrip("/"))
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                archive.addfile(info)
                continue
            if member.endswith("VERSION"):
                body = _WASI_VERSION
            elif member.endswith(".a") and builtin is not None:
                body = builtin
            else:
                body = payload
            info = tarfile.TarInfo(member)
            info.size = len(body)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(body))
    return buffer.getvalue()


_WASI_VERSION = b"99.0+m\nwasi-libc: 0000000\nllvm: 0000000\nllvm-version: 99.1.0\nconfig: 0000000\n"


def _zip(member: str, payload: bytes = b"binary") -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr(member, payload)
    return buffer.getvalue()


def _release_record(blobs: dict[str, bytes], release_id: int = 424242) -> dict:
    return {
        "id": release_id,
        "assets": [
            {
                "name": url.rsplit("/", 1)[-1],
                "size": len(data),
                "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
            }
            for url, data in blobs.items()
        ],
    }


class _Upstream:
    """One faked upstream: a latest tag, a release record and its blobs."""

    def __init__(self, latest_tag: str, record_url: str, blobs: dict[str, bytes]):
        self.latest_tag = latest_tag
        self.record_url = record_url
        self.blobs = blobs
        self.record = _release_record(blobs)

    def fetch(self, url: str) -> object:
        if url.endswith("/releases/latest"):
            return {"tag_name": self.latest_tag}
        if url == self.record_url:
            return self.record
        raise AssertionError(f"unexpected fetch {url}")

    def fetch_text(self, url: str) -> bytes:
        return self.blobs[url]

    def fetch_file(self, url: str, destination: Path) -> None:
        destination.write_bytes(self.blobs[url])

    def fetchers(self) -> pin_freshness.Fetchers:
        return pin_freshness.Fetchers(self.fetch, self.fetch_text, self.fetch_file)


def _tool_upstream(release: tool_releases.ToolRelease, new: str) -> _Upstream:
    blobs = {}
    for asset in release.assets.values():
        url = asset.url.replace(release.version, new)
        member = asset.archive_member.replace(release.version, new)
        blobs[url] = _zip(member) if url.endswith(".zip") else _tarball(member)
    return _Upstream(
        f"v{new}", release.provenance.url.replace(release.version, new), blobs
    )


def _pin_root(tmp_path: Path) -> Path:
    for relative in PIN_AUTHORITIES:
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, target)
    return tmp_path


def _changed_lines(before: str, after: str) -> list[tuple[str, str]]:
    return [
        (old, new)
        for old, new in zip(before.splitlines(), after.splitlines(), strict=True)
        if old != new
    ]


def test_tool_update_verifies_every_asset_and_moves_its_policy(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["wasm-tools"]
    upstream = _tool_upstream(release, "9.9.9")
    plan_before = (root / "tools/proof_plan.toml").read_text(encoding="utf-8")

    message = pin_freshness.update(root, "wasm-tools", upstream.fetchers())

    assert message.startswith(f"wasm-tools: {release.version} -> 9.9.9")
    moved = tool_releases.load_tool_releases(root)
    assert moved["wasm-tools"].version == "9.9.9"
    assert moved["wasm-tools"].provenance.release_id == 424242
    for key, asset in moved["wasm-tools"].assets.items():
        assert asset.sha256 == hashlib.sha256(upstream.blobs[asset.url]).hexdigest()
        assert asset.size == len(upstream.blobs[asset.url]), key
    for name, other in tool_releases.load_tool_releases(ROOT).items():
        if name != "wasm-tools":
            assert moved[name] == other
    changed = _changed_lines(
        plan_before, (root / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    assert changed and all(
        old.replace(release.version, "9.9.9").replace(
            re.escape(release.version), re.escape("9.9.9")
        )
        == new
        for old, new in changed
    )
    assert any(re.escape("9.9.9") in line for _, line in changed)


@pytest.mark.parametrize("defect", ["digest", "size", "missing", "member"])
def test_tool_update_fails_closed_and_leaves_files_unchanged(tmp_path, defect) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["wasm-tools"]
    upstream = _tool_upstream(release, "9.9.9")
    first = upstream.record["assets"][0]
    url = next(url for url in upstream.blobs if url.endswith(first["name"]))
    if defect == "digest":
        first["digest"] = "sha256:" + "0" * 64
    elif defect == "size":
        first["size"] += 1
    elif defect == "missing":
        del first["digest"]
    else:
        archive = _zip if first["name"].endswith(".zip") else _tarball
        upstream.blobs[url] = archive("unexpected/member")
        first["size"] = len(upstream.blobs[url])
        first["digest"] = "sha256:" + hashlib.sha256(upstream.blobs[url]).hexdigest()
    before = {relative: (root / relative).read_bytes() for relative in PIN_AUTHORITIES}
    with pytest.raises(PinFreshnessError):
        pin_freshness.update(root, "wasm-tools", upstream.fetchers())
    assert before == {
        relative: (root / relative).read_bytes() for relative in PIN_AUTHORITIES
    }


def test_checksum_manifest_update_verifies_against_shasums(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["node"]
    upstream = _tool_upstream(release, "99.0.0")
    manifest_url = release.provenance.url.replace(release.version, "99.0.0")
    upstream.blobs[manifest_url] = "\n".join(
        f"{hashlib.sha256(data).hexdigest()}  {url.rsplit('/', 1)[-1]}"
        for url, data in upstream.blobs.items()
    ).encode()

    def fetch(url: str) -> object:
        assert url == "https://nodejs.org/dist/index.json", url
        return [{"version": "v99.0.0"}]

    pin_freshness.update(
        root,
        "node",
        pin_freshness.Fetchers(fetch, upstream.fetch_text, upstream.fetch_file),
    )
    moved = tool_releases.load_tool_releases(root)["node"]
    assert moved.version == "99.0.0"
    assert moved.provenance.release_id is None


def test_binaryen_update_derives_tree_identity_with_the_provisioner(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = load_binaryen_manifest(root).release
    new = "999"
    archive_root = f"binaryen-version_{new}"
    blobs = {
        target.url.replace(f"version_{release.version}", f"version_{new}"): _tarball(
            f"{archive_root}/",
            f"{archive_root}/bin/",
            f"{archive_root}/{target.executable}",
        )
        for target in release.targets
    }
    upstream = _Upstream(
        f"version_{new}",
        release.provenance_url.replace(f"version_{release.version}", f"version_{new}"),
        blobs,
    )

    pin_freshness.update(root, "binaryen", upstream.fetchers())

    moved = load_binaryen_manifest(root).release
    assert moved.version == new
    for target in moved.targets:
        assert target.archive_root == archive_root
        assert target.tree_entries == 3
        assert target.executable_sha256 == hashlib.sha256(b"binary").hexdigest()
        assert target.sha256 == hashlib.sha256(blobs[target.url]).hexdigest()


def test_wasi_sdk_update_reads_versions_and_moves_wasm_ld(tmp_path) -> None:
    root = _pin_root(tmp_path)
    wasi = load_llvm_releases(root).wasi_sdk
    old_tag = wasi.archive_version.split(".")[0]

    def move(text: str) -> str:
        return text.replace(
            f"wasi-sdk-{wasi.archive_version}", "wasi-sdk-99.0"
        ).replace(f"wasi-sdk-{old_tag}/", "wasi-sdk-99/")

    builtin_paths = [
        "share/wasi-sysroot/lib/wasm32-wasip1/libc-printscan-long-double.a",
        "lib/clang/99/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
    ]
    builtins = {
        target.id: _wasm_archive(producer=target.id.encode()) for target in wasi.targets
    }
    blobs = {
        move(target.url): _tarball(
            f"{move(target.archive_root)}/",
            f"{move(target.archive_root)}/VERSION",
            *(f"{move(target.archive_root)}/{path}" for path in builtin_paths),
            builtin=builtins[target.id],
        )
        for target in wasi.targets
    }
    upstream = _Upstream(
        "wasi-sdk-99",
        wasi.provenance_url.replace(f"wasi-sdk-{old_tag}", "wasi-sdk-99"),
        blobs,
    )
    plan_before = (root / "tools/proof_plan.toml").read_text(encoding="utf-8")

    pin_freshness.update(root, "wasi-sdk", upstream.fetchers())

    moved = load_llvm_releases(root).wasi_sdk
    assert (moved.archive_version, moved.sdk_version, moved.llvm_version) == (
        "99.0",
        "99.0+m",
        "99.1.0",
    )
    changed = _changed_lines(
        plan_before, (root / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    assert changed and all(
        wasi.llvm_version in old or re.escape(wasi.llvm_version) in old
        for old, _ in changed
    )
    vendor = root / "vendor/wasm-builtins"
    provenance = tomllib.loads((vendor / "provenance.toml").read_text())
    assert provenance["wasi_sdk_archive_version"] == "99.0"
    assert provenance["llvm_version"] == "99.1.0"
    assert provenance["source_host"] == "linux-x86_64"
    reference = builtins["linux-x86_64"]
    for name, record in provenance["archives"].items():
        assert (vendor / name).read_bytes() == reference
        assert record["sha256"] == hashlib.sha256(reference).hexdigest()
        assert record["members"] == 1
        assert record["sdk_path"] in builtin_paths


def test_wasi_sdk_update_rejects_hosts_with_different_archive_members(
    tmp_path,
) -> None:
    root = _pin_root(tmp_path)
    wasi = load_llvm_releases(root).wasi_sdk
    old_tag = wasi.archive_version.split(".")[0]

    def move(text: str) -> str:
        return text.replace(
            f"wasi-sdk-{wasi.archive_version}", "wasi-sdk-99.0"
        ).replace(f"wasi-sdk-{old_tag}/", "wasi-sdk-99/")

    blobs = {}
    for index, target in enumerate(wasi.targets):
        archive_root = move(target.archive_root)
        members = (
            f"{archive_root}/",
            f"{archive_root}/VERSION",
            f"{archive_root}/share/wasi-sysroot/lib/wasm32-wasip1/"
            "libc-printscan-long-double.a",
            f"{archive_root}/lib/clang/99/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
        )
        blobs[move(target.url)] = _tarball(
            *members,
            builtin=_ar({f"m{index}.o": _wasm((1, b"\x00"))}),
        )
    upstream = _Upstream(
        "wasi-sdk-99",
        wasi.provenance_url.replace(f"wasi-sdk-{old_tag}", "wasi-sdk-99"),
        blobs,
    )
    before = (root / "config/llvm_toolchain_releases.toml").read_bytes()
    with pytest.raises(PinFreshnessError, match="different .* members"):
        pin_freshness.update(root, "wasi-sdk", upstream.fetchers())
    assert (root / "config/llvm_toolchain_releases.toml").read_bytes() == before


def test_update_rejects_pins_it_does_not_own(tmp_path) -> None:
    root = _pin_root(tmp_path)

    def fetch(url: str) -> object:
        return {"tag_name": "99.0.0"}

    with pytest.raises(PinFreshnessError, match="uv moves at its own authority"):
        pin_freshness.update(root, "uv", pin_freshness.Fetchers(fetch=fetch))


def test_rewrite_toml_fields_requires_every_field() -> None:
    text = '[a]\nx = "1"\n\n[b]\ny = 2\n'
    assert (
        pin_freshness.rewrite_toml_fields(text, {"a": {"x": "9"}, "b": {"y": 3}})
        == '[a]\nx = "9"\n\n[b]\ny = 3\n'
    )
    with pytest.raises(PinFreshnessError, match="fields not found"):
        pin_freshness.rewrite_toml_fields(text, {"a": {"z": "9"}})


def test_rewrite_policy_requires_exactly_one_policy() -> None:
    with pytest.raises(PinFreshnessError, match="0 'nope' policies"):
        pin_freshness.rewrite_policy(
            '[[toolchain_policy]]\nname = "x"\n', "nope", "1", "2"
        )


def test_manifest_loaders_reread_a_rewritten_file(tmp_path) -> None:
    root = _pin_root(tmp_path)
    assert load_binaryen_manifest(root).release.version != "0"
    manifest = root / "config/binaryen_releases.toml"
    manifest.write_text("schema_version = 1\n", encoding="utf-8")
    with pytest.raises(BinaryenConfigError):
        load_binaryen_manifest(root)
