from __future__ import annotations

from dataclasses import asdict, replace
import hashlib
import io
import json
from pathlib import Path
import tarfile

import pytest

from molt import llvm_toolchain, wasi_sdk_identity
from molt.wasi_sdk_identity import (
    INSTALL_RECEIPT_FILENAME,
    INSTALL_RECEIPT_SCHEMA,
    executable_filename,
    wasi_sdk_tree_identity,
)
from tools import provision_wasi_sdk as provisioner


# The pinned SDK's VERSION file, read from its one authority.
_WASI = llvm_toolchain.load_llvm_releases(provisioner.ROOT).wasi_sdk
VERSION_TEXT = (
    f"{_WASI.sdk_version}\nwasi-libc: test\nllvm-version: {_WASI.llvm_version}\n"
).encode()
_SDK_MAJOR = int(_WASI.archive_version.split(".", 1)[0])
_LLVM_MAJOR, _LLVM_MINOR, _LLVM_PATCH = _WASI.llvm_version.split(".")
# Near misses: the previous SDK release, and another patch of the LLVM line.
_PREVIOUS_SDK_VERSION_TEXT = (
    f"{_SDK_MAJOR - 1}.0\nwasi-libc: test\nllvm-version: {_WASI.llvm_version}\n"
).encode()
_OTHER_LLVM_PATCH = f"{_LLVM_MAJOR}.{_LLVM_MINOR}.{int(_LLVM_PATCH) + 8}"


def _host_asset() -> llvm_toolchain.WasiSdkHostAsset:
    return llvm_toolchain.wasi_sdk_host_asset(provisioner.ROOT)


def _file(name: str, payload: bytes) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.size = len(payload)
    return info, payload


def _directory(name: str) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.type = tarfile.DIRTYPE
    return info, b""


def _link(name: str, target: str, kind: bytes) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.type = kind
    info.linkname = target
    return info, b""


def _required_members(
    *,
    version_text: bytes = VERSION_TEXT,
    omit: str | None = None,
) -> list[tuple[tarfile.TarInfo, bytes]]:
    asset = _host_asset()
    root = asset.archive_root
    members = [
        _directory(f"{root}/"),
        _file(f"{root}/VERSION", version_text),
        _file(f"{root}/bin/{executable_filename('wasm-ld', asset.id)}", b"linker"),
        _file(f"{root}/bin/{executable_filename('llvm-nm', asset.id)}", b"reader"),
        *(
            _file(f"{root}/bin/{executable_filename(name, asset.id)}", name.encode())
            for name in ("clang", "clang++", "llvm-ar", "llvm-ranlib")
        ),
        _file(
            f"{root}/share/wasi-sysroot/include/wasm32-wasip1/errno.h",
            b"#define EINVAL 28\n",
        ),
        _file(f"{root}/share/wasi-sysroot/lib/wasm32-wasip1/libc.a", b"archive"),
        *(
            _file(f"{root}/share/wasi-sysroot/lib/wasm32-wasip1/{name}", name.encode())
            for name in (
                "libc-printscan-long-double.a",
                "crt1-command.o",
                "crt1-reactor.o",
            )
        ),
        _file(
            f"{root}/lib/clang/{asset.llvm_version.split('.')[0]}/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
            b"compiler-rt",
        ),
    ]
    return [member for member in members if member[0].name != f"{root}/{omit}"]


def _write_archive(
    path: Path,
    members: list[tuple[tarfile.TarInfo, bytes]],
) -> None:
    with tarfile.open(path, "w:gz") as archive:
        for info, payload in members:
            archive.addfile(info, io.BytesIO(payload) if info.isfile() else None)


