# Ecosystem Compatibility Ratchet

Arc 1 of [`docs/design/foundation/24_ecosystem_compat_gap_audit.md`](../../docs/design/foundation/24_ecosystem_compat_gap_audit.md).
Modeled on the satellite-parity guard (`tools/check_satellite_parity.py`):
a fail-closed, down-only ratchet so library-compatibility claims are **derived,
never asserted**.

## What lives here

| File | Role |
|---|---|
| `dynamism_features.json` | **Single source of truth** for every dynamism feature's `status` (Lane B taxonomy as data), each with `evidence` (file:line from doc 24). |
| `package_triage.json` | The audited packages, each with the features it **requires** on its import + commonly-used path. Verdicts here are *cached derivations*, re-checked by the tool. |
| `ecosystem_compat_baseline.json` | The committed one-way ratchet: per-package `{verdict, hardest_feature, sha256_of_evidence}` + `compatible_floor` / `incompatible_ceiling` / `partial_ceiling`. |
| `../check_ecosystem_compat.py` | The guard. Re-derives every verdict and fails on any drift/regression. |
| `../../docs/spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md` | Generated, never hand-edited; `--update-matrix` regenerates it. |

## The derivation (no hand-asserted verdicts)

A feature's `status` maps to a verdict class; a package's verdict is the **min**
(worst) over its `required_features`:

```
supported   -> compatible                  (top of the lattice)
typed-shim  -> compatible-via-typed-shim
bridge      -> compatible-via-bridge       (policy decision pending — doc 24 OQ1)
partial     -> partial
unsupported -> incompatible-by-design      (worst)
```

An empty `required_features` set derives `compatible`. `optional_features` are
recorded for provenance (doc 24 names them but marks them optional / path-
avoidable) and **do not** feed the min — see the manifests' `_interpretation_notes`.

## What the ratchet enforces (fail-closed)

The guard exits non-zero when any of these hold (run `check_ecosystem_compat.py`):

- a feature has an invalid `status`, an empty `evidence`, an `unsupported`
  status with no `excluded_feature`, or an `unsupported`/`partial` status with
  empty `tracking` (**anti-parking-lot**: every not-yet-supported feature must
  name its converting arc/baton — a verdict can never be silently parked);
- a package references an **unknown feature id** (fail-closed = worst class),
  lists an id in both required + optional, has an invalid `compile_probe_status`,
  or has a **stored `verdict`/`hardest_feature` that disagrees with the derived
  one** (no hand-edited verdicts);
- versus the baseline: a package's verdict **regressed** (moved down the
  lattice), its **evidence SHA changed** while the verdict was unchanged
  (re-verify), or it has **no baseline entry**;
- the distribution regressed across the package universe that already exists in
  the committed baseline: `compatible_floor` fell, or `incompatible_ceiling` /
  `partial_ceiling` rose. Existing packages move **one way only** (good metric
  up, bad metrics down) — the same asymmetry as the satellite ratchet ceiling.
  New audited packages may expand the universe with honest fail-closed verdicts;
  they must still be explicit baseline entries.

## How to add a package

1. Add an entry to `package_triage.json` under `packages`:
   - `required_features`: the feature ids it needs on the import + commonly-used
     path (Lane B scope). `optional_features`: ids doc 24 names but that are
     path-avoidable / optional.
   - `source` (e.g. `"doc-24 paper triage"` or an explicit priority lane),
     `compile_probe_status` (`"pending"` until a real `molt build` verifies it),
     and a `source_basis` justifying the required/optional split against the
     source evidence.
   - You *may* pre-fill `verdict`/`hardest_feature`, but they are **checked
     against the derivation** — if you guess wrong the guard tells you the
     derived value. Easiest: leave them out, run `--show <pkg>`, then fill them.
2. `python3 tools/check_ecosystem_compat.py --update-baseline` (allowed only in
   the improving direction) and `--update-matrix`.
3. Commit the manifest + baseline + regenerated matrix together.

## How to add / graduate a feature

1. Add or edit an entry in `dynamism_features.json`. Set `status`, `evidence`
   (file:line), and — for `unsupported` — `excluded_feature`; for
   `unsupported`/`partial` — `tracking` (the doc-24 arc that converts it).
2. **Graduating** a feature (e.g. D16 `unsupported` -> `supported` once PEP-562
   lands) automatically re-derives every dependent package verdict, raising the
   compatible floor and shrinking the incompatible set.
3. Re-derive cached verdicts in `package_triage.json` for any package the change
   touches (the guard will name the mismatches), then `--update-baseline` (the
   ratchet permits the improving direction) and `--update-matrix`.

## Commands

```bash
python3 tools/check_ecosystem_compat.py            # check vs baseline (CI gate)
python3 tools/check_ecosystem_compat.py --verbose  # + per-package table
python3 tools/check_ecosystem_compat.py --show PKG # one package's derivation
python3 tools/check_ecosystem_compat.py --matrix   # print the matrix (no write)
python3 tools/check_ecosystem_compat.py --update-baseline   # improving-only
python3 tools/check_ecosystem_compat.py --update-matrix     # regenerate the doc
```

Wired in CI as a `docs-gates` step in `.github/workflows/ci.yml` and as a `lint`
gate in `pyproject.toml` (`[tool.molt.dx.commands].lint`), alongside the
`check_stdlib_intrinsics` / `check_docs_architecture` family. The test suite is
`tests/test_ecosystem_compat.py`.

## One flagged interpretation worth knowing

doc 24's mission enumerates four verdict classes, but doc 24 Lane C verdicts ~8
packages **PARTIAL** (import-green, function-blocked on a named arc). Collapsing
those into any of the four would either falsely assert support or falsely assert
a permanent exclusion, so a fifth class **`partial`** is introduced (ordered
between `compatible-via-bridge` and `incompatible-by-design`). The four mission
classes remain the canonical-named set; `partial` is the honest, fail-closed
representation of doc 24's PARTIAL rows. Full rationale + every other
interpretation is in the `_interpretation_notes` arrays of both manifests.
