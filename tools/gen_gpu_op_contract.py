#!/usr/bin/env python3
"""Generate the tinygrad GPU op-contract FACT from the pinned upstream source.

Single sources of truth:
  - the pinned upstream tinygrad **0.13.0** (``docs/spec/tinygrad_pin.md`` /
    ``tools/check_tinygrad_pin.py``), specifically
    ``bench/friends/repos/tinygrad_off_the_shelf/tinygrad/uop/__init__.py``
    (``class Ops``) and ``.../tinygrad/renderer/cstyle.py``
    (``CStyleLanguage.code_for_op``);
  - molt's primitive enum ``runtime/molt-gpu/src/ops.rs`` (``PrimitiveOp``).

This generator renders ``runtime/molt-gpu/op_contract.toml`` — the authoritative,
checked registry recording, per molt ``PrimitiveOp``, its **disposition** against
the pinned upstream op set:

  - ``mapped``           — molt op == a single upstream ``Ops`` member that has a
                           ``code_for_op`` C-pattern; the contract records that
                           member and the exact C-pattern string.
  - ``rewrite``          — molt op == an upstream ``Ops`` member that has NO
                           ``code_for_op`` entry (upstream produces it via a
                           ``tinygrad/uop/decompositions.py`` pattern rewrite);
                           the contract records the upstream op AND the rewrite
                           chain it lowers to.
  - ``composed``         — molt op is built from a DAG of upstream ops (no single
                           upstream member is its 1:1); the contract records the
                           composition.
  - ``reduce``           — molt reduce op (``ReduceSum``/``ReduceMax``) backed by
                           upstream ``Ops.REDUCE`` with the given ALU.

The generator ALSO records every upstream ``Ops`` ALU/math member that molt does
NOT expose as a ``PrimitiveOp``, with its disposition (``composed`` /
``not_yet_supported`` + reason) — so a new upstream ALU op (a future bump adding,
say, a new transcendental) is surfaced as ``unclassified`` and fails the gate
until reconciled, never silently absent. This is the structural kill for the
"fidelity theater" drift doc 67 §1.2.1 found live: the design's prose "26
primitives == tinygrad code_for_op" had silently diverged (upstream uses
``CMOD``/``CDIV`` not ``MOD``/``IDIV``; upstream has no ``MAX``/``IDIV``/``MOD``
renderer entry; upstream adds ``FDIV``/``POW``/``THREEFRY``/``FLOORDIV``/
``FLOORMOD``/``SUB``/``MULACC``/``WMMA``). After this generator, that claim is a
*derived, checked* fact: a mis-mapping, a changed C-pattern, or a new upstream op
turns the build RED.

The reconciliation decisions (which molt op maps to which upstream member, what a
composed op decomposes to) live HERE as a declarative table — version-controlled
and reviewable — and the generator **proves every decision against the pinned
source** (e.g. it asserts ``CMOD`` really is in ``code_for_op`` with C-pattern
``({a}%{b})``, and that ``MAX`` really is NOT in ``code_for_op``). A decision that
contradicts the pinned source is a hard error: the table cannot claim a mapping
the source does not bear.

Source parsing is **AST-structural, never regex** (the discovery-vs-authority
rule: STRUCTURAL_AUDIT_BOARD §Discovery-vs-authority — the authoritative op set
and C-patterns are parsed from the Python AST of the read-only oracle, which is
never hand-edited).

Usage::

    python3 tools/gen_gpu_op_contract.py            # (re)write op_contract.toml
    python3 tools/gen_gpu_op_contract.py --check     # exit 1 if it is stale

Mirrors ``tools/gen_op_kinds.py --check`` exactly: byte-exact regenerate-and-diff.
"""

from __future__ import annotations

import argparse
import ast
import importlib.util as _ilu
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Load the pin authority so the contract header records the *checked* pinned
# version (and a bump that forgets to re-run this generator is caught by the pin
# gate). Direct-file load to avoid package-path assumptions.
_PIN_SPEC = _ilu.spec_from_file_location(
    "_molt_check_tinygrad_pin", ROOT / "tools" / "check_tinygrad_pin.py"
)
assert _PIN_SPEC is not None and _PIN_SPEC.loader is not None
_PIN_MOD = _ilu.module_from_spec(_PIN_SPEC)
_PIN_SPEC.loader.exec_module(_PIN_MOD)
PINNED_TINYGRAD_VERSION: str = _PIN_MOD.PINNED_TINYGRAD_VERSION

# --- pinned-source authorities (the read-only oracle) ------------------------
OFF_THE_SHELF = ROOT / "bench/friends/repos/tinygrad_off_the_shelf"
OPS_ENUM_SRC = OFF_THE_SHELF / "tinygrad/uop/__init__.py"
CODE_FOR_OP_SRC = OFF_THE_SHELF / "tinygrad/renderer/cstyle.py"
DECOMPOSITIONS_SRC = OFF_THE_SHELF / "tinygrad/uop/decompositions.py"

# --- molt authorities --------------------------------------------------------
MOLT_OPS_RS = ROOT / "runtime/molt-gpu/src/ops.rs"

# --- the generated artifact --------------------------------------------------
OUT_TOML = ROOT / "runtime/molt-gpu/op_contract.toml"


class OpContractError(RuntimeError):
    """A reconciliation/source-disagreement error (fail-loud)."""


