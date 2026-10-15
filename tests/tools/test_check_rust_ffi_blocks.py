from __future__ import annotations

import sys
import textwrap
from pathlib import Path

from tools import check_rust_ffi_blocks, ci_gate


def test_rust_ffi_block_checker_rejects_missing_unsafe(tmp_path: Path) -> None:
    source = tmp_path / "ffi.rs"
    source.write_text(
        textwrap.dedent(
            """
            extern "C" {
                fn imported();
            }
            """
        ),
        encoding="utf-8",
    )

    findings = check_rust_ffi_blocks.find_missing_unsafe_extern_blocks(source)

    assert len(findings) == 1
    assert findings[0].line == 2


def test_rust_ffi_block_checker_ignores_exports_and_embedded_headers(
    tmp_path: Path,
) -> None:
    source = tmp_path / "ffi.rs"
    source.write_text(
        textwrap.dedent(
            """
            pub extern "C" fn exported() {}

            unsafe extern "C" {
                fn imported();
            }

            pub const HEADER: &str = r#"
            #ifdef __cplusplus
            extern "C" {
            #endif
            "#;
            """
        ),
        encoding="utf-8",
    )

    assert check_rust_ffi_blocks.find_missing_unsafe_extern_blocks(source) == []


def test_ci_gate_uses_rust_ffi_block_gate_without_compile_slot() -> None:
    check = {entry.name: entry for entry in ci_gate._build_checks()}["rust-ffi-blocks"]

    assert check.cmd == [
        sys.executable,
        str(ci_gate.TOOLS / "check_rust_ffi_blocks.py"),
    ]
    assert check.needs_rust is False
    assert check.needs_cargo is False


def test_rust_ffi_block_checker_prunes_build_outputs(tmp_path: Path) -> None:
    source = tmp_path / "runtime" / "target" / "debug" / "out" / "bindgen.rs"
    source.parent.mkdir(parents=True)
    source.write_text('extern "C" {\n    fn generated();\n}\n', encoding="utf-8")

    assert check_rust_ffi_blocks._rust_files([tmp_path / "runtime"]) == []


def test_repository_rust_ffi_blocks_are_unsafe() -> None:
    assert check_rust_ffi_blocks.main([]) == 0
