from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

from tests.stdlib_intrinsic_registry import intrinsic_registry


ROOT = Path(__file__).resolve().parents[1]
TOKENIZER_PATH = ROOT / "demos" / "tinygrad" / "tokenizer.py"


def _load_tokenizer_module(monkeypatch: pytest.MonkeyPatch):
    module_name = "_molt_test_tinygrad_tokenizer_contract"
    spec = importlib.util.spec_from_file_location(module_name, TOKENIZER_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    # The module binds its GPU device intrinsic at import; the BPE paths under
    # test must never call it.
    with intrinsic_registry({"molt_gpu_prim_device": _device_unused}):
        spec.loader.exec_module(module)
    return module


def _device_unused(*_args: object) -> object:
    pytest.fail("the BPE tokenizer called the GPU device intrinsic")


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