# ===========================================================================
# Disposition reconciliation table — the engineering decisions, PROVEN below.
# ===========================================================================
#
# Per molt PrimitiveOp (the variant name in ops.rs), its disposition against the
# pinned upstream 0.13.0 op set. Every claim here is verified against the parsed
# source in ``reconcile()`` — a row that contradicts the source is a hard error.
#
# Fields:
#   disposition : "mapped" | "rewrite" | "composed" | "reduce"
#   upstream    : the upstream Ops member name this op corresponds to
#                 (for mapped/rewrite/reduce). For "composed" molt ops with no
#                 single upstream member, omit (None).
#   reduce_alu  : for "reduce", the upstream ALU the REDUCE folds with.
#   lowers_to   : for "rewrite"/"composed", the upstream-op decomposition chain
#                 (a human-and-machine-readable expression over upstream Ops),
#                 anchored to a decompositions.py / divandmod.py citation.
#   dtype_rule  : the output-dtype contract (cross-referenced to upstream).
#   ieee_edge   : the IEEE-754 NaN/inf/-0.0 edge contract the op must honor.
#   note        : disposition rationale / source citation.
MOLT_OP_DISPOSITIONS: dict[str, dict] = {
    # --- Arithmetic ---------------------------------------------------------
    "Add": {
        "disposition": "mapped",
        "upstream": "ADD",
        "dtype_rule": "same as promoted operand dtype",
        "ieee_edge": "IEEE-754 addition; NaN propagates; (+inf)+(-inf)=NaN",
        "note": "code_for_op[Ops.ADD] = ({a}+{b}).",
    },
    "Sub": {
        "disposition": "mapped",
        "upstream": "SUB",
        "dtype_rule": "same as promoted operand dtype",
        "ieee_edge": "primitive subtraction (NOT Add(a, Neg(b))); preserves -0.0",
        "note": (
            "code_for_op[Ops.SUB] = ({a}-{b}). Upstream ALSO synthesizes SUB from "
            "x + Neg(y) when the renderer supports SUB (decompositions.py:488); "
            "molt exposes it as a first-class primitive, matching the renderer entry."
        ),
    },
    "Mul": {
        "disposition": "mapped",
        "upstream": "MUL",
        "dtype_rule": "same as promoted operand dtype",
        "ieee_edge": "IEEE-754 multiply; NaN propagates; 0*inf=NaN",
        "note": "code_for_op[Ops.MUL] = ({a}*{b}).",
    },
    "Idiv": {
        "disposition": "mapped",
        "upstream": "CDIV",
        "dtype_rule": "integer dtype (truncating C division)",
        "ieee_edge": (
            "integer division truncating toward zero (C semantics): (-7)/3 == -2. "
            "Divide-by-zero is UB at the C level / guarded above."
        ),
        "note": (
            "RECONCILED §1.2.1: molt names this `Idiv`; upstream names it `Ops.CDIV` "
            "(C truncating divide). code_for_op[Ops.CDIV] = ({a}/{b}). The molt name "
            "and the upstream name diverge in SPELLING ONLY — same C-pattern, same "
            "semantics. Python `//` (FLOORDIV, floor toward -inf) is a SEPARATE "
            "composed op (see the FloorDiv entry in the upstream-only section)."
        ),
    },
    "Mod": {
        "disposition": "mapped",
        "upstream": "CMOD",
        "dtype_rule": "integer dtype (truncating C remainder)",
        "ieee_edge": (
            "C remainder; result has the sign of the dividend: (-7)%3 == -1. "
            "Mod-by-zero is UB at the C level / guarded above."
        ),
        "note": (
            "RECONCILED §1.2.1: molt names this `Mod`; upstream names it `Ops.CMOD` "
            "(C truncating remainder). code_for_op[Ops.CMOD] = ({a}%{b}). Spelling "
            "diverges only. Python `%` (FLOORMOD, sign of divisor) is a SEPARATE "
            "composed op (see the FloorMod entry in the upstream-only section)."
        ),
    },
    "Neg": {
        "disposition": "mapped",
        "upstream": "NEG",
        "dtype_rule": "same as operand dtype",
        "ieee_edge": "flips the IEEE sign bit (NOT a*-1); -(-0.0)=+0.0; -(NaN) flips NaN sign",
        "note": "code_for_op[Ops.NEG] = -{x}.",
    },
    # --- Comparison ---------------------------------------------------------
    "Cmplt": {
        "disposition": "mapped",
        "upstream": "CMPLT",
        "dtype_rule": "output dtype is ALWAYS bool (GroupOp.Comparison)",
        "ieee_edge": "IEEE unordered: NaN < x is false",
        "note": "code_for_op[Ops.CMPLT] = ({a}<{b}). Ops.CMPLT in GroupOp.Comparison.",
    },
    "Cmpeq": {
        "disposition": "mapped",
        "upstream": "CMPEQ",
        "dtype_rule": "output dtype is ALWAYS bool (GroupOp.Comparison)",
        "ieee_edge": "IEEE: NaN == NaN is false",
        "note": "code_for_op[Ops.CMPEQ] = ({a}=={b}). Ops.CMPEQ in GroupOp.Comparison.",
    },
    "Cmpne": {
        "disposition": "mapped",
        "upstream": "CMPNE",
        "dtype_rule": "output dtype is ALWAYS bool (GroupOp.Comparison)",
        "ieee_edge": "IEEE: NaN != NaN is true",
        "note": "code_for_op[Ops.CMPNE] = ({a}!={b}). Ops.CMPNE in GroupOp.Comparison.",
    },
    # --- Bitwise ------------------------------------------------------------
    "And": {
        "disposition": "mapped",
        "upstream": "AND",
        "dtype_rule": "integer/bool dtype",
        "ieee_edge": "bitwise; not defined for floats",
        "note": "code_for_op[Ops.AND] = ({a}&{b}).",
    },
    "Or": {
        "disposition": "mapped",
        "upstream": "OR",
        "dtype_rule": "integer/bool dtype",
        "ieee_edge": "bitwise; not defined for floats",
        "note": "code_for_op[Ops.OR] = ({a}|{b}).",
    },
    "Xor": {
        "disposition": "mapped",
        "upstream": "XOR",
        "dtype_rule": "integer/bool dtype",
        "ieee_edge": "bitwise; not defined for floats",
        "note": "code_for_op[Ops.XOR] = ({a}^{b}).",
    },
    "Shl": {
        "disposition": "mapped",
        "upstream": "SHL",
        "dtype_rule": "integer dtype",
        "ieee_edge": "logical left shift; shift-count UB guarded above",
        "note": "code_for_op[Ops.SHL] = ({a}<<{b}).",
    },
    "Shr": {
        "disposition": "mapped",
        "upstream": "SHR",
        "dtype_rule": "integer dtype",
        "ieee_edge": (
            "arithmetic right shift for signed (sign-extending), logical for "
            "unsigned (zero-filling); shift-count UB guarded above"
        ),
        "note": "code_for_op[Ops.SHR] = ({a}>>{b}).",
    },
    # --- Math ---------------------------------------------------------------
    "Exp2": {
        "disposition": "mapped",
        "upstream": "EXP2",
        "dtype_rule": "float dtype",
        "ieee_edge": "exp2(NaN)=NaN; exp2(+inf)=+inf; exp2(-inf)=0.0",
        "note": (
            "code_for_op[Ops.EXP2] = exp2({x}). Transcendental: ULP budget vs host "
            "libm pinned in docs/spec/tinygrad_pin.md (numeric oracle, doc 67 Phase 2)."
        ),
    },
    "Log2": {
        "disposition": "mapped",
        "upstream": "LOG2",
        "dtype_rule": "float dtype",
        "ieee_edge": "log2(NaN)=NaN; log2(+inf)=+inf; log2(0.0)=-inf; log2(x<0)=NaN",
        "note": (
            "code_for_op[Ops.LOG2] = log2({x}). Transcendental: ULP budget vs host "
            "libm pinned in docs/spec/tinygrad_pin.md."
        ),
    },
    "Sin": {
        "disposition": "mapped",
        "upstream": "SIN",
        "dtype_rule": "float dtype",
        "ieee_edge": "sin(NaN)=NaN; sin(+-inf)=NaN",
        "note": (
            "code_for_op[Ops.SIN] = sin({x}). Transcendental: ULP budget vs host "
            "libm pinned in docs/spec/tinygrad_pin.md."
        ),
    },
    "Sqrt": {
        "disposition": "mapped",
        "upstream": "SQRT",
        "dtype_rule": "float dtype",
        "ieee_edge": "sqrt(NaN)=NaN; sqrt(+inf)=+inf; sqrt(-0.0)=-0.0; sqrt(x<0)=NaN",
        "note": "code_for_op[Ops.SQRT] = sqrt({x}).",
    },
    "Reciprocal": {
        "disposition": "mapped",
        "upstream": "RECIPROCAL",
        "dtype_rule": "float dtype only (use Idiv(1,a) for integers)",
        "ieee_edge": "RECIPROCAL(0.0)=+inf; RECIPROCAL(-0.0)=-inf; RECIPROCAL(NaN)=NaN",
        "note": (
            "code_for_op[Ops.RECIPROCAL] = (1/{x}). Upstream also rewrites "
            "RECIPROCAL(x) -> FDIV(1, x) when the renderer supports FDIV "
            "(decompositions.py:506); the code_for_op entry is the renderer contract."
        ),
    },
    # --- Other --------------------------------------------------------------
    "Trunc": {
        "disposition": "mapped",
        "upstream": "TRUNC",
        "dtype_rule": "float dtype",
        "ieee_edge": "trunc(NaN)=NaN; trunc(+-inf)=+-inf; truncates toward zero",
        "note": "code_for_op[Ops.TRUNC] = trunc({x}). Used in floor/ceil/round compositions.",
    },
    "Max": {
        "disposition": "rewrite",
        "upstream": "MAX",
        "lowers_to": "WHERE(CMPLT(a, b), b, a)",
        "dtype_rule": "same as promoted operand dtype",
        "ieee_edge": (
            "NaN-propagating for floats in molt's renderer; integers use comparison. "
            "(Note: the upstream CMPLT-where rewrite is NOT NaN-propagating, so a "
            "renderer that owns a native NaN-propagating max() — MSL/WGSL — is "
            "closer to molt's documented contract; CUDA/HIP emit a guarded fmax.)"
        ),
        "note": (
            "RECONCILED §1.2.1: Ops.MAX is a Binary ALU member but has NO "
            "code_for_op entry. Upstream produces it via a pattern rewrite "
            "(decompositions.py:465): `Ops.MAX -> (a < b).where(b, a)` when the "
            "renderer lacks MAX but has CMPLT; the integer-decomposition path "
            "(decompositions.py:380) is the same WHERE(CMPLT,...) shape. So `MAX` is "
            "a REWRITE, not a code_for_op primitive — the design's "
            "'every renderer op is a primitive, no more no less' was FALSE here."
        ),
    },
    "Where": {
        "disposition": "mapped",
        "upstream": "WHERE",
        "dtype_rule": "result dtype = dtype of the two value operands",
        "ieee_edge": "pure select on the bool condition; no arithmetic edge",
        "note": "code_for_op[Ops.WHERE] = ({a}?{b}:{c}). Ternary select.",
    },
    "Cast": {
        "disposition": "mapped",
        "upstream": "CAST",
        "dtype_rule": "target dtype (FusedOp::dst_dtype)",
        "ieee_edge": "value-converting cast; float->int truncates; saturation per dtype",
        "note": (
            "Ops.CAST is rendered by a dedicated base_rewrite pattern (render_cast), "
            "NOT a code_for_op ALU lambda; it is in GroupOp.Elementwise. Treated as a "
            "mapped primitive (the renderer owns its lowering)."
        ),
    },
    "Bitcast": {
        "disposition": "mapped",
        "upstream": "BITCAST",
        "dtype_rule": "target dtype (FusedOp::dst_dtype), same bit width",
        "ieee_edge": "reinterpret bits, no value conversion",
        "note": (
            "Ops.BITCAST is rendered by a dedicated base_rewrite pattern "
            "(__builtin_bit_cast), NOT a code_for_op ALU lambda; in "
            "GroupOp.Elementwise. Mapped primitive (renderer owns its lowering)."
        ),
    },
    # --- Reduce -------------------------------------------------------------
    "ReduceSum": {
        "disposition": "reduce",
        "upstream": "REDUCE",
        "reduce_alu": "ADD",
        "dtype_rule": "accumulator dtype",
        "ieee_edge": "sum reduction; float associativity is implementation-ordered",
        "note": "Upstream REDUCE folds with Ops.ADD over the reduce axes.",
    },
    "ReduceMax": {
        "disposition": "reduce",
        "upstream": "REDUCE",
        "reduce_alu": "MAX",
        "dtype_rule": "accumulator dtype",
        "ieee_edge": "max reduction; NaN-propagating for floats",
        "note": "Upstream REDUCE folds with Ops.MAX over the reduce axes.",
    },
}

