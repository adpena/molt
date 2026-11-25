import hashlib
import json
import os
from pathlib import Path

import pytest
from collections.abc import Iterable, Iterator, Mapping, Sequence
from typing import NoReturn

from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkInput,
    SourceExtensionLinkLoadingPolicy,
    SourceExtensionLinkRequirements,
    source_extension_link_file,
)
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    provisioned_wasi_sdk_fixture,
)
from tests.process_guard_common import install_module_view
from tools import (
    wasm_link,
    wasm_link_command,
    wasm_link_fact_provider,
    wasm_link_format,
    wasm_link_native_inputs,
    wasm_link_optimizer_policy,
)
from molt import wasm_artifact
from molt._wasm_abi_generated import (
    compiled_execution_symbol,
)

from molt.cli import wasm_link_cache
from molt.temporary_artifacts import OwnedTemporaryDirectory


COMPILED_MAIN = compiled_execution_symbol("molt_main")
COMPILED_HOST_INIT = compiled_execution_symbol("molt_host_init")
COMPILED_ISOLATE_IMPORT = compiled_execution_symbol("molt_isolate_import")
COMPILED_F0 = compiled_execution_symbol("probe__f0")
COMPILED_F1 = compiled_execution_symbol("probe__f1")
COMPILED_F2 = compiled_execution_symbol("probe__f2")
APP_INIT = compiled_execution_symbol("py.main_molt.init")
APP_OCR = compiled_execution_symbol("py.main_molt.ocr_tokens")
APP_PRIVATE = compiled_execution_symbol("py.main_molt._private_helper")


@pytest.mark.parametrize(
    "stdout, message",
    [
        (
            '{"schema_version":9,"schema_version":10,"ok":true,'
            '"facts":{"schema_version":10}}',
            "duplicate JSON key",
        ),
        (
            '{"schema_version":10,"ok":true,"facts":{"schema_version":10}}',
            "unsupported response schema",
        ),
        (
            '{"schema_version":9,"ok":true,"facts":{"schema_version":9},'
            '"extension":true}',
            "invalid success response",
        ),
    ],
)
def test_wasm_facts_decoder_rejects_schema_ambiguity(
    stdout: str,
    message: str,
) -> None:
    with pytest.raises(ValueError, match=message):
        wasm_link_fact_provider._decode_wasm_facts_response(
            stdout, failed=False, stderr="", operation="test scan"
        )


def test_external_wasm_ld_uses_response_file_beyond_windows_command_limit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    if wasm_link_command.os.name != "nt":
        pytest.skip("Windows command-length response-file authority")
    observed: dict[str, object] = {}

    def guarded(_executor, command, **_kwargs):  # type: ignore[no-untyped-def]
        observed["command"] = command
        observed["response"] = Path(command[1][1:]).read_text(encoding="utf-8")
        return wasm_link_command.subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(
        wasm_link_command.CommandExecutor,
        "run",
        guarded,
    )
    arguments = ["--export-if-defined=" + "x" * 100 for _ in range(400)]

    result = wasm_link_command._run_external_tool(["wasm-ld.exe", *arguments])

    assert result.returncode == 0
    assert observed["command"][0] == "wasm-ld.exe"
    assert observed["command"][1].startswith("@")
    assert "\n".join(arguments) in observed["response"]


def test_wasm_link_uses_one_command_execution_module_identity() -> None:
    assert wasm_link_command.CommandExecutor is wasm_link_fact_provider.CommandExecutor


def test_stable_snapshot_streams_source_through_one_read_handle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    payload = bytes(range(251)) * 16_777
    source.write_bytes(payload)
    source_open_count = 0
    real_open = identity_authority.open_stable_read_descriptor
    real_read_bytes = Path.read_bytes

    def counting_open(path: Path) -> int:
        nonlocal source_open_count
        if Path(path) == source:
            source_open_count += 1
        return real_open(path)

    def reject_path_read_bytes(path: Path) -> bytes:
        if path in {source, snapshot}:
            raise AssertionError("stable snapshot must use its single open handle")
        return real_read_bytes(path)

    monkeypatch.setattr(
        identity_authority, "open_stable_read_descriptor", counting_open
    )
    monkeypatch.setattr(Path, "read_bytes", reject_path_read_bytes)
    monkeypatch.setattr(identity_authority, "_STABLE_SNAPSHOT_CHUNK_BYTES", 64 * 1024)

    captured = identity_authority.snapshot_stable_regular_file(
        source,
        snapshot,
        label="profiled source",
        capture_prefix_bytes=17,
    )

    assert source_open_count == 1
    assert captured.prefix == payload[:17]
    assert len(captured.prefix) == 17
    assert captured.source.size == captured.snapshot.size == len(payload)
    assert (
        captured.source.sha256
        == captured.snapshot.sha256
        == hashlib.sha256(payload).hexdigest()
    )


def test_stable_snapshot_preserves_unowned_existing_destination(tmp_path: Path) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"new snapshot")
    snapshot.write_bytes(b"pre-existing authority")

    with pytest.raises(FileExistsError):
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="owned source",
        )

    assert snapshot.read_bytes() == b"pre-existing authority"


