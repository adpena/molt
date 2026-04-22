import hashlib
import importlib.machinery
import importlib.util
import tempfile
from pathlib import Path

import pytest


def _load_wasm_link():
    root = Path(__file__).resolve().parents[1]
    path = root / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()


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


def test_verify_runtime_integrity_accepts_matching_sidecar_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    sidecar = runtime.with_name(f"{runtime.name}.sha256")
    sidecar.write_text(hashlib.sha256(runtime.read_bytes()).hexdigest() + "\n")
    monkeypatch.setattr(
        wasm_link,
        "RUNTIME_EXPECTED_HASHES",
        {"molt_runtime.wasm": "0" * 64},
        raising=True,
    )

    wasm_link._verify_runtime_integrity(runtime)


def test_verify_runtime_integrity_rejects_mismatched_sidecar_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_main"))
    sidecar = runtime.with_name(f"{runtime.name}.sha256")
    sidecar.write_text("0" * 64 + "\n")
    monkeypatch.setattr(
        wasm_link,
        "RUNTIME_EXPECTED_HASHES",
        {"molt_runtime.wasm": hashlib.sha256(runtime.read_bytes()).hexdigest()},
        raising=True,
    )

    with pytest.raises(SystemExit, match="sidecar"):
        wasm_link._verify_runtime_integrity(runtime)


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


def _build_host_call_indirect_module() -> bytes:
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
    import_payload.extend(wasm_link._write_string("molt_call_indirect3"))
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


def _parse_function_imports(wasm_bytes: bytes) -> list[tuple[str, str]]:
    imports: list[tuple[str, str]] = []
    offset = 8
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = wasm_link._read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 2:
            count, offset = wasm_link._read_varuint(wasm_bytes, offset)
            for _ in range(count):
                module, offset = wasm_link._read_string(wasm_bytes, offset)
                name, offset = wasm_link._read_string(wasm_bytes, offset)
                kind = wasm_bytes[offset]
                offset += 1
                offset = wasm_link._parse_import_desc(wasm_bytes, offset, kind)
                if kind == 0:
                    imports.append((module, name))
            return imports
        offset = section_end
    return imports


def _parse_function_exports(wasm_bytes: bytes) -> list[tuple[str, int]]:
    exports: list[tuple[str, int]] = []
    offset = 8
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = wasm_link._read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 7:
            count, offset = wasm_link._read_varuint(wasm_bytes, offset)
            for _ in range(count):
                name, offset = wasm_link._read_string(wasm_bytes, offset)
                kind = wasm_bytes[offset]
                offset += 1
                idx, offset = wasm_link._read_varuint(wasm_bytes, offset)
                if kind == 0:
                    exports.append((name, idx))
            return exports
        offset = section_end
    return exports


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


def test_stub_dead_functions_preserves_start_root_reachability() -> None:
    module = _build_start_root_module()
    assert wasm_link._stub_dead_functions(module) is None


def test_tree_shake_runtime_preserves_required_function_exports() -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    shaken = wasm_link._tree_shake_runtime(module, {"exception_pending"})
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
    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_run_wasm_opt_via_optimize", fake_final_optimize)

    first = wasm_link._tree_shake_runtime(module, {"exception_pending"})

    assert first == final_runtime
    assert calls["count"] == 1

    monkeypatch.setattr(
        wasm_link.subprocess,
        "run",
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
        linked.write_bytes(output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setenv("MOLT_WASM_DEPLOY_RUNTIME", str(stale_runtime))
    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)

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
        linked.write_bytes(output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_validate_elements", lambda _data: (True, None))
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args: set())
    monkeypatch.setattr(wasm_link, "_post_link_optimize", lambda data, **_kwargs: data)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)

    rc = wasm_link._run_wasm_ld("wasm-ld", runtime, output, linked)

    assert rc == 0
    assert str(reloc_runtime) in wasm_ld_inputs
    assert str(runtime) not in wasm_ld_inputs


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
    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_run_wasm_opt_via_optimize", lambda *_a, **_k: False)

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