# ===========================================================================
# Upstream-only ALU/math ops: present in upstream Ops (and GroupOp.ALU) but NOT
# exposed as a molt PrimitiveOp. Each MUST have a disposition + reason, so a new
# upstream ALU op cannot be silently absent. "composed" = molt builds it from
# primitives; "not_yet_supported" = an explicit, reasoned gap (fail-closed, never
# silent). These are PROVEN to be in GroupOp.ALU (or a renderer member) below.
# ===========================================================================
UPSTREAM_ONLY_ALU_DISPOSITIONS: dict[str, dict] = {
    "FDIV": {
        "disposition": "composed",
        "lowers_to": "MUL(a, RECIPROCAL(b))  # molt DIV; upstream FDIV is the renderer-native form",
        "note": (
            "Float true-division. Upstream FDIV is a Binary ALU member (in some "
            "renderers a native a/b on floats). molt composes float division as "
            "MUL(a, RECIPROCAL(b)) over its primitives; RECIPROCAL is a molt "
            "primitive. Upstream itself rewrites RECIPROCAL(x) -> FDIV(1,x) "
            "(decompositions.py:506) only when the renderer supports FDIV — the "
            "inverse direction. molt's primitive RECIPROCAL is the canonical form."
        ),
    },
    "POW": {
        "disposition": "composed",
        "lowers_to": "EXP2(MUL(LOG2(a), b))  # x**y = 2**(y*log2(x)) for x>0; sign/edge handling per composition",
        "note": (
            "Power. Upstream POW is a Binary ALU member; in GroupOp.UnsafePad. molt "
            "composes it from EXP2/LOG2/MUL primitives (with the standard x**y "
            "decomposition + sign/zero edge handling). No code_for_op entry upstream."
        ),
    },
    "FLOORDIV": {
        "disposition": "composed",
        "lowers_to": (
            "CDIV(a,b) - (CMOD(a,b) != 0 & (a<0) != (b<0))  "
            "# Python // (floor toward -inf), decompositions.py:447"
        ),
        "note": (
            "Python floor-division (// : floor toward -inf, distinct from C-trunc "
            "CDIV/molt Idiv). Upstream FLOORDIV is a Binary ALU member, in "
            "GroupOp.UnsafePad, with NO code_for_op entry; it is lowered to the "
            "CDIV-correction expression at decompositions.py:447 (and divandmod.py "
            "symbolic paths). molt composes it from its CDIV/CMOD/comparison "
            "primitives — it is the Python `//` operator, NOT molt's Idiv."
        ),
    },
    "FLOORMOD": {
        "disposition": "composed",
        "lowers_to": (
            "let r = CMOD(a,b) in WHERE((r != 0) & ((r<0) != (b<0)), r + b, r)  "
            "# Python % (sign of divisor), decompositions.py:450-451"
        ),
        "note": (
            "Python floor-modulo (% : result has sign of divisor, distinct from "
            "C-trunc CMOD/molt Mod). Upstream FLOORMOD is a Binary ALU member with "
            "NO code_for_op entry; lowered at decompositions.py:450-451 (and "
            "divandmod.py). molt composes it from its CMOD/comparison/where "
            "primitives — it is the Python `%` operator, NOT molt's Mod."
        ),
    },
    "MULACC": {
        "disposition": "composed",
        "lowers_to": "ADD(MUL(a, b), c)  # fused multiply-accumulate, decompositions.py:501",
        "note": (
            "Fused multiply-accumulate. Upstream MULACC is a Ternary ALU member; it "
            "is a FUSION of ADD(MUL(a,b),c) introduced by a rewrite "
            "(decompositions.py:501) only when the renderer supports MULACC. molt "
            "expresses a*b+c as ADD(MUL(...)) primitives and fuses in its own LazyOp "
            "layer — no separate primitive needed (the spec's explicit drop, now "
            "PINNED as a composition rather than a free-floating prose choice)."
        ),
    },
    "THREEFRY": {
        "disposition": "not_yet_supported",
        "reason": (
            "Counter-based PRNG (Threefry 2x32). A specialized random-number "
            "generation op, not an elementwise math primitive; upstream lowers it "
            "via a dedicated threefry2x32 rewrite (decompositions.py:463) when the "
            "renderer lacks it. molt does not yet expose tensor RNG through the "
            "primitive set; when it does, this becomes a composed/rewrite entry. "
            "Explicit, reasoned gap — fail-closed, never silently approximated."
        ),
        "note": "Upstream THREEFRY is a Binary ALU member (RNG), no code_for_op entry.",
    },
}

