from __future__ import annotations

from collections import OrderedDict
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
    assert not (
        ROOT / "runtime/molt-backend/src/intrinsic_symbol_overrides.rs"
    ).exists()


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

    generated = (
        ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers/ssl_resolver.rs"
    ).read_text()
    ssl_block = generated.split("pub(super) fn resolve_symbol", 1)[1].split(
        "        _ => None,",
        1,
    )[0]
    assert '#[cfg(feature = "stdlib_net")]' not in ssl_block


def test_generated_resolvers_are_split_from_manifest_table() -> None:
    """Resolver address-taking is generated into per-module Rust files."""
    generated_path = ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs"
    resolver_root = (
        ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
    )
    generated = generated_path.read_text()
    resolver_mod = (resolver_root / "mod.rs").read_text()
    core_resolver = (resolver_root / "core_resolver.rs").read_text()
    ssl_resolver = (resolver_root / "ssl_resolver.rs").read_text()

    assert '#[path = "generated_resolvers/mod.rs"]\nmod generated_resolvers;' in generated
    assert "pub(crate) use generated_resolvers::resolve_symbol;" in generated
    assert "IntrinsicSpec {" in generated
    assert "fn resolve_core_symbol" not in generated
    assert "mod core_resolver;" in resolver_mod
    assert "pub(crate) fn resolve_symbol" in resolver_mod
    assert "molt_capabilities_trusted" in core_resolver
    assert "molt_ssl_context_new" not in core_resolver
    assert "molt_ssl_context_new" in ssl_resolver


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


def test_rustfmt_failure_reports_guarded_output(monkeypatch, tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    target = tmp_path / "generated.rs"
    target.write_text("fn main( {\n", encoding="utf-8")

    def fake_guarded_completed_process(_cmd, **_kwargs):
        return SimpleNamespace(
            returncode=1,
            stdout="format stdout",
            stderr="format stderr",
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        module._rustfmt(target)
    except RuntimeError as exc:
        message = str(exc)
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("rustfmt failure did not raise RuntimeError")

    assert f"rustfmt failed for {target}" in message
    assert "stdout:\nformat stdout" in message
    assert "stderr:\nformat stderr" in message


def test_write_rust_if_changed_skips_rustfmt_for_exact_match(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_gen_intrinsics_module()
    target = tmp_path / "generated.rs"
    text = "// @generated by tools/gen_intrinsics.py. DO NOT EDIT.\nfn main() {}\n"
    target.write_text(text, encoding="utf-8")
    calls: list[Path] = []

    monkeypatch.setattr(module, "_rustfmt", lambda path: calls.append(path))

    assert module._write_rust_if_changed(target, text) is False
    assert calls == []
    assert target.read_text(encoding="utf-8") == text


def test_resolver_cleanup_preserves_concurrent_temp_files(monkeypatch, tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    resolver_root = tmp_path / "generated_resolvers"
    resolver_root.mkdir()
    hidden_temp = resolver_root / ".ssl_resolver.rs.12345.tmp.rs"
    hidden_temp.write_text("temp", encoding="utf-8")
    stale_resolver = resolver_root / "removed_resolver.rs"
    stale_resolver.write_text("stale", encoding="utf-8")

    monkeypatch.setattr(module, "OUT_RS_RESOLVERS_DIR", resolver_root)
    monkeypatch.setattr(module, "_rustfmt", lambda _path: None)

    module._write_resolver_modules(
        OrderedDict({"core": ["molt_capabilities_trusted"]})
    )

    assert hidden_temp.read_text(encoding="utf-8") == "temp"
    assert not stale_resolver.exists()
    assert (resolver_root / "core_resolver.rs").exists()
