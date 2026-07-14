from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools/gen_cpython_type_slots.py"


def _load_generator():
    spec = importlib.util.spec_from_file_location("gen_cpython_type_slots", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_type_slot_authority_is_complete_and_generated_header_is_current() -> None:
    generator = _load_generator()
    slots = generator.load_slots()

    assert len(slots) == 81
    assert slots[0] == ("Py_bf_getbuffer", 1)
    assert slots[-1] == ("Py_am_send", 81)
    assert generator.OUTPUT.read_text(encoding="utf-8") == generator.render(slots)
