from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


def _load_tool():
    root = Path(__file__).resolve().parents[2]
    path = root / "tools" / "check_runtime_serial_bridge_imports.py"
    spec = importlib.util.spec_from_file_location(
        "check_runtime_serial_bridge_imports_under_test", path
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_rejects_direct_molt_host_import(tmp_path: Path) -> None:
    module = _load_tool()
    serial = tmp_path / "runtime" / "molt-runtime-serial" / "src"
    (serial / "datetime.rs").parent.mkdir(parents=True)
    (serial / "datetime.rs").write_text(
        """
unsafe extern "C" {
    #[link_name = "molt_time_local_offset_host"]
    fn local_offset(secs: i64) -> i64;
}
""",
        encoding="utf-8",
    )

    violations = module.find_serial_bridge_import_violations(serial)

    assert [(v.symbol, v.reason) for v in violations] == [
        (
            "molt_time_local_offset_host",
            "direct Molt host import bypasses RuntimeVtable",
        )
    ]


def test_allows_serial_vtable_getter(tmp_path: Path) -> None:
    module = _load_tool()
    serial = tmp_path / "runtime" / "molt-runtime-serial" / "src"
    (serial / "bridge.rs").parent.mkdir(parents=True)
    (serial / "bridge.rs").write_text(
        """
unsafe extern "C" {
    fn __molt_serial_get_vtable() -> *const u8;
}
""",
        encoding="utf-8",
    )

    assert module.find_serial_bridge_import_violations(serial) == []


def test_allows_explicit_windows_crt_exception(tmp_path: Path) -> None:
    module = _load_tool()
    serial = tmp_path / "runtime" / "molt-runtime-serial" / "src"
    (serial / "datetime.rs").parent.mkdir(parents=True)
    (serial / "datetime.rs").write_text(
        """
unsafe extern "C" {
    #[link_name = "_mktime64"]
    fn windows_mktime64(tm: *mut u8) -> i64;
}
""",
        encoding="utf-8",
    )

    assert module.find_serial_bridge_import_violations(serial) == []
