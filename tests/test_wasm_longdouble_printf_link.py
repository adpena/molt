"""Regression gate for the wasi-libc long-double printf/scanf enablement.

E1 witness frontier: numpy's longdouble repr/parse (``NumPyOS_ascii_formatl`` /
``NumPyOS_ascii_strtold``) formats a ``%L`` conversion during ``_multiarray_umath``
import. wasi-libc's default ``libc.a`` links a ``long_double_not_supported`` stub
for ``%L`` float conversions that ``abort()``s -> a raw ``unreachable`` wasm trap.

``_link_runtime_staticlib_to_reloc_wasm`` whole-archives wasi-libc's companion
``libc-printscan-long-double.a`` (real formatters) + wasi-sdk's
``libclang_rt.builtins-wasm32.a`` (binary128 soft-float) so the stub is overridden
and the formatters' ``__addtf3``/``__multf3``/... resolve.

This test reproduces that link end-to-end against a minimal ``snprintf("%Lg", ...)``
translation unit and asserts BOTH halves so the fix cannot silently rot:

* BASELINE (Rust ``libc.a`` alone) still pulls the ``long_double_not_supported``
  stub -- if this ever stops being true the fix is no longer load-bearing and the
  test should be revisited rather than giving false confidence.
* FIXED (whole-archived long-double archive + builtins) drops the stub AND
  resolves the binary128 soft-float builtins to defined symbols, with a clean
  (exit 0, no duplicate-symbol) partial link.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest

from molt.cli import wasm_toolchain

_STUB_SYMBOL = "long_double_not_supported"


def _llvm_tool(name: str) -> str | None:
    found = shutil.which(name)
    if found:
        return found
    # LLVM's Windows installer is commonly on PATH only via its bin dir.
    for base in (r"C:\Program Files\LLVM\bin", r"C:\Program Files (x86)\LLVM\bin"):
        candidate = Path(base) / f"{name}.exe"
        if candidate.exists():
            return str(candidate)
    return None


def _require(tool: str | None, label: str) -> str:
    if tool is None:
        pytest.skip(f"{label} not available in this environment")
    return tool


def _nm_symbols(nm: str, artifact: Path) -> list[str]:
    proc = subprocess.run(
        [nm, str(artifact)],
        capture_output=True,
        text=True,
        check=True,
    )
    return proc.stdout.splitlines()


def test_reloc_link_long_double_printf_overrides_stub(tmp_path: Path) -> None:
    clang = _require(_llvm_tool("clang"), "clang (wasm cross-compiler)")
    wasm_ld = _require(shutil.which("wasm-ld") or _llvm_tool("wasm-ld"), "wasm-ld")
    nm = _require(_llvm_tool("llvm-nm"), "llvm-nm")

    sysroot = wasm_toolchain.resolve_wasi_sysroot()
    if sysroot is None:
        pytest.skip("no WASI sysroot resolved")
    libc = wasm_toolchain.wasm_wasi_libc_archive()
    longdouble = wasm_toolchain.wasm_wasi_printscan_long_double_archive()
    builtins = wasm_toolchain.wasm_clang_rt_builtins_archive()
    if libc is None:
        pytest.skip("Rust wasm32-wasip1 libc.a not found")
    if longdouble is None:
        pytest.skip("wasi-libc libc-printscan-long-double.a not provisioned")
    if builtins is None:
        pytest.skip("libclang_rt.builtins-wasm32.a not provisioned")

    src = tmp_path / "ld.c"
    src.write_text(
        "#include <stdio.h>\n"
        "long double gv = 1.5L;\n"
        "int run(char *buf, int n) { return snprintf(buf, n, \"%.10Lg\", gv); }\n",
        encoding="utf-8",
    )
    obj = tmp_path / "ld.o"
    subprocess.run(
        [
            clang,
            "--target=wasm32-wasip1",
            f"--sysroot={sysroot}",
            "-O2",
            "-c",
            str(src),
            "-o",
            str(obj),
        ],
        check=True,
        capture_output=True,
        text=True,
    )

    # BASELINE: the default libc.a alone must still contribute the abort stub,
    # otherwise this test is no longer guarding the real capability.
    baseline = tmp_path / "baseline.wasm"
    subprocess.run(
        [wasm_ld, "-r", "--whole-archive", str(obj), "--no-whole-archive",
         str(libc), "-o", str(baseline)],
        check=True,
        capture_output=True,
        text=True,
    )
    baseline_syms = _nm_symbols(nm, baseline)
    assert any(_STUB_SYMBOL in line for line in baseline_syms), (
        "baseline libc.a no longer links the long_double_not_supported stub; "
        "the long-double fix may no longer be load-bearing"
    )

    # FIXED: whole-archive the long-double formatter archive (overrides the stub
    # objects) and add the binary128 builtins. Mirrors
    # _link_runtime_staticlib_to_reloc_wasm's archive ordering.
    fixed = tmp_path / "fixed.wasm"
    result = subprocess.run(
        [
            wasm_ld,
            "-r",
            "--whole-archive",
            str(obj),
            str(longdouble),
            "--no-whole-archive",
            str(libc),
            str(builtins),
            "-o",
            str(fixed),
        ],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, (
        f"fixed reloc link failed (dup symbols?): {result.stderr or result.stdout}"
    )
    fixed_syms = _nm_symbols(nm, fixed)
    assert not any(_STUB_SYMBOL in line for line in fixed_syms), (
        "long_double_not_supported stub still present after linking the "
        "long-double formatter archive; the %L abort trap is not fixed"
    )
    # The real long-double formatter pulls binary128 soft-float; it must resolve
    # to a DEFINED symbol (upper-case type letter), never a dangling import.
    multf3 = [line for line in fixed_syms if line.rstrip().endswith("__multf3")]
    assert multf3, "no __multf3 symbol in the linked object at all"
    assert any(
        " T __multf3" in line or " t __multf3" in line for line in multf3
    ), f"__multf3 unresolved (would trap as a missing import): {multf3}"