def test_strip_unused_module_function_imports_remaps_indices() -> None:
    module = _build_runtime_import_strip_module()

    stripped = wasm_link._strip_unused_module_function_imports(
        module,
        module_name="molt_runtime",
    )

    imports_after = _parse_function_imports(stripped)
    assert imports_after == [("molt_runtime", "live_runtime_fn")]

    exports_after = _parse_function_exports(stripped)
    assert exports_after == [("molt_main", 1)]

    call_targets = _parse_code_section_call_targets(stripped)
    assert call_targets == [[0]]


def test_post_link_optimize_split_app_does_not_preserve_all_reference_exports() -> None:
    module = _build_exported_runtime_module_many(
        ["dead_user_export", "molt_main", "__molt_table_ref_7"]
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
    assert "__molt_table_ref_7" in split_exports


def test_collect_linking_function_symbols_parses_defined_and_undefined_entries() -> None:
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
            name == "molt_isolate_import"
            and (flags & wasm_link.FLAG_BINDING_GLOBAL)
            for flags, _, name, _ in symbols
        ), symbols
        assert any(
            name == f"{wasm_link._OUTPUT_EXPORT_ALIAS_PREFIX}molt_isolate_import"
            and (flags & wasm_link.FLAG_EXPORTED)
            for flags, _, name, _ in symbols
        ), symbols
        temp_dir.cleanup()


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

    func_payload = write_varuint(3) + write_varuint(0) + write_varuint(0) + write_varuint(0)
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


def test_split_app_reference_function_exports_preserves_public_and_isolate_exports() -> None:
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


def test_collect_output_wrapper_specs_prefers_main_module_prefix_over_dominant_stdlib(
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
    func_name_subsection.extend(
        wasm_link._write_string("__molt_output_export_1900")
    )
    name_custom_payload = bytearray()
    name_custom_payload.append(1)
    name_custom_payload.extend(write_varuint(len(func_name_subsection)))
    name_custom_payload.extend(func_name_subsection)
    sections.append((0, wasm_link._build_custom_section("name", bytes(name_custom_payload))))

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
    func_payload = write_varuint(3) + write_varuint(0) + write_varuint(0) + write_varuint(0)
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
        linked.write_bytes(output_bytes)
        class Result:
            returncode = 0
            stderr = ""
            stdout = ""
        return Result()

    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None)
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
        sections.append((0, wasm_link._build_custom_section("linking", linking_payload)))
        return wasm_link._build_sections(sections)

    output_bytes = _module(include_host_init_export=True)
    linked_without_host_init = _module(include_host_init_export=False)
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(output_bytes)
    output.write_bytes(output_bytes)

    def fake_run(_cmd, **_kwargs):
        linked.write_bytes(linked_without_host_init)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: True)
    monkeypatch.setattr(wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None)
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
                )
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
                )
            ]
        )
    )
    monkeypatch.setattr(wasm_link, "_find_tool", lambda _names: None)

    mangled = wasm_link._find_call_indirect_mangled(runtime)
    output_symbols = wasm_link._find_output_call_indirect_symbol(output)

    assert mangled == {
        "molt_call_indirect13": "_ZN4molt19molt_call_indirect1317hfeedfaceE"
    }
    assert output_symbols["molt_call_indirect13"] == (
        41,
        wasm_link.FLAG_BINDING_GLOBAL
        | wasm_link.FLAG_EXPLICIT_NAME
        | wasm_link.FLAG_EXPORTED,
    )


