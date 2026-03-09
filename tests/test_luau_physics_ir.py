"""
Tests for the Luau physics → Molt SimpleIR transpiler.

Validates that the generated IR correctly represents Luau physics functions
and can be consumed by the Molt WASM backend.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

# Import the transpiler
import sys
sys.path.insert(0, str(Path(__file__).parent.parent / "tools"))
from luau_physics_to_ir import (  # noqa: E402
    OpIR,
    SimpleIR,
    parse_luau_file,
    parse_physics_directory,
    tokenize,
    LuauPhysicsParser,
)

VERTIGO_PHYSICS_DIR = Path(__file__).parent.parent.parent / "vertigo" / "src" / "Shared" / "Util" / "Physics"
TRAJECTORY_LUAU = VERTIGO_PHYSICS_DIR / "Trajectory.luau"
SPRING_LUAU = VERTIGO_PHYSICS_DIR / "Spring.luau"


# ---------------------------------------------------------------------------
# IR contract validation
# ---------------------------------------------------------------------------


class TestIRContract:
    """Verify generated IR matches the molt.simple_ir contract."""

    def test_contract_fields_present(self):
        """IR JSON must contain ir_contract_name and ir_contract_version."""
        if not TRAJECTORY_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        ir = parse_luau_file(TRAJECTORY_LUAU, function_filter="applyDrag")
        d = ir.to_dict()
        assert d["ir_contract_name"] == "molt.simple_ir"
        assert d["ir_contract_version"] == 1
        assert "functions" in d

    def test_function_structure(self):
        """Each function must have name, params, and ops."""
        if not TRAJECTORY_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        ir = parse_luau_file(TRAJECTORY_LUAU, function_filter="applyDrag")
        assert len(ir.functions) == 1
        func = ir.functions[0]
        assert func.name == "Trajectory_applyDrag"
        assert func.params == ["velocity", "dragCoeff", "dt"]
        assert len(func.ops) > 0

    def test_all_ops_have_kind(self):
        """Every op must have a non-empty kind field."""
        if not TRAJECTORY_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        ir = parse_luau_file(TRAJECTORY_LUAU)
        for func in ir.functions:
            for i, op in enumerate(func.ops):
                assert op.kind, f"Op {i} in {func.name} has empty kind"

    def test_json_roundtrip(self):
        """IR → JSON → parse should produce valid JSON."""
        if not TRAJECTORY_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        ir = parse_luau_file(TRAJECTORY_LUAU, function_filter="applyDrag")
        json_str = ir.to_json()
        parsed = json.loads(json_str)
        assert parsed["ir_contract_name"] == "molt.simple_ir"
        assert len(parsed["functions"]) == 1
        assert parsed["functions"][0]["name"] == "Trajectory_applyDrag"


# ---------------------------------------------------------------------------
# Trajectory.applyDrag — simplest physics function
# ---------------------------------------------------------------------------


class TestApplyDragIR:
    """
    Trajectory.applyDrag is: velocity * math.exp(-dragCoeff * dt)

    Expected IR:
    1. neg(dragCoeff) → t1
    2. mul(t1, dt) → t2
    3. call_intrinsic("math_exp", [t2]) → t3
    4. mul(velocity, t3) → t4
    5. ret([t4])
    """

    @pytest.fixture
    def apply_drag_ops(self) -> list[dict]:
        if not TRAJECTORY_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        ir = parse_luau_file(TRAJECTORY_LUAU, function_filter="applyDrag")
        assert len(ir.functions) == 1
        return [op.to_dict() for op in ir.functions[0].ops]

    def test_op_count(self, apply_drag_ops: list[dict]):
        assert len(apply_drag_ops) == 5

    def test_negate_drag(self, apply_drag_ops: list[dict]):
        op = apply_drag_ops[0]
        assert op["kind"] == "neg"
        assert "dragCoeff" in op["args"]

    def test_mul_by_dt(self, apply_drag_ops: list[dict]):
        op = apply_drag_ops[1]
        assert op["kind"] == "mul"
        assert "dt" in op["args"]

    def test_math_exp_call(self, apply_drag_ops: list[dict]):
        op = apply_drag_ops[2]
        assert op["kind"] == "call_intrinsic"
        assert op["s_value"] == "math_exp"
        assert len(op["args"]) == 1

    def test_mul_velocity(self, apply_drag_ops: list[dict]):
        op = apply_drag_ops[3]
        assert op["kind"] == "mul"
        assert "velocity" in op["args"]

    def test_return(self, apply_drag_ops: list[dict]):
        op = apply_drag_ops[4]
        assert op["kind"] == "ret"
        assert len(op["args"]) == 1


# ---------------------------------------------------------------------------
# Spring module — analytical solver with branching
# ---------------------------------------------------------------------------


class TestSpringModuleIR:
    """Validate Spring.luau parsing produces correct function set."""

    @pytest.fixture
    def spring_ir(self) -> SimpleIR:
        if not SPRING_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        return parse_luau_file(SPRING_LUAU)

    def test_function_count(self, spring_ir: SimpleIR):
        """Spring.luau should produce 15 functions."""
        assert len(spring_ir.functions) == 15

    def test_solver_function_exists(self, spring_ir: SimpleIR):
        """The core solver function must be present."""
        names = [f.name for f in spring_ir.functions]
        assert "solveSpringNumber" in names

    def test_solver_params(self, spring_ir: SimpleIR):
        """solveSpringNumber takes 7 parameters: x0, v0, tgt, k, c, m, dt."""
        solver = next(f for f in spring_ir.functions if f.name == "solveSpringNumber")
        assert solver.params == ["x0", "v0", "tgt", "k", "c", "m", "dt"]

    def test_solver_has_branching(self, spring_ir: SimpleIR):
        """solveSpringNumber must contain if/else for 3 damping regimes."""
        solver = next(f for f in spring_ir.functions if f.name == "solveSpringNumber")
        jump_ops = [op for op in solver.ops if op.kind == "jump_if_false"]
        # At least 2 branches: over-damped and under-damped checks
        assert len(jump_ops) >= 2

    def test_solver_has_math_intrinsics(self, spring_ir: SimpleIR):
        """solveSpringNumber must call sqrt, exp, cos, sin."""
        solver = next(f for f in spring_ir.functions if f.name == "solveSpringNumber")
        intrinsics = {
            op.s_value for op in solver.ops if op.kind == "call_intrinsic"
        }
        assert "math_sqrt" in intrinsics
        assert "math_exp" in intrinsics
        assert "math_cos" in intrinsics
        assert "math_sin" in intrinsics

    def test_presets_produce_correct_ops(self, spring_ir: SimpleIR):
        """Preset functions should call constructors with literal arguments."""
        smooth = next(f for f in spring_ir.functions if f.name == "Spring_smooth")
        call_ops = [op for op in smooth.ops if op.kind == "call_func"]
        assert len(call_ops) == 1
        assert call_ops[0].s_value == "Spring.newNumber"

    def test_update_calls_solver(self, spring_ir: SimpleIR):
        """Spring.updateNumber should call solveSpringNumber."""
        update = next(f for f in spring_ir.functions if f.name == "Spring_updateNumber")
        calls = [op for op in update.ops if op.kind == "call_func"]
        assert any("solveSpringNumber" in (c.s_value or "") for c in calls)


# ---------------------------------------------------------------------------
# Trajectory module — ballistic arcs
# ---------------------------------------------------------------------------


class TestTrajectoryModuleIR:
    """Validate Trajectory.luau parsing."""

    @pytest.fixture
    def trajectory_ir(self) -> SimpleIR:
        if not TRAJECTORY_LUAU.exists():
            pytest.skip("Vertigo physics source not found")
        return parse_luau_file(TRAJECTORY_LUAU)

    def test_function_count(self, trajectory_ir: SimpleIR):
        assert len(trajectory_ir.functions) == 5

    def test_function_names(self, trajectory_ir: SimpleIR):
        names = sorted(f.name for f in trajectory_ir.functions)
        assert names == sorted([
            "Trajectory_predict",
            "Trajectory_predictWithDrag",
            "Trajectory_timeToTarget",
            "Trajectory_launchAngle",
            "Trajectory_applyDrag",
        ])

    def test_predict_has_loop(self, trajectory_ir: SimpleIR):
        """predict() uses a for loop over steps."""
        predict = next(f for f in trajectory_ir.functions if f.name == "Trajectory_predict")
        labels = [op for op in predict.ops if op.kind == "label"]
        jumps = [op for op in predict.ops if op.kind == "jump"]
        assert len(labels) >= 2  # loop start + end
        assert len(jumps) >= 1  # back-edge

    def test_launch_angle_has_discriminant_check(self, trajectory_ir: SimpleIR):
        """launchAngle() must check discriminant < 0."""
        la = next(f for f in trajectory_ir.functions if f.name == "Trajectory_launchAngle")
        compares = [op for op in la.ops if op.kind.startswith("compare")]
        assert len(compares) >= 2  # R < 1e-9 and discriminant < 0


# ---------------------------------------------------------------------------
# Tokenizer unit tests
# ---------------------------------------------------------------------------


class TestTokenizer:
    def test_basic_expression(self):
        tokens = tokenize("local x = 1 + 2")
        types = [t.type for t in tokens]
        assert types == ["KW_LOCAL", "IDENT", "ASSIGN", "NUMBER", "PLUS", "NUMBER"]

    def test_math_call(self):
        tokens = tokenize("math.sqrt(x)")
        types = [t.type for t in tokens]
        assert types == ["IDENT", "DOT", "IDENT", "LPAREN", "IDENT", "RPAREN"]

    def test_comparison(self):
        tokens = tokenize("if x > 1 then")
        types = [t.type for t in tokens]
        assert types == ["KW_IF", "IDENT", "GT", "NUMBER", "KW_THEN"]

    def test_float_literal(self):
        tokens = tokenize("1e-6")
        assert len(tokens) == 1
        assert tokens[0].type == "NUMBER"

    def test_comments_stripped(self):
        tokens = tokenize("-- this is a comment\nlocal x = 1")
        types = [t.type for t in tokens]
        assert "COMMENT" not in types
        assert types == ["KW_LOCAL", "IDENT", "ASSIGN", "NUMBER"]
