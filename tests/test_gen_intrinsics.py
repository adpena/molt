from __future__ import annotations

import importlib.util
from pathlib import Path


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


def test_backend_symbol_overrides_file_is_in_sync() -> None:
    """The backend's name->symbol override table is generated from the same
    `SYMBOL_OVERRIDES` source as the runtime's `generated.rs`. Because the
    backend crate does NOT depend on molt-runtime, this file is its only view of
    the name!=symbol mapping the per-app intrinsic resolver must key by. If it
    drifts from `gen_intrinsics.py`, the resolver silently mis-keys (the exact
    asyncio P0 this guard exists to prevent recurring). Pin freshness here so a
    forgotten regeneration fails at test time, not at runtime.
    """
    module = _load_gen_intrinsics_module()
    rendered = module.render_backend_overrides_rs()
    checked_in = (
        ROOT / "runtime/molt-backend/src/intrinsic_symbol_overrides.rs"
    ).read_text()
    assert checked_in == rendered, (
        "runtime/molt-backend/src/intrinsic_symbol_overrides.rs is stale — "
        "run `python3 tools/gen_intrinsics.py` to regenerate it from "
        "SYMBOL_OVERRIDES."
    )


def test_async_sleep_override_maps_name_to_two_arg_symbol() -> None:
    """Structural guard for the asyncio P0 root cause: the `molt_async_sleep`
    intrinsic NAME must map to the 2-arg `molt_async_sleep_new` SYMBOL, not the
    legacy 1-arg `molt_async_sleep` export. Both are real runtime symbols, so a
    resolver keyed by name would relocate against the wrong overload even if the
    lookup hit. This pins the override at the source.
    """
    module = _load_gen_intrinsics_module()
    assert module.SYMBOL_OVERRIDES.get("molt_async_sleep") == "molt_async_sleep_new"
    # The generated mapper must agree.
    rendered = module.render_backend_overrides_rs()
    assert '("molt_async_sleep", "molt_async_sleep_new")' in rendered


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
