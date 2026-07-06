"""Tests for the fail-closed poison gate (tools/fail_closed_gate.py).

Covers:
  * the gate is GREEN on the checked-in seeded registry (the real tree);
  * the seed rows are well-formed (valid class, resolution_row, justification);
  * the drift invariant holds on the real tree (every discovered site registered);
  * NEGATIVE CONTROLS that mutation-prove the drift teeth PER CLASS: injecting a
    synthetic UNREGISTERED poison site of each class into a scanned root makes the
    gate FAIL, and removing it makes the gate PASS again. A gate that cannot fail
    on a real violation is theater.
  * a ratchet negative control: registering one more row of a class than its
    baseline fails the ratchet.

The four poison classes and their injected fixtures:
  A ecosystem_baked    — a ``typedef struct PyArray_Descr`` in a Molt-owned header
  B fail_open_stub      — a ``#[no_mangle]`` ABI export using ``wrapping_add`` on int
  C todo_as_plan        — a ``status:divergent`` marker inside a public def that
                          returns a divergent value (does not fail closed)
  D duplicate_authority — the SAME CPython header in both authority trees
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[2]
_GATE_PATH = _REPO_ROOT / "tools" / "fail_closed_gate.py"


def _load_gate_module():
    """Import tools/fail_closed_gate.py as a module (it lives outside a package).
    Fresh import each call so nothing leaks across tests."""
    mod_name = "_fail_closed_gate_under_test"
    spec = importlib.util.spec_from_file_location(mod_name, _GATE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Register before exec so @dataclass field resolution can find the module.
    sys.modules[mod_name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(mod_name, None)
        raise
    return module


_EMPTY_REGISTRY = (
    "[baseline]\n"
    "ecosystem_baked = 0\n"
    "fail_open_stub = 0\n"
    "duplicate_authority = 0\n"
    "todo_as_plan = 0\n"
)


def _write_empty_registry(root: Path) -> Path:
    registry = root / "tools" / "fail_closed_registry.toml"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(_EMPTY_REGISTRY, encoding="utf-8")
    return registry


# ---------------------------------------------------------------------------
# The gate is green on the real, checked-in seed.
# ---------------------------------------------------------------------------
def test_gate_is_green_on_seeded_registry() -> None:
    gate = _load_gate_module()
    violations = gate.run_gate()
    assert not violations, "seeded registry must pass the gate; violations:\n" + "\n".join(
        str(v) for v in violations
    )


def test_seed_registry_rows_are_well_formed() -> None:
    gate = _load_gate_module()
    registry = gate.load_registry()
    assert registry.rows, "registry must have rows"
    seen_ids: set[str] = set()
    for row in registry.rows:
        assert row.cls in gate._VALID_CLASSES, f"bad class {row.cls!r} for {row.id}"
        assert row.resolution_row.strip(), f"{row.id} must name a resolution_row"
        assert row.justification.strip(), f"{row.id} has empty justification"
        assert row.id not in seen_ids, f"duplicate id {row.id}"
        seen_ids.add(row.id)


def test_every_discovered_site_is_registered_in_seed() -> None:
    """Drift invariant on the real tree: no discovered poison site lacks a row."""
    gate = _load_gate_module()
    registry = gate.load_registry()
    unmatched = [
        f"[{s.cls}] {s.file}:{s.line} {s.signature!r}"
        for s in gate.discover_all(gate.REPO_ROOT)
        if gate._row_matches_site(registry, s) is None
    ]
    assert not unmatched, "discovered poison sites with no registry row:\n" + "\n".join(
        unmatched
    )


def test_seed_covers_all_four_classes() -> None:
    """Sanity: the seed is non-trivial — every poison class has at least one row,
    so each class's teeth are actually exercised on the real tree."""
    gate = _load_gate_module()
    registry = gate.load_registry()
    classes = {row.cls for row in registry.rows}
    assert classes == gate._VALID_CLASSES, (
        f"seed must cover all classes; missing {gate._VALID_CLASSES - classes}"
    )