def _install_asset(
    monkeypatch: pytest.MonkeyPatch,
    archive: Path,
    downloads: list[str],
) -> llvm_toolchain.WasiSdkHostAsset:
    """Pin a test archive as this host's asset and record every download."""

    archive_bytes = archive.read_bytes()
    asset = replace(
        _host_asset(),
        url="https://example.invalid/wasi-sdk.tar.gz",
        size=len(archive_bytes),
        sha256=hashlib.sha256(archive_bytes).hexdigest(),
        record_sha256="1" * 64,
    )
    monkeypatch.setattr(llvm_toolchain, "wasi_sdk_host_asset", lambda _root: asset)

    def download(url: str, output: Path, *, size: int, sha256: str) -> None:
        assert (url, size, sha256) == (asset.url, asset.size, asset.sha256)
        downloads.append(url)
        output.write_bytes(archive_bytes)

    monkeypatch.setattr(provisioner, "_download", download)
    return asset


def test_download_accepts_only_the_exact_manifest_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload = b"exact wasi-sdk archive bytes"
    monkeypatch.setattr(
        provisioner.urllib.request,
        "urlopen",
        lambda *_args, **_kwargs: io.BytesIO(payload),
    )
    output = tmp_path / "sdk.tar.gz"

    provisioner._download(
        "https://example.invalid/sdk.tar.gz",
        output,
        size=len(payload),
        sha256=hashlib.sha256(payload).hexdigest(),
    )

    assert output.read_bytes() == payload


@pytest.mark.parametrize("mismatch", ("size", "overflow", "digest"))
def test_download_rejects_manifest_identity_mismatch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mismatch: str,
) -> None:
    payload = b"substituted wasi-sdk archive bytes"
    monkeypatch.setattr(
        provisioner.urllib.request,
        "urlopen",
        lambda *_args, **_kwargs: io.BytesIO(payload),
    )

    with pytest.raises(ValueError, match="manifest"):
        provisioner._download(
            "https://example.invalid/sdk.tar.gz",
            tmp_path / f"{mismatch}.tar.gz",
            size=len(payload) + {"size": 1, "overflow": -1, "digest": 0}[mismatch],
            sha256=(
                "0" * 64
                if mismatch == "digest"
                else hashlib.sha256(payload).hexdigest()
            ),
        )


@pytest.mark.parametrize(
    ("template", "kind", "target", "message"),
    (
        ("{root}/../escape", tarfile.REGTYPE, "", "portable relative"),
        ("another-sdk-root/escape", tarfile.REGTYPE, "", "outside"),
        ("{root}/version", tarfile.REGTYPE, "", "collision"),
        ("{root}/bin/escape", tarfile.SYMTYPE, "../../../outside", "link escapes"),
        ("{root}/bin/escape", tarfile.LNKTYPE, "another-root/file", "link escapes"),
        ("{root}/bin/pipe", tarfile.FIFOTYPE, "", "special node"),
        ("{root}/bin/unknown", b"?", "", "special node"),
    ),
)
def test_archive_admission_rejects_unsafe_members(
    tmp_path: Path,
    template: str,
    kind: bytes,
    target: str,
    message: str,
) -> None:
    root = _host_asset().archive_root
    member = (
        _file(template.format(root=root), b"escape")
        if kind == tarfile.REGTYPE
        else _link(template.format(root=root), target, kind)
    )
    archive_path = tmp_path / "invalid.tar.gz"
    _write_archive(archive_path, [*_required_members(), member])

    with tarfile.open(archive_path, "r:gz") as archive:
        with pytest.raises(ValueError, match=message):
            provisioner._validate_archive(archive, expected_root=root)


