from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[1]
GEN_INTRINSICS = ROOT / "tools" / "gen_intrinsics.py"


def _load_gen_intrinsics_module():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_intrinsics", GEN_INTRINSICS
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_backend_symbol_overrides_file_is_removed() -> None:
    assert not (ROOT / "runtime/molt-backend/src/intrinsic_symbol_overrides.rs").exists()


def test_async_sleep_intrinsic_symbol_matches_public_name() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()
    symbols = {name: symbol for name, symbol, _arity in entries}
    assert symbols["molt_async_sleep"] == "molt_async_sleep"


def test_ssl_intrinsic_abi_is_not_profile_gated() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()

    ssl_symbols = sorted(
        {symbol for _name, symbol, _arity in entries if symbol.startswith("molt_ssl_")}
    )
    assert ssl_symbols
    for symbol in ssl_symbols:
        assert module._feature_gate_for_symbol(symbol) is None

    assert module._feature_gate_for_symbol("molt_http_client_execute") == "stdlib_net"

    generated = (ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs").read_text()
    ssl_block = generated.split("fn resolve_ssl_symbol", 1)[1].split(
        "        _ => None,",
        1,
    )[0]
    assert '#[cfg(feature = "stdlib_net")]' not in ssl_block


def test_rustfmt_uses_shared_memory_guard(monkeypatch, tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    calls: list[dict[str, object]] = []
    target = tmp_path / "generated.rs"
    target.write_text("fn main(){}\n", encoding="utf-8")

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(
            returncode=0,
            stdout="",
            stderr="",
            check_returncode=lambda: None,
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    module._rustfmt(target)

    assert calls == [
        {
            "cmd": ["rustfmt", str(target)],
            "prefix": "MOLT_GENERATOR",
            "cwd": ROOT,
            "capture_output": True,
            "text": True,
            "timeout": 60.0,
        }
    ]
