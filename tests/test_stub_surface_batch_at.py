from __future__ import annotations

import importlib.util
import types
from pathlib import Path

from tests.stdlib_intrinsic_registry import intrinsic_registry


ROOT = Path(__file__).resolve().parents[1]
ALL_PATHS = [
    ROOT / "src/molt/stdlib/xml/etree/cElementTree.py",
    ROOT / "src/molt/stdlib/test/list_tests.py",
    ROOT / "src/molt/stdlib/test/seq_tests.py",
    ROOT / "src/molt/stdlib/test/tokenizedata/__init__.py",
    ROOT / "src/molt/stdlib/test/tokenizedata/badsyntax_3131.py",
    ROOT / "src/molt/stdlib/test/tokenizedata/badsyntax_pep3120.py",
    ROOT / "src/molt/stdlib/compression/__init__.py",
    ROOT / "src/molt/stdlib/compression/_common/__init__.py",
    ROOT / "src/molt/stdlib/compression/bz2.py",
    ROOT / "src/molt/stdlib/compression/gzip.py",
    ROOT / "src/molt/stdlib/compression/lzma.py",
    ROOT / "src/molt/stdlib/compression/zlib.py",
    ROOT / "src/molt/stdlib/dbm/ndbm.py",
]
RUNTIME_NONSTUB_PATHS = [
    ROOT / "src/molt/stdlib/test/tokenizedata/__init__.py",
]


def _load_module(path: Path, index: int) -> types.ModuleType:
    module_name = f"_molt_test_stub_surface_batch_at_{index}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_capability_anchor_batch_hides_raw_capability_intrinsic() -> None:
    for path in ALL_PATHS:
        source = path.read_text(encoding="utf-8")
        assert '_require_intrinsic("molt_capabilities_has", globals())' not in source
        assert (
            '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")'
            in source
        )

    with intrinsic_registry():
        for index, path in enumerate(RUNTIME_NONSTUB_PATHS):
            module = _load_module(path, index)
            assert "molt_capabilities_has" not in module.__dict__
            assert "_MOLT_CAPABILITIES_HAS" in module.__dict__

        for path in [
            ROOT / "src/molt/stdlib/test/tokenizedata/badsyntax_3131.py",
            ROOT / "src/molt/stdlib/test/tokenizedata/badsyntax_pep3120.py",
        ]:
            try:
                _load_module(path, 1000 + hash(path.name) % 1000)
            except SyntaxError:
                pass
            else:
                raise AssertionError(f"{path} did not raise SyntaxError")
