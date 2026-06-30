from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CPYTHON_ABI_DUPLICATE_SYMBOLS = {
    "PyObject_Type": "api/typeobj.rs",
    "PyDict_Contains": "api/mapping.rs",
    "PyDict_DelItem": "api/mapping.rs",
    "PyMem_Free": "api/memory.rs",
    "PyMem_Malloc": "api/memory.rs",
    "PyMem_Realloc": "api/memory.rs",
    "PyIter_Check": "api/object.rs",
    "PyBytes_AsString": "api/strings.rs",
}


def test_runtime_cpython_compat_does_not_export_cpython_symbols() -> None:
    source = (
        ROOT / "runtime" / "molt-runtime" / "src" / "c_api" / "cpython_compat.rs"
    ).read_text(encoding="utf-8")

    assert "#[unsafe(no_mangle)]" not in source
    assert "#[no_mangle]" not in source
    assert "export_name" not in source
    assert "link_name" not in source


def test_cpython_abi_crate_owns_duplicate_cpython_symbol_exports() -> None:
    for symbol, relative_path in CPYTHON_ABI_DUPLICATE_SYMBOLS.items():
        source_path = ROOT / "runtime" / "molt-cpython-abi" / "src" / relative_path
        lines = source_path.read_text(encoding="utf-8").splitlines()
        def_index = next(
            index
            for index, line in enumerate(lines)
            if re.match(rf"pub (unsafe )?extern \"C\" fn {symbol}\(", line)
        )

        assert "#[unsafe(no_mangle)]" in lines[max(0, def_index - 3) : def_index], (
            f"{symbol} must be exported by molt-cpython-abi, not molt-runtime"
        )