def test_stable_snapshot_discard_preserves_replacement_file(tmp_path: Path) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"owned snapshot")
    captured = identity_authority.snapshot_stable_regular_file(
        source,
        snapshot,
        label="owned source",
    )
    snapshot.unlink()
    snapshot.write_bytes(b"replacement authority")

    captured.discard()

    assert snapshot.read_bytes() == b"replacement authority"


def test_stable_snapshot_rejects_same_size_tamper_before_snapshot_attestation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"original")
    real_identity = identity_authority.stable_regular_file_identity
    tampered = False

    def tamper_before_identity(
        path: Path, *, label: str
    ) -> identity_authority.StableRegularFileIdentity:
        nonlocal tampered
        if path == snapshot and not tampered:
            tampered = True
            snapshot.write_bytes(b"tampered")
        return real_identity(path, label=label)

    monkeypatch.setattr(
        identity_authority,
        "stable_regular_file_identity",
        tamper_before_identity,
    )

    with pytest.raises(
        identity_authority.StableRegularFileSnapshotError,
        match="snapshot content changed",
    ):
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="tampered source",
        )

    assert tampered
    assert not snapshot.exists()


def test_stable_snapshot_rejects_midstream_source_change(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"abcdefgh")
    initial_stat = source.stat()
    real_sha256 = identity_authority.hashlib.sha256
    changed = False

    class ChangingHasher:
        def __init__(self) -> None:
            self._inner = real_sha256()

        def update(self, data: bytes) -> None:
            nonlocal changed
            self._inner.update(data)
            if not changed:
                changed = True
                os.utime(
                    source,
                    ns=(
                        initial_stat.st_atime_ns,
                        initial_stat.st_mtime_ns + 1_000_000_000,
                    ),
                )

        def hexdigest(self) -> str:
            return self._inner.hexdigest()

    monkeypatch.setattr(identity_authority, "_STABLE_SNAPSHOT_CHUNK_BYTES", 4)
    install_module_view(
        monkeypatch, "hashlib", hashlib, identity_authority, sha256=ChangingHasher
    )

    with pytest.raises(
        identity_authority.StableRegularFileChangedError,
        match="changed during identity read",
    ):
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="racing source",
        )

    assert changed
    assert not snapshot.exists()


def test_stable_snapshot_rejects_symlink_source(tmp_path: Path) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    source.write_bytes(b"source")
    indirect = tmp_path / "indirect.bin"
    try:
        indirect.symlink_to(source)
    except OSError as exc:
        pytest.skip(f"symlinks unavailable on this host: {exc}")

    with pytest.raises(
        identity_authority.StableRegularFileError,
        match="not one stable regular file",
    ):
        identity_authority.snapshot_stable_regular_file(
            indirect,
            tmp_path / "snapshot.bin",
            label="indirect source",
        )


def test_stable_snapshot_rejects_reparse_metadata_as_path_indirection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    source.write_bytes(b"source")
    source_stat = source.lstat()
    source_identity = identity_authority._stat_identity(source_stat)
    real_is_reparse = identity_authority.metadata_is_link_like

    def mark_source_as_reparse(metadata: os.stat_result) -> bool:
        return identity_authority._stat_identity(
            metadata
        ) == source_identity or real_is_reparse(metadata)

    monkeypatch.setattr(
        identity_authority,
        "metadata_is_link_like",
        mark_source_as_reparse,
    )

    with pytest.raises(
        identity_authority.StableRegularFileError,
        match="not one stable regular file",
    ):
        identity_authority.snapshot_stable_regular_file(
            source,
            tmp_path / "snapshot.bin",
            label="reparse source",
        )


def test_reparse_metadata_capability_uses_platform_file_attributes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    reparse_attribute = 0x00000400
    metadata = type(
        "_WindowsStatMetadata",
        (),
        {"st_file_attributes": reparse_attribute, "st_mode": 0o100600},
    )()
    monkeypatch.setattr(
        identity_authority.stat,
        "FILE_ATTRIBUTE_REPARSE_POINT",
        reparse_attribute,
        raising=False,
    )

    assert identity_authority.metadata_is_link_like(metadata)


def test_stable_snapshot_missing_change_time_is_permanent_custody_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    snapshot = tmp_path / "snapshot.bin"
    source.write_bytes(b"stable")
    monkeypatch.setattr(
        identity_authority,
        "content_change_time_ns_from_fd",
        lambda *_args: None,
    )

    with pytest.raises(identity_authority.StableRegularFileError) as error:
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="unsupported custody source",
        )

    assert type(error.value) is identity_authority.StableRegularFileError
    assert "change-time identity" in str(error.value)
    assert not snapshot.exists()