def test_provision_publishes_one_identity_addressed_verified_sdk(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "sdk.tar.gz"
    _write_archive(archive_path, _required_members())
    downloads: list[str] = []
    asset = _install_asset(monkeypatch, archive_path, downloads)
    custody = tmp_path / "custody"

    installed = provisioner.provision_wasi_sdk(
        custody, downloads=tmp_path / "downloads"
    )

    expected = llvm_toolchain.wasi_sdk_install_prefix(custody, asset)
    assert installed == expected.resolve()
    assert expected.name == f"{asset.archive_root}-{'1' * 16}"
    sdk = installed / "sdk"
    wasm_ld = executable_filename("wasm-ld", asset.id)
    assert (sdk / "bin" / wasm_ld).read_bytes() == b"linker"
    assert not (installed / "wasm-bin").exists()
    receipt = json.loads(
        (installed / INSTALL_RECEIPT_FILENAME).read_text(encoding="utf-8")
    )
    assert set(receipt) == {"schema", "asset", "tree", "facts"}
    assert receipt["schema"] == INSTALL_RECEIPT_SCHEMA
    assert receipt["asset"] == asdict(asset)
    assert receipt["tree"] == wasi_sdk_tree_identity(sdk).as_record()
    assert set(receipt["facts"]) == {"tools", "members", "resources"}
    assert set(receipt["facts"]["tools"]) == {
        "clang",
        "clang++",
        "llvm-ar",
        "llvm-ranlib",
        "wasm-ld",
        "llvm-nm",
        "llvm-strip",
    }
    assert receipt["facts"]["tools"]["llvm-strip"] is None
    assert set(receipt["facts"]["members"]) == {
        "libc",
        "long_double",
        "compiler_rt",
        "crt_command",
        "crt_reactor",
    }
    for group in ("tools", "members"):
        for fact in receipt["facts"][group].values():
            if fact is None:
                continue
            actual = (sdk / fact["path"]).read_bytes()
            assert fact["size"] == len(actual)
            assert fact["sha256"] == hashlib.sha256(actual).hexdigest()
            assert sdk / fact["content_path"] == (sdk / fact["path"]).resolve()
    assert set(receipt["facts"]["resources"]) == {"lib", "share/wasi-sysroot"}
    for relative, fact in receipt["facts"]["resources"].items():
        assert fact == wasi_sdk_tree_identity(sdk / relative).as_record()
    cached = tmp_path / "downloads" / f"{asset.archive_root}-{asset.sha256}.tar.gz"
    assert cached.read_bytes() == archive_path.read_bytes()
    assert list(installed.parent.glob(".molt-*")) == []
    installation = llvm_toolchain.load_wasi_sdk_installation(
        provisioner.ROOT, installed, verify_tree=True
    )
    assert installation.sysroot == sdk / "share" / "wasi-sysroot"
    assert downloads == [asset.url]


def test_provision_reuses_verified_install_and_archive_by_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "sdk.tar.gz"
    _write_archive(archive_path, _required_members())
    downloads: list[str] = []
    _install_asset(monkeypatch, archive_path, downloads)
    cache = tmp_path / "downloads"

    first = provisioner.provision_wasi_sdk(tmp_path / "one", downloads=cache)
    again = provisioner.provision_wasi_sdk(tmp_path / "one", downloads=cache)
    other = provisioner.provision_wasi_sdk(tmp_path / "two", downloads=cache)

    assert again == first
    assert other != first
    assert len(downloads) == 1


def test_provision_replaces_a_corrupt_cached_archive_only_after_verification(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "sdk.tar.gz"
    _write_archive(archive_path, _required_members())
    downloads: list[str] = []
    asset = _install_asset(monkeypatch, archive_path, downloads)
    cached = tmp_path / "downloads" / f"{asset.archive_root}-{asset.sha256}.tar.gz"
    cached.parent.mkdir()
    cached.write_bytes(b"truncated")

    provisioner.provision_wasi_sdk(tmp_path / "custody", downloads=cached.parent)

    assert downloads == [asset.url]
    assert cached.read_bytes() == archive_path.read_bytes()


def test_provision_never_repairs_a_modified_installation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "sdk.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path, [])
    custody = tmp_path / "custody"
    installed = provisioner.provision_wasi_sdk(custody, downloads=tmp_path / "dl")
    libc = installed / "sdk/share/wasi-sysroot/lib/wasm32-wasip1/libc.a"
    libc.write_bytes(b"tampered")

    with pytest.raises(ValueError, match="remove .* explicitly"):
        provisioner.provision_wasi_sdk(custody, downloads=tmp_path / "dl")

    assert libc.read_bytes() == b"tampered"


@pytest.mark.parametrize(
    ("version_text", "omit", "message"),
    (
        (_PREVIOUS_SDK_VERSION_TEXT, None, "VERSION identity"),
        (
            VERSION_TEXT.replace(
                _WASI.llvm_version.encode(), _OTHER_LLVM_PATCH.encode()
            ),
            None,
            "LLVM producer identity",
        ),
        (VERSION_TEXT, "share/wasi-sysroot/lib/wasm32-wasip1/libc.a", "missing"),
        (
            VERSION_TEXT,
            "share/wasi-sysroot/lib/wasm32-wasip1/libc-printscan-long-double.a",
            "missing",
        ),
        (
            VERSION_TEXT,
            "share/wasi-sysroot/lib/wasm32-wasip1/crt1-command.o",
            "missing",
        ),
        (
            VERSION_TEXT,
            "share/wasi-sysroot/lib/wasm32-wasip1/crt1-reactor.o",
            "missing",
        ),
        (
            VERSION_TEXT,
            f"lib/clang/{_WASI.llvm_version.split('.')[0]}/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
            "missing",
        ),
        (VERSION_TEXT, "bin/llvm-nm", "missing"),
        (VERSION_TEXT, "bin/clang", "missing"),
        (VERSION_TEXT, "bin/clang++", "missing"),
        (VERSION_TEXT, "bin/llvm-ar", "missing"),
        (VERSION_TEXT, "bin/llvm-ranlib", "missing"),
    ),
)
def test_provision_failure_never_publishes_a_partial_sdk(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    version_text: bytes,
    omit: str | None,
    message: str,
) -> None:
    asset = _host_asset()
    if omit is not None and omit.startswith("bin/"):
        omit = f"bin/{executable_filename(omit.removeprefix('bin/'), asset.id)}"
    archive_path = tmp_path / "rejected.tar.gz"
    _write_archive(
        archive_path, _required_members(version_text=version_text, omit=omit)
    )
    asset = _install_asset(monkeypatch, archive_path, [])
    custody = tmp_path / "custody"

    with pytest.raises(ValueError, match=message):
        provisioner.provision_wasi_sdk(custody, downloads=tmp_path / "dl")

    prefix = llvm_toolchain.wasi_sdk_install_prefix(custody, asset)
    assert not prefix.exists()
    assert list(prefix.parent.iterdir()) == []


def test_provision_refuses_a_non_directory_at_the_identity_prefix(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "sdk.tar.gz"
    _write_archive(archive_path, _required_members())
    downloads: list[str] = []
    asset = _install_asset(monkeypatch, archive_path, downloads)
    custody = tmp_path / "custody"
    occupant = llvm_toolchain.wasi_sdk_install_prefix(custody, asset)
    occupant.parent.mkdir(parents=True)
    occupant.write_text("not an sdk", encoding="utf-8")

    with pytest.raises(ValueError, match="not its exact provisioned identity"):
        provisioner.provision_wasi_sdk(custody, downloads=tmp_path / "dl")

    assert occupant.read_text(encoding="utf-8") == "not an sdk"
    assert downloads == []


def test_cli_projects_the_published_install_to_github_outputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    archive_path = tmp_path / "sdk.tar.gz"
    _write_archive(archive_path, _required_members())
    _install_asset(monkeypatch, archive_path, [])
    output = tmp_path / "github-output"

    status = provisioner.main(
        [
            "--toolchain-root",
            str(tmp_path / "custody"),
            "--downloads",
            str(tmp_path / "dl"),
            "--github-output",
            str(output),
        ]
    )

    assert status == 0
    lines = dict(
        line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines()
    )
    installed = Path(lines["install"])
    assert Path(lines["sdk"]) == installed / "sdk"
    assert Path(lines["sysroot"]) == installed / "sdk" / "share" / "wasi-sysroot"
    assert (installed / INSTALL_RECEIPT_FILENAME).is_file()


def test_sdk_tree_byte_limit_is_checked_before_file_content_is_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "oversized").write_bytes(b"too large")
    monkeypatch.setattr(wasi_sdk_identity, "MAX_TREE_BYTES", 1)

    def forbidden_hash(*_args, **_kwargs):
        pytest.fail("out-of-policy file content must not be read")

    with monkeypatch.context() as hashing:
        hashing.setattr(
            wasi_sdk_identity, "stable_regular_file_handle_identity", forbidden_hash
        )
        with pytest.raises(ValueError, match="total-byte policy"):
            wasi_sdk_tree_identity(tmp_path)


@pytest.mark.parametrize(
    "contents",
    [b'{"schema":1,"schema":2}', b'{"value":NaN}', b" " * (64 * 1024 + 1)],
    ids=["duplicate-key", "non-finite", "oversized"],
)
def test_sdk_receipt_uses_bounded_exact_json(tmp_path: Path, contents: bytes) -> None:
    (tmp_path / INSTALL_RECEIPT_FILENAME).write_bytes(contents)
    with pytest.raises(ValueError, match="provision receipt is invalid"):
        wasi_sdk_identity.load_wasi_sdk_install_receipt(tmp_path)


@pytest.mark.parametrize("corrupt", [False, True])
def test_v1_receipt_upgrade_verifies_once_and_preserves_sdk_payload(
    tmp_path, monkeypatch, corrupt
):
    archive = tmp_path / "sdk.tar.gz"
    _write_archive(archive, _required_members())
    downloads = []
    _install_asset(monkeypatch, archive, downloads)
    custody = tmp_path / "custody"
    prefix = provisioner.provision_wasi_sdk(custody, downloads=tmp_path / "downloads")
    receipt_path = prefix / INSTALL_RECEIPT_FILENAME
    original = json.loads(receipt_path.read_text(encoding="utf-8"))
    legacy = {key: value for key, value in original.items() if key != "facts"}
    legacy["schema"] = "molt.wasi-sdk-install.v1"
    receipt_path.write_text(json.dumps(legacy), encoding="utf-8")
    legacy_bytes = receipt_path.read_bytes()
    with pytest.raises(llvm_toolchain.LlvmToolchainConfigError, match="v2"):
        llvm_toolchain.load_wasi_sdk_installation(
            provisioner.ROOT, prefix, verify_tree=False
        )
    sdk = prefix / "sdk"
    if corrupt:
        (sdk / "share/wasi-sysroot/include/wasm32-wasip1/errno.h").write_bytes(
            b"changed"
        )
    before = {
        path.relative_to(sdk): (
            path.read_bytes(),
            path.stat().st_ino,
            path.stat().st_mtime_ns,
        )
        for path in sdk.rglob("*")
        if path.is_file()
    }
    observed = []
    capture = wasi_sdk_identity.wasi_sdk_tree_identity

    def once(path):
        observed.append(path)
        return capture(path)

    monkeypatch.setattr(provisioner, "wasi_sdk_tree_identity", once)
    monkeypatch.setattr(llvm_toolchain, "wasi_sdk_tree_identity", once)
    if corrupt:
        with pytest.raises(ValueError, match="tree changed"):
            provisioner.provision_wasi_sdk(custody)
        assert receipt_path.read_bytes() == legacy_bytes
    else:
        assert provisioner.provision_wasi_sdk(custody) == prefix
        assert json.loads(receipt_path.read_text(encoding="utf-8")) == original
    assert observed == [sdk]
    assert len(downloads) == 1
    assert before == {
        path.relative_to(sdk): (
            path.read_bytes(),
            path.stat().st_ino,
            path.stat().st_mtime_ns,
        )
        for path in sdk.rglob("*")
        if path.is_file()
    }


def test_receipt_facts_use_captured_tree_without_reopening_members(
    tmp_path, monkeypatch
):
    archive = tmp_path / "sdk.tar.gz"
    _write_archive(archive, _required_members())
    downloads = []
    asset = _install_asset(monkeypatch, archive, downloads)
    prefix = provisioner.provision_wasi_sdk(tmp_path / "custody")
    sdk = prefix / "sdk"
    captured = wasi_sdk_tree_identity(sdk)
    expected = json.loads(
        (prefix / INSTALL_RECEIPT_FILENAME).read_text(encoding="utf-8")
    )
    (sdk / "bin" / executable_filename("clang", asset.id)).write_bytes(
        b"different generation"
    )
    monkeypatch.setattr(
        wasi_sdk_identity,
        "open_stable_regular_file",
        lambda *args, **kwargs: pytest.fail("receipt re-read a live member"),
    )
    assert (
        json.loads(
            wasi_sdk_identity.render_wasi_sdk_install_receipt(asdict(asset), captured)
        )
        == expected
    )


@pytest.mark.parametrize("mode", ["version", "tree-version", "tree-file"])
def test_sdk_identity_reads_only_its_initial_extent(tmp_path, monkeypatch, mode):
    from contextlib import contextmanager
    from tests.operation_probe import same_thread_probe

    sdk = tmp_path / "sdk"
    sdk.mkdir()
    version = mode != "tree-file"
    path = sdk / ("VERSION" if version else "payload")
    raw = VERSION_TEXT if version else b"SDK bytes"
    path.write_bytes(raw)

    def read():
        if mode == "version":
            return wasi_sdk_identity.read_wasi_sdk_version_identity(path)
        return wasi_sdk_tree_identity(sdk)

    observed = read()
    if mode == "version":
        assert observed.sdk_version == _WASI.sdk_version
        assert observed.llvm_version == _WASI.llvm_version
    else:
        assert (
            path.name,
            "file",
            len(raw),
            hashlib.sha256(raw).hexdigest(),
        ) in observed.records
        assert (observed.version is not None) is version
    original = wasi_sdk_identity.open_stable_regular_file
    consumed = []
    attempts = []

    class GrowingStream:
        def __init__(self, stream):
            self.stream = stream
            self.grown = False

        def __getattr__(self, name):
            return getattr(self.stream, name)

        def grow(self):
            if not self.grown:
                self.grown = True
                attempts.append(path)
                # The real POSIX file grows after admitted metadata was read.
                # Windows may refuse this write under its existing read lease.
                with path.open("ab") as writer:
                    writer.write(b"late bytes" * 10000)

        def read(self, size=-1):
            self.grow()
            data = self.stream.read(size)
            consumed.append(len(data))
            return data

        def readinto(self, buffer):
            # The predecessor hashlib.file_digest uses readinto, so count its
            # actual bytes as well; do not assume the repaired read mechanism.
            self.grow()
            count = self.stream.readinto(buffer)
            consumed.append(count)
            return count

    @contextmanager
    def growing_open(selected, **kwargs):
        with original(selected, **kwargs) as opened:
            yield replace(opened, stream=GrowingStream(opened.stream))

    monkeypatch.setattr(
        wasi_sdk_identity,
        "open_stable_regular_file",
        same_thread_probe(original, growing_open),
    )
    with pytest.raises(wasi_sdk_identity.WasiSdkIdentityError) as failure:
        read()
    assert attempts == [path]
    assert sum(consumed) <= len(raw) + 1
    assert "changed" in str(failure.value)


@pytest.mark.parametrize("explicit_cli", [False, True])
def test_cli_provisioner_and_reader_share_selected_tool_state(
    tmp_path, monkeypatch, explicit_cli
):
    archive = tmp_path / "fixture.tar.gz"
    _write_archive(archive, _required_members())
    downloads = []
    asset = _install_asset(monkeypatch, archive, downloads)
    environment_root = tmp_path / "environment tools"
    selected = tmp_path / "explicit tools" if explicit_cli else environment_root
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(environment_root))
    output = tmp_path / "outputs.txt"
    args = ["--github-output", str(output)]
    if explicit_cli:
        args += ["--toolchain-root", str(selected)]
    assert provisioner.main(args) == 0
    prefix = llvm_toolchain.wasi_sdk_install_prefix(selected, asset)
    assert f"install={prefix}\n" in output.read_text(encoding="utf-8")
    assert (
        llvm_toolchain.provisioned_wasi_sdk_prefix(
            provisioner.ROOT, environ={"MOLT_TARGET_ROOT": str(selected)}
        )
        == prefix
    )
    assert len(downloads) == 1
    if explicit_cli:
        assert not environment_root.exists()