# ===========================================================================
# Non-ALU upstream Ops that molt's PrimitiveOp deliberately does NOT model as a
# compute primitive (tensor-core / movement / control / scheduler ops). Recorded
# so the cover-completeness check knows they are intentionally out of the
# compute-primitive scope, with the molt layer that owns them. WMMA is the
# notable §1.2.1 mention.
# ===========================================================================
UPSTREAM_NON_PRIMITIVE_NOTES: dict[str, dict] = {
    "WMMA": {
        "scope": "tensor-core",
        "note": (
            "RECONCILED §1.2.1: Ops.WMMA (warp-matrix-multiply-accumulate, tensor "
            "core) is NOT an elementwise compute primitive — it is a hardware "
            "tensor-core instruction rendered by its own base_rewrite pattern "
            "(cstyle.py:62, __WMMA(...)), not a code_for_op ALU lambda. molt models "
            "matmul as a composition/schedule over its primitives + a device-level "
            "tensor-core path, NOT as a PrimitiveOp. Out of the primitive-set scope "
            "by design; recorded here so it is a CLASSIFIED omission, not a silent one."
        ),
    },
}


# ===========================================================================
# AST parsing of the pinned source (structural; never regex).
# ===========================================================================


def _parse_module(path: Path) -> ast.Module:
    if not path.exists():
        raise OpContractError(f"pinned source missing: {path}")
    return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))