def test_wasm_link_external_tool_uses_command_executor(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_run(_executor, cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return wasm_link_command.subprocess.CompletedProcess(
            cmd, 0, stdout="ok\n", stderr=""
        )

    monkeypatch.setattr(wasm_link_command.CommandExecutor, "run", fake_run)

    result = wasm_link_command._run_external_tool(["wasm-tools", "validate", "x.wasm"])

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["wasm-tools", "validate", "x.wasm"]
    assert captured["kwargs"]["capture_output"] is True
    assert captured["kwargs"]["text"] is True


def test_wasm_link_external_tool_preserves_timeout_semantics(monkeypatch) -> None:
    def fake_run(_executor, cmd, **_kwargs):  # type: ignore[no-untyped-def]
        raise wasm_link_command.subprocess.TimeoutExpired(
            cmd,
            timeout=1,
            output="partial",
            stderr="memory_guard: timeout after 1.00s\n",
        )

    monkeypatch.setattr(wasm_link_command.CommandExecutor, "run", fake_run)

    with pytest.raises(wasm_link_command.subprocess.TimeoutExpired) as exc_info:
        wasm_link_command._run_external_tool(["wasm-opt", "x.wasm"], timeout=1)

    assert exc_info.value.cmd == ["wasm-opt", "x.wasm"]
    assert exc_info.value.output == "partial"
    assert exc_info.value.stderr == "memory_guard: timeout after 1.00s\n"


def test_wasm_link_default_artifact_paths_use_canonical_dist(
    monkeypatch, unbound_checkout_family
) -> None:
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)

    assert wasm_link._default_input_path() == Path("dist") / "output.wasm"
    assert wasm_link._default_output_path() == Path("dist") / "output_linked.wasm"


def test_wasm_link_default_artifact_paths_follow_external_root(
    tmp_path: Path,
    monkeypatch,
) -> None:
    ext_root = tmp_path / "ext-root"
    ext_root.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))

    assert wasm_link._default_input_path() == (
        ext_root.resolve() / "dist" / "output.wasm"
    )
    assert wasm_link._default_output_path() == (
        ext_root.resolve() / "dist" / "output_linked.wasm"
    )


def test_python_whole_module_facts_authority_is_deleted() -> None:
    assert not hasattr(wasm_link_format, "WasmModuleFacts")
    assert not hasattr(wasm_link_format, "parse_wasm_module_facts")


@pytest.mark.parametrize("extension", ("unknown-root-key", "duplicate-root-key"))
def test_wasm_link_cache_metadata_rejects_json_root_extensions(
    extension: str,
    tmp_path: Path,
) -> None:
    entry = wasm_link_cache._wasm_link_cache_entry(
        "runtime_tree_shake",
        "test-v1",
        "a" * 64,
        cache_root=tmp_path,
    )
    wasm_link_cache._publish_wasm_link_cache_entry(
        entry,
        b"\x00asm\x01\x00\x00\x00",
    )
    metadata = json.loads(entry.metadata.read_text(encoding="utf-8"))
    assert set(metadata) == {"schema", "cache", "payload"}
    assert metadata["schema"] == wasm_link_cache.WASM_LINK_CACHE_ENTRY_SCHEMA
    if extension == "unknown-root-key":
        metadata["unknown"] = True
        entry.metadata.write_text(json.dumps(metadata), encoding="utf-8")
    else:
        encoded = entry.metadata.read_text(encoding="utf-8")
        entry.metadata.write_text(
            encoded.replace("{", '{"schema":"duplicate",', 1),
            encoding="utf-8",
        )

    assert wasm_link_cache._read_wasm_link_cache_entry(entry).status == "corrupt"


def test_oz_publication_pipeline_is_bounded_and_size_focused() -> None:
    assert list(wasm_link_optimizer_policy.wasm_link_policy("Oz").extra_passes) == [
        "--remove-unused-module-elements",
        "--strip-debug",
        "--strip-producers",
        "--dae-optimizing",
        "--simplify-locals",
        "--merge-blocks",
        "--dce",
        "--vacuum",
        "--memory-packing",
    ]


# ---------------------------------------------------------------------------
# Allowlist validation
# ---------------------------------------------------------------------------


def _parse_allowlist(path: Path) -> set[str]:
    lines = path.read_text(encoding="utf-8").splitlines()
    return {
        line.strip()
        for line in lines
        if line.strip() and not line.strip().startswith("#")
    }


def test_native_object_link_allowlist_includes_generated_external_imports(
    tmp_path: Path,
) -> None:
    base = tmp_path / "base_allowlist.txt"
    base.write_text("fd_write\n", encoding="utf-8")
    native = tmp_path / "extension.molt.wasm"
    native.write_bytes(b"\0asm\x01\0\0\0")

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        assert (
            wasm_link_native_inputs._compose_wasm_ld_allowlist(
                base_allowlist=base,
                native_link_requirements=_native_link_requirements(),
                temp_dir=temp_dir,
            )
            == base
        )

        composed = wasm_link_native_inputs._compose_wasm_ld_allowlist(
            base_allowlist=base,
            native_link_requirements=_native_link_requirements(native),
            temp_dir=temp_dir,
        )

        symbols = _parse_allowlist(composed)
        assert "fd_write" in symbols
        assert "__cpp_exception" in symbols
        assert "malloc" not in symbols
        assert "__trunctfdf2" not in symbols

        with_provider = wasm_link_native_inputs._compose_wasm_ld_allowlist(
            base_allowlist=base,
            native_link_requirements=_native_link_requirements(native),
            temp_dir=temp_dir,
            provider_symbols=frozenset({"malloc"}),
        )
        assert _parse_allowlist(with_provider) == symbols | {"malloc"}
        assert base.read_text(encoding="utf-8") == "fd_write\n"