# ---------------------------------------------------------------------------
# Negative control A — ecosystem_baked
# ---------------------------------------------------------------------------
def test_negative_control_ecosystem_baked_fails_then_passes(tmp_path: Path) -> None:
    gate = _load_gate_module()
    registry = _write_empty_registry(tmp_path)
    hdr = tmp_path / "include" / "numpy" / "synthetic.h"
    hdr.parent.mkdir(parents=True, exist_ok=True)
    baked = (
        "#ifndef SYNTHETIC_H\n#define SYNTHETIC_H\n"
        "typedef struct PyArray_Descr {\n"
        "    int type_num;\n"
        "} PyArray_Descr;\n"
        "#endif\n"
    )
    hdr.write_text(baked, encoding="utf-8")

    discovered = gate.discover_ecosystem_baked(tmp_path)
    assert any(
        s.file == "include/numpy/synthetic.h" for s in discovered
    ), f"Scan A failed to discover injected baked header; got {discovered}"

    fail = gate.run_gate(tmp_path, registry)
    assert any(
        v.kind == "unregistered-poison-site" and "synthetic.h" in v.detail for v in fail
    ), f"gate MUST fail on unregistered ecosystem_baked site; got {fail}"

    # Remove the baked definition (leave a thin forwarder) -> passes.
    hdr.write_text(
        '#ifndef SYNTHETIC_H\n#define SYNTHETIC_H\n#include <numpy/arrayobject.h>\n#endif\n',
        encoding="utf-8",
    )
    assert not gate.discover_ecosystem_baked(tmp_path), "forwarder must not be flagged"
    assert not gate.run_gate(tmp_path, registry)


# ---------------------------------------------------------------------------
# Negative control B — fail_open_stub
# ---------------------------------------------------------------------------
def test_negative_control_fail_open_abi_fails_then_passes(tmp_path: Path) -> None:
    gate = _load_gate_module()
    registry = _write_empty_registry(tmp_path)
    api_dir = tmp_path / "runtime" / "molt-cpython-abi" / "src" / "api"
    api_dir.mkdir(parents=True, exist_ok=True)
    stub = (
        "#[unsafe(no_mangle)]\n"
        'pub unsafe extern "C" fn PyNumber_Synthetic(x: i64, y: i64) -> *mut PyObject {\n'
        "    pyobj_from_int(x.wrapping_add(y))\n"
        "}\n"
    )
    (api_dir / "synthetic.rs").write_text(stub, encoding="utf-8")

    discovered = gate.discover_fail_open_abi(tmp_path)
    assert any(
        s.file == "runtime/molt-cpython-abi/src/api/synthetic.rs"
        and "wrapping_add" in s.signature
        for s in discovered
    ), f"Scan B failed to discover injected fail-open stub; got {discovered}"

    fail = gate.run_gate(tmp_path, registry)
    assert any(
        v.kind == "unregistered-poison-site" and "synthetic.rs" in v.detail for v in fail
    ), f"gate MUST fail on unregistered fail_open_stub; got {fail}"

    # Replace wrapping_add with a fail-closed impl (checked add or null) -> passes.
    (api_dir / "synthetic.rs").write_text(
        "#[unsafe(no_mangle)]\n"
        'pub unsafe extern "C" fn PyNumber_Synthetic(x: i64, y: i64) -> *mut PyObject {\n'
        "    match x.checked_add(y) {\n"
        "        Some(v) => pyobj_from_int(v),\n"
        "        None => ptr::null_mut(),\n"
        "    }\n"
        "}\n",
        encoding="utf-8",
    )
    assert not gate.discover_fail_open_abi(tmp_path), "checked_add must not be flagged"
    assert not gate.run_gate(tmp_path, registry)


