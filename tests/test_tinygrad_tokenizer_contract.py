from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest


ROOT = Path(__file__).resolve().parents[1]
TOKENIZER_PATH = ROOT / "src" / "molt" / "stdlib" / "tinygrad" / "tokenizer.py"


def _load_tokenizer_module(monkeypatch: pytest.MonkeyPatch):
    fake_intrinsics = SimpleNamespace(require_intrinsic=lambda _name: object())
    monkeypatch.setitem(sys.modules, "_intrinsics", fake_intrinsics)
    module_name = "_molt_test_tinygrad_tokenizer_contract"
    sys.modules.pop(module_name, None)
    spec = importlib.util.spec_from_file_location(module_name, TOKENIZER_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def test_tokenizer_decode_rejects_unknown_token_id(monkeypatch: pytest.MonkeyPatch):
    module = _load_tokenizer_module(monkeypatch)
    tokenizer = module.Tokenizer(vocab={"A": 1}, merges=[], added_tokens={})

    with pytest.raises(ValueError, match="Unknown token id: 999"):
        tokenizer.decode([999])


def test_tokenizer_encode_rejects_missing_byte_level_vocab(
    monkeypatch: pytest.MonkeyPatch,
):
    module = _load_tokenizer_module(monkeypatch)
    tokenizer = module.Tokenizer(vocab={}, merges=[], added_tokens={})

    with pytest.raises(ValueError, match="missing byte-level token"):
        tokenizer.encode("A")