# --- Split-runtime CPython-ABI data-symbol aliasing ------------------------
#
# A native extension (numpy/scipy) references the runtime's canonical
# singletons/type/exception objects (Py_None, Py_False, PyExc_*, Py*_Type) as
# *undefined data symbols*. In a split-runtime build the app links against
# imported memory and must resolve those references to the DEPLOY runtime's
# single copy â€” otherwise the extension links its own zero-initialized
# duplicate and pointer-identity bridges (pyobj_to_handle) fail. The deploy
# runtime publishes each address as an exported i32 global (wasm-ld's encoding
# for --export-if-defined of a defined data symbol); the aliaser reads those
# addresses. See tools/wasm_link_runtime_data.py::_split_runtime_data_alias_object.


@pytest.mark.parametrize(
    ("bits", "value"),
    [
        (32, -(1 << 31)),
        (32, (1 << 31) - 1),
        (33, -(1 << 32)),
        (33, (1 << 32) - 1),
        (64, -(1 << 63)),
        (64, (1 << 63) - 1),
    ],
)
def test_wasm_signed_leb128_round_trips_boundary_values(bits: int, value: int) -> None:
    encoded = wasm_artifact._encode_wasm_varint(value, bits)
    decoded, offset = wasm_artifact._read_wasm_varint(encoded, 0, bits)
    assert (decoded, offset) == (value, len(encoded))


def test_wasm_signed_leb128_rejects_overlong_width() -> None:
    with pytest.raises(ValueError, match="varint too large"):
        wasm_artifact._read_wasm_varint(b"\x80\x80\x80\x80\x80\x00", 0, 32)


def test_shared_runtime_exports_cpython_abi_data_symbols_as_globals() -> None:
    # The shared/deploy runtime link surface must publish the canonical
    # CPython-ABI data symbols so the aliaser can read their addresses.
    from molt._wasm_runtime_exports import (
        wasm_cpython_abi_data_symbol_names,
        wasm_runtime_shared_export_link_args,
    )

    data_symbols = wasm_cpython_abi_data_symbol_names()
    assert {"Py_None", "PyExc_ValueError", "PyList_Type"}.issubset(data_symbols)
    args = wasm_runtime_shared_export_link_args()
    for name in ("Py_None", "PyBool_Type", "PyExc_ValueError", "PyList_Type"):
        assert f"link-arg=--export-if-defined={name}" in args, name


def _native_link_requirements(
    *paths: Path,
    retained_symbols: tuple[str, ...] = (),
) -> SourceExtensionLinkRequirements:
    return SourceExtensionLinkRequirements(
        "wasm32-wasip1",
        tuple(source_extension_link_file(path) for path in paths),
        retained_symbols,
    )


def test_scanner_expected_identity_is_checked_before_snapshot(tmp_path: Path) -> None:
    scanner = tmp_path / "scanner"
    scanner.write_bytes(b"replacement scanner")
    scratch = tmp_path / "snapshot"
    with pytest.raises(ValueError, match="expected input identity"):
        wasm_link_fact_provider._snapshot_rust_wasm_facts_scanner(
            scanner,
            scratch,
            expected_sha256="0" * 64,
        )
    assert not scratch.exists()


def test_linker_tool_and_sdk_generation_share_request_cwd(tmp_path, monkeypatch):
    from molt.cli import wasm_toolchain

    cwd = tmp_path / "request"
    (cwd / "relative-tools").mkdir(parents=True)
    installation = provisioned_wasi_sdk_fixture(
        RuntimeFixtureRoot(cwd / "relative-tools")
    )
    prefix = installation.prefix
    ambient = tmp_path / "ambient"
    ambient.mkdir()
    monkeypatch.chdir(ambient)
    monkeypatch.setattr(
        wasm_toolchain,
        "_wasm_linker_binary_identity",
        lambda *_a, **_k: pytest.fail("managed receipt owns tool identity"),
    )
    actual = wasm_toolchain.resolve_wasm_linker(
        Path(__file__).resolve().parents[1],
        env={"MOLT_TARGET_ROOT": "relative-tools/toolchains"},
        cwd=cwd,
    )
    assert actual.path.parent == prefix / "sdk/bin"
    assert actual.wasi_sdk_llvm_version == installation.asset.llvm_version
    assert not (ambient / "relative-tools").exists()


# Native-input provider selection and the optimizer export contract read module
# facts through the real Rust scanner: the facts of each fixture module come
# from `molt-wasm-facts`, an oracle independent of the link tools under test.

from molt.llvm_toolchain import WasiSdkInstallation  # noqa: E402
from molt.wasi_sdk_identity import WasiCAbiProjection  # noqa: E402

