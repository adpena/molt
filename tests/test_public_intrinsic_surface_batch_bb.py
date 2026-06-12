from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/logging/config.py",
    ROOT / "src/molt/stdlib/concurrent/__init__.py",
    ROOT / "src/molt/stdlib/html/__init__.py",
    ROOT / "src/molt/stdlib/importlib/__init__.py",
    ROOT / "src/molt/stdlib/importlib/metadata/_text.py",
    ROOT / "src/molt/stdlib/socketserver.py",
    ROOT / "src/molt/stdlib/stringprep.py",
    ROOT / "src/molt/stdlib/weakref.py",
    ROOT / "src/molt/stdlib/importlib/machinery.py",
    ROOT / "src/molt/stdlib/urllib/request.py",
    ROOT / "src/molt/stdlib/ctypes/__init__.py",
    ROOT / "src/molt/stdlib/http/cookiejar.py",
    ROOT / "src/molt/stdlib/importlib/metadata/__init__.py",
    ROOT / "src/molt/stdlib/string/__init__.py",
    ROOT / "src/molt/stdlib/typing.py",
    ROOT / "src/molt/stdlib/urllib/error.py",
    ROOT / "src/molt/stdlib/ast.py",
    ROOT / "src/molt/stdlib/secrets.py",
    ROOT / "src/molt/stdlib/textwrap.py",
    ROOT / "src/molt/stdlib/traceback.py",
]


def test_public_intrinsic_surface_batch_bb_avoids_globals_injection() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        for line in source.splitlines():
            if "require_intrinsic(" in line:
                assert "globals()" not in line, path


def test_importlib_import_module_uses_rust_intrinsic() -> None:
    source = (ROOT / "src/molt/stdlib/importlib/__init__.py").read_text()

    assert "molt_importlib_import_transaction" in source
    assert "_MODULE_ALIASES" not in source
    assert "_builtins.__import__" not in source
    assert (
        "mod = _MOLT_IMPORTLIB_IMPORT_TRANSACTION("
        "resolved, globals(), locals(), (\"*\",), 0"
    ) in source


def test_importlib_import_module_has_single_transaction_intrinsic_authority() -> None:
    checked_paths = [
        (
            ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi",
            "molt_importlib_import_transaction",
        ),
        (ROOT / "src/molt/_intrinsics.pyi", "molt_importlib_import_transaction"),
        (
            ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs",
            "molt_importlib_import_transaction",
        ),
        (
            ROOT / "runtime/molt-runtime/src/builtins/platform_importlib_ffi.rs",
            "molt_importlib_import_transaction",
        ),
        (
            ROOT / "runtime/molt-backend/src/wasm.rs",
            "molt_importlib_import_transaction",
        ),
        (
            ROOT / "runtime/molt-backend/src/wasm_imports.rs",
            "importlib_import_transaction",
        ),
    ]

    for path, transaction_token in checked_paths:
        source = path.read_text()
        assert transaction_token in source, path
        assert "molt_importlib_import_module" not in source, path
        assert "importlib_import_module" not in source, path