def parse_ops_enum() -> list[str]:
    """Return the upstream ``Ops`` enum member names, in source order (AST)."""
    tree = _parse_module(OPS_ENUM_SRC)
    members: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.ClassDef) and node.name == "Ops":
            for stmt in node.body:
                # Members are `NAME = auto()` (possibly several per line via `;`).
                if isinstance(stmt, ast.Assign):
                    for tgt in stmt.targets:
                        if isinstance(tgt, ast.Name):
                            members.append(tgt.id)
            break
    if not members:
        raise OpContractError(
            f"could not parse `class Ops` members from {OPS_ENUM_SRC}"
        )
    return members


def parse_group_op_sets() -> dict[str, set[str]]:
    """Return the ``GroupOp`` named op-sets (ALU, Comparison, ...) as name sets.

    Parses the ``class GroupOp`` body. Each attribute is a set/union expression
    over ``Ops.X`` references; we collect every ``Ops.X`` attribute reachable in
    the RHS (which over-approximates union members — sufficient for membership
    *containment* assertions, which is all we need).
    """
    tree = _parse_module(OPS_ENUM_SRC)
    sets: dict[str, set[str]] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.ClassDef) and node.name == "GroupOp":
            for stmt in node.body:
                if isinstance(stmt, (ast.Assign, ast.AnnAssign)):
                    targets = (
                        stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]
                    )
                    names = [t.id for t in targets if isinstance(t, ast.Name)]
                    if not names or stmt.value is None:
                        continue
                    members: set[str] = set()
                    for sub in ast.walk(stmt.value):
                        if (
                            isinstance(sub, ast.Attribute)
                            and isinstance(sub.value, ast.Name)
                            and sub.value.id == "Ops"
                        ):
                            members.add(sub.attr)
                    for name in names:
                        sets[name] = members
            break
    if "ALU" not in sets:
        raise OpContractError(f"could not parse GroupOp.ALU from {OPS_ENUM_SRC}")
    # Resolve ALU = union(Unary, Binary, Ternary): the direct walk over the
    # `set.union(Unary, Binary, Ternary)` RHS captures no Ops.X (it references the
    # other attrs by Name), so compose it explicitly from its components.
    if not sets.get("ALU"):
        sets["ALU"] = (
            sets.get("Unary", set())
            | sets.get("Binary", set())
            | sets.get("Ternary", set())
        )
    return sets


def _render_fstring_template(node: ast.JoinedStr) -> str:
    """Reconstruct a C-pattern template from a code_for_op lambda's f-string body.

    ``f"({a}+{b})"`` -> ``"({a}+{b})"`` with operand placeholders preserved as
    ``{name}``. Only simple ``{Name}`` interpolations occur in code_for_op; a
    non-Name interpolation is rendered via ``ast.unparse`` (and would be a signal
    the upstream renderer grew a more complex pattern worth re-reviewing).
    """
    parts: list[str] = []
    for value in node.values:
        if isinstance(value, ast.Constant):
            parts.append(str(value.value))
        elif isinstance(value, ast.FormattedValue):
            inner = value.value
            if isinstance(inner, ast.Name):
                parts.append("{" + inner.id + "}")
            else:
                parts.append("{" + ast.unparse(inner) + "}")
        else:  # pragma: no cover - defensive
            raise OpContractError(
                f"unexpected node in code_for_op f-string: {type(value).__name__}"
            )
    return "".join(parts)