RustFacts = wasm_link_fact_provider.RustWasmFactsProvider

_I32 = 0x7F
_I64 = 0x7E


def _function_type(params: tuple[int, ...], results: tuple[int, ...]) -> bytes:
    write = wasm_link_format._write_varuint
    return (
        b"\x60"
        + write(len(params))
        + bytes(params)
        + write(len(results))
        + bytes(results)
    )


def _code_section(body: bytes, count: int) -> bytes:
    write = wasm_link_format._write_varuint
    entry = write(len(body)) + body
    return write(count) + entry * count


def _env_import_module(names: list[str]) -> bytes:
    """A module that imports each name as an ``env`` function."""
    write = wasm_link_format._write_varuint
    imports = b"".join(
        wasm_link_format._write_string("env")
        + wasm_link_format._write_string(name)
        + b"\x00"
        + write(0)
        for name in names
    )
    return wasm_link_format._build_sections(
        [
            (1, write(1) + _function_type((_I64,), ())),
            (2, write(len(names)) + imports),
        ]
    )


def _exported_function_module(names: list[str]) -> bytes:
    """A module that defines and exports one ``() -> i64`` function per name."""
    write = wasm_link_format._write_varuint
    exports = b"".join(
        wasm_link_format._write_string(name) + b"\x00" + write(index)
        for index, name in enumerate(names)
    )
    return wasm_link_format._build_sections(
        [
            (1, write(1) + _function_type((), (_I64,))),
            (3, write(len(names)) + write(0) * len(names)),
            (7, write(len(names)) + exports),
            (10, _code_section(b"\x00\x42\x00\x0b", len(names))),
        ]
    )


def _linking_function_object(name: str) -> bytes:
    """A relocatable object whose symbol table defines one global function."""
    write = wasm_link_format._write_varuint
    # One function symbol: kind 0, binding global, function index 0, its name.
    symbols = (
        write(1) + b"\x00" + write(0) + write(0) + wasm_link_format._write_string(name)
    )
    linking = (
        wasm_link_format._write_string("linking")
        + write(2)
        + b"\x08"
        + write(len(symbols))
        + symbols
    )
    return wasm_link_format._build_sections(
        [
            (1, write(1) + _function_type((), (_I32,))),
            (3, write(1) + write(0)),
            (10, _code_section(b"\x00\x41\x00\x0b", 1)),
            (0, linking),
        ]
    )


def _wasm_archive(*members: tuple[str, bytes]) -> bytes:
    archive = bytearray(b"!<arch>\n")
    for name, data in members:
        archive.extend(f"{name}/".encode("ascii").ljust(16))
        archive.extend(b"0".ljust(12) + b"0".ljust(6) + b"0".ljust(6))
        archive.extend(b"100644".ljust(8) + str(len(data)).encode("ascii").ljust(10))
        archive.extend(b"`\n" + data + (b"\n" if len(data) & 1 else b""))
    return bytes(archive)


def _compiler_rt_provider_archive() -> bytes:
    return _wasm_archive(("trunctfdf2.o", _linking_function_object("__trunctfdf2")))


@pytest.fixture(scope="module")
def rust_facts(
    tmp_path_factory: pytest.TempPathFactory,
) -> wasm_link_fact_provider.RustWasmFactsProvider:
    """The real scanner product, built through the compiler's tool custody."""
    from molt.cli.wasm_facts_tool import ensure_wasm_facts_tool

    root = Path(__file__).resolve().parents[1]
    scanner = ensure_wasm_facts_tool(root, env=os.environ)
    work = tmp_path_factory.mktemp("rust-wasm-facts")
    return wasm_link_fact_provider.make_rust_wasm_facts_provider(
        scanner, work / "scratch", evidence_root=work / "evidence"
    )


def _install_compiler_rt_sdk(
    tmp_path: Path,
) -> tuple[WasiSdkInstallation, WasiCAbiProjection]:
    """A provisioned SDK whose compiler-rt archive defines ``__trunctfdf2``."""
    from dataclasses import asdict

    from molt import llvm_toolchain
    from molt.wasi_sdk_identity import (
        render_wasi_sdk_install_receipt,
        wasi_sdk_tree_identity,
    )

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    paths = llvm_toolchain.wasi_c_abi_plan(installation)
    paths.path("compiler_rt").write_bytes(_compiler_rt_provider_archive())
    # The fixture publishes final member bytes before any consumer gets a plan.
    (installation.prefix / ".molt-wasi-sdk.json").write_text(
        render_wasi_sdk_install_receipt(
            asdict(installation.asset), wasi_sdk_tree_identity(installation.sdk)
        ),
        encoding="utf-8",
        newline="",
    )
    return installation, paths


def _compiler_rt_sdk_fixture(tmp_path: Path) -> WasiCAbiProjection:
    from tests.runtime_build_identity_helper import runtime_wasi_c_abi_plan

    _install_compiler_rt_sdk(tmp_path)
    return runtime_wasi_c_abi_plan(RuntimeFixtureRoot(tmp_path))