# ---------------------------------------------------------------------------
# Negative control C — todo_as_plan
# ---------------------------------------------------------------------------
def test_negative_control_todo_as_plan_fails_then_passes(tmp_path: Path) -> None:
    gate = _load_gate_module()
    registry = _write_empty_registry(tmp_path)
    mod = tmp_path / "src" / "molt" / "stdlib" / "synthetic.py"
    mod.parent.mkdir(parents=True, exist_ok=True)
    # A public def whose body ships a divergent value with a status:divergent TODO.
    poison = (
        "def public_api(method):\n"
        "    if method != 'spawn':\n"
        "        # TODO(runtime, status:divergent): map fork to spawn silently\n"
        "        return spawn_context()\n"
        "    return spawn_context()\n"
    )
    mod.write_text(poison, encoding="utf-8")

    discovered = gate.discover_todo_as_plan(tmp_path)
    assert any(
        s.file == "src/molt/stdlib/synthetic.py" for s in discovered
    ), f"Scan C failed to discover injected todo-as-plan; got {discovered}"

    fail = gate.run_gate(tmp_path, registry)
    assert any(
        v.kind == "unregistered-poison-site" and "synthetic.py" in v.detail for v in fail
    ), f"gate MUST fail on unregistered todo_as_plan; got {fail}"

    # Fail closed instead of diverging (raise) -> not poison, passes.
    mod.write_text(
        "def public_api(method):\n"
        "    if method != 'spawn':\n"
        "        # TODO(runtime, status:divergent): fork not yet supported\n"
        "        raise NotImplementedError('fork unsupported')\n"
        "    return spawn_context()\n",
        encoding="utf-8",
    )
    assert not gate.discover_todo_as_plan(tmp_path), "fail-closed raise must not be flagged"
    assert not gate.run_gate(tmp_path, registry)


def test_negative_control_module_level_todo_is_honest(tmp_path: Path) -> None:
    """A module-level status:partial header (an honest scope declaration, not a
    per-call divergence) must NOT be flagged — the gate must not fight honesty."""
    gate = _load_gate_module()
    _write_empty_registry(tmp_path)
    mod = tmp_path / "src" / "molt" / "stdlib" / "honest.py"
    mod.parent.mkdir(parents=True, exist_ok=True)
    mod.write_text(
        "# TODO(stdlib-compat, status:partial): expand coverage beyond core subset.\n"
        "def public_api():\n    return real_result()\n",
        encoding="utf-8",
    )
    assert not gate.discover_todo_as_plan(tmp_path), (
        "a module-level status:partial header must not be flagged as poison"
    )


def test_negative_control_status_planned_is_honest(tmp_path: Path) -> None:
    """status:planned is an honest not-yet-supported declaration and is EXCLUDED
    even inside a def body (it does not claim the capability works)."""
    gate = _load_gate_module()
    _write_empty_registry(tmp_path)
    mod = tmp_path / "src" / "molt" / "stdlib" / "planned.py"
    mod.parent.mkdir(parents=True, exist_ok=True)
    mod.write_text(
        "def public_api():\n"
        "    # TODO(runtime, status:planned): implement later\n"
        "    return divergent()\n",
        encoding="utf-8",
    )
    assert not gate.discover_todo_as_plan(tmp_path), (
        "status:planned must not be flagged"
    )


