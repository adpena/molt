from __future__ import annotations

import hashlib
import importlib.util
import os
from pathlib import Path
import sys
import time

from molt.cli.wasm_link_cache import (
    _locked_wasm_link_cache_entry,
    _publish_wasm_link_cache_entry,
    _wasm_link_cache_entry,
)


REPO_ROOT = Path(__file__).resolve().parents[2]
MOLT_CACHE_PRUNE = REPO_ROOT / "tools" / "molt_cache_prune.py"
WASM_BYTES = b"\x00asm\x01\x00\x00\x00cache-entry"


def _load_molt_cache_prune():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_molt_cache_prune", MOLT_CACHE_PRUNE
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _cache_key(label: str) -> str:
    """A real SHA-256 cache key; an `xx-` label prefix pins the lock stripe."""
    digest = hashlib.sha256(label.encode("utf-8")).hexdigest()
    stripe, _, _rest = label.partition("-")
    if len(stripe) == 2 and all(char in "0123456789abcdef" for char in stripe):
        return stripe + digest[2:]
    return digest


def _cache_entry(cache_root: Path, label: str):
    return _wasm_link_cache_entry(
        "runtime_tree_shake",
        "runtime-tree-shake-test",
        _cache_key(label),
        cache_root=cache_root / "wasm_link",
    )


def test_pruner_treats_wasm_link_content_keys_as_independent_entries(
    tmp_path: Path,
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    first = _cache_entry(cache_root, "first")
    second = _cache_entry(cache_root, "second")
    for entry in (first, second):
        with _locked_wasm_link_cache_entry(entry):
            _publish_wasm_link_cache_entry(entry, WASM_BYTES)

    entries = module._collect_entries(cache_root)

    assert {entry.path for entry in entries} == {first.root, second.root}
    assert all(entry.lock_path is not None for entry in entries)


def test_pruner_preserves_locked_wasm_link_entry_then_removes_it(
    tmp_path: Path,
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    entry = _cache_entry(cache_root, "active")
    with _locked_wasm_link_cache_entry(entry):
        _publish_wasm_link_cache_entry(entry, WASM_BYTES)
        old = time.time() - 86400
        os.utime(entry.root, (old, old))
        active_result = module._prune(
            cache_root, max_bytes=None, max_age_days=0, dry_run=False
        )
        assert active_result["entries_removed"] == 0
        assert entry.artifact.is_file()

    old = time.time() - 86400
    os.utime(entry.root, (old, old))
    released_result = module._prune(
        cache_root, max_bytes=None, max_age_days=0, dry_run=False
    )
    assert released_result["entries_removed"] == 1
    assert not entry.root.exists()


def test_pruner_default_root_reuses_canonical_molt_cache(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "canonical-cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))

    assert module._default_cache_root() == cache_root


def test_wasm_link_cache_locks_use_bounded_stable_stripes(tmp_path: Path) -> None:
    cache_root = tmp_path / "cache"
    first = _cache_entry(cache_root, "aa-first")
    same_stripe = _cache_entry(cache_root, "aa-second")
    other_stripe = _cache_entry(cache_root, "bb-third")

    assert first.lock == same_stripe.lock
    assert first.lock != other_stripe.lock
    assert first.lock.name == "aa.lock"
    assert first.key not in first.lock.name
    assert (
        first.lock.parent == cache_root / "wasm_link" / "runtime_tree_shake" / ".locks"
    )


def test_pruner_breaks_equal_mtime_ties_by_normalized_key_path(tmp_path: Path) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    first = _cache_entry(cache_root, "aa-first")
    second = _cache_entry(cache_root, "bb-second")
    for entry in (second, first):
        with _locked_wasm_link_cache_entry(entry):
            _publish_wasm_link_cache_entry(entry, WASM_BYTES)
    tied_mtime = time.time() - 60
    for entry in (first, second):
        os.utime(entry.root, (tied_mtime, tied_mtime))

    result = module._prune(
        cache_root,
        max_bytes=max(
            module._entry_size_bytes(first.root),
            module._entry_size_bytes(second.root),
        ),
        max_age_days=None,
        dry_run=False,
    )

    assert result["entries_removed"] == 1
    assert not first.root.exists()
    assert second.root.exists()


def test_budget_comes_from_flags_then_projected_env_then_dx_defaults() -> None:
    module = _load_molt_cache_prune()
    from molt import dx

    assert module.resolve_budget(None, None, {}) == (
        float(dx.DEFAULT_MOLT_CACHE_MAX_GB),
        int(dx.DEFAULT_MOLT_CACHE_MAX_AGE_DAYS),
    )
    projected = {"MOLT_CACHE_MAX_GB": "7.5", "MOLT_CACHE_MAX_AGE_DAYS": "3"}
    assert module.resolve_budget(None, None, projected) == (7.5, 3)
    assert module.resolve_budget(2.0, 1, projected) == (2.0, 1)


def test_pruner_never_evicts_the_selected_home_or_bin(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_molt_cache_prune()
    for name in ("MOLT_HOME", "MOLT_BIN"):
        monkeypatch.delenv(name, raising=False)
    cache_root = tmp_path / "cache"
    tools = cache_root / "home" / "target-root" / "toolchains" / "tool"
    tools.parent.mkdir(parents=True)
    tools.write_bytes(b"provisioned tool state")
    stale = cache_root / "stale-entry"
    stale.mkdir()
    (stale / "payload").write_bytes(b"evictable")
    old = time.time() - 30 * 86400
    for path in (tools, tools.parent, cache_root / "home", stale / "payload", stale):
        os.utime(path, (old, old))

    summary = module._prune(cache_root, max_bytes=0, max_age_days=0, dry_run=False)

    assert not stale.exists()
    assert tools.read_bytes() == b"provisioned tool state"
    assert summary["protected_state"] == [str((cache_root / "home").resolve())]


def test_pruner_evicts_a_cache_home_that_is_not_selected(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_molt_cache_prune()
    cache_root = tmp_path / "cache"
    unselected = cache_root / "home"
    unselected.mkdir(parents=True)
    (unselected / "payload").write_bytes(b"old build output")
    selected = tmp_path / "selected-home"
    monkeypatch.setenv("MOLT_HOME", str(selected))
    monkeypatch.setenv("MOLT_BIN", str(selected / "bin"))

    summary = module._prune(cache_root, max_bytes=0, max_age_days=None, dry_run=False)

    assert not unselected.exists()
    assert summary["protected_state"] == []