def test_rust_facts_fixtures_carry_the_symbols_the_planner_reads(
    rust_facts: RustFacts,
) -> None:
    """The scanner, not the planner, says what each fixture imports and defines."""
    imports = rust_facts(_env_import_module(["__trunctfdf2", "malloc"]))
    assert [(fact.module, fact.name, fact.kind) for fact in imports.imports] == [
        ("env", "__trunctfdf2", 0),
        ("env", "malloc", 0),
    ]
    exports = rust_facts(_exported_function_module([COMPILED_MAIN, COMPILED_HOST_INIT]))
    assert exports.function_exports == {COMPILED_MAIN: 0, COMPILED_HOST_INIT: 1}
    provider = rust_facts(_linking_function_object("__trunctfdf2"))
    assert provider.linking_symbols.defined_functions == frozenset({"__trunctfdf2"})


def test_resolve_native_link_inputs_adds_compiler_rt_provider(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    native = tmp_path / "native.molt.wasm"
    sdk_plan = _compiler_rt_sdk_fixture(tmp_path)
    provider = sdk_plan.path("compiler_rt")
    native.write_bytes(_env_import_module(["__trunctfdf2", "malloc"]))
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda **_kwargs: sdk_plan,
    )
    monkeypatch.setattr(
        wasm_link_native_inputs,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: frozenset({"__trunctfdf2"}),
    )

    requirements = wasm_link_native_inputs._resolve_native_link_requirements(
        _native_link_requirements(native),
        source_paths={native: native},
        facts_provider=rust_facts,
        capture_input=lambda item: item,
    ).requirements

    assert tuple(Path(item.path) for item in requirements.inputs) == (native, provider)
    assert requirements.inputs[1].sha256 == source_extension_link_file(provider).sha256


def test_resolve_native_link_inputs_rejects_missing_compiler_rt_provider(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    native = tmp_path / "native.molt.wasm"
    native.write_bytes(_env_import_module(["__trunctfdf2"]))

    def absent_sdk(**_kwargs: object) -> NoReturn:
        raise ValueError("selected complete WASI SDK is unavailable")

    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs, "resolve_wasi_c_abi_plan", absent_sdk
    )
    with pytest.raises(ValueError, match="selected complete WASI SDK is unavailable"):
        wasm_link_native_inputs._resolve_native_link_requirements(
            _native_link_requirements(native),
            source_paths={native: native},
            facts_provider=rust_facts,
            capture_input=lambda item: item,
        )


def test_lazy_archive_compiler_rt_candidates_retain_lazy_selected_sdk_provider(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    from tests.runtime_build_identity_helper import clear_wasm_toolchain_selection

    archive = tmp_path / "lazy.a"
    archive.write_bytes(
        _wasm_archive(("dormant.o", _env_import_module(["__trunctfdf2"])))
    )
    installation, sdk_plan = _install_compiler_rt_sdk(tmp_path)
    provider = sdk_plan.path("compiler_rt")
    clear_wasm_toolchain_selection(monkeypatch)
    monkeypatch.setenv("WASI_SDK_PATH", str(installation.sdk))
    # Native symbol inspection is independent of installation and file custody;
    # the selected-SDK readiness and plan readers run for real.
    monkeypatch.setattr(
        wasm_link_native_inputs,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: frozenset({"__trunctfdf2"}),
    )
    original = _native_link_requirements(archive)
    selected = wasm_link_native_inputs._resolve_native_link_requirements(
        original,
        source_paths={archive: archive},
        facts_provider=rust_facts,
        capture_input=lambda item: item,
    ).requirements
    assert selected.inputs[0] == original.inputs[0]
    assert tuple(Path(item.path) for item in selected.inputs) == (archive, provider)
    assert (
        selected.inputs[1].sha256 == hashlib.sha256(provider.read_bytes()).hexdigest()
    )
    assert all(
        item.loading is SourceExtensionLinkLoadingPolicy.DEFAULT
        for item in selected.inputs
    )


def test_lazy_archive_compiler_rt_candidates_do_not_require_an_unused_provider(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    archive = tmp_path / "lazy.a"
    archive.write_bytes(
        _wasm_archive(("dormant.o", _env_import_module(["__trunctfdf2"])))
    )
    # A residual in a dormant member defers acquisition; eager members require it.
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_sysroot",
        lambda **_kwargs: None,
    )

    def unavailable_sdk(**_kwargs: object) -> NoReturn:
        raise ValueError("selected complete WASI SDK is unavailable")

    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        unavailable_sdk,
    )
    original = _native_link_requirements(archive)
    assert (
        wasm_link_native_inputs._resolve_native_link_requirements(
            original,
            source_paths={archive: archive},
            facts_provider=rust_facts,
            capture_input=lambda item: item,
        ).requirements
        == original
    )
    eager = SourceExtensionLinkRequirements(
        "wasm32-wasip1",
        (
            source_extension_link_file(
                archive, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
            ),
        ),
    )
    with pytest.raises(ValueError, match="selected complete WASI SDK is unavailable"):
        wasm_link_native_inputs._resolve_native_link_requirements(
            eager,
            source_paths={archive: archive},
            facts_provider=rust_facts,
            capture_input=lambda item: item,
        )


