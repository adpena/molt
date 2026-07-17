import hashlib
import importlib.machinery
import importlib.util
import json
import tempfile
from pathlib import Path

import pytest
from molt import wasm_artifact
from molt.wasm_artifact import parse_wasm_exports, parse_wasm_imports


def _load_wasm_link():
    root = Path(__file__).resolve().parents[1]
    path = root / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()


def _rust_facts_fixture(data: bytes) -> dict[str, object]:
    sections = wasm_link._parse_sections(data)
    import_count = wasm_link._count_func_imports(sections)
    defined_count = 0
    for section_id, payload in sections:
        if section_id == 3:
            defined_count, _ = wasm_link._read_varuint(payload, 0)
            break
    module_facts = wasm_link.parse_wasm_module_facts(data)
    total_functions = import_count + defined_count
    exported_tables = sorted(
        index
        for kind, index in module_facts.export_kinds.values()
        if kind == 1
    )
    table_min = wasm_link._table_import_min(data)
    app_base = table_min or 0
    return {
        "schema_version": 3,
        "function_import_count": import_count,
        "defined_function_count": defined_count,
        "function_references": [],
        "root_function_indices": list(range(total_functions)),
        "element_function_indices": [],
        "declared_function_indices": [],
        "active_function_elements": [],
        "callable_table_entries": [],
        "callable_table_attestation_present": True,
        "callable_table_layout": {
            "fixed_prefix_base": 0,
            "fixed_prefix_len": 0,
            "finalized_app_base": app_base,
            "app_entry_count": 0,
        },
        "table_mutations": [],
        "reachable_table_mutations": [],
        "dynamic_table_dispatch": False,
        "dynamic_dispatch_functions": [],
        "reachable_dynamic_dispatch": False,
        "indirect_call_tables": [],
        "reachable_indirect_call_tables": [],
        "indirect_calls": [],
        "exported_table_indices": exported_tables,
        "tables": [],
    }


@pytest.fixture(autouse=True)
def _rust_facts_authority_fixture(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        wasm_link,
        "_make_rust_wasm_facts_provider",
        lambda _scanner, _scratch_root: _rust_facts_fixture,
    )

    def publish(_scanner, artifact: Path, *, layout=None, role="monolithic"):
        del role
        facts = _rust_facts_fixture(artifact.read_bytes())
        if layout is not None:
            facts["callable_table_layout"] = {
                "fixed_prefix_base": layout.fixed_prefix_base,
                "fixed_prefix_len": layout.fixed_prefix_len,
                "finalized_app_base": layout.finalized_app_base,
                "app_entry_count": layout.app_entry_count,
            }
        return facts

    monkeypatch.setattr(wasm_link, "_publish_rust_wasm_link_facts", publish)


_REAL_RUN_WASM_LD = wasm_link._run_wasm_ld


def _run_wasm_ld_with_rust_facts(*args, **kwargs):  # type: ignore[no-untyped-def]
    kwargs.setdefault("wasm_facts_scanner", Path("rust-facts-fixture"))
    return _REAL_RUN_WASM_LD(*args, **kwargs)


def _facts_provider(data: bytes) -> dict[str, object]:
    return _rust_facts_fixture(data)


def test_snapshot_link_input_retries_until_source_is_stable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "output.wasm"
    source.write_bytes(b"partial")
    reads = 0
    original_read_bytes = Path.read_bytes

    def racing_read_bytes(path: Path) -> bytes:
        nonlocal reads
        data = original_read_bytes(path)
        if path == source:
            reads += 1
            if reads == 1:
                source.write_bytes(b"complete-molt-main")
        return data

    monkeypatch.setattr(Path, "read_bytes", racing_read_bytes)

    snapshot = wasm_link._snapshot_link_input(
        source,
        tmp_path / "snapshots",
        label="app",
        retry_delay_seconds=0,
        accept=lambda data: b"molt-main" in data,
    )

    assert original_read_bytes(snapshot) == b"complete-molt-main"
    assert snapshot != source


def test_snapshot_link_input_retries_failed_path_attestation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "runtime_reloc.wasm"
    source.write_bytes(b"bad-generation")
    attempts = 0

    def accept_path(snapshot: Path) -> bool:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            source.write_bytes(b"good-generation")
            return False
        return snapshot.read_bytes() == b"good-generation"

    snapshot = wasm_link._snapshot_link_input(
        source,
        tmp_path / "snapshots",
        label="runtime",
        retry_delay_seconds=0,
        accept_path=accept_path,
    )

    assert snapshot.read_bytes() == b"good-generation"
    assert attempts == 2


def test_snapshot_link_input_rejects_stable_stripped_restoration_source(
    tmp_path: Path,
) -> None:
    source = tmp_path / "output.wasm"
    source.write_bytes(b"stable-but-stripped")

    with pytest.raises(OSError, match="stable bytes failed linker input contract"):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="app",
            attempts=2,
            retry_delay_seconds=0,
            accept=lambda data: b"molt-main" in data,
        )


def test_split_contract_rejects_missing_app_owned_molt_main() -> None:
    module = b"\0asm\x01\0\0\0"

    with pytest.raises(
        ValueError,
        match="missing app-owned function export molt_main after symbol restoration at unspecified",
    ):
        wasm_link._restore_split_runtime_contract_exports(module, artifact="app")


def test_output_export_symbol_map_prefers_unique_restoration_alias() -> None:
    module = _build_exported_runtime_module("molt_main")
    module = wasm_link._append_linking_function_symbols(
        module,
        [
            (
                "molt_main",
                0,
                wasm_link.FLAG_BINDING_GLOBAL
                | wasm_link.FLAG_EXPLICIT_NAME
                | wasm_link.FLAG_EXPORTED
                | wasm_link.FLAG_NO_STRIP,
            ),
            (
                "__molt_output_export_0",
                0,
                wasm_link.FLAG_BINDING_GLOBAL
                | wasm_link.FLAG_EXPLICIT_NAME
                | wasm_link.FLAG_EXPORTED
                | wasm_link.FLAG_NO_STRIP,
            ),
        ],
    )
    assert module is not None

    assert wasm_link._collect_output_export_symbol_map(module)["molt_main"] == (
        "__molt_output_export_0"
    )


def test_deduplicated_export_flags_preserve_first_contract_order() -> None:
    assert wasm_link._deduplicated_export_flags(
        ("--export-if-defined=molt_Py_None", "--export=molt_main"),
        ("--export-if-defined=molt_Py_None", "--export=PyInit_numpy"),
        ("--export=molt_main",),
    ) == [
        "--export-if-defined=molt_Py_None",
        "--export=molt_main",
        "--export=PyInit_numpy",
    ]


def test_relocatable_runtime_preflight_classifies_linker_crash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    runtime.write_bytes(b"\0asm\x01\0\0\0")

    def fake_run(cmd, **_kwargs):  # type: ignore[no-untyped-def]
        return wasm_link.subprocess.CompletedProcess(
            cmd,
            3221225477,
            stdout="",
            stderr="PLEASE submit a bug report to llvm-project",
        )

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)

    error = wasm_link._preflight_relocatable_runtime(
        "wasm-ld", runtime, type("TempDir", (), {"name": str(tmp_path)})()
    )

    assert error is not None
    assert "linking/reloc custom-section indices" in error
    assert "returncode=3221225477" in error


