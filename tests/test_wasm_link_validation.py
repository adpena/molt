import hashlib
import importlib.machinery
import importlib.util
import tempfile
from pathlib import Path

import pytest
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


def test_verify_runtime_integrity_accepts_matching_sidecar_hash(tmp_path: Path) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    sidecar = runtime.with_name(f"{runtime.name}.sha256")
    sidecar.write_text(hashlib.sha256(runtime.read_bytes()).hexdigest() + "\n")

    wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_retries_stale_sidecar_publish_window(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    digest = hashlib.sha256(runtime.read_bytes()).hexdigest()
    reads: list[Path] = []

    def read_sidecar(path: Path) -> str:
        reads.append(path)
        return "0" * 64 if len(reads) == 1 else digest

    monkeypatch.setattr(
        wasm_link,
        "_RUNTIME_INTEGRITY_PAIR_ATTEMPTS",
        2,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link,
        "_read_runtime_integrity_sidecar",
        read_sidecar,
        raising=True,
    )
    monkeypatch.setattr(wasm_link.time, "sleep", lambda _delay: None, raising=True)

    wasm_link._verify_runtime_integrity(runtime)

    assert reads == [runtime, runtime]


def test_verify_runtime_integrity_rejects_mismatched_sidecar_hash(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    sidecar = runtime.with_name(f"{runtime.name}.sha256")
    sidecar.write_text("0" * 64 + "\n")

    with pytest.raises(SystemExit, match="sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_env_cannot_bypass_mismatched_sidecar_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    sidecar = runtime.with_name(f"{runtime.name}.sha256")
    sidecar.write_text("0" * 64 + "\n")
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_VERIFY", "1")

    with pytest.raises(SystemExit, match="sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_rejects_missing_sidecar(tmp_path: Path) -> None:
    runtime = tmp_path / "custom_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))

    with pytest.raises(SystemExit, match="publish the matching .sha256 sidecar"):
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

    # Element section.
    sections.append((9, element_payload))

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
        code_payload.extend(write_varuint(2))
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


def _build_runtime_import_data_module(
    import_names: list[str], *, memory_min: int, data_offset: int
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
    import_payload.extend(write_varuint(len(import_names) + 1))
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


def test_stub_dead_functions_preserves_start_root_reachability() -> None:
    module = _build_start_root_module()
    assert wasm_link._stub_dead_functions(module) is None


def test_tree_shake_runtime_preserves_required_function_exports() -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    shaken = wasm_link._tree_shake_runtime(module, {"exception_pending"})
    exports = wasm_link._collect_function_exports(shaken)
    assert "molt_exception_pending" in exports


def test_build_runtime_stub_publishes_exported_functions_as_link_symbols() -> None:
    module = _build_exported_runtime_module_many(["molt_err_pending", "molt_none"])

    stub = wasm_link._build_runtime_stub(module)

    symbols = {
        name: (flags, index)
        for flags, index, name, _ in wasm_link._collect_linking_function_symbols(stub)
    }
    assert symbols["molt_err_pending"] == (
        wasm_link.FLAG_BINDING_GLOBAL
        | wasm_link.FLAG_EXPLICIT_NAME
        | wasm_link.FLAG_EXPORTED,
        0,
    )
    assert symbols["molt_none"] == (
        wasm_link.FLAG_BINDING_GLOBAL
        | wasm_link.FLAG_EXPLICIT_NAME
        | wasm_link.FLAG_EXPORTED,
        1,
    )


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
    shaken = wasm_link._tree_shake_runtime(module, {"exception_pending"})
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


def test_validate_split_runtime_outputs_requires_shared_app_memory(
    tmp_path: Path,
    capsys,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))

    app.write_bytes(_build_runtime_import_module(["molt_err_pending"], memory_min=1))
    assert wasm_link._validate_split_runtime_outputs(app, runtime)

    app.write_bytes(_build_defined_memory_module(1))
    assert not wasm_link._validate_split_runtime_outputs(app, runtime)
    captured = capsys.readouterr()
    assert "Split-runtime app must import env.memory" in captured.err


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
    shaken = wasm_link._tree_shake_runtime(module, {"exception_pending"})
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

    def fake_final_optimize(path: Path, level: str = "Oz") -> bool:
        assert level == "Oz"
        path.write_bytes(final_runtime)
        return True

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(wasm_link.shutil, "which", lambda _name: "/usr/bin/wasm-opt")
    monkeypatch.setattr(wasm_link, "_wasm_opt_version", lambda _path: "wasm-opt 1.0")
    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_run_wasm_opt_via_optimize", fake_final_optimize)

    first = wasm_link._tree_shake_runtime(module, {"exception_pending"})

    assert first == final_runtime
    assert calls["count"] == 1

    monkeypatch.setattr(
        wasm_link,
        "_run_external_tool",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("wasm-opt should not rerun for cached tree-shake output")
        ),
    )

    second = wasm_link._tree_shake_runtime(module, {"exception_pending"})

    assert second == final_runtime


def test_run_wasm_ld_split_runtime_falls_back_when_env_deploy_runtime_is_stale(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    stale_runtime = tmp_path / "missing-runtime.wasm"

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setenv("MOLT_WASM_DEPLOY_RUNTIME", str(stale_runtime))
    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)
    monkeypatch.setattr(
        wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes
    )

    rc = wasm_link._run_wasm_ld(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
    )

    assert rc == 0
    assert (split_dir / "molt_runtime.wasm").read_bytes() == runtime.read_bytes()


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

    rc = wasm_link._run_wasm_ld("wasm-ld", runtime, output, linked)

    assert rc == 0
    assert str(reloc_runtime) in wasm_ld_inputs
    assert str(runtime) not in wasm_ld_inputs


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

    rc = wasm_link._run_wasm_ld(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_objects=(native_object,),
    )

    assert rc == 0
    output_index = wasm_ld_inputs.index("-o") + 2
    assert wasm_ld_inputs[output_index + 2] == str(native_object)


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

    rc = wasm_link._run_wasm_ld(
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

    rc = wasm_link._run_wasm_ld(
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


def test_run_wasm_ld_split_runtime_links_native_objects_into_app(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _module_with_linking_symbols([])
    app_data_offset = 2 * 65536
    output_bytes = _build_runtime_import_data_module(
        [], memory_min=37, data_offset=app_data_offset
    )
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.o"
    link_calls: list[list[str]] = []
    app_link_bytes = _build_runtime_import_module([], memory_min=2)

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(b"\x00asm\x01\x00\x00\x00native-object")

    def fake_run(cmd, **kwargs):
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
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
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_build_runtime_stub", lambda data: runtime_bytes)
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

    rc = wasm_link._run_wasm_ld(
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
    assert str(native_object) in monolithic_cmd
    assert str(native_object) in split_app_cmd
    assert str(runtime) not in split_app_cmd
    assert "--stack-first" in monolithic_cmd
    assert "--import-memory" in split_app_cmd
    assert "--no-stack-first" in split_app_cmd
    assert "--stack-first" not in split_app_cmd
    assert f"--global-base={app_data_offset}" in split_app_cmd
    assert any("molt_runtime_stub" in part for part in monolithic_cmd)
    assert not any("molt_runtime_stub" in part for part in split_app_cmd)
    app_wasm = (split_dir / "app.wasm").read_bytes()
    assert wasm_link._memory_import_min(app_wasm) == 37
    assert _defined_memory_min(app_wasm) is None


def test_run_wasm_ld_split_runtime_uses_stub_and_deploy_import_namespaces(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _build_exported_runtime_module_many(["molt_err_pending"])
    output_bytes = _build_runtime_import_module(["molt_err_pending"])
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.molt.wasm"
    link_calls: list[list[str]] = []
    stub_app_imports: list[list[tuple[str, str]]] = []
    deployed_native_imports: list[list[tuple[str, str]]] = []
    allowlists: list[set[str]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(
        _build_env_function_import_module(
            ["molt_err_pending", "malloc", "__trunctfdf2"]
        )
    )
    compiler_rt_provider = tmp_path / "rustlib" / "libcompiler_builtins-x.rlib"
    compiler_rt_provider.parent.mkdir()
    compiler_rt_provider.write_bytes(b"!<arch>\ncompiler-rt")

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            link_calls.append(list(cmd))
            for part in cmd:
                path = Path(part)
                if path.name == "output_stub_link_imports.wasm":
                    stub_app_imports.append(_function_import_pairs(path.read_bytes()))
                if path.name.startswith("native_runtime_imports_"):
                    deployed_native_imports.append(
                        _function_import_pairs(path.read_bytes())
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
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_build_runtime_stub", lambda data: runtime_bytes)
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

    rc = wasm_link._run_wasm_ld(
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
    assert str(native_object) in monolithic_cmd
    assert str(compiler_rt_provider) in monolithic_cmd
    assert str(native_object) not in split_app_cmd
    assert str(compiler_rt_provider) in split_app_cmd
    assert "--import-memory" not in monolithic_cmd
    assert "--import-memory" in split_app_cmd
    assert "--stack-first" in monolithic_cmd
    assert "--no-stack-first" in split_app_cmd
    assert "--stack-first" not in split_app_cmd
    assert "--global-base=67108864" in split_app_cmd
    assert any(
        Path(part).name == "output_stub_link_imports.wasm" for part in monolithic_cmd
    )
    assert not any(
        Path(part).name.startswith("native_runtime_imports_") for part in monolithic_cmd
    )
    assert any(
        Path(part).name.startswith("native_runtime_imports_") for part in split_app_cmd
    )
    assert stub_app_imports == [[("env", "molt_err_pending")]]
    assert deployed_native_imports == [
        [
            ("molt_runtime", "molt_err_pending"),
            ("env", "malloc"),
            ("env", "__trunctfdf2"),
        ]
    ]
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

    shaken = wasm_link._tree_shake_runtime(module, {"exception_pending"})

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
            ):
                seen["input_path"] = input_path
                seen["level"] = level
                seen["converge"] = converge
                seen["required_exports"] = set(required_exports)
                output_path.write_bytes(input_path.read_bytes())
                return {
                    "ok": True,
                    "output_bytes": output_path.stat().st_size,
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


def test_neutralize_dead_element_entries_preserves_host_call_indirect_modules() -> None:
    module = _build_host_call_indirect_module()
    assert wasm_link._neutralize_dead_element_entries(module) is None


def test_import_walkers_handle_tag_imports_before_host_call_indirect() -> None:
    module = _build_tag_then_host_call_indirect_import_module()
    sections = wasm_link._parse_sections(module)

    assert wasm_link._count_func_imports(sections) == 1
    assert wasm_link._neutralize_dead_element_entries(module) is None


def test_strip_unused_module_function_imports_remaps_indices() -> None:
    module = _build_runtime_import_strip_module()

    stripped = wasm_link._strip_unused_module_function_imports(
        module,
        module_name="molt_runtime",
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

    rewritten = wasm_link._rewrite_output_imports(
        output,
        {"molt_socket_drop", "molt_alloc"},
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
    app.write_bytes(_build_runtime_import_module(["socket_drop", "unknown_probe"]))
    runtime.write_bytes(_build_exported_runtime_module("molt_socket_drop"))

    assert not wasm_link._validate_split_runtime_outputs(app, runtime)

    app.write_bytes(_build_runtime_import_module(["socket_drop"]))
    assert wasm_link._validate_split_runtime_outputs(app, runtime)


def test_post_link_optimize_split_app_does_not_preserve_all_reference_exports() -> None:
    table_ref = wasm_link.table_ref_export_name(7)
    module = _build_exported_runtime_module_many(
        ["dead_user_export", "molt_main", table_ref]
    )

    default_optimized = wasm_link._post_link_optimize(
        module,
        reference_data=module,
    )
    assert "dead_user_export" in wasm_link._collect_exports(default_optimized)

    split_app_optimized = wasm_link._post_link_optimize(
        module,
        reference_data=module,
        preserve_exports=wasm_link._split_app_reference_function_exports(module),
        preserve_reference_exports=False,
    )
    split_exports = wasm_link._collect_exports(split_app_optimized)
    assert "dead_user_export" not in split_exports
    assert "molt_main" in split_exports
    assert table_ref in split_exports


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
        updated = wasm_link._inject_output_export_aliases(output, temp_dir)
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
        updated = wasm_link._inject_output_export_aliases(output, temp_dir)
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
    monkeypatch.setattr(
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)

    rc = wasm_link._run_wasm_ld("wasm-ld", runtime, output, linked)

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
        updated = wasm_link._inject_output_export_aliases(output, temp_dir)
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

    specs = wasm_link._collect_output_wrapper_specs(wasm_link._build_sections(sections))
    kept = {name for name, _alias, _type_idx, _func_idx in specs}
    assert "main_molt__init" in kept
    assert "molt_isolate_import" in kept
    assert "main_molt__molt_module_chunk_1" not in kept
    assert "__future_____Feature___init__" not in kept


def test_split_app_reference_function_exports_preserves_public_and_isolate_exports() -> (
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
        ("molt_isolate_import", 2),
        ("molt_isolate_bootstrap", 3),
        ("molt_main", 4),
        ("molt_set_wasm_table_base", 5),
    ):
        export_payload.extend(wasm_link._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(6))
    for _ in range(6):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x42)
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    keep = wasm_link._split_app_reference_function_exports(
        wasm_link._build_sections(sections)
    )
    assert "main_molt__init" in keep
    assert "main_molt__ocr_tokens" in keep
    assert "molt_isolate_import" in keep
    assert "molt_isolate_bootstrap" in keep
    assert "molt_main" in keep
    assert "molt_set_wasm_table_base" in keep


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
    assert wasm_link._entry_module_prefix_from_main_init(module, exports) == "main_molt"


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
    specs = wasm_link._collect_output_wrapper_specs(module)
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
    monkeypatch.setattr(
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))

    rc = wasm_link._run_wasm_ld(
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
    monkeypatch.setattr(
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))

    rc = wasm_link._run_wasm_ld("wasm-ld", runtime, output, linked)

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
    monkeypatch.setattr(
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_build_runtime_stub", lambda data: runtime_bytes)
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

    rc = wasm_link._run_wasm_ld(
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
    monkeypatch.setattr(
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_collect_custom_names", lambda _data: [])
    monkeypatch.setattr(wasm_link, "_collect_imports", lambda _data: [])
    monkeypatch.setattr(
        wasm_link, "_collect_exports", lambda _data: {"molt_memory", "molt_table"}
    )
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))

    rc = wasm_link._run_wasm_ld("wasm-ld", runtime, output, linked)

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
        wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_build_runtime_stub", lambda data: runtime_bytes)
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

    rc = wasm_link._run_wasm_ld(
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
    assert app_wasm.read_bytes() == output_bytes
    assert rt_wasm.read_bytes() == runtime_bytes


def test_run_wasm_ld_fails_when_ref_func_declaration_cannot_be_materialized(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime_bytes = _build_exported_runtime_module("molt_main")
    output_bytes = _build_exported_runtime_module("molt_main")
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **_kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link, "_scan_code_ref_funcs", lambda _data: {0})
    monkeypatch.setattr(
        wasm_link,
        "_declare_ref_func_elements",
        lambda _data: (_ for _ in ()).throw(ValueError("boom")),
    )

    rc = wasm_link._run_wasm_ld("wasm-ld", runtime, output, linked)

    assert rc == 1
    captured = capsys.readouterr()
    assert "Failed to declare ref.func elements: boom" in captured.err


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


def test_append_table_ref_elements_tolerates_malformed_name_utf8() -> None:
    write_varuint = wasm_link._write_varuint
    data = _build_minimal_module(write_varuint(0))
    sections = wasm_link._parse_sections(data)

    func_name_subsection = bytearray()
    func_name_subsection.extend(write_varuint(1))  # one function-name mapping
    func_name_subsection.extend(write_varuint(0))  # func index
    func_name_subsection.extend(write_varuint(1))  # name length
    func_name_subsection.extend(b"\x97")  # invalid UTF-8 byte

    custom_name_payload = bytearray()
    custom_name_payload.extend(wasm_link._write_string("name"))
    custom_name_payload.append(1)  # function names subsection
    custom_name_payload.extend(write_varuint(len(func_name_subsection)))
    custom_name_payload.extend(func_name_subsection)

    sections.insert(0, (0, bytes(custom_name_payload)))
    malformed = wasm_link._build_sections(sections)

    # Malformed name entries should be ignored, not crash wasm linking.
    result = wasm_link._append_table_ref_elements(malformed)
    assert result is None or isinstance(result, bytes)


def test_append_table_ref_elements_uses_export_names_without_name_section() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(8))
    sections.append((4, bytes(table_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string(wasm_link.table_ref_export_name(7)))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._append_table_ref_elements(data)
    assert updated is not None
    ok, err = wasm_link._validate_elements(updated)
    assert ok, err

    element_section = next(
        payload
        for section_id, payload in wasm_link._parse_sections(updated)
        if section_id == 9
    )
    count, offset = wasm_link._read_varuint(element_section, 0)
    assert count == 1
    assert element_section[offset] == 0x00
    offset += 1
    assert element_section[offset] == 0x41
    table_offset, offset = wasm_link._read_varuint(element_section, offset + 1)
    assert table_offset == 7
    assert element_section[offset] == 0x0B


def test_append_table_ref_elements_filters_to_allowed_output_refs() -> None:
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

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(16))
    sections.append((4, bytes(table_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(2))
    ref_3 = wasm_link.table_ref_export_name(3)
    ref_9 = wasm_link.table_ref_export_name(9)
    export_payload.extend(wasm_link._write_string(ref_3))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string(ref_9))
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

    updated = wasm_link._append_table_ref_elements(
        wasm_link._build_sections(sections),
        allowed_table_indices={9},
    )
    assert updated is not None
    text_refs = wasm_link._collect_function_exports(updated)
    assert ref_3 in text_refs

    element_section = next(
        payload
        for section_id, payload in wasm_link._parse_sections(updated)
        if section_id == 9
    )
    _count, offset = wasm_link._read_varuint(element_section, 0)
    assert element_section[offset] == 0x00
    offset += 1
    assert element_section[offset] == 0x41
    table_offset, _offset = wasm_link._read_varuint(element_section, offset + 1)
    assert table_offset == 9


def test_strip_internal_exports_keeps_table_ref_exports() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(2))
    table_ref = wasm_link.table_ref_export_name(7)
    export_payload.extend(wasm_link._write_string(table_ref))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._strip_internal_exports(data)
    exports = wasm_link._collect_function_exports(updated or data)
    assert table_ref in exports
    assert "molt_main" in exports


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
    table_ref = wasm_link.table_ref_export_name(7)
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
    assert table_ref in exports
    assert "molt_main" in exports
    assert "main_molt__ocr_tokens" in exports


def test_strip_internal_exports_can_remove_linked_table_refs() -> None:
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
    table_ref = wasm_link.table_ref_export_name(7)
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

    updated = wasm_link._strip_internal_exports(
        wasm_link._build_sections(sections),
        preserve_exports={"main_molt__ocr_tokens"},
        preserve_table_refs=False,
    )
    exports = wasm_link._collect_function_exports(updated or b"")
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
    table_ref = wasm_link.table_ref_export_name(7)
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
    assert list(name for name in exports if name == table_ref) == [table_ref]
    assert list(name for name in exports if name == "molt_main") == ["molt_main"]


def test_linked_table_cleanup_preserves_table_init_body() -> None:
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
    export_payload.extend(write_varuint(2))
    export_payload.extend(wasm_link._write_string("molt_table_init"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(1))
    sections.append((7, bytes(export_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.append(0x00)
    element_payload.append(0x41)
    element_payload.extend(write_varuint(256))
    element_payload.append(0x0B)
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    sections.append((9, bytes(element_payload)))

    table_init_body = bytes([0x00, 0x41, 0x01, 0x1A, 0x0B])
    main_body = bytes([0x00, 0x10, 0x00, 0x0B])
    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    code_payload.extend(write_varuint(len(table_init_body)))
    code_payload.extend(table_init_body)
    code_payload.extend(write_varuint(len(main_body)))
    code_payload.extend(main_body)
    sections.append((10, bytes(code_payload)))

    updated = wasm_link._drop_linked_app_active_table_elements(
        wasm_link._build_sections(sections)
    )
    assert updated is not None

    assert all(section_id != 9 for section_id, _ in wasm_link._parse_sections(updated))
    code_section = next(
        payload
        for section_id, payload in wasm_link._parse_sections(updated)
        if section_id == 10
    )
    count, offset = wasm_link._read_varuint(code_section, 0)
    assert count == 2
    table_body_size, offset = wasm_link._read_varuint(code_section, offset)
    table_body = code_section[offset : offset + table_body_size]
    assert table_body == table_init_body


def test_required_linked_table_min_respects_exported_table_refs() -> None:
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

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string(wasm_link.table_ref_export_name(20)))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)

    assert wasm_link._table_import_min(data) == 10
    assert wasm_link._required_linked_table_min(data, 5) == 21
    updated = wasm_link._rewrite_table_import_min(
        data, wasm_link._required_linked_table_min(data, 5)
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
    assert wasm_link._neutralize_dead_element_entries(data) is None


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

    with pytest.raises(ValueError, match="wasm_compiler_rt_link_import"):
        wasm_link._resolve_native_link_inputs((native,))