def test_no_compiler_rt_candidates_never_selects_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    archive = tmp_path / "lazy-no-binary128.a"
    archive.write_bytes(
        _wasm_archive(("dormant.o", _env_import_module(["PyLong_FromLong"])))
    )
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda **_kwargs: pytest.fail("unused SDK selected"),
    )
    original = _native_link_requirements(archive)
    assert (
        wasm_link_native_inputs._resolve_native_link_requirements(
            original,
            source_paths={archive: archive},
            facts_provider=rust_facts,
            capture_input=lambda item: item,
        ).requirements
        == original
    )


def test_locally_satisfied_native_imports_do_not_discover_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    imported = tmp_path / "import.o"
    defined = tmp_path / "define.o"
    imported.write_bytes(
        _env_import_module(["ordinary_local_function", "PyLong_FromLong"])
    )
    defined.write_bytes(_exported_function_module(["ordinary_local_function"]))
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda **_kwargs: pytest.fail("locally satisfied import selected SDK"),
    )
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_sysroot",
        lambda **_kwargs: pytest.fail("locally satisfied import queried SDK"),
    )
    original = _native_link_requirements(imported, defined)
    selected = wasm_link_native_inputs._resolve_native_link_requirements(
        original,
        source_paths={imported: imported, defined: defined},
        facts_provider=rust_facts,
        capture_input=lambda item: item,
    )
    assert selected.requirements == original
    assert not selected.provider_paths


def test_native_provider_planning_scans_members_and_compiler_rt_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    archive = tmp_path / "lazy.a"
    archive.write_bytes(
        _wasm_archive(
            ("a.o", _env_import_module(["__trunctfdf2"])),
            ("b.o", _env_import_module(["__trunctfdf2", "PyLong_FromLong"])),
        )
    )
    sdk = _compiler_rt_sdk_fixture(tmp_path)
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_sysroot",
        lambda **_kwargs: sdk.sysroot,
    )
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "resolve_wasi_c_abi_plan",
        lambda **_kwargs: sdk,
    )
    queries = []

    def provider(
        *,
        primitive_classes: frozenset[str],
        plan: WasiCAbiProjection,
        archive_paths: Mapping[Path, Path],
    ) -> frozenset[str]:
        assert archive_paths == {sdk.path("compiler_rt"): sdk.path("compiler_rt")}
        assert plan is sdk
        queries.append(primitive_classes)
        return frozenset({"__trunctfdf2"})

    monkeypatch.setattr(
        wasm_link_native_inputs, "wasm_external_link_provider_symbols", provider
    )
    scanned = []

    class RecordingFacts:
        def scan_many(
            self, modules: Iterable[bytes]
        ) -> Iterator[wasm_link_fact_provider.WasmLinkFacts]:
            for data in modules:
                scanned.append(data)
                yield rust_facts(data)

    selected = wasm_link_native_inputs._resolve_native_link_requirements(
        _native_link_requirements(archive),
        source_paths={archive: archive},
        facts_provider=RecordingFacts(),
        capture_input=lambda item: item,
    )
    assert len(scanned) == 2
    assert queries == [
        frozenset({wasm_link_native_inputs.WASM_COMPILER_RT_LINK_IMPORT_CLASS})
    ]
    assert selected.provider_paths["compiler_rt"] == sdk.path("compiler_rt")
    assert all(
        item.loading is SourceExtensionLinkLoadingPolicy.DEFAULT
        for item in selected.requirements.inputs
    )


@pytest.mark.parametrize("tampered", (False, True))
def test_admitted_sdk_provider_keeps_snapshot_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    rust_facts: RustFacts,
    tampered: bool,
) -> None:
    plan = _compiler_rt_sdk_fixture(tmp_path)
    original = plan.path("compiler_rt")
    snapshot = tmp_path / "captured-provider.a"
    snapshot.write_bytes(original.read_bytes())
    if tampered:
        snapshot.write_bytes(
            _wasm_archive(("different.o", _exported_function_module(["wrong"])))
        )
    requirements = _native_link_requirements(snapshot)
    # Admission already selected the original role and digest. Later live bytes
    # cannot change either the symbol surface or final arguments of this link.
    original.unlink()
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "admit_wasi_provider_inputs",
        lambda _paths: pytest.fail("captured provider was readmitted"),
    )
    monkeypatch.setattr(
        wasm_link_native_inputs,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: pytest.fail("captured member facts were rediscovered"),
    )

    def resolve() -> wasm_link_native_inputs.ResolvedNativeLinkInputs:
        return wasm_link_native_inputs._resolve_native_link_requirements(
            requirements,
            source_paths={snapshot: original},
            wasi_plan=plan,
            capture_input=lambda _item: pytest.fail("existing capture repeated"),
            facts_provider=rust_facts,
        )

    if tampered:
        with pytest.raises(ValueError, match="captured SDK provider differs"):
            resolve()
    else:
        selected = resolve()
        assert selected.requirements == requirements
        assert selected.provider_paths == {"compiler_rt": snapshot}
        assert selected.provider_symbols == frozenset({"__trunctfdf2"})


