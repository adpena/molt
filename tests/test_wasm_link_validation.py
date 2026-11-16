import hashlib
import json
import os
import subprocess
from pathlib import Path

import pytest
from molt.cli.source_extension_link_requirements import (
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
COMPILED_INDIRECT3 = compiled_execution_symbol("molt_call_indirect3")
UNKNOWN_COMPILED_INDIRECT99 = compiled_execution_symbol("molt_call_indirect99")
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
            '{"schema_version":8,"schema_version":9,"ok":true,'
            '"facts":{"schema_version":9}}',
            "duplicate JSON key",
        ),
        (
            '{"schema_version":9,"ok":true,"facts":{"schema_version":9}}',
            "unsupported response schema",
        ),
        (
            '{"schema_version":8,"ok":true,"facts":{"schema_version":8},'
            '"extension":true}',
            "invalid success response",
        ),
    ],
)
def test_wasm_facts_decoder_rejects_schema_ambiguity(
    stdout: str,
    message: str,
) -> None:
    process = subprocess.CompletedProcess(["scanner"], 0, stdout, "")

    with pytest.raises(ValueError, match=message):
        wasm_link_fact_provider._decode_wasm_facts_response(
            process,
            operation="test scan",
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
        env={"MOLT_TARGET_ROOT": "relative-tools/toolchains"}, cwd=cwd
    )
    assert actual.path.parent == prefix / "sdk/bin"
    assert actual.wasi_sdk_llvm_version == installation.asset.llvm_version
    assert not (ambient / "relative-tools").exists()