# ---------------------------------------------------------------------------
# Negative control D — duplicate_authority
# ---------------------------------------------------------------------------
def test_negative_control_duplicate_authority_fails_then_passes(tmp_path: Path) -> None:
    gate = _load_gate_module()
    registry = _write_empty_registry(tmp_path)
    # The SAME CPython header, substantive on both sides, in both authority trees.
    body = "#ifndef SYN_PYERRORS_H\n#define SYN_PYERRORS_H\n" + (
        "\n".join(f"int syn_decl_{i}(void);" for i in range(40)) + "\n#endif\n"
    )
    molt_hdr = tmp_path / "include" / "pyerrors.h"
    abi_hdr = tmp_path / "runtime" / "molt-cpython-abi" / "include" / "pyerrors.h"
    molt_hdr.parent.mkdir(parents=True, exist_ok=True)
    abi_hdr.parent.mkdir(parents=True, exist_ok=True)
    molt_hdr.write_text(body, encoding="utf-8")
    abi_hdr.write_text(body, encoding="utf-8")

    discovered = gate.discover_duplicate_authority(tmp_path)
    assert any(
        s.file == "include/pyerrors.h" for s in discovered
    ), f"Scan D failed to discover duplicate header; got {discovered}"

    fail = gate.run_gate(tmp_path, registry)
    assert any(
        v.kind == "unregistered-poison-site" and "pyerrors.h" in v.detail for v in fail
    ), f"gate MUST fail on unregistered duplicate_authority; got {fail}"

    # Delete the abi-side twin (one authority) -> passes.
    abi_hdr.unlink()
    assert not gate.discover_duplicate_authority(tmp_path), (
        "a single-home header must not be flagged"
    )
    assert not gate.run_gate(tmp_path, registry)


# ---------------------------------------------------------------------------
# Ratchet negative control
# ---------------------------------------------------------------------------
def test_negative_control_ratchet_regression_fails(tmp_path: Path) -> None:
    """One more registered row of a class than its baseline fails the ratchet."""
    gate = _load_gate_module()
    # A real anchored file so the row's anchor check passes; the FAILURE under test
    # is the ratchet, not a missing anchor.
    hdr = tmp_path / "include" / "numpy" / "real.h"
    hdr.parent.mkdir(parents=True, exist_ok=True)
    hdr.write_text("typedef struct PyArray_Descr { int x; } PyArray_Descr;\n", "utf-8")
    registry = tmp_path / "tools" / "fail_closed_registry.toml"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(
        "[baseline]\n"
        "ecosystem_baked = 0\n"  # baseline 0 but one row registered => regression
        "fail_open_stub = 0\n"
        "duplicate_authority = 0\n"
        "todo_as_plan = 0\n\n"
        "[[site]]\n"
        'id = "SYN"\n'
        'class = "ecosystem_baked"\n'
        'file = "include/numpy/real.h"\n'
        'anchor = "typedef struct PyArray_Descr"\n'
        'resolution_row = "task#7"\n'
        'justification = "synthetic row over baseline"\n',
        encoding="utf-8",
    )
    fail = gate.run_gate(tmp_path, registry)
    assert any(v.kind == "ratchet-regression" for v in fail), fail


def test_negative_control_anchor_drift_fails(tmp_path: Path) -> None:
    """A registered row whose anchor no longer exists in its file fails the anchor
    integrity check (a site cannot silently move out from under its row)."""
    gate = _load_gate_module()
    hdr = tmp_path / "include" / "numpy" / "real.h"
    hdr.parent.mkdir(parents=True, exist_ok=True)
    hdr.write_text("/* the baked typedef was ripped */\n", encoding="utf-8")
    registry = tmp_path / "tools" / "fail_closed_registry.toml"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(
        "[baseline]\n"
        "ecosystem_baked = 1\n"
        "fail_open_stub = 0\n"
        "duplicate_authority = 0\n"
        "todo_as_plan = 0\n\n"
        "[[site]]\n"
        'id = "SYN"\n'
        'class = "ecosystem_baked"\n'
        'file = "include/numpy/real.h"\n'
        'anchor = "typedef struct PyArray_Descr"\n'
        'resolution_row = "task#7"\n'
        'justification = "row whose anchor was ripped"\n',
        encoding="utf-8",
    )
    fail = gate.run_gate(tmp_path, registry)
    assert any(v.kind == "anchor-drift" for v in fail), fail


if __name__ == "__main__":  # pragma: no cover
    sys.exit(pytest.main([__file__, "-q"]))