def test_find_wasm_ld_uses_attested_toolchain_authority(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    identity = wasm_link.wasm_toolchain.WasmLinkerIdentity(
        Path("C:/wasi-sdk/bin/wasm-ld.exe"), "22.1.7", "22.1.0"
    )
    monkeypatch.setattr(
        wasm_link.wasm_toolchain, "resolve_wasm_linker", lambda: identity
    )

    assert wasm_link._find_wasm_ld() == str(identity.path)
    assert "version=22.1.7 wasi-sdk-llvm=22.1.0" in capsys.readouterr().err


def _write_wasm_ld_output(cmd: list[str], data: bytes) -> Path | None:
    if "-o" not in cmd:
        return None
    output_path = Path(cmd[cmd.index("-o") + 1])
    output_path.write_bytes(data)
    return output_path


def test_wasm_link_external_tool_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return wasm_link.subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        wasm_link.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = wasm_link._run_external_tool(["wasm-tools", "validate", "x.wasm"])

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["wasm-tools", "validate", "x.wasm"]
    assert captured["kwargs"]["prefix"] == "MOLT_WASM_LINK"
    assert captured["kwargs"]["capture_output"] is True


def test_wasm_link_external_tool_preserves_timeout_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return wasm_link.subprocess.CompletedProcess(
            cmd,
            wasm_link.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            stdout="partial",
            stderr="memory_guard: timeout after 1.00s\n",
        )

    monkeypatch.setattr(
        wasm_link.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    with pytest.raises(wasm_link.subprocess.TimeoutExpired) as exc_info:
        wasm_link._run_external_tool(["wasm-opt", "x.wasm"], timeout=1)

    assert exc_info.value.cmd == ["wasm-opt", "x.wasm"]
    assert exc_info.value.output == "partial"
    assert exc_info.value.stderr == "memory_guard: timeout after 1.00s\n"


def test_wasm_link_default_artifact_paths_use_canonical_dist(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)

    assert wasm_link._default_input_path() == Path("dist") / "output.wasm"
    assert wasm_link._default_output_path() == Path("dist") / "output_linked.wasm"


def test_wasm_link_default_artifact_paths_follow_external_root(
    tmp_path: Path,
    monkeypatch,
) -> None:
    ext_root = tmp_path / "ext-root"
    ext_root.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)

    assert wasm_link._default_input_path() == ext_root / "dist" / "output.wasm"
    assert wasm_link._default_output_path() == Path(
        ext_root / "dist" / "output_linked.wasm"
    )


# Integrity pins are keyed by the runtime fingerprint meta digest (one slot
# per resolved profile/feature identity); any 64-hex value is a valid key.
_PIN_KEY_A = "ab" * 32
_PIN_KEY_B = "cd" * 32


def _write_keyed_pin(runtime: Path, digest: str, key: str = _PIN_KEY_A) -> Path:
    pin = runtime.with_name(f"{runtime.name}.{key}.sha256")
    pin.write_text(digest + "\n", encoding="utf-8")
    return pin


def test_verify_runtime_integrity_accepts_matching_sidecar_hash(tmp_path: Path) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    _write_keyed_pin(runtime, hashlib.sha256(runtime.read_bytes()).hexdigest())

    wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_accepts_pin_of_another_build_identity(
    tmp_path: Path,
) -> None:
    """Different-profile pins coexist; the artifact must match its own pin."""
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    _write_keyed_pin(runtime, "0" * 64, key=_PIN_KEY_A)
    _write_keyed_pin(
        runtime,
        hashlib.sha256(runtime.read_bytes()).hexdigest(),
        key=_PIN_KEY_B,
    )

    wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_retries_stale_sidecar_publish_window(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    digest = hashlib.sha256(runtime.read_bytes()).hexdigest()
    pin = runtime.with_name(f"{runtime.name}.{_PIN_KEY_A}.sha256")
    reads: list[Path] = []

    def read_pins(path: Path) -> dict[Path, str]:
        reads.append(path)
        return {pin: "0" * 64} if len(reads) == 1 else {pin: digest}

    monkeypatch.setattr(
        wasm_link,
        "_RUNTIME_INTEGRITY_PAIR_ATTEMPTS",
        2,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link,
        "_read_runtime_integrity_pins",
        read_pins,
        raising=True,
    )
    monkeypatch.setattr(wasm_link.time, "sleep", lambda _delay: None, raising=True)

    wasm_link._verify_runtime_integrity(runtime)

    assert reads == [runtime, runtime]


def test_verify_runtime_integrity_rejects_mismatched_sidecar_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    _write_keyed_pin(runtime, "0" * 64)
    monkeypatch.setattr(wasm_link.time, "sleep", lambda _delay: None, raising=True)

    with pytest.raises(SystemExit, match="sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_env_cannot_bypass_mismatched_sidecar_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    _write_keyed_pin(runtime, "0" * 64)
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_VERIFY", "1")
    monkeypatch.setattr(wasm_link.time, "sleep", lambda _delay: None, raising=True)

    with pytest.raises(SystemExit, match="sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_rejects_missing_sidecar(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "custom_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    monkeypatch.setattr(wasm_link.time, "sleep", lambda _delay: None, raising=True)

    with pytest.raises(SystemExit, match="publish the matching keyed .sha256 sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_accepts_writer_published_pin(tmp_path: Path) -> None:
    """Round trip: the CLI writer's keyed pin satisfies the linker check and
    deterministically supersedes every stale contract for the same filename."""
    from molt.cli.runtime_wasm_validation import _write_runtime_wasm_integrity_sidecar

    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    legacy = runtime.with_name(f"{runtime.name}.sha256")
    legacy.write_text("0" * 64 + "\n", encoding="utf-8")
    stale_keyed = runtime.with_name(f"{runtime.name}.{'b' * 64}.sha256")
    stale_keyed.write_text("0" * 64 + "\n", encoding="utf-8")

    _write_runtime_wasm_integrity_sidecar(runtime, integrity_key=_PIN_KEY_A)

    assert not legacy.exists()
    assert not stale_keyed.exists()
    assert list(tmp_path.glob(f"{runtime.name}.*.sha256")) == [
        runtime.with_name(f"{runtime.name}.{_PIN_KEY_A}.sha256")
    ]
    wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_rejects_retired_single_slot_sidecar(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An unkeyed `<artifact>.sha256` pin is dead; even a matching hash fails."""
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    legacy = runtime.with_name(f"{runtime.name}.sha256")
    legacy.write_text(
        hashlib.sha256(runtime.read_bytes()).hexdigest() + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(wasm_link.time, "sleep", lambda _delay: None, raising=True)

    with pytest.raises(SystemExit, match="no keyed integrity sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


def test_runtime_integrity_has_no_hardcoded_hash_authority() -> None:
    root = Path(__file__).resolve().parents[1]
    assert not hasattr(wasm_link, "RUNTIME_EXPECTED_HASHES")
    assert not (root / "tools" / "update_runtime_hash.py").exists()


def _build_minimal_module(element_payload: bytes) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections = []

    # Type section: one empty function type.
    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    # Function section: one function of type 0.
    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, func_payload))

    # Table section: one funcref table with min 1.
    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    # Code section: one empty function body.
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    # Element section. The payload is a wasm vector, so it must at least
    # encode its segment count; an empty payload would be an invalid module
    # that the strict module-facts parser rejects at link time.
    sections.append((9, element_payload or write_varuint(0)))

    return wasm_link._build_sections(sections)


def _build_start_root_module() -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(2))
    func_payload.extend(write_varuint(0))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    sections.append((8, write_varuint(0)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    code_payload.extend(write_varuint(4))
    code_payload.append(0x00)  # local decl count
    code_payload.append(0x10)  # call
    code_payload.extend(write_varuint(1))
    code_payload.append(0x0B)  # end
    code_payload.extend(write_varuint(3))
    code_payload.append(0x00)  # local decl count
    code_payload.append(0x01)  # nop
    code_payload.append(0x0B)  # end
    sections.append((10, bytes(code_payload)))

    return wasm_link._build_sections(sections)


def _build_exported_runtime_module(export_name: str) -> bytes:
    return _build_exported_runtime_module_many([export_name])


def _build_exported_runtime_module_many(export_names: list[str]) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(len(export_names)))
    for _ in export_names:
        func_payload.extend(write_varuint(0))
    sections.append((3, func_payload))

    export_payload = bytearray()
    export_payload.extend(write_varuint(len(export_names)))
    for index, export_name in enumerate(export_names):
        export_payload.extend(wasm_link._write_string(export_name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(len(export_names)))
    for _ in export_names:
        code_payload.extend(write_varuint(4))
        code_payload.append(0x00)
        code_payload.append(0x42)
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    return wasm_link._build_sections(sections)


def _build_host_call_indirect_module(
    import_name: str = "molt_call_indirect3",
) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(2))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(3))
    type_payload.extend(b"\x7e\x7e\x7e")
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string(import_name))
    import_payload.append(0x00)
    import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(1))
    func_payload.extend(write_varuint(1))
    sections.append((3, bytes(func_payload)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(1))
    sections.append((9, bytes(element_payload)))

    return wasm_link._build_sections(sections)


def _build_tag_then_host_call_indirect_import_module() -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(2))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(3))
    type_payload.extend(b"\x7e\x7e\x7e")
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(2))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("__cpp_exception"))
    import_payload.append(0x04)  # tag import
    import_payload.append(0x00)  # exception attribute
    import_payload.extend(write_varuint(0))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("molt_call_indirect3"))
    import_payload.append(0x00)  # function import
    import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(1))
    func_payload.extend(write_varuint(1))
    sections.append((3, bytes(func_payload)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(1))
    sections.append((9, bytes(element_payload)))

    return wasm_link._build_sections(sections)


def _function_import_pairs(wasm_bytes: bytes) -> list[tuple[str, str]]:
    return [
        (wasm_import.module, wasm_import.name)
        for wasm_import in parse_wasm_imports(wasm_bytes, on_error="ignore")
        if wasm_import.kind == 0
    ]


def _function_export_pairs(wasm_bytes: bytes) -> list[tuple[str, int]]:
    return [
        (wasm_export.name, wasm_export.index)
        for wasm_export in parse_wasm_exports(wasm_bytes, kind=0, on_error="ignore")
    ]


def _parse_code_section_call_targets(wasm_bytes: bytes) -> list[list[int]]:
    targets: list[list[int]] = []
    offset = 8
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = wasm_link._read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 10:
            func_count, offset = wasm_link._read_varuint(wasm_bytes, offset)
            for _ in range(func_count):
                body_size, offset = wasm_link._read_varuint(wasm_bytes, offset)
                body_end = offset + body_size
                local_count, pos = wasm_link._read_varuint(wasm_bytes, offset)
                for _ in range(local_count):
                    _, pos = wasm_link._read_varuint(wasm_bytes, pos)
                    pos += 1
                func_targets: list[int] = []
                while pos < body_end:
                    opcode = wasm_bytes[pos]
                    pos += 1
                    if opcode in (0x10, 0x12):
                        idx, pos = wasm_link._read_varuint(wasm_bytes, pos)
                        func_targets.append(idx)
                    elif opcode == 0x0B:
                        break
                    else:
                        raise AssertionError(
                            f"unexpected opcode 0x{opcode:02x} in test helper"
                        )
                targets.append(func_targets)
                offset = body_end
            return targets
        offset = section_end
    return targets


def _build_runtime_import_strip_module() -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(2))
    for name in ("unused_runtime_fn", "live_runtime_fn"):
        import_payload.extend(wasm_link._write_string("molt_runtime"))
        import_payload.extend(wasm_link._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(2))
    sections.append((7, bytes(export_payload)))

    body = bytearray()
    body.append(0x00)
    body.append(0x10)
    body.extend(write_varuint(1))
    body.append(0x0B)
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link._build_sections(sections)


def _build_runtime_import_module(
    import_names: list[str], *, memory_min: int | None = None
) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(
        write_varuint(len(import_names) + (1 if memory_min is not None else 0))
    )
    for name in import_names:
        import_payload.extend(wasm_link._write_string("molt_runtime"))
        import_payload.extend(wasm_link._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    if memory_min is not None:
        import_payload.extend(wasm_link._write_string("env"))
        import_payload.extend(wasm_link._write_string("memory"))
        import_payload.append(0x02)
        import_payload.append(0x00)
        import_payload.extend(write_varuint(memory_min))
    sections.append((2, bytes(import_payload)))

    return wasm_link._build_sections(sections)


def _build_split_runtime_app_module(
    import_names: list[str], *, memory_min: int = 1
) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(len(import_names) + 2))
    for name in import_names:
        import_payload.extend(wasm_link._write_string("molt_runtime"))
        import_payload.extend(wasm_link._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("__indirect_function_table"))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("memory"))
    import_payload.append(0x02)
    import_payload.append(0x00)
    import_payload.extend(write_varuint(memory_min))
    sections.append((2, bytes(import_payload)))
    sections.append((3, write_varuint(1) + write_varuint(0)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    for name, kind, index in (
        ("molt_main", 0x00, len(import_names)),
        ("molt_table", 0x01, 0),
        ("molt_memory", 0x02, 0),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(kind)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))
    code_payload = write_varuint(1) + write_varuint(2) + b"\x00\x0b"
    sections.append((10, code_payload))
    return wasm_link._build_sections(sections)


def _strip_export(data: bytes, export_name: str) -> bytes:
    rebuilt_sections: list[tuple[int, bytes]] = []
    for section_id, payload in wasm_link._parse_sections(data):
        if section_id != 7:
            rebuilt_sections.append((section_id, payload))
            continue
        count, offset = wasm_link._read_varuint(payload, 0)
        exports: list[tuple[str, int, int]] = []
        for _ in range(count):
            name, offset = wasm_link._read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = wasm_link._read_varuint(payload, offset)
            if name != export_name:
                exports.append((name, kind, index))
        rebuilt = bytearray(wasm_link._write_varuint(len(exports)))
        for name, kind, index in exports:
            rebuilt.extend(wasm_link._write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(wasm_link._write_varuint(index))
        rebuilt_sections.append((7, bytes(rebuilt)))
    return wasm_link._build_sections(rebuilt_sections)


def _build_memory_import_ref_func_app_module(func_index: int = 0) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("memory"))
    import_payload.append(0x02)
    import_payload.append(0x00)
    import_payload.extend(write_varuint(1))
    sections.append((2, bytes(import_payload)))

    sections.append((3, write_varuint(1) + write_varuint(0)))

    body = bytearray()
    body.extend(write_varuint(0))
    body.append(0xD2)
    body.extend(write_varuint(func_index))
    body.append(0x1A)
    body.append(0x0B)
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link._build_sections(sections)


def _build_linked_ref_func_module(func_index: int = 0) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    sections.append((3, write_varuint(1) + write_varuint(0)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.append(0x00)
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(2))
    export_payload.extend(wasm_link._write_string("molt_memory"))
    export_payload.append(0x02)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("molt_table"))
    export_payload.append(0x01)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    body = bytearray()
    body.extend(write_varuint(0))
    body.append(0xD2)
    body.extend(write_varuint(func_index))
    body.append(0x1A)
    body.append(0x0B)
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link._build_sections(sections)


def _build_native_direct_import_module(import_name: str) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7F)
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("molt_native"))
    import_payload.extend(wasm_link._write_string(import_name))
    import_payload.append(0x00)
    import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    return wasm_link._build_sections(sections)


def _build_exported_function_module(
    export_name: str, *, trap_body: bool = False
) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7F)
    sections.append((1, bytes(type_payload)))

    sections.append((3, write_varuint(1) + write_varuint(0)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string(export_name))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    body = bytes([0x00, 0x00, 0x0B]) if trap_body else bytes([0x00, 0x41, 0x01, 0x0B])
    code_payload = write_varuint(1) + write_varuint(len(body)) + body
    sections.append((10, code_payload))

    return wasm_link._build_sections(sections)


def _build_runtime_import_data_module(
    import_names: list[str],
    *,
    memory_min: int,
    data_offset: int,
    table_min: int | None = None,
) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(
        write_varuint(len(import_names) + 1 + (1 if table_min is not None else 0))
    )
    for name in import_names:
        import_payload.extend(wasm_link._write_string("molt_runtime"))
        import_payload.extend(wasm_link._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("memory"))
    import_payload.append(0x02)
    import_payload.append(0x00)
    import_payload.extend(write_varuint(memory_min))
    if table_min is not None:
        import_payload.extend(wasm_link._write_string("env"))
        import_payload.extend(wasm_link._write_string("__indirect_function_table"))
        import_payload.append(0x01)
        import_payload.append(0x70)
        import_payload.extend(write_varuint(0))
        import_payload.extend(write_varuint(table_min))
    sections.append((2, bytes(import_payload)))

    data_payload = bytearray()
    data_payload.extend(write_varuint(1))
    data_payload.extend(write_varuint(0))
    data_payload.append(0x41)
    data_payload.extend(write_varuint(data_offset))
    data_payload.append(0x0B)
    data_payload.extend(write_varuint(1))
    data_payload.extend(b"x")
    sections.append((11, bytes(data_payload)))

    return wasm_link._build_sections(sections)


def _build_defined_memory_module(min_pages: int) -> bytes:
    write_varuint = wasm_link._write_varuint
    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(min_pages))
    return wasm_link._build_sections([(5, bytes(memory_payload))])


def _defined_memory_min(wasm_bytes: bytes) -> int | None:
    for section_id, payload in wasm_link._parse_sections(wasm_bytes):
        if section_id != 5:
            continue
        offset = 0
        count, offset = wasm_link._read_varuint(payload, offset)
        if count == 0:
            return None
        _flags, offset = wasm_link._read_varuint(payload, offset)
        minimum, _offset = wasm_link._read_varuint(payload, offset)
        return minimum
    return None


def _build_env_function_import_module(import_names: list[str]) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(len(import_names)))
    for name in import_names:
        import_payload.extend(wasm_link._write_string("env"))
        import_payload.extend(wasm_link._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    return wasm_link._build_sections(sections)


def _build_symbol_subsection(entries: list[bytes]) -> bytes:
    return wasm_link._write_varuint(len(entries)) + b"".join(entries)


def _function_symbol_entry(*, flags: int, index: int | None, name: str | None) -> bytes:
    entry = bytearray()
    entry.append(wasm_link.SYMBOL_KIND_FUNCTION)
    entry.extend(wasm_link._write_varuint(flags))
    assert index is not None
    entry.extend(wasm_link._write_varuint(index))
    if not (flags & wasm_link.FLAG_UNDEFINED) or (flags & wasm_link.FLAG_EXPLICIT_NAME):
        assert name is not None
        entry.extend(wasm_link._write_string(name))
    return bytes(entry)


def _data_symbol_entry(
    *,
    flags: int,
    name: str | None,
    segment_index: int = 0,
    offset: int = 0,
    size: int = 0,
) -> bytes:
    entry = bytearray()
    entry.append(1)
    entry.extend(wasm_link._write_varuint(flags))
    if flags & (wasm_link.FLAG_EXPLICIT_NAME | wasm_link.FLAG_UNDEFINED):
        assert name is not None
        entry.extend(wasm_link._write_string(name))
    if not (flags & wasm_link.FLAG_UNDEFINED):
        entry.extend(wasm_link._write_varuint(segment_index))
        entry.extend(wasm_link._write_varuint(offset))
        entry.extend(wasm_link._write_varuint(size))
    return bytes(entry)


def _module_with_linking_symbols(entries: list[bytes]) -> bytes:
    linking_payload = wasm_link._build_linking_payload(
        2,
        [(wasm_link.SYMTAB_SUBSECTION_ID, _build_symbol_subsection(entries))],
    )
    custom = wasm_link._build_custom_section("linking", linking_payload)
    return wasm_link._build_sections([(0, custom)])


def _linking_data_symbol_names(data: bytes) -> list[tuple[int, str]]:
    """Return ``(flags, name)`` for every ``data`` symbol in the linking symtab."""
    names: list[tuple[int, str]] = []
    for section_id, payload in wasm_link._parse_sections(data):
        if section_id != 0:
            continue
        name, custom_payload = wasm_link._parse_custom_section(payload)
        if name != "linking":
            continue
        _version, subsections = wasm_link._parse_linking_payload(custom_payload)
        for sub_id, sub_payload in subsections:
            if sub_id != wasm_link.SYMTAB_SUBSECTION_ID:
                continue
            count, offset = wasm_link._read_varuint(sub_payload, 0)
            for _ in range(count):
                kind = sub_payload[offset]
                offset += 1
                flags, offset = wasm_link._read_varuint(sub_payload, offset)
                if kind == 1:
                    symbol_name, offset = wasm_link._read_string(sub_payload, offset)
                    names.append((flags, symbol_name))
                    if not (flags & wasm_link.FLAG_UNDEFINED):
                        _seg, offset = wasm_link._read_varuint(sub_payload, offset)
                        _off, offset = wasm_link._read_varuint(sub_payload, offset)
                        _sz, offset = wasm_link._read_varuint(sub_payload, offset)
                elif kind in (0, 2, 4, 5):
                    _idx, _nm, offset = wasm_link._parse_indexed_symbol(
                        sub_payload, offset, flags
                    )
                elif kind == 3:
                    _sec, offset = wasm_link._read_varuint(sub_payload, offset)
    return names


def _module_with_flattenable_rec_group_type() -> bytes:
    func_type = b"\x60\x00\x00"
    type_payload = bytearray()
    type_payload.extend(wasm_link._write_varuint(1))
    type_payload.append(0x4E)
    type_payload.extend(wasm_link._write_varuint(1))
    type_payload.extend(func_type)
    return wasm_link._build_sections([(1, bytes(type_payload))])


def test_strip_debug_sections_removes_all_dwarf_custom_sections() -> None:
    debug_info = wasm_link._build_custom_section(".debug_info", b"old")
    debug_line_str = wasm_link._build_custom_section(".debug_line_str", b"new")
    keep = wasm_link._build_custom_section("molt.keep", b"payload")
    module = wasm_link._build_sections(
        [
            (0, debug_info),
            (0, debug_line_str),
            (0, keep),
        ]
    )

    stripped = wasm_link._strip_debug_sections(module)

    assert stripped is not None
    custom_names = [
        wasm_link._parse_custom_section(payload)[0]
        for section_id, payload in wasm_link._parse_sections(stripped)
        if section_id == 0
    ]
    assert custom_names == ["molt.keep"]


def test_publication_strip_removes_link_metadata_after_export_rewrite() -> None:
    module = wasm_link._build_sections(
        [
            (7, b"exports-already-canonical"),
            (0, wasm_link._build_custom_section("linking", b"symbols")),
            (0, wasm_link._build_custom_section("reloc.CODE", b"relocs")),
            (0, wasm_link._build_custom_section("name", b"debug names")),
        ]
    )

    stripped = wasm_link.strip_wasm_publication_sections(
        module,
        final_artifact=True,
        preserve_debug=False,
    )

    sections = wasm_link._parse_sections(stripped)
    assert sections == [(7, b"exports-already-canonical")]


def test_canonicalize_standard_section_order_moves_element_before_code_data() -> None:
    sections = [
        (1, b"type"),
        (7, b"export"),
        (10, b"code"),
        (11, b"data"),
        (9, b"elem"),
    ]
    module = wasm_link._build_sections(sections)

    canonical = wasm_link._canonicalize_standard_section_order(module)

    assert canonical is not None
    assert [section_id for section_id, _ in wasm_link._parse_sections(canonical)] == [
        1,
        7,
        9,
        10,
        11,
    ]


def test_canonicalize_standard_section_order_places_tag_after_memory() -> None:
    module = wasm_link._build_sections(
        [
            (6, b"\x00"),
            (13, b"\x00"),
            (5, b"\x00"),
            (7, b"\x00"),
        ]
    )

    canonical = wasm_link._canonicalize_standard_section_order(module)

    assert canonical is not None
    assert [section_id for section_id, _ in wasm_link._parse_sections(canonical)] == [
        5,
        13,
        6,
        7,
    ]


def test_canonicalize_standard_section_order_merges_duplicate_export_sections() -> None:
    first_export = (
        wasm_link._write_varuint(1)
        + wasm_link._write_string("molt_main")
        + bytes([0])
        + wasm_link._write_varuint(0)
    )
    second_export = (
        wasm_link._write_varuint(2)
        + wasm_link._write_string("molt_main")
        + bytes([0])
        + wasm_link._write_varuint(2)
        + wasm_link._write_string("PyInit__multiarray_umath")
        + bytes([0])
        + wasm_link._write_varuint(1)
    )
    module = wasm_link._build_sections(
        [(1, bytes([1, 0x60, 0, 0])), (7, first_export), (7, second_export)]
    )

    canonical = wasm_link._canonicalize_standard_section_order(module)

    assert canonical is not None
    sections = wasm_link._parse_sections(canonical)
    assert [section_id for section_id, _payload in sections].count(7) == 1
    assert wasm_link._standard_section_order_error(canonical) is None
    assert wasm_link.parse_wasm_module_facts(canonical).export_kinds == {
        "molt_main": (0, 0),
        "PyInit__multiarray_umath": (0, 1),
    }


def test_canonicalize_standard_section_order_rejects_duplicate_start_sections() -> None:
    module = wasm_link._build_sections([(8, bytes([0])), (8, bytes([1]))])

    with pytest.raises(ValueError, match="duplicate singleton standard section id 8"):
        wasm_link._canonicalize_standard_section_order(module)


def _build_linked_host_table_module(table_import_name: str) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string(table_import_name))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(1))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    for name, kind, index in (
        ("molt_main", 0x00, 0),
        ("molt_table", 0x01, 0),
        ("molt_memory", 0x02, 0),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(kind)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    return wasm_link._build_sections(sections)


def _parse_data_segments(data: bytes) -> list[bytes]:
    sections = wasm_link._parse_sections(data)
    for section_id, payload in sections:
        if section_id != 11:
            continue
        offset = 0
        seg_count, offset = wasm_link._read_varuint(payload, offset)
        out: list[bytes] = []
        parse_offset = offset
        for _ in range(seg_count):
            flags = payload[parse_offset]
            parse_offset += 1
            if flags == 0:
                parse_offset = wasm_link._skip_init_expr(payload, parse_offset)
            elif flags == 1:
                pass
            elif flags == 2:
                _, parse_offset = wasm_link._read_varuint(payload, parse_offset)
                parse_offset = wasm_link._skip_init_expr(payload, parse_offset)
            else:
                raise AssertionError(f"unexpected data segment flags: {flags}")
            data_len, parse_offset = wasm_link._read_varuint(payload, parse_offset)
            out.append(payload[parse_offset : parse_offset + data_len])
            parse_offset += data_len
        return out
    return []


def test_wasm_link_allows_ref_func_element_expr() -> None:
    write_varuint = wasm_link._write_varuint
    payload = bytearray()
    payload.extend(write_varuint(1))  # count
    payload.extend(write_varuint(0x04))  # active, elemtype + exprs
    payload.extend(b"\x41\x00\x0b")  # i32.const 0; end
    payload.append(0x70)  # funcref
    payload.extend(write_varuint(1))
    payload.append(0xD2)  # ref.func
    payload.extend(write_varuint(0))
    payload.append(0x0B)  # end
    data = _build_minimal_module(bytes(payload))
    ok, err = wasm_link._validate_elements(data)
    assert ok, err


def test_wasm_module_facts_capture_link_validation_surface() -> None:
    data = _build_memory_import_ref_func_app_module(func_index=0)

    facts = wasm_link.parse_wasm_module_facts(data)

    assert facts.imports == (("env", "memory", 2, b"\x00\x01"),)
    assert facts.module_imports["env"] == frozenset({"memory"})
    assert facts.memory_import_mins[("env", "memory")] == 1


def test_validate_linked_parses_module_facts_once(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_linked_host_table_module("__indirect_function_table"))
    calls: list[int] = []
    original_parse = wasm_link.parse_wasm_module_facts

    def parse_once(data: bytes):
        calls.append(len(data))
        return original_parse(data)

    monkeypatch.setattr(wasm_link, "parse_wasm_module_facts", parse_once)
    monkeypatch.setattr(wasm_link, "_validate_wasm_structural", lambda *_a, **_k: True)

    assert wasm_link._validate_linked(linked)
    assert calls == [len(linked.read_bytes())]


def test_validate_split_runtime_outputs_parses_each_artifact_once(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))
    calls: list[int] = []
    original_parse = wasm_link.parse_wasm_module_facts

    def parse_once(data: bytes):
        calls.append(len(data))
        return original_parse(data)

    monkeypatch.setattr(wasm_link, "parse_wasm_module_facts", parse_once)
    monkeypatch.setattr(wasm_link, "_validate_wasm_structural", lambda *_a, **_k: True)

    assert wasm_link._validate_split_runtime_outputs(app, runtime)
    assert calls == [len(app.read_bytes()), len(runtime.read_bytes())]


def test_validate_linked_accepts_known_host_table_contract(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: None)

    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_linked_host_table_module("__indirect_function_table"))

    assert wasm_link._validate_linked(linked)
    captured = capsys.readouterr()
    assert "host-table contract" in captured.err


def test_validate_linked_rejects_unexpected_table_import_contract(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: None)

    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_linked_host_table_module("mystery_table"))

    assert not wasm_link._validate_linked(linked)
    captured = capsys.readouterr()
    assert "unsupported table" in captured.err


def test_validate_linked_rejects_only_manifest_call_indirect_imports(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: None)

    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_host_call_indirect_module("molt_call_indirect3"))

    assert not wasm_link._validate_linked(linked)
    captured = capsys.readouterr()
    assert "molt_call_indirect3" in captured.err

    linked.write_bytes(_build_host_call_indirect_module("molt_call_indirect99"))

    assert not wasm_link._validate_linked(linked)
    captured = capsys.readouterr()
    assert "molt_call_indirect99" not in captured.err
    assert "missing exported memory" in captured.err


def test_validate_wasm_structural_falls_back_when_debug_strip_fails(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _build_runtime_import_module([], memory_min=1)
    validated_inputs: list[bytes] = []

    def fake_run(cmd, **_kwargs):
        validated_inputs.append(Path(cmd[-1]).read_bytes())
        return wasm_link.subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: "wasm-tools")
    monkeypatch.setattr(
        wasm_link,
        "_strip_debug_sections",
        lambda _data: (_ for _ in ()).throw(ValueError("bad debug section")),
    )
    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)

    assert wasm_link._validate_wasm_structural(module, description="Probe wasm")
    assert validated_inputs == [module]


def test_stub_dead_functions_preserves_start_root_reachability() -> None:
    module = _build_start_root_module()
    assert wasm_link._stub_dead_functions(module, _rust_facts_fixture(module)) is None


def test_tree_shake_runtime_preserves_required_function_exports() -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    shaken = wasm_link._tree_shake_runtime(
        module, {"exception_pending"}, facts_provider=_facts_provider
    )
    exports = wasm_link._collect_function_exports(shaken)
    assert "molt_exception_pending" in exports


def test_tree_shake_runtime_preserves_direct_runner_exception_debug_exports() -> None:
    module = _build_exported_runtime_module_many(
        [
            "molt_exception_pending",
            "molt_alloc",
            "molt_handle_resolve",
            "molt_header_size",
            "molt_scratch_alloc",
            "molt_scratch_free",
            "molt_bytes_from_bytes",
            "molt_string_from_bytes",
            "molt_string_as_ptr",
            "molt_exception_kind",
            "molt_exception_message",
            "molt_exception_last",
            "molt_traceback_format_exc",
            "molt_type_tag_of_bits",
            "molt_dec_ref_obj",
        ]
    )
    shaken = wasm_link._tree_shake_runtime(
        module, {"exception_pending"}, facts_provider=_facts_provider
    )
    exports = wasm_link._collect_function_exports(shaken)
    assert "molt_alloc" in exports
    assert "molt_handle_resolve" in exports
    assert "molt_header_size" in exports
    assert "molt_scratch_alloc" in exports
    assert "molt_scratch_free" in exports
    assert "molt_bytes_from_bytes" in exports
    assert "molt_string_from_bytes" in exports
    assert "molt_string_as_ptr" in exports
    assert "molt_exception_kind" in exports
    assert "molt_exception_message" in exports
    assert "molt_exception_last" in exports
    assert "molt_traceback_format_exc" in exports
    assert "molt_type_tag_of_bits" in exports
    assert "molt_dec_ref_obj" in exports


def test_validate_split_runtime_outputs_rejects_stripped_contract_memory_export(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(
        _strip_export(
            _build_split_runtime_app_module(["molt_err_pending"]),
            "molt_memory",
        )
    )

    assert not wasm_link._validate_split_runtime_outputs(app, runtime)
    assert "missing contract export molt_memory (kind 2)" in capsys.readouterr().err


def test_restore_split_runtime_contract_exports_reemits_memory_and_table() -> None:
    app = _build_split_runtime_app_module([])
    stripped = _strip_export(_strip_export(app, "molt_memory"), "molt_table")

    restored = wasm_link._restore_split_runtime_contract_exports(
        stripped,
        artifact="app",
    )

    assert wasm_link.parse_wasm_module_facts(restored).export_kinds == {
        "molt_main": (0, 0),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }


def test_restore_split_runtime_contract_exports_parses_contract_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app = _build_split_runtime_app_module([])
    stripped = _strip_export(_strip_export(app, "molt_memory"), "molt_table")
    parse_calls = 0
    raw_parse = wasm_link._parse_wasm_module_facts_raw

    def counted_parse(data: bytes) -> wasm_link.WasmModuleFacts:
        nonlocal parse_calls
        parse_calls += 1
        return raw_parse(data)

    monkeypatch.setattr(wasm_link, "_parse_wasm_module_facts_raw", counted_parse)
    operation_counts: dict[str, int] = {}

    restored = wasm_link._restore_split_runtime_contract_exports(
        stripped,
        artifact="app",
        operation_counts=operation_counts,
    )

    assert parse_calls == 1
    assert operation_counts == {"wasm_whole_artifact_redundant_parses_eliminated": 2}
    assert raw_parse(restored).export_kinds == {
        "molt_main": (0, 0),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }


def test_split_runtime_contract_keep_set_includes_all_external_kinds() -> None:
    assert wasm_link._split_runtime_contract_export_names("app") == {
        "__indirect_function_table",
        "memory",
        "molt_main",
        "molt_memory",
        "molt_table",
    }

    assert wasm_link._split_artifact_contract_keep_set(
        "app",
        public_export_map={"user_export": "internal_user_export"},
        required_native_direct_symbols=("PyInit__demo",),
    ) == {
        "__indirect_function_table",
        "memory",
        "molt_main",
        "molt_memory",
        "molt_table",
        "PyInit__demo",
        "user_export",
    }


def test_split_app_post_link_preserves_and_restores_contract_exports() -> None:
    app = _build_split_runtime_app_module([])
    molt_main_index = wasm_link._collect_function_exports(app)["molt_main"]
    app = wasm_link._append_linking_function_symbols(
        app,
        [
            (
                "molt_main",
                molt_main_index,
                wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME,
            )
        ],
    )
    assert app is not None
    optimized = wasm_link._post_link_optimize(
        app,
        reference_data=_build_exported_runtime_module_many(["reference_only"]),
        preserve_exports=wasm_link._split_runtime_contract_export_names("app"),
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )

    assert wasm_link.parse_wasm_module_facts(optimized).export_kinds == {
        "molt_main": (0, molt_main_index),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }

    masked = _strip_export(
        _strip_export(_strip_export(app, "molt_main"), "molt_memory"),
        "molt_table",
    )
    restored = wasm_link._restore_split_runtime_contract_exports(
        masked,
        artifact="app",
        stage="test-post-link-mask",
    )

    assert wasm_link.parse_wasm_module_facts(restored).export_kinds == {
        "molt_main": (0, molt_main_index),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }


def test_split_app_optimization_cache_eliminates_repeat_wasm_opt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    app = _build_split_runtime_app_module([])
    optimize_calls = 0
    monkeypatch.setattr(
        wasm_link, "_split_app_optimize_cache_root", lambda: tmp_path / "cache"
    )
    monkeypatch.setattr(wasm_link, "find_wasm_opt", lambda: "wasm-opt")
    monkeypatch.setattr(wasm_link, "_wasm_opt_version", lambda _path: "test")
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_: data)
    monkeypatch.setattr(
        wasm_link,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )

    def fake_optimize(path: Path, **kwargs) -> bool:  # type: ignore[no-untyped-def]
        nonlocal optimize_calls
        optimize_calls += 1
        kwargs["attestation"]["pipeline"] = ["test-pass"]
        return True

    monkeypatch.setattr(wasm_link, "_run_wasm_opt_via_optimize", fake_optimize)
    cold_counts: dict[str, int] = {}
    warm_counts: dict[str, int] = {}
    cold_attestation: dict[str, object] = {}
    warm_attestation: dict[str, object] = {}

    cold = wasm_link._optimize_split_app_module(
        app,
        reference_data=None,
        optimize=True,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        attestation=cold_attestation,
        operation_counts=cold_counts,
        facts_provider=_facts_provider,
    )
    warm = wasm_link._optimize_split_app_module(
        app,
        reference_data=None,
        optimize=True,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        attestation=warm_attestation,
        operation_counts=warm_counts,
        facts_provider=_facts_provider,
    )

    assert warm == cold
    assert optimize_calls == 1
    assert cold_counts == {
        "split_app_optimize_requests": 1,
        "split_app_optimize_cache_misses": 1,
        "split_app_wasm_opt_runs": 1,
    }
    assert warm_counts == {
        "split_app_optimize_requests": 1,
        "split_app_optimize_cache_hits": 1,
    }
    assert warm_attestation["pipeline"] == ["test-pass"]
    assert warm_attestation["cache_hit"] is True


def test_python_callable_table_consumer_rejects_dynamic_active_offset() -> None:
    write_varuint = wasm_link._write_varuint
    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x23\x00\x0b")  # global.get 0; end
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    module = _build_minimal_module(bytes(element_payload))

    with pytest.raises(ValueError, match="Dynamic active wasm element offsets"):
        wasm_artifact._collect_wasm_active_table_function_slots(module)


def test_split_contract_restoration_keeps_function_exports_when_adding_memory_and_table() -> (
    None
):
    required_native = ("PyInit__demo",)
    app = _build_split_runtime_app_module([])
    main_index = wasm_link._collect_function_exports(app)["molt_main"]
    app = wasm_link._ensure_export_by_index(
        app, name=required_native[0], kind=0, index=main_index
    )
    assert app is not None
    app = _strip_export(_strip_export(app, "molt_memory"), "molt_table")

    restored = wasm_link._restore_split_runtime_contract_exports(
        app,
        artifact="app",
        stage="mask-proof",
        required_native_direct_symbols=required_native,
    )

    facts = wasm_link.parse_wasm_module_facts(restored)
    assert facts.export_kinds[required_native[0]][0] == 0
    assert facts.export_kinds["molt_main"][0] == 0
    assert facts.export_kinds["molt_memory"][0] == 2
    assert facts.export_kinds["molt_table"][0] == 1


def test_split_combined_post_link_preserves_linker_memory_and_table_aliases() -> None:
    linked = wasm_link._rename_export_names(
        _build_linked_ref_func_module(),
        {
            "molt_memory": "memory",
            "molt_table": "__indirect_function_table",
        },
    )
    assert linked is not None

    optimized = wasm_link._post_link_optimize(
        linked,
        preserve_exports=wasm_link._split_runtime_contract_export_names("app"),
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )

    assert wasm_link.parse_wasm_module_facts(optimized).export_kinds == {
        "memory": (2, 0),
        "__indirect_function_table": (1, 0),
    }


def test_split_combined_post_link_restores_real_defined_memory_export() -> None:
    linked = _strip_export(_build_linked_ref_func_module(), "molt_memory")
    optimized = wasm_link._post_link_optimize(
        linked,
        preserve_exports=wasm_link._split_runtime_contract_export_names("app"),
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )

    restored = wasm_link._ensure_defined_memory_export(optimized)

    assert restored is not None
    facts = wasm_link.parse_wasm_module_facts(restored)
    assert not [entry for entry in facts.imports if entry[2] == 2]
    assert facts.export_kinds["molt_memory"] == (2, 0)


def test_defined_memory_export_restoration_rejects_imported_memory() -> None:
    app = _strip_export(_build_split_runtime_app_module([]), "molt_memory")

    with pytest.raises(
        ValueError, match="cannot restore linked memory export from an imported memory"
    ):
        wasm_link._ensure_defined_memory_export(app)


def test_split_app_shared_memory_contract_passes_split_validation(
    tmp_path: Path,
) -> None:
    app = tmp_path / "app.wasm"
    runtime = tmp_path / "molt_runtime.wasm"
    app.write_bytes(_build_split_runtime_app_module([]))
    runtime.write_bytes(_build_exported_runtime_module_many([]))

    assert wasm_link._validate_split_runtime_outputs(app, runtime)


def test_validate_split_runtime_outputs_requires_shared_app_memory(
    tmp_path: Path,
    capsys,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))

    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))
    assert wasm_link._validate_split_runtime_outputs(app, runtime)

    app.write_bytes(_build_defined_memory_module(1))
    assert not wasm_link._validate_split_runtime_outputs(app, runtime)
    captured = capsys.readouterr()
    assert "Split-runtime app must import env.memory" in captured.err


def test_validate_split_runtime_outputs_rejects_structurally_invalid_app(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))

    seen: list[str] = []

    def validate(data: bytes, *, description: str) -> bool:
        seen.append(description)
        return description != "Split-runtime app"

    monkeypatch.setattr(wasm_link, "_validate_wasm_structural", validate)

    assert not wasm_link._validate_split_runtime_outputs(app, runtime)
    assert seen == ["Split-runtime app"]


def test_validate_split_runtime_outputs_rejects_structurally_invalid_runtime(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))

    seen: list[str] = []

    def validate(data: bytes, *, description: str) -> bool:
        seen.append(description)
        return description != "Split-runtime shared runtime"

    monkeypatch.setattr(wasm_link, "_validate_wasm_structural", validate)

    assert not wasm_link._validate_split_runtime_outputs(app, runtime)
    assert seen == ["Split-runtime app", "Split-runtime shared runtime"]


def test_tree_shake_runtime_preserves_dynamic_required_exports(monkeypatch) -> None:
    module = _build_exported_runtime_module_many(
        [
            "molt_exception_pending",
            "molt_gpu_linear_contiguous",
            "molt_gpu_tensor__tensor_scaled_dot_product_attention",
            "molt_gpu_turboquant_attention_packed",
        ]
    )
    monkeypatch.setenv(
        "MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS",
        "molt_gpu_linear_contiguous,molt_gpu_tensor__tensor_scaled_dot_product_attention,molt_gpu_turboquant_attention_packed",
    )
    shaken = wasm_link._tree_shake_runtime(
        module, {"exception_pending"}, facts_provider=_facts_provider
    )
    exports = wasm_link._collect_function_exports(shaken)
    assert "molt_gpu_linear_contiguous" in exports
    assert "molt_gpu_tensor__tensor_scaled_dot_product_attention" in exports
    assert "molt_gpu_turboquant_attention_packed" in exports


def test_tree_shake_runtime_reuses_cached_result(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    target_root = tmp_path / "target"
    final_runtime = b"\x00asm\x01\x00\x00\x00tree-shaken-runtime"
    calls = {"count": 0}

    def fake_run(cmd, capture_output, text, timeout):  # type: ignore[no-untyped-def]
        del capture_output, text, timeout
        calls["count"] += 1
        output_path = Path(cmd[cmd.index("-o") + 1])
        output_path.write_bytes(b"\x00asm\x01\x00\x00\x00shaken")
        return wasm_link.subprocess.CompletedProcess(cmd, 0, "", "")

    def fake_final_optimize(path: Path, level: str = "Oz", **_kwargs) -> bool:
        assert level == "Oz"
        path.write_bytes(final_runtime)
        return True

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: "/usr/bin/wasm-opt")
    monkeypatch.setattr(wasm_link, "_wasm_opt_version", lambda _path: "wasm-opt 1.0")
    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_run_wasm_opt_via_optimize", fake_final_optimize)

    first = wasm_link._tree_shake_runtime(
        module, {"exception_pending"}, facts_provider=_facts_provider
    )

    assert first == final_runtime
    assert calls["count"] == 1

    monkeypatch.setattr(
        wasm_link,
        "_run_external_tool",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("wasm-opt should not rerun for cached tree-shake output")
        ),
    )

    second = wasm_link._tree_shake_runtime(
        module, {"exception_pending"}, facts_provider=_facts_provider
    )

    assert second == final_runtime


def test_run_wasm_ld_split_runtime_falls_back_when_env_deploy_runtime_is_stale(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_split_runtime_app_module([])
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    control_linked = tmp_path / "control_linked.wasm"
    control_split_dir = tmp_path / "control_split"
    timings_path = tmp_path / "phase_timings.json"
    stale_runtime = tmp_path / "missing-runtime.wasm"

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    wasm_ld_commands: list[list[str]] = []

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_commands.append(list(cmd))
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setenv("MOLT_WASM_DEPLOY_RUNTIME", str(stale_runtime))
    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        phase_timings_file=timings_path,
    )
    first_link_commands = list(wasm_ld_commands)
    control_rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        control_linked,
        split_runtime=True,
        split_output_dir=control_split_dir,
    )

    assert rc == 0
    assert control_rc == 0
    assert len(first_link_commands) == 2, (
        "split-runtime builds without native objects must run wasm-ld for both "
        "the monolithic artifact and the split app"
    )
    assert all("--no-entry" in cmd for cmd in first_link_commands)
    expected_runtime = wasm_link.strip_wasm_publication_sections(
        runtime.read_bytes(), final_artifact=True, preserve_debug=False
    )
    actual_runtime = wasm_link.strip_wasm_publication_sections(
        (split_dir / "molt_runtime.wasm").read_bytes(),
        final_artifact=True,
        preserve_debug=False,
    )
    assert actual_runtime == expected_runtime
    assert linked.read_bytes() == control_linked.read_bytes()
    assert (split_dir / "app.wasm").read_bytes() == (
        control_split_dir / "app.wasm"
    ).read_bytes()
    assert (split_dir / "molt_runtime.wasm").read_bytes() == (
        control_split_dir / "molt_runtime.wasm"
    ).read_bytes()
    size_attestation = json.loads(
        (split_dir / "wasm_size_attestation.json").read_text(encoding="utf-8")
    )
    assert size_attestation["published"]["app"]["sections"]["export"] > 0
    assert size_attestation["published"]["runtime"]["sections"]["export"] > 0
    timings = json.loads(timings_path.read_text(encoding="utf-8"))
    assert set(timings) >= {
        "wasm_link_total",
        "split_runtime_processing",
        "wasm_strip",
        "fail_closed_validation",
    }


def test_run_wasm_ld_monolithic_prefers_relocatable_runtime_for_table_relocations(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    wasm_ld_inputs: list[str] = []

    runtime.write_bytes(runtime_bytes)
    reloc_runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 0
    assert any(Path(part).name == reloc_runtime.name for part in wasm_ld_inputs)
    assert not any(Path(part).name == runtime.name for part in wasm_ld_inputs)


def test_run_wasm_ld_links_staged_native_objects(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.o"
    wasm_ld_inputs: list[str] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(b"\x00asm\x01\x00\x00\x00native-object")

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_objects=(native_object,),
    )

    assert rc == 0
    output_index = wasm_ld_inputs.index("-o") + 2
    assert Path(wasm_ld_inputs[output_index + 2]).name == native_object.name


def test_run_wasm_ld_rejects_signature_mismatch_warning(
    tmp_path: Path,
    monkeypatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            _write_wasm_ld_output(cmd, output_bytes)
            return wasm_link.subprocess.CompletedProcess(
                cmd,
                0,
                "",
                "wasm-ld: warning: function signature mismatch: molt_guarded_class_def\n",
            )
        return wasm_link.subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 1
    assert (
        "function signature mismatch: molt_guarded_class_def" in capsys.readouterr().err
    )


def test_run_wasm_ld_links_rewritten_native_runtime_imports(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_add")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.molt.wasm"
    wasm_ld_inputs: list[str] = []
    rewritten_native_imports: list[list[tuple[str, str]]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(_build_env_function_import_module(["molt_add", "malloc"]))

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
            for part in cmd:
                path = Path(part)
                if path.name.startswith("native_runtime_imports_"):
                    rewritten_native_imports.append(
                        _function_import_pairs(path.read_bytes())
                    )
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_objects=(native_object,),
    )

    assert rc == 0
    assert str(native_object) not in wasm_ld_inputs
    assert rewritten_native_imports == [
        [
            ("molt_runtime", "molt_add"),
            ("env", "malloc"),
        ]
    ]


def test_run_wasm_ld_rejects_missing_native_object(
    tmp_path: Path,
    monkeypatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    missing_native_object = tmp_path / "external_static_packages" / "missing.o"

    runtime.write_bytes(_build_exported_runtime_module("molt_exception_pending"))
    output.write_bytes(_build_minimal_module(b""))
    monkeypatch.setattr(
        wasm_link,
        "_run_external_tool",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("wasm-ld must not run without staged native input")
        ),
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_objects=(missing_native_object,),
    )

    assert rc == 1
    captured = capsys.readouterr()
    assert "Native WASM link input not found" in captured.err
    assert str(missing_native_object) in captured.err


def test_split_native_app_uses_unique_molt_main_restoration_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime_bytes = _build_exported_runtime_module_many(["molt_main"])
    output_bytes = _build_exported_runtime_module("molt_main")
    output_bytes = wasm_link._append_linking_function_symbols(
        output_bytes,
        [
            (
                "__molt_output_export_0",
                0,
                wasm_link.FLAG_BINDING_GLOBAL
                | wasm_link.FLAG_EXPLICIT_NAME
                | wasm_link.FLAG_EXPORTED
                | wasm_link.FLAG_NO_STRIP,
            )
        ],
    )
    assert output_bytes is not None
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "native.molt.wasm"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.write_bytes(b"\0asm\x01\0\0\0native")
    commands: list[list[str]] = []

    def fake_run(cmd, **_kwargs):
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            commands.append(list(cmd))
        _write_wasm_ld_output(cmd, output_bytes)
        return wasm_link.subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _path: True)
    monkeypatch.setattr(wasm_link, "_validate_split_runtime_outputs", lambda *_a: True)
    monkeypatch.setattr(
        wasm_link,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_a, **_k: runtime_bytes
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    assert (
        _run_wasm_ld_with_rust_facts(
            "wasm-ld",
            runtime,
            output,
            linked,
            split_runtime=True,
            split_output_dir=tmp_path / "split",
            native_objects=(native_object,),
        )
        == 0
    )
    split_cmd = commands[1]
    assert "--export=molt_main" not in split_cmd
    assert "--export=__molt_output_export_0" in split_cmd


def test_run_wasm_ld_split_runtime_links_native_objects_into_app(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _module_with_linking_symbols([])
    app_data_offset = 2 * 65536
    app_table_base = 4096
    output_bytes = _build_runtime_import_data_module(
        [], memory_min=37, data_offset=app_data_offset, table_min=app_table_base
    )
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.o"
    link_calls: list[list[str]] = []
    app_link_bytes = _build_split_runtime_app_module([], memory_min=2)

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(b"\x00asm\x01\x00\x00\x00native-object")

    def fake_run(cmd, **kwargs):
        del kwargs
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            link_calls.append(list(cmd))
        link_output = app_link_bytes if len(link_calls) == 2 else output_bytes
        _write_wasm_ld_output(cmd, link_output)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_split_runtime_outputs", lambda *_a: True)
    monkeypatch.setattr(
        wasm_link,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_optimize_split_app_module", lambda data, **_: data)
    monkeypatch.setattr(
        wasm_link, "_collect_module_imports", lambda *_args, **_kwargs: set()
    )
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )
    monkeypatch.setattr(wasm_link, "_collect_custom_names", lambda _data: [])
    monkeypatch.setattr(
        wasm_link, "_collect_exports", lambda _data: {"molt_memory", "molt_table"}
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        native_objects=(native_object,),
    )

    assert rc == 0
    assert len(link_calls) == 2
    monolithic_cmd, split_app_cmd = link_calls
    assert any(Path(part).name == native_object.name for part in monolithic_cmd)
    assert any(Path(part).name == native_object.name for part in split_app_cmd)
    assert any(Path(part).name == runtime.name for part in monolithic_cmd)
    assert not any(Path(part).name == runtime.name for part in split_app_cmd)
    assert "--stack-first" in monolithic_cmd
    assert "--import-memory" in split_app_cmd
    assert "--no-stack-first" in split_app_cmd
    assert "--stack-first" not in split_app_cmd
    assert f"--global-base={app_data_offset}" in split_app_cmd
    assert f"--table-base={app_table_base}" in split_app_cmd
    assert not any("molt_runtime_stub" in part for part in monolithic_cmd)
    assert not any("molt_runtime_stub" in part for part in split_app_cmd)
    app_wasm = (split_dir / "app.wasm").read_bytes()
    assert wasm_link._memory_import_min(app_wasm) == 37
    assert _defined_memory_min(app_wasm) is None


def test_run_wasm_ld_split_runtime_forces_native_direct_symbols(
    tmp_path: Path,
    monkeypatch,
) -> None:
    symbol = "PyInit__demo"
    sealed_symbol = "PyInit__sealed_only"
    runtime_bytes = _build_exported_runtime_module_many(["molt_main"])
    output_bytes = _build_native_direct_import_module(symbol)
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "_demo.molt.wasm"
    link_calls: list[list[str]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(b"\x00asm\x01\x00\x00\x00native-object")
    native_object.with_name(native_object.name + ".extension_manifest.json").write_text(
        json.dumps(
            {
                "module": "nativepkg._demo",
                "init_symbol": sealed_symbol,
            }
        ),
        encoding="utf-8",
    )

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            link_calls.append(list(cmd))
            linked_input = Path(cmd[cmd.index("-o") + 2]).read_bytes()
            function_imports = {
                (module, name)
                for module, name, kind, _desc in wasm_link._collect_imports(
                    linked_input
                )
                if kind == 0
            }
            assert ("env", symbol) in function_imports
            assert ("molt_native", symbol) not in function_imports
        _write_wasm_ld_output(
            cmd,
            _build_exported_runtime_module_many([symbol, sealed_symbol]),
        )

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_split_runtime_outputs", lambda *_a: True)
    monkeypatch.setattr(
        wasm_link,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_optimize_split_app_module", lambda data, **_: data)
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        native_objects=(native_object,),
    )

    assert rc == 0
    assert len(link_calls) == 2
    for cmd in link_calls:
        assert f"--export={symbol}" in cmd
        assert f"--export={sealed_symbol}" in cmd
        assert f"--undefined={symbol}" not in cmd
        assert f"--export-if-defined={symbol}" not in cmd


def test_sealed_native_init_symbols_fail_closed_on_invalid_manifest(
    tmp_path: Path,
) -> None:
    native_object = tmp_path / "_demo.molt.wasm"
    native_object.write_bytes(b"\x00asm\x01\x00\x00\x00")
    native_object.with_name(native_object.name + ".extension_manifest.json").write_text(
        '{"init_symbol": "demo"}', encoding="utf-8"
    )

    with pytest.raises(ValueError, match="invalid init_symbol"):
        wasm_link._sealed_native_init_symbols((native_object,))


def test_public_export_restoration_preserves_sealed_init_symbol() -> None:
    symbol = "PyInit__demo"
    linked = _build_exported_function_module(symbol)

    restored = wasm_link._restore_public_output_exports(
        linked,
        {"nativepkg___demo": symbol},
        preserved_symbol_names=(symbol,),
    )

    exports = wasm_link._collect_function_exports(restored)
    assert symbol in exports


def test_public_export_restoration_recovers_sealed_init_symbol_from_linking_name() -> (
    None
):
    symbol = "PyInit__demo"
    linked = _build_exported_function_module(symbol)
    linked = wasm_link._append_linking_function_symbols(
        linked,
        [(symbol, 0, wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME)],
    )
    assert linked is not None
    stripped = wasm_link._strip_internal_exports(linked)
    assert stripped is not None
    assert symbol not in wasm_link._collect_function_exports(stripped)

    restored = wasm_link._restore_public_output_exports(
        stripped,
        {},
        preserved_symbol_names=(symbol,),
    )

    assert wasm_link._collect_function_exports(restored)[symbol] == 0
    assert (
        wasm_link._validate_required_native_direct_symbols(
            restored,
            (symbol,),
            description="Split-runtime native app link",
        )
        is None
    )


def test_native_direct_contract_restoration_recovers_stripped_real_body_by_name() -> (
    None
):
    symbol = "PyInit__demo"
    linked = _build_exported_function_module(symbol)
    linked = wasm_link._append_linking_function_symbols(
        linked,
        [(symbol, 0, wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME)],
    )
    assert linked is not None
    function_index = wasm_link._collect_function_exports(linked)[symbol]
    stripped = wasm_link._strip_internal_exports(linked)
    assert stripped is not None

    restored = wasm_link._restore_public_output_exports(
        stripped,
        {},
        preserved_symbol_names=(symbol,),
    )

    assert wasm_link._collect_function_exports(restored)[symbol] == function_index
    assert (
        wasm_link._validate_required_native_direct_symbols(
            restored,
            (symbol,),
            description="Split-runtime native app link",
        )
        is None
    )


def test_native_direct_symbol_validation_rejects_trap_stub() -> None:
    error = wasm_link._validate_required_native_direct_symbols(
        _build_exported_function_module("PyInit__demo", trap_body=True),
        ("PyInit__demo",),
        description="Split-runtime native app link",
    )

    assert error is not None
    assert "trap stub(s): PyInit__demo" in error


def test_run_wasm_ld_split_runtime_uses_linked_and_deploy_import_namespaces(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = wasm_link._build_sections(
        [
            *wasm_link._parse_sections(
                _add_data_address_global_exports(
                    _build_exported_runtime_module_many(["molt_err_pending"]),
                    {"PyLong_Type": 4096},
                )
            ),
            (
                0,
                wasm_link._build_custom_section(
                    "linking",
                    wasm_link._build_linking_payload(
                        2,
                        [
                            (
                                wasm_link.SYMTAB_SUBSECTION_ID,
                                _build_symbol_subsection(
                                    [
                                        _data_symbol_entry(
                                            flags=wasm_link.FLAG_EXPLICIT_NAME,
                                            name="PyLong_Type",
                                            segment_index=0,
                                            offset=4096,
                                            size=208,
                                        )
                                    ]
                                ),
                            )
                        ],
                    ),
                ),
            ),
        ]
    )
    output_bytes = _build_runtime_import_module(["molt_err_pending"])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.molt.wasm"
    link_calls: list[list[str]] = []
    linked_app_imports: list[list[tuple[str, str]]] = []
    deployed_native_imports: list[list[tuple[str, str]]] = []
    data_alias_symbols: list[list[tuple[int, str]]] = []
    allowlists: list[set[str]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(
        wasm_link._build_sections(
            [
                *wasm_link._parse_sections(
                    _build_env_function_import_module(
                        ["molt_err_pending", "malloc", "__trunctfdf2"]
                    )
                ),
                (
                    0,
                    wasm_link._build_custom_section(
                        "linking",
                        wasm_link._build_linking_payload(
                            2,
                            [
                                (
                                    wasm_link.SYMTAB_SUBSECTION_ID,
                                    _build_symbol_subsection(
                                        [
                                            _data_symbol_entry(
                                                flags=(
                                                    wasm_link.FLAG_UNDEFINED
                                                    | wasm_link.FLAG_EXPLICIT_NAME
                                                ),
                                                name="PyLong_Type",
                                            )
                                        ]
                                    ),
                                )
                            ],
                        ),
                    ),
                ),
            ]
        )
    )
    compiler_rt_provider = tmp_path / "rustlib" / "libcompiler_builtins-x.rlib"
    compiler_rt_provider.parent.mkdir()
    compiler_rt_provider.write_bytes(b"!<arch>\ncompiler-rt")

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            link_calls.append(list(cmd))
            for part in cmd:
                path = Path(part)
                if path.name == "output_linked_runtime_imports.wasm":
                    linked_app_imports.append(_function_import_pairs(path.read_bytes()))
                if path.name.startswith("native_runtime_imports_"):
                    deployed_native_imports.append(
                        _function_import_pairs(path.read_bytes())
                    )
                if path.name == "split_runtime_data_aliases.wasm":
                    data_alias_symbols.append(
                        _linking_data_symbol_names(path.read_bytes())
                    )
                if part.startswith("--allow-undefined-file="):
                    allowlists.append(_parse_allowlist(Path(part.split("=", 1)[1])))
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_split_runtime_outputs", lambda *_a: True)
    monkeypatch.setattr(
        wasm_link,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_optimize_split_app_module", lambda data, **_: data)
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))
    monkeypatch.setattr(
        wasm_link.wasm_toolchain,
        "wasm_compiler_builtins_archive",
        lambda: compiler_rt_provider,
        raising=True,
    )

    def provider_symbols(*, primitive_classes=None, **_kwargs):
        symbols: set[str] = set()
        if (
            primitive_classes is None
            or wasm_link.WASM_LIBC_LINK_IMPORT_CLASS in primitive_classes
        ):
            symbols.add("malloc")
        if (
            primitive_classes is None
            or wasm_link.WASM_COMPILER_RT_LINK_IMPORT_CLASS in primitive_classes
        ):
            symbols.add("__trunctfdf2")
        return frozenset(symbols)

    monkeypatch.setattr(
        wasm_link,
        "wasm_external_link_provider_symbols",
        provider_symbols,
        raising=True,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        native_objects=(native_object,),
    )

    assert rc == 0
    assert len(link_calls) == 2
    monolithic_cmd, split_app_cmd = link_calls
    assert any(Path(part).name == native_object.name for part in monolithic_cmd)
    assert str(compiler_rt_provider) in monolithic_cmd
    assert any(Path(part).name == runtime.name for part in monolithic_cmd)
    assert not any(Path(part).name == native_object.name for part in split_app_cmd)
    assert str(compiler_rt_provider) in split_app_cmd
    assert "--import-memory" not in monolithic_cmd
    assert "--import-memory" in split_app_cmd
    assert "--stack-first" in monolithic_cmd
    assert "--no-stack-first" in split_app_cmd
    assert "--stack-first" not in split_app_cmd
    assert "--global-base=67108864" in split_app_cmd
    assert any(
        Path(part).name == "output_linked_runtime_imports.wasm"
        for part in monolithic_cmd
    )
    assert not any("molt_runtime_stub" in part for part in monolithic_cmd)
    assert not any(
        Path(part).name.startswith("native_runtime_imports_") for part in monolithic_cmd
    )
    assert not any(
        Path(part).name == "split_runtime_data_aliases.wasm" for part in monolithic_cmd
    )
    assert any(
        Path(part).name.startswith("native_runtime_imports_") for part in split_app_cmd
    )
    assert any(
        Path(part).name == "split_runtime_data_aliases.wasm" for part in split_app_cmd
    )
    assert linked_app_imports == [[("env", "molt_err_pending")]]
    assert deployed_native_imports == [
        [
            ("molt_runtime", "molt_err_pending"),
            ("env", "malloc"),
            ("env", "__trunctfdf2"),
        ]
    ]
    assert data_alias_symbols == [[(wasm_link.FLAG_EXPLICIT_NAME, "molt_PyLong_Type")]]
    assert "molt_err_pending" not in allowlists[0]
    assert "molt_err_pending" in allowlists[1]
    assert "malloc" in allowlists[1]
    assert "__trunctfdf2" not in allowlists[0]
    assert "__trunctfdf2" not in allowlists[1]


def test_canonical_split_runtime_required_exports_uses_runtime_export_surface() -> None:
    module = _build_exported_runtime_module_many(
        [
            "molt_exception_pending",
            "molt_object_field_get",
            "molt_object_field_set",
            "molt_guarded_field_get_ptr",
        ]
    )

    exports = wasm_link._canonical_split_runtime_required_exports(module)

    assert exports == {
        "molt_object_field_get",
        "molt_object_field_set",
        "molt_guarded_field_get_ptr",
    }


def test_tree_shake_runtime_uses_converge_flag(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    target_root = tmp_path / "target"
    calls: list[list[str]] = []

    def fake_run(cmd, capture_output, text, timeout):  # type: ignore[no-untyped-def]
        del capture_output, text, timeout
        calls.append(list(cmd))
        output_path = Path(cmd[cmd.index("-o") + 1])
        output_path.write_bytes(b"\x00asm\x01\x00\x00\x00shaken")
        return wasm_link.subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: "/usr/bin/wasm-opt")
    monkeypatch.setattr(wasm_link, "_wasm_opt_version", lambda _path: "wasm-opt 1.0")
    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link, "_run_wasm_opt_via_optimize", lambda *_a, **_k: False
    )

    shaken = wasm_link._tree_shake_runtime(
        module, {"exception_pending"}, facts_provider=_facts_provider
    )

    assert shaken.startswith(b"\x00asm\x01\x00\x00\x00")
    assert calls, "expected wasm-opt tree-shake invocation"
    assert "--converge" in calls[0]


def test_run_wasm_opt_via_optimize_enforces_current_export_contract(
    tmp_path: Path,
    monkeypatch,
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(
        _build_exported_runtime_module_many(["molt_main", "molt_host_init"])
    )
    seen: dict[str, object] = {}

    class _Loader:
        def create_module(self, _spec):  # noqa: ANN001
            return None

        def exec_module(self, module):  # noqa: ANN001
            def fake_optimize(
                input_path,
                *,
                output_path,
                level,
                extra_passes,
                converge,
                required_exports,
                apply_level,
            ):
                seen["input_path"] = input_path
                seen["level"] = level
                seen["converge"] = converge
                seen["required_exports"] = set(required_exports)
                seen["apply_level"] = apply_level
                output_path.write_bytes(input_path.read_bytes())
                return {
                    "ok": True,
                    "output_bytes": output_path.stat().st_size,
                    "pipeline": extra_passes,
                    "before": {"file_bytes": input_path.stat().st_size},
                    "after": {"file_bytes": output_path.stat().st_size},
                    "error": "",
                }

            module.optimize = fake_optimize

    monkeypatch.setattr(
        importlib.util,
        "spec_from_file_location",
        lambda _name, _path: importlib.machinery.ModuleSpec(
            "wasm_optimize",
            _Loader(),
        ),
    )

    assert wasm_link._run_wasm_opt_via_optimize(linked, level="Oz")
    assert seen["required_exports"] == {"molt_main", "molt_host_init"}
    assert seen["apply_level"] is True


def test_oz_publication_pipeline_is_bounded_and_size_focused() -> None:
    assert wasm_link._OZ_PASSES == [
        "--remove-unused-module-elements",
        "--strip-debug",
        "--strip-producers",
        "--dae-optimizing",
        "--simplify-locals",
        "--merge-blocks",
        "--dce",
        "--vacuum",
        "--zero-filled-memory",
        "--memory-packing",
    ]


def test_neutralize_dead_element_entries_preserves_host_call_indirect_modules() -> None:
    facts = {
        "root_function_indices": [],
        "element_function_indices": [],
        "declared_function_indices": [],
        "function_references": [],
        "active_function_elements": [],
        "reachable_dynamic_dispatch": True,
        "exported_table_indices": [],
        "table_mutations": [],
    }
    assert (
        wasm_link._neutralize_dead_element_entries(
            _build_host_call_indirect_module(), facts
        )
        is None
    )


def test_neutralize_dead_element_entries_uses_reachable_roots_and_fail_closed_controls() -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = [
        (1, write_varuint(1) + b"\x60\x00\x00"),
        (3, write_varuint(2) + write_varuint(0) + write_varuint(0)),
        (4, write_varuint(1) + b"\x70\x00" + write_varuint(1)),
        (
            9,
            write_varuint(1)
            + b"\x00\x41\x00\x0b"
            + write_varuint(1)
            + write_varuint(1),
        ),
        (10, write_varuint(2) + b"\x02\x00\x0b\x02\x00\x0b"),
    ]
    module = wasm_link._build_sections(sections)
    facts = {
        "root_function_indices": [0],
        "element_function_indices": [1],
        "declared_function_indices": [],
        "function_references": [],
        "active_function_elements": [[0, 0, 1]],
        "reachable_dynamic_dispatch": False,
        "exported_table_indices": [],
        "table_mutations": [],
    }

    neutralized = wasm_link._neutralize_dead_element_entries(module, facts)
    assert neutralized is not None
    element_payload = next(
        payload for section_id, payload in wasm_link._parse_sections(neutralized)
        if section_id == 9
    )
    assert element_payload.endswith(b"\x00")
    assert wasm_link._reachable_function_indices(facts) == {0}

    observable = dict(facts)
    observable["exported_table_indices"] = [0]
    assert wasm_link._reachable_function_indices(observable) == {0, 1}
    nonzero_observable = dict(facts)
    nonzero_observable["exported_table_indices"] = [1]
    assert wasm_link._reachable_function_indices(nonzero_observable) == {0}
    dynamic = dict(facts)
    dynamic["reachable_dynamic_dispatch"] = True
    assert wasm_link._reachable_function_indices(dynamic) == {0, 1}
    reachable_ref = dict(facts)
    reachable_ref["function_references"] = [[0, [], [1]], [2, [], [1]]]
    assert wasm_link._reachable_function_indices(reachable_ref) == {0, 1}
    table_init = dict(facts)
    table_init["table_mutations"] = [[0, "table.init", 0, None]]
    assert wasm_link._reachable_function_indices(table_init) == {0, 1}

    for override in (
        {"reachable_dynamic_dispatch": True},
        {"exported_table_indices": [0]},
        {
            "table_mutations": [[0, "table.init", 0, None]]
        },
    ):
        controlled = dict(facts)
        controlled.update(override)
        assert wasm_link._neutralize_dead_element_entries(module, controlled) is None


def test_import_walkers_handle_tag_imports_before_host_call_indirect() -> None:
    module = _build_tag_then_host_call_indirect_import_module()
    sections = wasm_link._parse_sections(module)

    assert wasm_link._count_func_imports(sections) == 1


def test_strip_unused_module_function_imports_remaps_indices() -> None:
    module = _build_runtime_import_strip_module()
    facts = _rust_facts_fixture(module)
    facts["root_function_indices"] = [2]
    facts["function_references"] = [[2, [1], []]]

    stripped = wasm_link._strip_unused_module_function_imports(
        module,
        module_name="molt_runtime",
        facts=facts,
    )

    imports_after = _function_import_pairs(stripped)
    assert imports_after == [("molt_runtime", "live_runtime_fn")]

    exports_after = _function_export_pairs(stripped)
    assert exports_after == [("molt_main", 1)]

    call_targets = _parse_code_section_call_targets(stripped)
    assert call_targets == [[0]]


def test_rewrite_output_imports_uses_generated_runtime_export_names(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_runtime_import_module(["socket_drop", "molt_alloc"]))

    owned_temp_dir = tempfile.TemporaryDirectory()
    rewritten = wasm_link._rewrite_output_imports(
        output,
        {"molt_socket_drop", "molt_alloc"},
        owned_temp_dir,
    )

    assert rewritten is not None
    rewritten_path, temp_dir, force_exports = rewritten
    try:
        assert force_exports == []
        assert _function_import_pairs(rewritten_path.read_bytes()) == [
            ("molt_runtime", "molt_socket_drop"),
            ("molt_runtime", "molt_alloc"),
        ]
    finally:
        temp_dir.cleanup()


def test_rewrite_native_runtime_imports_canonicalizes_env_molt_abi_only(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["molt_add", "malloc"]))

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            {"molt_add"},
            temp_dir,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "molt_add"),
            ("env", "malloc"),
        ]
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "molt_add"),
            ("env", "malloc"),
        ]


def test_rewrite_native_runtime_imports_routes_canonical_cpython_abi_symbols(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(
        _build_env_function_import_module(
            [
                "PyErr_Format",
                "PyArg_ParseTuple",
                "PyObject_CallFunction",
                "PyArg_ParseTupleAndKeywords",
                "PyTuple_Pack",
                "molt_cpython_abi_date_from_date",
                "malloc",
            ]
        )
    )

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            {
                "PyErr_Format",
                "PyArg_ParseTuple",
                "PyObject_CallFunction",
                "PyArg_ParseTupleAndKeywords",
                "PyTuple_Pack",
                "molt_cpython_abi_date_from_date",
            },
            temp_dir,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "PyErr_Format"),
            ("molt_runtime", "PyArg_ParseTuple"),
            ("molt_runtime", "PyObject_CallFunction"),
            ("molt_runtime", "PyArg_ParseTupleAndKeywords"),
            ("molt_runtime", "PyTuple_Pack"),
            ("molt_runtime", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "PyErr_Format"),
            ("env", "PyArg_ParseTuple"),
            ("env", "PyObject_CallFunction"),
            ("env", "PyArg_ParseTupleAndKeywords"),
            ("env", "PyTuple_Pack"),
            ("env", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]


def test_rewrite_native_runtime_imports_split_runtime_uses_public_cpython_abi_exports(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(
        _build_env_function_import_module(
            [
                "PyType_Ready",
                "Py_DECREF",
                "molt_cpython_abi_date_from_date",
                "malloc",
            ]
        )
    )

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            {
                "molt_PyType_Ready",
                "molt_Py_DECREF",
                "molt_cpython_abi_date_from_date",
            },
            temp_dir,
            split_runtime=True,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "molt_PyType_Ready"),
            ("molt_runtime", "molt_Py_DECREF"),
            ("molt_runtime", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "PyType_Ready"),
            ("env", "Py_DECREF"),
            ("env", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]


def test_rewrite_native_runtime_imports_split_runtime_prefixes_cpython_abi_data_symbols(
    tmp_path: Path,
) -> None:
    # CPython ABI type objects (PyLong_Type, PyType_Type, ...) surface as
    # undefined *data* symbols in the relocatable object's linking symtab, not
    # as function imports. The deployed split app must carry them as the
    # molt_-prefixed public export names the shared split runtime provides.
    native = tmp_path / "multiarray.molt.wasm"
    native.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    name="PyLong_Type",
                ),
                _data_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    name="PyType_Type",
                ),
                _data_symbol_entry(
                    flags=wasm_link.FLAG_EXPLICIT_NAME,
                    name="local_defined_datum",
                    segment_index=0,
                    offset=4,
                    size=8,
                ),
            ]
        )
    )

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            {"molt_PyLong_Type", "molt_PyType_Type"},
            temp_dir,
            split_runtime=True,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _linking_data_symbol_names(rewritten_paths[0].read_bytes()) == [
            (
                wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                "molt_PyLong_Type",
            ),
            (
                wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                "molt_PyType_Type",
            ),
            (wasm_link.FLAG_EXPLICIT_NAME, "local_defined_datum"),
        ]
    # The original relocatable object is never mutated in place.
    assert _linking_data_symbol_names(native.read_bytes()) == [
        (wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME, "PyLong_Type"),
        (wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME, "PyType_Type"),
        (wasm_link.FLAG_EXPLICIT_NAME, "local_defined_datum"),
    ]


def test_rewrite_native_runtime_imports_reloc_keeps_unprefixed_cpython_abi_data_symbols(
    tmp_path: Path,
) -> None:
    # The monolithic runnable statically links native objects against the
    # relocatable runtime, whose CPython ABI type objects are the real
    # unprefixed `#[no_mangle]` symbols. The reloc naming convention
    # (split_runtime=False) must leave the data symbol names untouched so
    # wasm-ld resolves them directly against the relocatable runtime.
    native = tmp_path / "multiarray.molt.wasm"
    native.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    name="PyLong_Type",
                ),
            ]
        )
    )

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            {"molt_PyLong_Type"},
            temp_dir,
            split_runtime=False,
        )

    # No molt_-prefix churn under the reloc convention: the data symbol name is
    # left untouched (it already matches the relocatable runtime symbol) so the
    # object is returned unchanged. The unprefixed name is flagged for
    # force-export so wasm-ld retains it from the relocatable runtime.
    assert force_exports == ["PyLong_Type"]
    assert rewritten_paths == (native,)
    assert _linking_data_symbol_names(native.read_bytes()) == [
        (wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME, "PyLong_Type"),
    ]


def test_split_runtime_data_alias_object_uses_deploy_runtime_export_addresses(
    tmp_path: Path,
) -> None:
    # The alias must point the native object's undefined (split-renamed) data
    # symbol at the DEPLOY runtime's canonical address, read from the runtime's
    # exported address global â€” NOT the relocatable runtime's segment-relative
    # offset (which is not a final address).
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x2E1000})
    )
    native.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    name="molt_PyLong_Type",
                ),
            ]
        )
    )

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        alias = wasm_link._split_runtime_data_alias_object(
            native_objects=(native,),
            deploy_runtime=deploy_runtime,
            temp_dir=temp_dir,
        )

        assert alias is not None
        assert alias != native
        alias_bytes = alias.read_bytes()
        assert _linking_data_symbol_names(alias_bytes) == [
            (wasm_link.FLAG_EXPLICIT_NAME, "molt_PyLong_Type"),
        ]
        # The aliased symbol resolves to the deploy runtime's exported address.
        seg_addresses = _alias_segment_addresses(alias_bytes)
        assert seg_addresses[0] == 0x2E1000
        data_sections = [
            payload
            for section_id, payload in wasm_link._parse_sections(alias_bytes)
            if section_id == 11
        ]
        assert data_sections
        assert b"PyLong_Type" not in data_sections[0]


def test_rewrite_native_runtime_imports_rejects_non_manifest_raw_c_api_symbol(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["PyArray_NDIM"]))

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            {"PyArray_NDIM"},
            temp_dir,
        )

        assert force_exports == []
        assert rewritten_paths == (native,)
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "PyArray_NDIM"),
        ]


def test_rewrite_native_runtime_imports_forces_generated_runtime_exports(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["add"]))

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link._rewrite_native_runtime_imports(
            (native,),
            set(),
            temp_dir,
        )

        assert force_exports == ["molt_add"]
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "molt_add"),
        ]


def test_split_runtime_validation_uses_generated_runtime_export_names(
    tmp_path: Path,
) -> None:
    app = tmp_path / "app.wasm"
    runtime = tmp_path / "runtime.wasm"
    app.write_bytes(
        _build_split_runtime_app_module(["socket_drop", "unknown_probe"], memory_min=1)
    )
    runtime.write_bytes(_build_exported_runtime_module("molt_socket_drop"))

    assert not wasm_link._validate_split_runtime_outputs(app, runtime)

    app.write_bytes(_build_split_runtime_app_module(["socket_drop"], memory_min=1))
    assert wasm_link._validate_split_runtime_outputs(app, runtime)

    app.write_bytes(_build_split_runtime_app_module(["PyType_Ready"], memory_min=1))
    runtime.write_bytes(_build_exported_runtime_module("PyType_Ready"))
    assert not wasm_link._validate_split_runtime_outputs(app, runtime)

    runtime.write_bytes(_build_exported_runtime_module("molt_PyType_Ready"))
    assert wasm_link._validate_split_runtime_outputs(app, runtime)


def test_post_link_optimize_split_app_drops_numeric_table_aliases() -> None:
    table_ref = "__molt_table_ref_7"
    module = _build_exported_runtime_module_many(
        ["dead_user_export", "molt_main", table_ref]
    )

    default_optimized = wasm_link._post_link_optimize(
        module,
        reference_data=module,
        facts_provider=_facts_provider,
    )
    assert "dead_user_export" in wasm_link._collect_exports(default_optimized)

    split_app_optimized = wasm_link._post_link_optimize(
        module,
        reference_data=module,
        preserve_exports={"molt_main"},
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )
    split_exports = wasm_link._collect_exports(split_app_optimized)
    assert "dead_user_export" not in split_exports
    assert "molt_main" in split_exports
    assert table_ref not in split_exports


def test_collect_linking_function_symbols_parses_defined_and_undefined_entries() -> (
    None
):
    data = _module_with_linking_symbols(
        [
            _data_symbol_entry(
                flags=wasm_link.FLAG_EXPLICIT_NAME,
                name="not_a_function",
                segment_index=3,
                offset=12,
                size=8,
            ),
            _function_symbol_entry(
                flags=wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME,
                index=7,
                name="molt_call_indirect0",
            ),
            _function_symbol_entry(
                flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                index=11,
                name="molt_call_indirect13",
            ),
        ]
    )

    symbols = wasm_link._collect_linking_function_symbols(data)

    assert [(flags, index, name) for flags, index, name, _ in symbols] == [
        (
            wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME,
            7,
            "molt_call_indirect0",
        ),
        (
            wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
            11,
            "molt_call_indirect13",
        ),
    ]


def test_inject_output_export_aliases_preserves_export_flag_for_user_exports(
    tmp_path: Path,
) -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("main_molt__ocr_tokens"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link._build_linking_payload(
        2,
        [
            (
                wasm_link.SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL,
                            index=0,
                            name="func0",
                        )
                    ]
                ),
            )
        ],
    )
    sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))
    module = wasm_link._build_sections(sections)
    output = tmp_path / "output.wasm"
    output.write_bytes(module)

    with tempfile.TemporaryDirectory() as tmp:
        temp_dir = tempfile.TemporaryDirectory(dir=tmp)
        updated = wasm_link._inject_output_export_aliases(
            output, temp_dir, _rust_facts_fixture(output.read_bytes())
        )
        symbols = wasm_link._collect_linking_function_symbols(updated.read_bytes())
        assert any(
            name == f"{wasm_link._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__ocr_tokens"
            and (flags & wasm_link.FLAG_EXPORTED)
            for flags, _, name, _ in symbols
        ), symbols
        temp_dir.cleanup()


def test_restore_output_export_aliases_renames_user_exports() -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(
        wasm_link._write_string(
            f"{wasm_link._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__ocr_tokens"
        )
    )
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    restored = wasm_link._restore_output_export_aliases(
        wasm_link._build_sections(sections)
    )
    assert restored is not None
    exports = wasm_link._collect_exports(restored)
    assert "main_molt__ocr_tokens" in exports
    assert (
        f"{wasm_link._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__ocr_tokens" not in exports
    )


def test_inject_output_export_aliases_adds_runtime_entrypoint_symbols(
    tmp_path: Path,
) -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("molt_isolate_import"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(4))
    code_payload.append(0x00)
    code_payload.append(0x20)
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link._build_linking_payload(
        2,
        [
            (
                wasm_link.SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL,
                            index=0,
                            name="func0",
                        )
                    ]
                ),
            )
        ],
    )
    sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))
    module = wasm_link._build_sections(sections)
    output = tmp_path / "output.wasm"
    output.write_bytes(module)

    with tempfile.TemporaryDirectory() as tmp:
        temp_dir = tempfile.TemporaryDirectory(dir=tmp)
        updated = wasm_link._inject_output_export_aliases(
            output, temp_dir, _rust_facts_fixture(output.read_bytes())
        )
        symbols = wasm_link._collect_linking_function_symbols(updated.read_bytes())
        assert any(
            name == "molt_isolate_import" and (flags & wasm_link.FLAG_BINDING_GLOBAL)
            for flags, _, name, _ in symbols
        ), symbols
        assert any(
            name == f"{wasm_link._OUTPUT_EXPORT_ALIAS_PREFIX}molt_isolate_import"
            and (flags & wasm_link.FLAG_EXPORTED)
            for flags, _, name, _ in symbols
        ), symbols
        temp_dir.cleanup()


def test_run_wasm_ld_preserves_runtime_entrypoint_without_prelink_alias_object(
    tmp_path: Path,
    monkeypatch,
) -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("molt_isolate_import"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(4))
    code_payload.append(0x00)
    code_payload.append(0x20)
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link._build_linking_payload(
        2,
        [
            (
                wasm_link.SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL,
                            index=0,
                            name="func0",
                        )
                    ]
                ),
            )
        ],
    )
    sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))

    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(
        _build_exported_runtime_module_many(["molt_main", "molt_isolate_import"])
    )
    output.write_bytes(wasm_link._build_sections(sections))

    captured_cmds: list[list[str]] = []

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        captured_cmds.append(list(cmd))
        if cmd and cmd[0] == "wasm-ld":
            _write_wasm_ld_output(cmd, Path(cmd[-2]).read_bytes())

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 0
    cmd = next(cmd for cmd in captured_cmds if cmd and cmd[0] == "wasm-ld")
    assert not any("output_runtime_aliases.wasm" in part for part in cmd)
    assert "molt_isolate_import" in wasm_link._collect_function_exports(
        linked.read_bytes()
    )


def test_inject_output_export_aliases_skips_void_user_exports(
    tmp_path: Path,
) -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(2))
    for i in range(2):
        import_payload.extend(wasm_link._write_string("env"))
        import_payload.extend(wasm_link._write_string(f"imp{i}"))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = (
        write_varuint(3) + write_varuint(0) + write_varuint(0) + write_varuint(0)
    )
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    for name, index in (
        ("main_molt__init", 2),
        ("main_molt__ocr_tokens", 3),
        ("molt_main", 4),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(3))
    for _ in range(3):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link._build_linking_payload(
        2,
        [
            (
                wasm_link.SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME,
                            index=2,
                            name="__molt_output_export_2",
                        ),
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME,
                            index=3,
                            name="__molt_output_export_3",
                        ),
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME
                            | wasm_link.FLAG_EXPORTED
                            | wasm_link.FLAG_NO_STRIP,
                            index=4,
                            name="molt_main",
                        ),
                    ]
                ),
            )
        ],
    )
    sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))

    output = tmp_path / "output.wasm"
    output.write_bytes(wasm_link._build_sections(sections))

    with tempfile.TemporaryDirectory() as tmp:
        temp_dir = tempfile.TemporaryDirectory(dir=tmp)
        updated = wasm_link._inject_output_export_aliases(
            output, temp_dir, _rust_facts_fixture(output.read_bytes())
        )
        assert updated == output
        temp_dir.cleanup()


def test_collect_output_wrapper_specs_skips_internal_module_helpers() -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(4))
    for _ in range(4):
        func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(4))
    for name, index in (
        ("main_molt__init", 0),
        ("main_molt__molt_module_chunk_1", 1),
        ("__future_____Feature___init__", 2),
        ("molt_isolate_import", 3),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(4))
    for _ in range(4):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    specs = wasm_link._collect_output_wrapper_specs(
        wasm_link._build_sections(sections),
        _rust_facts_fixture(wasm_link._build_sections(sections)),
    )
    kept = {name for name, _alias, _type_idx, _func_idx in specs}
    assert "main_molt__init" in kept
    assert "molt_isolate_import" in kept
    assert "main_molt__molt_module_chunk_1" not in kept
    assert "__future_____Feature___init__" not in kept


def test_restore_public_output_exports_renames_native_split_alias_exports() -> None:
    alias_name = f"{wasm_link._OUTPUT_EXPORT_ALIAS_PREFIX}molt_isolate_import"
    module = _build_exported_runtime_module(alias_name)

    restored = wasm_link._restore_public_output_exports(
        module,
        {"molt_isolate_import": alias_name},
    )

    exports = wasm_link._collect_function_exports(restored)
    assert exports["molt_isolate_import"] == 0
    assert alias_name not in exports


def test_entry_module_prefix_from_main_init_prefers_main_module() -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(2))
    func_payload.extend(write_varuint(0))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(2))
    for name, index in (
        ("molt_init_main_molt", 0),
        ("molt_init___main__", 1),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    body = bytearray()
    body.append(0x00)
    body.append(0x10)
    body.extend(write_varuint(0))
    body.append(0x0B)
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    module = wasm_link._build_sections(sections)
    exports = wasm_link._collect_function_exports(module)
    facts = _rust_facts_fixture(module)
    facts["function_references"] = [[1, [0], []]]
    assert wasm_link._entry_module_prefix_from_main_init(
        exports, facts
    ) == "main_molt"


def test_collect_output_wrapper_specs_prefers_main_module_prefix_over_dominant_stdlib() -> (
    None
):
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(6))
    for _ in range(6):
        func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(6))
    for name, index in (
        ("main_molt__init", 0),
        ("main_molt__ocr_tokens", 1),
        ("os__path", 2),
        ("os__stat", 3),
        ("os__walk", 4),
        ("molt_init___main__", 5),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(6))
    for index in range(4):
        code_payload.extend(write_varuint(4))
        code_payload.append(0x00)
        code_payload.append(0x42)
        code_payload.append(0x00)
        code_payload.append(0x0B)
    code_payload.extend(write_varuint(4))
    code_payload.append(0x00)
    code_payload.append(0x42)
    code_payload.append(0x00)
    code_payload.append(0x0B)
    body = bytearray()
    body.append(0x00)
    body.append(0x10)
    body.extend(write_varuint(0))
    body.append(0x0B)
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    module = wasm_link._build_sections(sections)
    facts = _rust_facts_fixture(module)
    facts["function_references"] = [[5, [0], []]]
    specs = wasm_link._collect_output_wrapper_specs(module, facts)
    kept = {name for name, _alias, _type_idx, _func_idx in specs}
    assert "main_molt__init" in kept
    assert "main_molt__ocr_tokens" in kept
    assert "os__path" not in kept
    assert "os__stat" not in kept


def test_ensure_function_exports_by_symbol_names_adds_public_exports() -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(2))
    func_payload.extend(write_varuint(0))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(1))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    for _ in range(2):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link._build_linking_payload(
        2,
        [
            (
                wasm_link.SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME
                            | wasm_link.FLAG_EXPORTED
                            | wasm_link.FLAG_NO_STRIP,
                            index=0,
                            name="__molt_output_export_0",
                        ),
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME
                            | wasm_link.FLAG_EXPORTED
                            | wasm_link.FLAG_NO_STRIP,
                            index=1,
                            name="molt_main",
                        ),
                    ]
                ),
            )
        ],
    )
    sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))

    updated = wasm_link._ensure_function_exports_by_symbol_names(
        wasm_link._build_sections(sections),
        {"main_molt__init": "__molt_output_export_0"},
    )
    assert updated is not None
    exports = wasm_link._collect_exports(updated)
    assert "main_molt__init" in exports
    assert "molt_main" in exports


def test_ensure_function_exports_by_symbol_names_uses_name_section_fallback() -> None:
    write_varuint = wasm_link._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(1))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    func_name_subsection = bytearray()
    func_name_subsection.extend(write_varuint(1))
    func_name_subsection.extend(write_varuint(0))
    func_name_subsection.extend(wasm_link._write_string("__molt_output_export_1900"))
    name_custom_payload = bytearray()
    name_custom_payload.append(1)
    name_custom_payload.extend(write_varuint(len(func_name_subsection)))
    name_custom_payload.extend(func_name_subsection)
    sections.append(
        (0, wasm_link._build_custom_section("name", bytes(name_custom_payload)))
    )

    updated = wasm_link._ensure_function_exports_by_symbol_names(
        wasm_link._build_sections(sections),
        {"main_molt__init": "__molt_output_export_1900"},
    )
    assert updated is not None
    exports = wasm_link._collect_exports(updated)
    assert "main_molt__init" in exports


def test_run_wasm_ld_force_exports_user_module_exports(
    tmp_path: Path, monkeypatch
) -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []
    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))
    func_payload = (
        write_varuint(3) + write_varuint(0) + write_varuint(0) + write_varuint(0)
    )
    sections.append((3, bytes(func_payload)))
    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    for name, index in (
        ("main_molt__init", 0),
        ("main_molt__ocr_tokens", 1),
        ("molt_main", 2),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))
    code_payload = bytearray()
    code_payload.extend(write_varuint(3))
    for _ in range(3):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))
    linking_payload = wasm_link._build_linking_payload(
        2,
        [
            (
                wasm_link.SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME
                            | wasm_link.FLAG_EXPORTED
                            | wasm_link.FLAG_NO_STRIP,
                            index=0,
                            name="__molt_output_export_0",
                        ),
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME
                            | wasm_link.FLAG_EXPORTED
                            | wasm_link.FLAG_NO_STRIP,
                            index=1,
                            name="__molt_output_export_1",
                        ),
                        _function_symbol_entry(
                            flags=wasm_link.FLAG_BINDING_GLOBAL
                            | wasm_link.FLAG_EXPLICIT_NAME
                            | wasm_link.FLAG_EXPORTED
                            | wasm_link.FLAG_NO_STRIP,
                            index=2,
                            name="molt_main",
                        ),
                    ]
                ),
            )
        ],
    )
    sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))
    output_bytes = wasm_link._build_sections(sections)

    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(output_bytes)
    output.write_bytes(output_bytes)

    captured_cmds: list[list[str]] = []

    def fake_run(cmd, **kwargs):
        captured_cmds.append(list(cmd))
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
    )
    assert rc == 0
    cmd = next(cmd for cmd in captured_cmds if cmd and cmd[0] == "wasm-ld")
    assert "--export=__molt_output_export_0" in cmd
    assert "--export=__molt_output_export_1" in cmd


def test_run_wasm_ld_repairs_linked_host_init_export(
    tmp_path: Path, monkeypatch
) -> None:
    write_varuint = wasm_link._write_varuint

    def _module(*, include_host_init_export: bool) -> bytes:
        sections: list[tuple[int, bytes]] = []
        type_payload = bytearray()
        type_payload.extend(write_varuint(1))
        type_payload.append(0x60)
        type_payload.extend(write_varuint(0))
        type_payload.extend(write_varuint(0))
        sections.append((1, bytes(type_payload)))

        func_payload = write_varuint(2) + write_varuint(0) + write_varuint(0)
        sections.append((3, bytes(func_payload)))

        exports: list[tuple[str, int]] = [("molt_main", 1)]
        if include_host_init_export:
            exports.insert(0, ("molt_host_init", 0))
        export_payload = bytearray()
        export_payload.extend(write_varuint(len(exports)))
        for name, index in exports:
            export_payload.extend(wasm_link._write_string(name))
            export_payload.append(0x00)
            export_payload.extend(write_varuint(index))
        sections.append((7, bytes(export_payload)))

        code_payload = bytearray()
        code_payload.extend(write_varuint(2))
        for _ in range(2):
            code_payload.extend(write_varuint(2))
            code_payload.append(0x00)
            code_payload.append(0x0B)
        sections.append((10, bytes(code_payload)))

        linking_payload = wasm_link._build_linking_payload(
            2,
            [
                (
                    wasm_link.SYMTAB_SUBSECTION_ID,
                    _build_symbol_subsection(
                        [
                            _function_symbol_entry(
                                flags=wasm_link.FLAG_BINDING_GLOBAL
                                | wasm_link.FLAG_EXPLICIT_NAME
                                | wasm_link.FLAG_EXPORTED
                                | wasm_link.FLAG_NO_STRIP,
                                index=0,
                                name="molt_host_init",
                            ),
                            _function_symbol_entry(
                                flags=wasm_link.FLAG_BINDING_GLOBAL
                                | wasm_link.FLAG_EXPLICIT_NAME
                                | wasm_link.FLAG_EXPORTED
                                | wasm_link.FLAG_NO_STRIP,
                                index=1,
                                name="molt_main",
                            ),
                        ]
                    ),
                )
            ],
        )
        sections.append(
            (0, wasm_link._build_custom_section("linking", linking_payload))
        )
        return wasm_link._build_sections(sections)

    output_bytes = _module(include_host_init_export=True)
    linked_without_host_init = _module(include_host_init_export=False)
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(output_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **_kwargs):
        _write_wasm_ld_output(cmd, linked_without_host_init)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 0
    exports = wasm_link._collect_function_exports(linked.read_bytes())
    assert "molt_host_init" in exports


def test_call_indirect_symbol_discovery_does_not_require_wasm_tools(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "runtime_reloc.wasm"
    runtime.write_bytes(
        _module_with_linking_symbols(
            [
                _function_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    index=3,
                    name="_ZN4molt19molt_call_indirect1317hfeedfaceE",
                ),
                _function_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    index=4,
                    name="_ZN4molt19molt_call_indirect9917hfeedfaceE",
                ),
            ]
        )
    )
    output = tmp_path / "output.wasm"
    output.write_bytes(
        _module_with_linking_symbols(
            [
                _function_symbol_entry(
                    flags=wasm_link.FLAG_BINDING_GLOBAL
                    | wasm_link.FLAG_EXPLICIT_NAME
                    | wasm_link.FLAG_EXPORTED,
                    index=41,
                    name="molt_call_indirect13",
                ),
                _function_symbol_entry(
                    flags=wasm_link.FLAG_BINDING_GLOBAL
                    | wasm_link.FLAG_EXPLICIT_NAME
                    | wasm_link.FLAG_EXPORTED,
                    index=42,
                    name="molt_call_indirect99",
                ),
            ]
        )
    )
    monkeypatch.setattr(wasm_link, "_find_tool", lambda _names: None)

    mangled = wasm_link._find_call_indirect_mangled(runtime)
    output_symbols = wasm_link._find_output_call_indirect_symbol(output)

    assert mangled == {
        "molt_call_indirect13": "_ZN4molt19molt_call_indirect1317hfeedfaceE"
    }
    assert "molt_call_indirect99" not in output_symbols
    assert output_symbols["molt_call_indirect13"] == (
        41,
        wasm_link.FLAG_BINDING_GLOBAL
        | wasm_link.FLAG_EXPLICIT_NAME
        | wasm_link.FLAG_EXPORTED,
    )


def test_run_wasm_ld_split_runtime_preserves_old_outputs_if_linked_validation_fails(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _module_with_linking_symbols([])
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    split_dir.mkdir()
    linked.write_bytes(b"old-linked")
    (split_dir / "app.wasm").write_bytes(b"old-app")
    (split_dir / "molt_runtime.wasm").write_bytes(b"old-runtime")

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: False)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_optimize_split_app_module", lambda data, **_: data)
    monkeypatch.setattr(
        wasm_link, "_collect_module_imports", lambda *_args, **_kwargs: set()
    )
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )
    monkeypatch.setattr(wasm_link, "_collect_custom_names", lambda _data: [])
    monkeypatch.setattr(wasm_link, "_collect_imports", lambda _data: [])
    monkeypatch.setattr(
        wasm_link, "_collect_exports", lambda _data: {"molt_memory", "molt_table"}
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
    )

    assert rc == 1
    assert linked.read_bytes() == b"old-linked"
    assert (split_dir / "app.wasm").read_bytes() == b"old-app"
    assert (split_dir / "molt_runtime.wasm").read_bytes() == b"old-runtime"


def test_run_wasm_ld_preserves_old_output_if_linked_validation_fails(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _module_with_linking_symbols([])
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    linked.write_bytes(b"old-linked")

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: False)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_collect_custom_names", lambda _data: [])
    monkeypatch.setattr(wasm_link, "_collect_imports", lambda _data: [])
    monkeypatch.setattr(
        wasm_link, "_collect_exports", lambda _data: {"molt_memory", "molt_table"}
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 1
    assert linked.read_bytes() == b"old-linked"


def test_run_wasm_ld_split_runtime_publishes_only_after_staged_validation(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _module_with_linking_symbols([])
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    split_dir.mkdir()
    linked.write_bytes(b"old-linked")
    app_wasm = split_dir / "app.wasm"
    rt_wasm = split_dir / "molt_runtime.wasm"
    app_wasm.write_bytes(b"old-app")
    rt_wasm.write_bytes(b"old-runtime")
    validate_seen: list[Path] = []
    split_validate_seen: list[tuple[Path, Path]] = []

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    def validate_linked(path: Path) -> bool:
        validate_seen.append(path)
        assert path != linked
        assert linked.read_bytes() == b"old-linked"
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return True

    def validate_split(app_stage: Path, rt_stage: Path) -> bool:
        split_validate_seen.append((app_stage, rt_stage))
        assert app_stage != app_wasm
        assert rt_stage != rt_wasm
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return True

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", validate_linked)
    monkeypatch.setattr(wasm_link, "_validate_split_runtime_outputs", validate_split)
    monkeypatch.setattr(
        wasm_link,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_optimize_split_app_module", lambda data, **_: data)
    monkeypatch.setattr(
        wasm_link, "_collect_module_imports", lambda *_args, **_kwargs: set()
    )
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )
    monkeypatch.setattr(wasm_link, "_collect_custom_names", lambda _data: [])
    monkeypatch.setattr(wasm_link, "_collect_imports", lambda _data: [])
    monkeypatch.setattr(
        wasm_link, "_collect_exports", lambda _data: {"molt_memory", "molt_table"}
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
    )

    assert rc == 0
    assert validate_seen
    assert split_validate_seen
    assert linked.read_bytes() != b"old-linked"
    expected_app = wasm_link.strip_wasm_publication_sections(
        output_bytes,
        final_artifact=True,
        preserve_debug=False,
    )
    expected_runtime = wasm_link.strip_wasm_publication_sections(
        runtime_bytes,
        final_artifact=True,
        preserve_debug=False,
    )
    assert (
        wasm_link.strip_wasm_publication_sections(
            app_wasm.read_bytes(), final_artifact=True, preserve_debug=False
        )
        == expected_app
    )
    assert (
        wasm_link.strip_wasm_publication_sections(
            rt_wasm.read_bytes(), final_artifact=True, preserve_debug=False
        )
        == expected_runtime
    )


def test_wasm_link_allows_ref_null_element_expr() -> None:
    write_varuint = wasm_link._write_varuint
    payload = bytearray()
    payload.extend(write_varuint(1))
    payload.extend(write_varuint(0x04))
    payload.extend(b"\x41\x00\x0b")
    payload.append(0x70)
    payload.extend(write_varuint(1))
    payload.append(0xD0)  # ref.null
    payload.append(0x70)  # funcref
    payload.append(0x0B)
    data = _build_minimal_module(bytes(payload))
    ok, err = wasm_link._validate_elements(data)
    assert ok, err


def test_strip_internal_exports_preserves_user_module_exports() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(2) + write_varuint(0) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    table_ref = "__molt_table_ref_7"
    export_payload.extend(wasm_link._write_string(table_ref))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("main_molt__ocr_tokens"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(1))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    for _ in range(2):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._strip_internal_exports(
        data, preserve_exports={"main_molt__ocr_tokens"}
    )
    exports = wasm_link._collect_function_exports(updated or data)
    assert table_ref not in exports
    assert "molt_main" in exports
    assert "main_molt__ocr_tokens" in exports


def test_strip_internal_exports_keeps_linked_host_call_helpers() -> None:
    data = _build_exported_runtime_module_many(
        [
            "molt_main",
            "molt_scratch_alloc",
            "molt_scratch_free",
            "molt_bytes_from_bytes",
            "molt_string_from_bytes",
            "molt_list_builder_new",
            "molt_list_builder_append",
            "molt_list_builder_finish",
            "molt_object_repr",
            "dead_internal_export",
        ]
    )
    updated = wasm_link._strip_internal_exports(data)
    exports = wasm_link._collect_function_exports(updated or data)
    assert "molt_scratch_alloc" in exports
    assert "molt_scratch_free" in exports
    assert "molt_bytes_from_bytes" in exports
    assert "molt_string_from_bytes" in exports
    assert "molt_list_builder_new" in exports
    assert "molt_list_builder_append" in exports
    assert "molt_list_builder_finish" in exports
    assert "molt_object_repr" in exports
    assert "dead_internal_export" not in exports


def test_strip_internal_exports_dedupes_duplicate_export_names() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(2) + write_varuint(0) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(4))
    table_ref = "__molt_table_ref_7"
    for name, index in (
        (table_ref, 0),
        (table_ref, 1),
        ("molt_main", 0),
        ("molt_main", 1),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    for _ in range(2):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._strip_internal_exports(data)
    exports = wasm_link._collect_function_exports(updated or data)
    assert table_ref not in exports
    assert list(name for name in exports if name == "molt_main") == ["molt_main"]


def test_required_linked_table_min_respects_final_active_elements() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("__indirect_function_table"))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(10))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    element_payload = (
        write_varuint(1)
        + b"\x00\x41"
        + write_varuint(20)
        + b"\x0b"
        + write_varuint(1)
        + write_varuint(0)
    )
    sections.append((9, element_payload))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)

    facts = {
        "callable_table_entries": [[20, 0, 0, 0]]
    }
    assert wasm_link._table_import_min(data) == 10
    assert wasm_link._required_linked_table_min(data, 5, facts) == 21
    updated = wasm_link._rewrite_table_import_min(
        data, wasm_link._required_linked_table_min(data, 5, facts)
    )
    assert updated is not None
    assert wasm_link._table_import_min(updated) == 21


def test_neutralize_dead_element_entries_skips_modules_with_call_indirect() -> None:
    write_varuint = wasm_link._write_varuint
    sections = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, func_payload))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    sections.append((9, bytes(element_payload)))

    code_payload = bytearray()
    body = bytearray()
    body.extend(write_varuint(0))  # local decl count
    body.extend(b"\x41\x00")  # i32.const 0
    body.extend(b"\x11\x00\x00")  # call_indirect type 0 table 0
    body.append(0x0B)  # end
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    facts = {
        "root_function_indices": [],
        "element_function_indices": [0],
        "declared_function_indices": [],
        "function_references": [],
        "active_function_elements": [[0, 0, 0]],
        "reachable_dynamic_dispatch": True,
        "exported_table_indices": [],
        "table_mutations": [],
    }
    assert wasm_link._neutralize_dead_element_entries(data, facts) is None


def test_dedup_data_segments_stops_scrub_at_path_extension_boundary() -> None:
    write_varuint = wasm_link._write_varuint
    sections = []

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    path_and_adjacent = b"/Users/alice/project/tmp/class_method_probe.pyf__name__hi"
    second_segment = b"keep-me"

    data_payload = bytearray()
    data_payload.extend(write_varuint(2))
    for offset, raw in ((0, path_and_adjacent), (128, second_segment)):
        data_payload.append(0x00)
        data_payload.extend(b"\x41")
        data_payload.extend(write_varuint(offset))
        data_payload.extend(b"\x0b")
        data_payload.extend(write_varuint(len(raw)))
        data_payload.extend(raw)
    sections.append((11, bytes(data_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._dedup_data_segments(data)
    assert updated is not None

    segs = _parse_data_segments(updated)
    assert segs[0].endswith(b"f__name__hi")
    assert b"/Users/" not in segs[0]
    assert segs[1] == second_segment


# ---------------------------------------------------------------------------
# Allowlist validation
# ---------------------------------------------------------------------------


def _parse_allowlist(path: Path) -> set[str]:
    lines = path.read_text().splitlines()
    return {
        line.strip()
        for line in lines
        if line.strip() and not line.strip().startswith("#")
    }


def test_allowlist_file_exists():
    """The WASI allowlist must exist and contain the expected symbols."""
    allowlist = (
        Path(__file__).resolve().parents[1] / "tools" / "wasm_allowed_imports.txt"
    )
    assert allowlist.exists(), f"Missing allowlist: {allowlist}"
    symbols = _parse_allowlist(allowlist)
    from molt._wasm_abi_generated import WASM_CALL_INDIRECT_IMPORTS

    # Must contain core WASI symbols
    assert "fd_write" in symbols
    assert "proc_exit" in symbols
    assert "__indirect_function_table" in symbols
    # Must contain indirect call trampolines
    assert set(WASM_CALL_INDIRECT_IMPORTS) <= symbols
    # Must NOT contain molt_runtime namespace symbols (those are resolved by linking),
    # except for serialization/compression builtins that are direct WASM imports.
    _ALLOWED_MOLT_PREFIXES = (
        "molt_call_indirect",
        "molt_cbor_",
        "molt_msgpack_",
        "molt_deflate_",
        "molt_inflate_",
    )
    runtime_syms = {
        s
        for s in symbols
        if s.startswith("molt_")
        and not any(s.startswith(p) for p in _ALLOWED_MOLT_PREFIXES)
    }
    assert runtime_syms == set(), (
        f"Unexpected molt_runtime symbols in allowlist: {runtime_syms}"
    )


def test_native_object_link_allowlist_includes_generated_external_imports(tmp_path):
    base = tmp_path / "base_allowlist.txt"
    base.write_text("fd_write\n", encoding="utf-8")
    native = tmp_path / "extension.molt.wasm"
    native.write_bytes(b"\0asm\x01\0\0\0")

    with tempfile.TemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        assert (
            wasm_link._compose_wasm_ld_allowlist(
                base_allowlist=base,
                native_objects=(),
                temp_dir=temp_dir,
            )
            == base
        )

        composed = wasm_link._compose_wasm_ld_allowlist(
            base_allowlist=base,
            native_objects=(native,),
            temp_dir=temp_dir,
        )

        symbols = _parse_allowlist(composed)
        assert "fd_write" in symbols
        assert "__cpp_exception" in symbols
        assert "malloc" in symbols
        assert "__trunctfdf2" not in symbols
        assert "__cpp_exception" not in _parse_allowlist(base)


def test_resolve_native_link_inputs_adds_compiler_rt_provider(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native = tmp_path / "native.molt.wasm"
    provider = tmp_path / "rustlib" / "wasm32-wasip1" / "libcompiler_builtins-x.rlib"
    native.write_bytes(_build_env_function_import_module(["__trunctfdf2", "malloc"]))
    provider.parent.mkdir(parents=True)
    provider.write_bytes(b"!<arch>\ncompiler-rt")

    monkeypatch.setattr(
        wasm_link.wasm_toolchain,
        "wasm_compiler_builtins_archive",
        lambda: provider,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: frozenset({"__trunctfdf2"}),
        raising=True,
    )

    inputs = wasm_link._resolve_native_link_inputs((native,))

    assert inputs == (native, provider)


def test_resolve_native_link_inputs_rejects_missing_compiler_rt_provider(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native = tmp_path / "native.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["__trunctfdf2"]))

    monkeypatch.setattr(
        wasm_link.wasm_toolchain,
        "wasm_compiler_builtins_archive",
        lambda: None,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: frozenset({"__trunctfdf2"}),
        raising=True,
    )

    with pytest.raises(ValueError, match="wasm_compiler_rt_link_import"):
        wasm_link._resolve_native_link_inputs((native,))


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
# addresses. See tools/wasm_link.py::_split_runtime_data_alias_object.


def _build_data_address_export_runtime(addresses: dict[str, int]) -> bytes:
    """Synthetic deploy runtime exporting each data symbol as an immutable i32
    global whose init value is the symbol's linear-memory address."""
    write_varuint = wasm_link._write_varuint
    names = list(addresses)
    sections: list[tuple[int, bytes]] = []

    global_payload = bytearray()
    global_payload.extend(write_varuint(len(names)))
    for name in names:
        global_payload.append(0x7F)  # i32
        global_payload.append(0x00)  # immutable
        global_payload.append(0x41)  # i32.const
        global_payload.extend(write_varuint(addresses[name]))
        global_payload.append(0x0B)  # end
    sections.append((6, bytes(global_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(len(names)))
    for index, name in enumerate(names):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x03)  # global export
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    return wasm_link._build_sections(sections)


def _add_data_address_global_exports(module: bytes, addresses: dict[str, int]) -> bytes:
    """Return *module* with an added i32 global (init = address) exported under
    each name â€” the wasm-ld shape for --export-if-defined of a defined data
    symbol. Appends to any existing global/export sections."""
    write_varuint = wasm_link._write_varuint
    sections = wasm_link._parse_sections(module)

    existing_global_count = 0
    for section_id, payload in sections:
        if section_id == 6:
            existing_global_count, _ = wasm_link._read_varuint(payload, 0)
            break

    names = list(addresses)
    new_globals = bytearray()
    for name in names:
        new_globals.append(0x7F)  # i32
        new_globals.append(0x00)  # immutable
        new_globals.append(0x41)  # i32.const
        new_globals.extend(write_varuint(addresses[name]))
        new_globals.append(0x0B)  # end

    new_export_entries = bytearray()
    for offset, name in enumerate(names):
        new_export_entries.extend(wasm_link._write_string(name))
        new_export_entries.append(0x03)  # global export
        new_export_entries.extend(write_varuint(existing_global_count + offset))

    rebuilt: list[tuple[int, bytes]] = []
    saw_global = False
    saw_export = False
    for section_id, payload in sections:
        if section_id == 6:
            saw_global = True
            count, offset = wasm_link._read_varuint(payload, 0)
            merged = bytearray()
            merged.extend(write_varuint(count + len(names)))
            merged.extend(payload[offset:])
            merged.extend(new_globals)
            rebuilt.append((6, bytes(merged)))
        elif section_id == 7:
            saw_export = True
            count, offset = wasm_link._read_varuint(payload, 0)
            merged = bytearray()
            merged.extend(write_varuint(count + len(names)))
            merged.extend(payload[offset:])
            merged.extend(new_export_entries)
            rebuilt.append((7, bytes(merged)))
        else:
            rebuilt.append((section_id, payload))
    if not saw_global:
        global_section = bytearray()
        global_section.extend(write_varuint(len(names)))
        global_section.extend(new_globals)
        rebuilt.append((6, bytes(global_section)))
    if not saw_export:
        export_section = bytearray()
        export_section.extend(write_varuint(len(names)))
        export_section.extend(new_export_entries)
        rebuilt.append((7, bytes(export_section)))
    return wasm_link._build_sections(rebuilt)


def _build_undefined_data_symbol_object(names: list[str]) -> bytes:
    """Synthetic relocatable native object with the given undefined data
    symbols in its linking symtab (the shape numpy's *.molt.wasm carries)."""
    symbol_entries: list[bytes] = []
    for name in names:
        entry = bytearray()
        entry.append(wasm_link._SYMBOL_KIND_DATA)
        entry.extend(
            wasm_link._write_varuint(
                wasm_link.FLAG_EXPLICIT_NAME | wasm_link.FLAG_UNDEFINED
            )
        )
        entry.extend(wasm_link._write_string(name))
        symbol_entries.append(bytes(entry))
    symbol_payload = wasm_link._write_varuint(len(symbol_entries)) + b"".join(
        symbol_entries
    )
    linking = wasm_link._build_custom_section(
        "linking",
        wasm_link._build_linking_payload(
            2, [(wasm_link.SYMTAB_SUBSECTION_ID, symbol_payload)]
        ),
    )
    return wasm_link._build_sections([(0, linking)])


def _alias_segment_addresses(alias_bytes: bytes) -> dict[int, int]:
    """Map data-segment index -> i32.const address for an alias object."""
    addresses: dict[int, int] = {}
    for section_id, payload in wasm_link._parse_sections(alias_bytes):
        if section_id != 11:
            continue
        offset = 0
        count, offset = wasm_link._read_varuint(payload, offset)
        for index in range(count):
            flags, offset = wasm_link._read_varuint(payload, offset)
            if flags == 2:
                _memory_index, offset = wasm_link._read_varuint(payload, offset)
            address, offset = wasm_link._read_const_i32_init_expr(payload, offset)
            size, offset = wasm_link._read_varuint(payload, offset)
            offset += size
            addresses[index] = address
    return addresses


def test_runtime_exported_data_symbol_addresses_reads_global_exports() -> None:
    runtime = _build_data_address_export_runtime(
        {"Py_None": 0x2E1688, "Py_False": 0x2E1680, "PyExc_ValueError": 0x2E1500}
    )
    addresses = wasm_link._runtime_exported_data_symbol_addresses(runtime)
    assert addresses == {
        "Py_None": 0x2E1688,
        "Py_False": 0x2E1680,
        "PyExc_ValueError": 0x2E1500,
    }


def test_split_runtime_data_alias_points_at_deploy_runtime_addresses(
    tmp_path: Path,
) -> None:
    # A real CPython-ABI data-symbol subset numpy references undefined.
    names = ["Py_None", "PyExc_ValueError", "PyList_Type"]
    deploy_addresses = {
        "Py_None": 0x2E1680,
        "PyExc_ValueError": 0x2E1400,
        "PyList_Type": 0x2E0000,
    }
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    deploy_runtime.write_bytes(_build_data_address_export_runtime(deploy_addresses))
    native = tmp_path / "ext.molt.wasm"
    native.write_bytes(_build_undefined_data_symbol_object(names))

    # Precondition: the object surfaces exactly these undefined data symbols.
    assert set(wasm_link._undefined_cpython_abi_data_symbols([native])) == set(names)

    with tempfile.TemporaryDirectory() as tmp:
        temp_dir = type("_TD", (), {"name": tmp})()
        alias_path = wasm_link._split_runtime_data_alias_object(
            native_objects=[native],
            deploy_runtime=deploy_runtime,
            temp_dir=temp_dir,
        )
        assert alias_path is not None
        alias_bytes = Path(alias_path).read_bytes()

    # Each aliased data symbol must resolve to the deploy runtime's address.
    # The alias emits symtab entries and data segments in the same order, so the
    # Nth defined symbol owns the Nth segment.
    ordered_names = [
        name
        for name, _offset, _size in wasm_link._iter_linking_data_symbols(
            alias_bytes, undefined=False
        )
    ]
    seg_addresses = _alias_segment_addresses(alias_bytes)
    assert set(ordered_names) == set(names)
    for index, name in enumerate(ordered_names):
        assert seg_addresses[index] == deploy_addresses[name], name


def test_split_runtime_data_alias_fails_loud_on_missing_deploy_export(
    tmp_path: Path,
) -> None:
    # Deploy runtime is missing an address global for PyExc_ValueError -> must raise
    # rather than silently emit a wrong/zero address (M34: degrade loudly).
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"Py_None": 0x2E1680})
    )
    native = tmp_path / "ext.molt.wasm"
    native.write_bytes(
        _build_undefined_data_symbol_object(["Py_None", "PyExc_ValueError"])
    )

    with tempfile.TemporaryDirectory() as tmp:
        temp_dir = type("_TD", (), {"name": tmp})()
        with pytest.raises(ValueError, match="PyExc_ValueError"):
            wasm_link._split_runtime_data_alias_object(
                native_objects=[native],
                deploy_runtime=deploy_runtime,
                temp_dir=temp_dir,
            )


# --- Final callable-table attestation -------------------------------------


def _callable_entry(slot: int) -> wasm_link.CallableTableEntry:
    return wasm_link.CallableTableEntry(
        slot=slot,
        func_index=slot,
        type_index=0,
        params=(),
        results=(),
    )


def test_split_callable_table_ownership_accepts_disjoint_contiguous_publication() -> (
    None
):
    layout = wasm_link.CallableTableLayout(10, 2, 20, 2)
    wasm_link._validate_split_callable_table_ownership(
        (_callable_entry(20), _callable_entry(21)),
        (_callable_entry(10), _callable_entry(11), _callable_entry(15)),
        layout,
    )


@pytest.mark.parametrize(
    ("app_slots", "runtime_slots", "message"),
    [
        ((20,), (10, 11), "not contiguous"),
        ((20, 21), (10, 20), "overlaps at slot 20"),
        ((20, 21), (10, 11, 22), "reaches finalized app base"),
        ((20, 21), (10,), "fixed prefix is incomplete"),
        ((20, 21), (9, 10, 11), "fixed prefix base is not the runtime base"),
    ],
)
def test_split_callable_table_ownership_rejects_drift(
    app_slots: tuple[int, ...],
    runtime_slots: tuple[int, ...],
    message: str,
) -> None:
    layout = wasm_link.CallableTableLayout(10, 2, 20, 2)
    with pytest.raises(ValueError, match=message):
        wasm_link._validate_split_callable_table_ownership(
            tuple(_callable_entry(slot) for slot in app_slots),
            tuple(_callable_entry(slot) for slot in runtime_slots),
            layout,
        )


def test_split_callable_table_ownership_rejects_huge_app_span_in_bounded_space() -> (
    None
):
    import tracemalloc

    layout = wasm_link.CallableTableLayout(0, 0, 0, 0xFFFF_FFFF)
    tracemalloc.start()
    try:
        with pytest.raises(ValueError, match="missing app slot 1"):
            wasm_link._validate_split_callable_table_ownership(
                (_callable_entry(0),),
                (),
                layout,
            )
        _current, peak = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    assert peak < 1_000_000


def test_split_callable_table_ownership_rejects_huge_fixed_span_in_bounded_space() -> (
    None
):
    import tracemalloc

    layout = wasm_link.CallableTableLayout(0, 0xFFFF_FFFE, 0xFFFF_FFFE, 1)
    tracemalloc.start()
    try:
        with pytest.raises(ValueError, match="missing slot 1"):
            wasm_link._validate_split_callable_table_ownership(
                (_callable_entry(0xFFFF_FFFE),),
                (_callable_entry(0),),
                layout,
            )
        _current, peak = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    assert peak < 1_000_000


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
        assert f"--export-if-defined={name}" in args, name