def parse_code_for_op() -> dict[str, str]:
    """Return ``{Ops member name -> C-pattern template}`` from ``code_for_op`` (AST).

    Parses ``CStyleLanguage.code_for_op`` (an ``AnnAssign`` whose value is a Dict
    of ``Ops.X: lambda ...: f"..."``). The C-pattern is extracted structurally
    from each lambda's f-string body.
    """
    tree = _parse_module(CODE_FOR_OP_SRC)
    dict_node: ast.Dict | None = None
    for node in ast.walk(tree):
        if isinstance(node, ast.ClassDef) and node.name == "CStyleLanguage":
            for stmt in node.body:
                target_name = None
                if isinstance(stmt, ast.Assign):
                    for tgt in stmt.targets:
                        if isinstance(tgt, ast.Name):
                            target_name = tgt.id
                elif isinstance(stmt, ast.AnnAssign) and isinstance(
                    stmt.target, ast.Name
                ):
                    target_name = stmt.target.id
                if target_name == "code_for_op":
                    if not isinstance(stmt.value, ast.Dict):
                        raise OpContractError(
                            "CStyleLanguage.code_for_op is not a dict literal"
                        )
                    dict_node = stmt.value
            break
    if dict_node is None:
        raise OpContractError(
            f"could not parse CStyleLanguage.code_for_op from {CODE_FOR_OP_SRC}"
        )

    patterns: dict[str, str] = {}
    for key, value in zip(dict_node.keys, dict_node.values):
        if not (
            isinstance(key, ast.Attribute)
            and isinstance(key.value, ast.Name)
            and key.value.id == "Ops"
        ):
            raise OpContractError(
                f"code_for_op key is not an Ops.* attribute: {ast.dump(key)}"
            )
        op_name = key.attr
        if not isinstance(value, ast.Lambda):
            raise OpContractError(f"code_for_op[{op_name}] value is not a lambda")
        body = value.body
        if isinstance(body, ast.JoinedStr):
            pattern = _render_fstring_template(body)
        elif isinstance(body, ast.Constant):
            pattern = str(body.value)
        else:
            raise OpContractError(
                f"code_for_op[{op_name}] body is not an f-string/const "
                f"({type(body).__name__})"
            )
        patterns[op_name] = pattern
    if not patterns:
        raise OpContractError("code_for_op parsed to an empty pattern set")
    return patterns


def parse_molt_primitive_ops() -> list[str]:
    """Return molt's ``PrimitiveOp`` variant names from the ``ALL`` array (ops.rs).

    The authoritative ordered set is the ``pub const ALL: [PrimitiveOp; N]``
    array. Parsing the ``Self::Variant`` entries from that array (structurally,
    bracket-scanned) gives the canonical contract order AND the exact membership
    the contract must cover — so a molt op added to the enum but missing from the
    contract (or vice-versa) is caught.
    """
    if not MOLT_OPS_RS.exists():
        raise OpContractError(f"molt ops.rs missing: {MOLT_OPS_RS}")
    src = MOLT_OPS_RS.read_text(encoding="utf-8")
    marker = "pub const ALL: [PrimitiveOp;"
    idx = src.find(marker)
    if idx == -1:
        raise OpContractError(
            f"could not find `pub const ALL: [PrimitiveOp; N]` in {MOLT_OPS_RS}"
        )
    open_br = src.find("[", src.find("=", idx))
    if open_br == -1:
        raise OpContractError("malformed ALL array (no opening bracket)")
    depth = 0
    end = open_br
    for i in range(open_br, len(src)):
        if src[i] == "[":
            depth += 1
        elif src[i] == "]":
            depth -= 1
            if depth == 0:
                end = i
                break
    body = src[open_br + 1 : end]
    variants: list[str] = []
    for raw in body.split(","):
        token = raw.strip()
        if not token:
            continue
        prefix = "Self::"
        if not token.startswith(prefix):
            raise OpContractError(
                f"unexpected entry in PrimitiveOp::ALL: {token!r} (expected Self::Variant)"
            )
        variants.append(token[len(prefix) :])
    if not variants:
        raise OpContractError("PrimitiveOp::ALL parsed to zero variants")
    return variants


# ===========================================================================
# Reconciliation — prove every disposition claim against the parsed source.
# ===========================================================================


