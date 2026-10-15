from __future__ import annotations

from pathlib import Path

from tools import check_runtime_serial_bridge_imports


def test_rejects_direct_molt_host_import(tmp_path: Path) -> None:
    module = check_runtime_serial_bridge_imports
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
    module = check_runtime_serial_bridge_imports
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
    module = check_runtime_serial_bridge_imports
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