def test_discovered_provider_is_captured_before_symbol_inspection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    from tools import wasm_archive

    plan = _compiler_rt_sdk_fixture(tmp_path)
    original = plan.path("compiler_rt")
    provider_bytes = original.read_bytes()
    native = tmp_path / "native.o"
    native.write_bytes(_env_import_module(["__trunctfdf2"]))
    captured = tmp_path / "captured.a"
    captures = []

    def capture(item: SourceExtensionLinkInput) -> SourceExtensionLinkInput:
        captures.append(item)
        assert Path(item.path) == original
        captured.write_bytes(provider_bytes)
        original.write_bytes(b"live generation changed after capture")
        return source_extension_link_file(captured)

    def inspect(
        *,
        primitive_classes: frozenset[str],
        plan: WasiCAbiProjection,
        archive_paths: Mapping[Path, Path],
    ) -> frozenset[str]:
        assert primitive_classes == frozenset(
            {wasm_link_native_inputs.WASM_COMPILER_RT_LINK_IMPORT_CLASS}
        )
        assert archive_paths == {original: captured}
        return frozenset(
            symbol
            for member in wasm_archive.iter_wasm_object_members(archive_paths[original])
            for symbol in rust_facts(member.data).linking_symbols.defined_functions
        )

    monkeypatch.setattr(
        wasm_link_native_inputs, "wasm_external_link_provider_symbols", inspect
    )
    selected = wasm_link_native_inputs._resolve_native_link_requirements(
        _native_link_requirements(native),
        source_paths={native: native},
        wasi_plan=plan,
        capture_input=capture,
        facts_provider=rust_facts,
    )
    assert len(captures) == 1
    assert selected.provider_paths == {"compiler_rt": captured}
    assert tuple(Path(item.path) for item in selected.requirements.inputs) == (
        native,
        captured,
    )
    assert selected.provider_symbols == frozenset({"__trunctfdf2"})


def test_run_wasm_opt_via_optimize_enforces_current_export_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rust_facts: RustFacts
) -> None:
    from molt.toolchain_identity import StableRegularFileIdentity

    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_exported_function_module([COMPILED_MAIN, COMPILED_HOST_INIT]))
    optimizer_identity = wasm_link_optimizer_policy.WasmOptimizerExecutableIdentity(
        executable=StableRegularFileIdentity(
            path=tmp_path / "wasm-opt",
            size=0,
            sha256="a" * 64,
            _stat_identity=(0, 0, 0, 0, 0, 0),
            _content_change_time_ns=0,
        ),
        binaryen_version="wasm-opt version 130 (version_130)",
    )
    seen: dict[str, object] = {}

    def fake_optimize(
        input_path: Path,
        *,
        output_path: Path,
        level: str,
        extra_passes: Sequence[str],
        converge: bool,
        required_exports: set[str],
        apply_level: bool,
        optimizer_identity: wasm_link_optimizer_policy.WasmOptimizerExecutableIdentity,
        preserve_debug: bool = False,
    ) -> dict[str, object]:
        seen["required_exports"] = set(required_exports)
        seen["apply_level"] = apply_level
        seen["optimizer_identity"] = optimizer_identity
        output_path.write_bytes(input_path.read_bytes())
        digest = hashlib.sha256(input_path.read_bytes()).hexdigest()
        return {
            "ok": True,
            "status": "success",
            "output_bytes": output_path.stat().st_size,
            "binaryen_version": optimizer_identity.binaryen_version,
            "wasm_opt_path": str(optimizer_identity.path),
            "wasm_opt_sha256": optimizer_identity.sha256,
            "pipeline": list(
                wasm_link_optimizer_policy.wasm_link_policy(level).pipeline
            ),
            "before": {"file_bytes": input_path.stat().st_size, "sha256": digest},
            "after": {"file_bytes": output_path.stat().st_size, "sha256": digest},
            "error": "",
        }

    monkeypatch.setattr(wasm_link_optimizer_policy, "optimize_wasm", fake_optimize)
    attestation: dict[str, object] = {}
    telemetry: dict[str, object] = {}
    assert wasm_link_optimizer_policy._run_wasm_opt_via_optimize(
        linked,
        level="Oz",
        required_exports=set(rust_facts(linked.read_bytes()).function_exports),
        attestation=attestation,
        telemetry=telemetry,
        optimizer_identity=optimizer_identity,
    )
    assert seen["required_exports"] == {COMPILED_MAIN, COMPILED_HOST_INIT}
    assert seen["apply_level"] is True
    assert seen["optimizer_identity"] is optimizer_identity
    assert "wasm_opt_path" not in attestation
    assert "wasm_opt_wall_ms" not in attestation
    assert (
        attestation["optimizer_input_sha256"]
        == hashlib.sha256(linked.read_bytes()).hexdigest()
    )
    assert telemetry["wasm_opt_path"] == str(optimizer_identity.path)
    assert telemetry["wasm_opt_wall_ms"] == 0.0