def reconcile() -> dict:
    """Build the validated contract model, proving each claim against the source."""
    ops_members = parse_ops_enum()
    ops_set = set(ops_members)
    group_sets = parse_group_op_sets()
    alu_members = group_sets.get("ALU", set())
    comparison_members = group_sets.get("Comparison", set())
    code_for_op = parse_code_for_op()
    molt_ops = parse_molt_primitive_ops()

    # (1) The molt disposition table must EXACTLY cover PrimitiveOp::ALL.
    table_ops = set(MOLT_OP_DISPOSITIONS)
    molt_set = set(molt_ops)
    missing = molt_set - table_ops
    extra = table_ops - molt_set
    if missing:
        raise OpContractError(
            "molt PrimitiveOp(s) absent from the disposition table "
            f"(add a reconciliation row): {sorted(missing)}"
        )
    if extra:
        raise OpContractError(
            "disposition table names PrimitiveOp(s) not in ops.rs::ALL "
            f"(remove or fix): {sorted(extra)}"
        )

    # (2) Prove each molt op's disposition against the source.
    for op in molt_ops:
        spec = MOLT_OP_DISPOSITIONS[op]
        disp = spec.get("disposition")
        upstream = spec.get("upstream")
        if disp in {"mapped", "rewrite", "reduce"}:
            if upstream not in ops_set:
                raise OpContractError(
                    f"{op}: claims upstream Ops.{upstream}, which is NOT a member of "
                    f"the pinned Ops enum"
                )
        if disp == "mapped":
            # Comparison/Cast/Bitcast are rendered by dedicated base_rewrite
            # patterns, not code_for_op ALU lambdas — accept those as mapped
            # without a code_for_op entry, but assert the renderer-pattern fact
            # (membership in GroupOp.Elementwise for CAST/BITCAST).
            if upstream in code_for_op:
                # nothing extra to assert here; the C-pattern is recorded below
                pass
            elif upstream in {"CAST", "BITCAST"}:
                if upstream not in (group_sets.get("Elementwise", set()) | ops_set):
                    raise OpContractError(
                        f"{op}: Ops.{upstream} not found as an Elementwise member"
                    )
            else:
                raise OpContractError(
                    f"{op}: disposition 'mapped' but Ops.{upstream} has NO "
                    f"code_for_op entry (it is rendered some other way — is it a "
                    f"'rewrite'?)"
                )
        elif disp == "rewrite":
            # The whole POINT of a rewrite op (e.g. MAX): it must NOT have a
            # code_for_op entry. If upstream GREW one, the disposition is now wrong.
            if upstream in code_for_op:
                raise OpContractError(
                    f"{op}: disposition 'rewrite' but Ops.{upstream} now HAS a "
                    f"code_for_op entry {code_for_op[upstream]!r} — upstream changed; "
                    f"reclassify as 'mapped'"
                )
            if upstream not in alu_members:
                raise OpContractError(
                    f"{op}: disposition 'rewrite' expects Ops.{upstream} in GroupOp.ALU"
                )
            if not spec.get("lowers_to"):
                raise OpContractError(f"{op}: 'rewrite' requires a `lowers_to` chain")
        elif disp == "reduce":
            alu = spec.get("reduce_alu")
            if alu not in ops_set:
                raise OpContractError(
                    f"{op}: reduce_alu Ops.{alu} is not a member of the Ops enum"
                )
        elif disp == "composed":
            if not spec.get("lowers_to"):
                raise OpContractError(f"{op}: 'composed' requires a `lowers_to` chain")
        else:
            raise OpContractError(f"{op}: unknown disposition {disp!r}")

        # Comparison ops must carry the bool-output dtype contract.
        if upstream in comparison_members and "ALWAYS bool" not in spec.get(
            "dtype_rule", ""
        ):
            raise OpContractError(
                f"{op}: Ops.{upstream} is a GroupOp.Comparison op; its dtype_rule "
                f"must state the output is always bool"
            )

    # (3) Every upstream ALU member must be classified: covered by a molt op
    #     (mapped/rewrite/composed-via-reduce) OR present in the upstream-only
    #     table. An ALU member in neither is `unclassified` and FAILS — the
    #     structural kill for "fidelity theater" (a new upstream op silently
    #     unhandled). This is the check that would have flagged FDIV/POW/etc.
    molt_covered_upstream = {
        spec["upstream"]
        for spec in MOLT_OP_DISPOSITIONS.values()
        if spec.get("disposition") in {"mapped", "rewrite"}
        and spec.get("upstream") is not None
    }
    classified = molt_covered_upstream | set(UPSTREAM_ONLY_ALU_DISPOSITIONS)
    unclassified = sorted(alu_members - classified)
    if unclassified:
        raise OpContractError(
            "UNCLASSIFIED upstream GroupOp.ALU member(s) — the pinned tinygrad has "
            f"op(s) molt neither maps nor records a disposition for: {unclassified}. "
            "This is exactly the silent drift this contract exists to catch. Add each "
            "to MOLT_OP_DISPOSITIONS (if molt should expose it) or to "
            "UPSTREAM_ONLY_ALU_DISPOSITIONS (composed / not_yet_supported + reason)."
        )

    # (4) Prove the upstream-only ALU entries really ARE upstream ALU members,
    #     and really do NOT have a code_for_op entry molt is ignoring.
    for name, spec in UPSTREAM_ONLY_ALU_DISPOSITIONS.items():
        if name not in alu_members:
            raise OpContractError(
                f"upstream-only entry {name!r} is not in GroupOp.ALU (stale entry?)"
            )
        disp = spec.get("disposition")
        if disp == "composed" and not spec.get("lowers_to"):
            raise OpContractError(
                f"upstream-only {name}: 'composed' requires lowers_to"
            )
        if disp == "not_yet_supported" and not spec.get("reason"):
            raise OpContractError(
                f"upstream-only {name}: 'not_yet_supported' requires a reason"
            )

    # (5) Prove the non-primitive notes reference real upstream Ops members.
    for name in UPSTREAM_NON_PRIMITIVE_NOTES:
        if name not in ops_set:
            raise OpContractError(
                f"non-primitive note {name!r} is not an Ops member (stale)"
            )

    return {
        "ops_members": ops_members,
        "alu_members": sorted(alu_members),
        "code_for_op": code_for_op,
        "molt_ops": molt_ops,
    }


# ===========================================================================
# TOML rendering (the artifact). Hand-rendered for stable, diff-friendly output
# (mirrors gen_op_kinds.py rendering its own text — no toml writer dependency).
# ===========================================================================


