"""Drift, algebra, and hot-path teeth for the Python effect authority."""

from __future__ import annotations

import ast
import importlib
import importlib.util
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _gen():
    return importlib.import_module("tools.gen_python_effects")


def _generated() -> ModuleType:
    path = _gen().OUT_PYTHON
    spec = importlib.util.spec_from_file_location("_molt_python_effects_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    for path, expected in gen.render_all(gen.load_schema()).items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(ROOT)} is stale; run "
            "`python tools/gen_python_effects.py`"
        )


def test_effect_bits_are_dense_raw_masks_with_unknown_as_top() -> None:
    schema = _gen().load_schema()
    generated = _generated()
    masks = [effect.mask for effect in schema.effects]
    assert [effect.bit for effect in schema.effects] == list(range(len(masks)))
    assert masks == [1 << bit for bit in range(len(masks))]
    assert generated.NO_EFFECTS == 0
    assert generated.ALL_EFFECTS == sum(masks)
    assert generated.UNKNOWN_EFFECTS == generated.ALL_EFFECTS
    assert generated.EFFECT_MASK_BITS == schema.mask_bits


def test_join_is_one_raw_or_and_obeys_bounded_lattice_laws() -> None:
    generated = _generated()
    function = next(
        node
        for node in ast.parse(_gen().OUT_PYTHON.read_text(encoding="utf-8")).body
        if isinstance(node, ast.FunctionDef) and node.name == "join_effects"
    )
    assert len(function.body) == 2  # docstring + return; no normalization branch
    result = function.body[-1]
    assert isinstance(result, ast.Return)
    assert isinstance(result.value, ast.BinOp)
    assert isinstance(result.value.op, ast.BitOr)

    masks = [row[1] for row in generated.EFFECT_ROWS]
    samples = [generated.NO_EFFECTS, generated.UNKNOWN_EFFECTS, *masks]
    samples.extend(masks[index] | masks[index + 1] for index in range(len(masks) - 1))
    for left in samples:
        assert generated.join_effects(left, generated.NO_EFFECTS) == left
        assert generated.join_effects(left, left) == left
        assert (
            generated.join_effects(left, generated.UNKNOWN_EFFECTS)
            == generated.UNKNOWN_EFFECTS
        )
        for right in samples:
            joined = generated.join_effects(left, right)
            assert joined == generated.join_effects(right, left)
            assert generated.effect_mask_has_all(joined, left)
            assert generated.effect_mask_has_all(joined, right)


def test_capabilities_are_fail_closed_monotone_projections() -> None:
    generated = _generated()
    effects = [row[1] for row in generated.EFFECT_ROWS]
    for _, forbidden, _ in generated.CAPABILITY_ROWS:
        assert generated.effect_mask_satisfies_capability(
            generated.NO_EFFECTS, forbidden
        )
        assert not generated.effect_mask_satisfies_capability(
            generated.UNKNOWN_EFFECTS, forbidden
        )
        for effect in effects:
            expected = effect & forbidden == 0
            assert (
                generated.effect_mask_satisfies_capability(effect, forbidden)
                == expected
            )
            joined = generated.join_effects(effect, forbidden)
            assert not generated.effect_mask_satisfies_capability(joined, forbidden)

    future_bit = 1 << (generated.EFFECT_MASK_BITS - 1)
    assert not generated.effect_mask_is_known(future_bit)
    assert generated.canonicalize_effects(future_bit) == generated.UNKNOWN_EFFECTS
    assert not generated.effect_mask_satisfies_capability(future_bit, 0)
    assert not generated.effect_mask_is_known(-1)


def test_import_and_lifetime_capabilities_cover_reentrant_python_paths() -> None:
    generated = _generated()
    import_forbidden = generated.PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS
    release_forbidden = generated.NO_REFERENCE_RELEASE_FORBIDDEN_EFFECTS
    for effect in (
        generated.EXECUTES_ARBITRARY_PYTHON,
        generated.INVOKES_IMPORT_SYSTEM,
        generated.RUNS_FINALIZER,
        generated.RUNS_WEAKREF_CALLBACK,
        generated.INVOKES_DESCRIPTOR,
        generated.INVOKES_ITERATION_CALLBACK,
        generated.INVOKES_CONTEXT_CALLBACK,
        generated.INVOKES_COMPARISON_CALLBACK,
    ):
        assert generated.effect_mask_has_any(import_forbidden, effect)
        assert generated.effect_mask_has_any(release_forbidden, effect)
    for effect in (
        generated.REFLECTS_NAMESPACE,
        generated.WRITES_MODULE_METADATA,
        generated.WRITES_GLOBAL_NAMESPACE,
        generated.WRITES_FRAME_STATE,
    ):
        assert generated.effect_mask_has_any(import_forbidden, effect)
    assert generated.effect_mask_has_any(
        release_forbidden, generated.RELEASES_REFERENCE
    )


def test_schema_rejects_sparse_bits_and_unknown_capability_effects(
    tmp_path: Path,
) -> None:
    gen = _gen()
    source = gen.SOURCE.read_text(encoding="utf-8")
    sparse = tmp_path / "sparse.toml"
    sparse.write_text(source.replace("bit = 1\n", "bit = 2\n", 1), encoding="utf-8")
    with pytest.raises(gen.SchemaError, match="effect bits must be dense"):
        gen.load_schema(sparse)

    unknown = tmp_path / "unknown.toml"
    unknown.write_text(
        source.replace(
            '  "executes_arbitrary_python",\n',
            '  "not_an_effect",\n',
            1,
        ),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="unknown effects"):
        gen.load_schema(unknown)


def test_schema_rejects_duplicate_capability_projections(tmp_path: Path) -> None:
    gen = _gen()
    source = gen.SOURCE.read_text(encoding="utf-8")
    duplicate = tmp_path / "duplicate.toml"
    duplicate.write_text(
        source
        + "\n[[capability]]\n"
        + 'name = "duplicate_projection"\n'
        + 'description = "Must not duplicate an existing capability mask."\n'
        + 'forbidden = ["raises"]\n',
        encoding="utf-8",
    )
    # Make the existing cannot_raise projection identical to the appended row.
    duplicate.write_text(
        duplicate.read_text(encoding="utf-8").replace(
            'name = "cannot_raise"\n'
            'description = "Proves the operation cannot raise or transfer into code that may raise."\n'
            "forbidden = [\n"
            '  "executes_arbitrary_python",\n'
            '  "invokes_import_system",\n'
            '  "runs_finalizer",\n'
            '  "runs_weakref_callback",\n'
            '  "invokes_descriptor",\n'
            '  "invokes_iteration_callback",\n'
            '  "invokes_context_callback",\n'
            '  "invokes_comparison_callback",\n'
            '  "raises",\n'
            "]\n",
            'name = "cannot_raise"\n'
            'description = "Proves the operation cannot raise."\n'
            'forbidden = ["raises"]\n',
            1,
        ),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="must not duplicate"):
        gen.load_schema(duplicate)


def test_rust_projection_is_public_and_compiles_as_molt_ir_authority() -> None:
    lib = (ROOT / "runtime/molt-ir/src/lib.rs").read_text(encoding="utf-8")
    rust = _gen().OUT_RUST.read_text(encoding="utf-8")
    assert "pub mod python_effects_generated;" in lib
    assert "pub type PythonEffectMask = u32;" in rust
    assert "pub const UNKNOWN_PYTHON_EFFECTS" in rust
    assert "left | right" in rust
    assert "python_effect_mask_satisfies_capability" in rust
