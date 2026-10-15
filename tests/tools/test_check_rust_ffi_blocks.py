from __future__ import annotations

import textwrap
from pathlib import Path

from tools import check_rust_ffi_blocks, proof_plan

PLAN = proof_plan.ProofPlan.load()


def _plan_gate_toolchains(gate: str) -> set[str]:
    """The toolchain closure of every plan rule or landing gate that runs ``gate``."""
    closures = [
        set(PLAN.toolchain_closure(rule["toolchains"]))
        for rule in PLAN.local_rules
        if gate in rule.get("gates", ())
    ]
    if gate in PLAN.always:
        closures.append(set(PLAN.toolchain_closure(PLAN.always_toolchains)))
    assert closures, f"no proof-plan gate runs {gate!r}"
    return set().union(*closures)


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


def test_rust_ffi_block_checker_prunes_build_outputs(tmp_path: Path) -> None:
    source = tmp_path / "runtime" / "target" / "debug" / "out" / "bindgen.rs"
    source.parent.mkdir(parents=True)
    source.write_text('extern "C" {\n    fn generated();\n}\n', encoding="utf-8")

    assert check_rust_ffi_blocks._rust_files([tmp_path / "runtime"]) == []


def test_repository_rust_ffi_blocks_are_unsafe() -> None:
    assert check_rust_ffi_blocks.main([]) == 0


def test_plan_runs_the_rust_ffi_block_gate_without_cargo() -> None:
    # A static check reads sources; it must never wait for a Cargo build slot.
    assert not {"cargo", "rustc"} & _plan_gate_toolchains(
        "python3 tools/venv_exec.py python tools/check_rust_ffi_blocks.py"
    )
