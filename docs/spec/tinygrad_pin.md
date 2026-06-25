# tinygrad upstream pin — the ML-contract source of truth

<!--
authority: this file is the SINGLE pinned upstream-tinygrad revision the ML
parity facts derive from. doc 67 (Tinygrad + DFlash Fidelity) Phase 0.
constitution: CLAUDE.md "Top Priority: Tinygrad + DFlash Fidelity" (turn-blocking).
-->

## The pin

| field | value |
|---|---|
| **upstream package** | `tinygrad` |
| **pinned version** | **`0.13.0`** |
| **author** | George Hotz / the tinygrad authors |
| **in-tree oracle** | `bench/friends/repos/tinygrad_off_the_shelf/` |
| **version manifest** | `bench/friends/repos/tinygrad_off_the_shelf/pyproject.toml` (`[project] version`) |
| **op enum authority** | `bench/friends/repos/tinygrad_off_the_shelf/tinygrad/uop/__init__.py` — `class Ops(FastEnum)` |
| **backend op contract authority** | `bench/friends/repos/tinygrad_off_the_shelf/tinygrad/renderer/cstyle.py` — `CStyleLanguage.code_for_op` |
| **non-`code_for_op` lowering authority** | `bench/friends/repos/tinygrad_off_the_shelf/tinygrad/uop/decompositions.py` (the `MAX`/`FLOORDIV`/`FLOORMOD`/`FDIV`/`THREEFRY`/`MULACC` pattern rewrites) |

`0.13.0` is verified in-tree, not asserted: `tools/check_tinygrad_pin.py` reads the
off-the-shelf `pyproject.toml` and fails the build if its `version` is not exactly the
string pinned here. A silent dependency bump (someone re-vendoring a newer tinygrad)
therefore turns RED at the gate instead of silently invalidating every derived parity
fact. This is the literal encoding of doc 67 §R1: *"`check_tinygrad_pin.py` fails the
build on an un-regenerated bump."*

## Why a pin at all

The ML surface's fidelity claim ("molt's `import tinygrad` is upstream tinygrad's public
contract") is only meaningful relative to a *specific* upstream revision. Without a pin,
"matches upstream" silently means "matched whatever upstream looked like when the prose
was last hand-edited" — the exact "fidelity theater" failure mode doc 67 §1.2.1 found
already live (the design's "26 primitives == tinygrad `code_for_op`" prose had drifted:
upstream uses `Ops.CMOD`/`Ops.CDIV`, has no `MAX`/`IDIV`/`MOD` renderer entry, and adds
`FDIV`/`POW`/`THREEFRY`/`FLOORDIV`/`FLOORMOD`/`SUB`/`MULACC`/`WMMA`).

The pin makes the upstream revision a named, immutable, in-tree reference that the
generated parity facts (`runtime/molt-gpu/op_contract.toml`, etc.) are *derived from* and
*checked against*, mechanically, on every CI run.

## The pinned-fact family this anchors

| fact | generator | gate | status |
|---|---|---|---|
| `gpu_op_contract` — the primitive-set / per-op semantics fact | `tools/gen_gpu_op_contract.py` | `tools/gen_gpu_op_contract.py --check` | **LANDED** (doc 67 Phase 1a/1b) |
| `gpu_api_contract` — the Tensor/nn public-API-shape fact | `tools/gen_tinygrad_api_contract.py` | `--check` | planned (doc 67 Phase 3) |
| tinygrad differential oracle — numeric parity vs the pin executing the same program | `tools/tinygrad_diff_oracle.py` | `pytest tests/gpu/parity/` | planned (doc 67 Phase 2) |

## Bump protocol (how a future upgrade is ratified)

Upgrading the pinned tinygrad revision is a **single atomic change**, never a piecemeal
edit, because every derived parity fact is a function of the pin. To bump
`0.13.0 → <next>`:

1. **Re-vendor** the new upstream tinygrad into
   `bench/friends/repos/tinygrad_off_the_shelf/` (the read-only oracle; never hand-edit
   it — it is the source of truth, not a molt artifact).
2. **Update this pin** (`docs/spec/tinygrad_pin.md`): change the pinned version string and
   record any new authority-file paths (e.g. if upstream relocates `code_for_op`).
3. **Regenerate every `gpu_*_contract` fact** from the new source:
   `python3 tools/gen_gpu_op_contract.py` (and, when they land, the API-contract
   generator). Re-run each `--check` to confirm idempotence.
4. **Reconcile every new divergence the generator surfaces** in the contract itself
   (a new upstream `Ops` member, a changed `code_for_op` C-pattern, a relocated rewrite):
   give it its correct disposition (`mapped` / `composed` / `rewrite` /
   `not-yet-supported` with a reason). A `not_yet_supported` disposition is allowed but
   must carry a `reason`; a *silent* gap is not — the generator fails closed on an
   unclassified upstream op.
5. **Re-run the differential oracle** (when it lands, doc 67 Phase 2) so numeric parity is
   re-proven against the new revision executing the same programs.
6. **Land it as ONE change.** Steps 1–5 are a single commit (or a single tightly-scoped
   series): the pin, the re-vendored source, the regenerated contracts, and the
   reconciliations move together. A bump that updates the source without regenerating the
   contracts is exactly the drift `check_tinygrad_pin.py` + `gen_gpu_op_contract.py
   --check` exist to make impossible — both gates will be RED until the change is
   complete.

## IEEE-754 / ULP budget policy (referenced by the numeric oracle, doc 67 Phase 2)

Recorded here so the numeric-parity tolerance is pinned alongside the revision it applies
to (doc 67 §5 / §R2):

- **Bit-exact required** for every op upstream renders as an identical C expression in
  `code_for_op` (the integer/bitwise/comparison/`ADD`/`SUB`/`MUL` ALU set) and for all
  integer/movement ops. No tolerance may hide a real divergence on these.
- **Documented-ULP allowed only** for the transcendental ops whose result depends on the
  host libm implementation (`EXP2`, `LOG2`, `SIN`) — and a widened ULP budget must cite
  the specific libm difference that forces it. An unexplained float divergence stays RED.

Until the numeric oracle (Phase 2) lands and populates concrete per-op budgets, the policy
above is the standing contract; the op-level `ieee_edge` annotations in
`runtime/molt-gpu/op_contract.toml` enumerate the NaN/inf/`-0.0` edges each op must honor.