def test_run_wasm_ld_split_runtime_emits_outputs_even_if_linked_validation_fails(
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

    def fake_run(cmd, **kwargs):
        linked.write_bytes(output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link.subprocess, "run", fake_run)
    monkeypatch.setattr(wasm_link, "_validate_linked", lambda _p: False)
    monkeypatch.setattr(wasm_link, "_append_table_ref_elements", lambda data, **_kwargs: None)
    monkeypatch.setattr(wasm_link, "_declare_ref_func_elements", lambda data: None)
    monkeypatch.setattr(wasm_link, "_ensure_table_export", lambda data: None)
    monkeypatch.setattr(wasm_link, "_build_runtime_stub", lambda data: runtime_bytes)
    monkeypatch.setattr(wasm_link, "_restore_output_export_aliases", lambda data: None)
    monkeypatch.setattr(wasm_link, "_optimize_split_app_module", lambda data, **_: data)
    monkeypatch.setattr(wasm_link, "_collect_module_imports", lambda *_args, **_kwargs: set())
    monkeypatch.setattr(wasm_link, "_tree_shake_runtime", lambda *_args, **_kwargs: runtime_bytes)
    monkeypatch.setattr(wasm_link, "_collect_custom_names", lambda _data: [])
    monkeypatch.setattr(wasm_link, "_collect_imports", lambda _data: [])
    monkeypatch.setattr(wasm_link, "_collect_exports", lambda _data: {"molt_memory", "molt_table"})
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
    assert (split_dir / "app.wasm").exists()
    assert (split_dir / "molt_runtime.wasm").exists()


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
    export_payload.extend(wasm_link._write_string("__molt_table_ref_7"))
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
    export_payload.extend(wasm_link._write_string("__molt_table_ref_3"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("__molt_table_ref_9"))
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
    assert "__molt_table_ref_3" in text_refs

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
    export_payload.extend(wasm_link._write_string("__molt_table_ref_7"))
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
    assert "__molt_table_ref_7" in exports
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
    export_payload.extend(wasm_link._write_string("__molt_table_ref_7"))
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
    assert "__molt_table_ref_7" in exports
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
    export_payload.extend(wasm_link._write_string("__molt_table_ref_7"))
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
    assert "__molt_table_ref_7" not in exports
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
    for name, index in (
        ("__molt_table_ref_7", 0),
        ("__molt_table_ref_7", 1),
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
    assert list(name for name in exports if name == "__molt_table_ref_7") == [
        "__molt_table_ref_7"
    ]
    assert list(name for name in exports if name == "molt_main") == ["molt_main"]


def test_neutralize_linked_table_init_replaces_body_with_noop() -> None:
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

    table_init_body = bytes([0x00, 0x41, 0x01, 0x1A, 0x0B])
    main_body = bytes([0x00, 0x10, 0x00, 0x0B])
    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    code_payload.extend(write_varuint(len(table_init_body)))
    code_payload.extend(table_init_body)
    code_payload.extend(write_varuint(len(main_body)))
    code_payload.extend(main_body)
    sections.append((10, bytes(code_payload)))

    updated = wasm_link._neutralize_linked_table_init(
        wasm_link._build_sections(sections)
    )
    assert updated is not None

    code_section = next(
        payload
        for section_id, payload in wasm_link._parse_sections(updated)
        if section_id == 10
    )
    count, offset = wasm_link._read_varuint(code_section, 0)
    assert count == 2
    table_body_size, offset = wasm_link._read_varuint(code_section, offset)
    table_body = code_section[offset: offset + table_body_size]
    assert table_body == bytes([0x00, 0x0B])


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
    export_payload.extend(wasm_link._write_string("__molt_table_ref_20"))
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
    body.extend(b"\x41\x00")      # i32.const 0
    body.extend(b"\x11\x00\x00")  # call_indirect type 0 table 0
    body.append(0x0B)               # end
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

    path_and_adjacent = (
        b"/Users/alice/project/tmp/class_method_probe.py"
        b"f__name__hi"
    )
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
    allowlist = Path(__file__).resolve().parents[1] / "tools" / "wasm_allowed_imports.txt"
    assert allowlist.exists(), f"Missing allowlist: {allowlist}"
    symbols = _parse_allowlist(allowlist)
    # Must contain core WASI symbols
    assert "fd_write" in symbols
    assert "proc_exit" in symbols
    assert "__indirect_function_table" in symbols
    # Must contain indirect call trampolines
    assert "molt_call_indirect0" in symbols
    assert "molt_call_indirect13" in symbols
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
        s for s in symbols
        if s.startswith("molt_") and not any(s.startswith(p) for p in _ALLOWED_MOLT_PREFIXES)
    }
    assert runtime_syms == set(), f"Unexpected molt_runtime symbols in allowlist: {runtime_syms}"