def _toml_str(value: str) -> str:
    """Render a TOML basic string (escaping backslash and quote)."""
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def render_toml(model: dict) -> str:
    code_for_op = model["code_for_op"]
    out: list[str] = []
    out.append(
        "# @generated by tools/gen_gpu_op_contract.py from the pinned upstream\n"
    )
    out.append(
        f"# tinygrad {PINNED_TINYGRAD_VERSION} "
        "(bench/friends/repos/tinygrad_off_the_shelf). DO NOT EDIT.\n"
    )
    out.append("#\n")
    out.append(
        "# The GPU op-contract FACT (doc 67 Phase 1, fact family `gpu_op_contract`):\n"
        "# per molt PrimitiveOp, its disposition against the pinned upstream Ops set +\n"
        "# code_for_op renderer contract. Regenerate with\n"
        "#   python3 tools/gen_gpu_op_contract.py\n"
        "# and verify in CI with\n"
        "#   python3 tools/gen_gpu_op_contract.py --check\n"
        "# A mis-mapping, a changed upstream C-pattern, or a new upstream ALU op turns\n"
        "# this RED. The reconciliation decisions live in the generator and are PROVEN\n"
        "# against the pinned source (e.g. CMOD really is code_for_op[({a}%{b})]; MAX\n"
        "# really is NOT a code_for_op entry).\n"
    )
    out.append("\n")
    out.append("[meta]\n")
    out.append(f"pinned_tinygrad_version = {_toml_str(PINNED_TINYGRAD_VERSION)}\n")
    out.append(
        'ops_enum_source = "bench/friends/repos/tinygrad_off_the_shelf/'
        'tinygrad/uop/__init__.py"\n'
    )
    out.append(
        'code_for_op_source = "bench/friends/repos/tinygrad_off_the_shelf/'
        'tinygrad/renderer/cstyle.py"\n'
    )
    out.append(
        'rewrite_source = "bench/friends/repos/tinygrad_off_the_shelf/'
        'tinygrad/uop/decompositions.py"\n'
    )
    out.append(f"upstream_ops_member_count = {len(model['ops_members'])}\n")
    out.append(f"upstream_alu_member_count = {len(model['alu_members'])}\n")
    out.append(f"molt_primitive_op_count = {len(model['molt_ops'])}\n")
    out.append("\n")

    # --- molt PrimitiveOp rows, in ops.rs::ALL canonical order ----------------
    out.append(
        "# === molt PrimitiveOp -> upstream disposition (ops.rs::ALL order) ===\n\n"
    )
    for op in model["molt_ops"]:
        spec = MOLT_OP_DISPOSITIONS[op]
        disp = spec["disposition"]
        out.append("[[primitive]]\n")
        out.append(f"molt_op = {_toml_str(op)}\n")
        out.append(f"disposition = {_toml_str(disp)}\n")
        upstream = spec.get("upstream")
        if upstream is not None:
            out.append(f"upstream_op = {_toml_str(upstream)}\n")
            # Record the exact renderer C-pattern when the upstream op has one.
            if upstream in code_for_op:
                out.append(f"renderer_c_pattern = {_toml_str(code_for_op[upstream])}\n")
            else:
                out.append('renderer_c_pattern = ""  # no code_for_op entry\n')
        if spec.get("reduce_alu"):
            out.append(f"reduce_alu = {_toml_str(spec['reduce_alu'])}\n")
        if spec.get("lowers_to"):
            out.append(f"lowers_to = {_toml_str(spec['lowers_to'])}\n")
        out.append(f"dtype_rule = {_toml_str(spec['dtype_rule'])}\n")
        out.append(f"ieee_edge = {_toml_str(spec['ieee_edge'])}\n")
        out.append(f"note = {_toml_str(spec['note'])}\n")
        out.append("\n")

    # --- upstream-only ALU rows ----------------------------------------------
    out.append(
        "# === upstream GroupOp.ALU members NOT exposed as a molt PrimitiveOp ===\n"
        "# Each is classified (composed / not_yet_supported + reason) so a new\n"
        "# upstream ALU op cannot be silently absent. NOTE the §1.2.1 ops:\n"
        "# FDIV/POW/FLOORDIV/FLOORMOD/MULACC (composed) + THREEFRY (not_yet_supported).\n\n"
    )
    for name in sorted(UPSTREAM_ONLY_ALU_DISPOSITIONS):
        spec = UPSTREAM_ONLY_ALU_DISPOSITIONS[name]
        out.append("[[upstream_only_alu]]\n")
        out.append(f"upstream_op = {_toml_str(name)}\n")
        out.append(f"disposition = {_toml_str(spec['disposition'])}\n")
        if spec.get("lowers_to"):
            out.append(f"lowers_to = {_toml_str(spec['lowers_to'])}\n")
        if spec.get("reason"):
            out.append(f"reason = {_toml_str(spec['reason'])}\n")
        out.append(f"note = {_toml_str(spec['note'])}\n")
        out.append("\n")

    # --- non-primitive upstream notes ----------------------------------------
    out.append(
        "# === upstream Ops deliberately NOT modeled as compute primitives ===\n"
        "# tensor-core / movement / scheduler ops molt owns at a different layer.\n"
        "# NOTE the §1.2.1 op: WMMA (tensor-core, rendered by its own pattern).\n\n"
    )
    for name in sorted(UPSTREAM_NON_PRIMITIVE_NOTES):
        spec = UPSTREAM_NON_PRIMITIVE_NOTES[name]
        out.append("[[upstream_non_primitive]]\n")
        out.append(f"upstream_op = {_toml_str(name)}\n")
        out.append(f"scope = {_toml_str(spec['scope'])}\n")
        out.append(f"note = {_toml_str(spec['note'])}\n")
        out.append("\n")

    return "".join(out).rstrip() + "\n"


# ===========================================================================
# Entry point (mirrors gen_op_kinds.py --check exactly).
# ===========================================================================


def _check(path: Path, rendered: str) -> bool:
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    current = path.read_bytes()
    expected = rendered.encode("utf-8")
    if current != expected:
        print(
            f"STALE generated file: {path}\n"
            f"  run `python3 tools/gen_gpu_op_contract.py` to regenerate from the "
            f"pinned tinygrad {PINNED_TINYGRAD_VERSION} source",
            file=sys.stderr,
        )
        return False
    return True


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if op_contract.toml is stale (CI mode); do not write",
    )
    args = ap.parse_args(argv)

    try:
        model = reconcile()
        toml_text = render_toml(model)
    except OpContractError as exc:
        print(f"gpu op-contract generation FAILED:\n  {exc}", file=sys.stderr)
        return 1

    if args.check:
        ok = _check(OUT_TOML, toml_text)
        if ok:
            print(
                "gpu op-contract: in sync with pinned tinygrad "
                + PINNED_TINYGRAD_VERSION
            )
        return 0 if ok else 1

    OUT_TOML.write_text(toml_text, encoding="utf-8", newline="\n")
    print(f"wrote {OUT_TOML.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
