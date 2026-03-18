/-
  MoltPython.VersionGated -- Version-gated semantics for Python 3.12/3.13/3.14.

  Molt supports Python 3.12, 3.13, and 3.14. Some language features and runtime
  behaviors differ across these versions. This module formalizes version-gating:
  a parameterized mechanism that enables or disables features based on a target
  Python version.

  Mirrors the Quint model `formal/quint/molt_cross_version.qnt` but with
  machine-checked proofs in Lean.
-/
import MoltPython.Syntax

set_option autoImplicit false

namespace MoltPython

/-- Supported Python versions for Molt compilation. -/
inductive PythonVersion where
  | py312
  | py313
  | py314
  deriving DecidableEq, Repr

namespace PythonVersion

/-- Numeric representation for ordering. -/
def toNat : PythonVersion → Nat
  | .py312 => 312
  | .py313 => 313
  | .py314 => 314

/-- Version ordering: v1 <= v2. -/
def le (v1 v2 : PythonVersion) : Bool :=
  v1.toNat ≤ v2.toNat

instance : LE PythonVersion where
  le v1 v2 := v1.toNat ≤ v2.toNat

instance : LT PythonVersion where
  lt v1 v2 := v1.toNat < v2.toNat

instance (v1 v2 : PythonVersion) : Decidable (v1 ≤ v2) :=
  inferInstanceAs (Decidable (v1.toNat ≤ v2.toNat))

instance (v1 v2 : PythonVersion) : Decidable (v1 < v2) :=
  inferInstanceAs (Decidable (v1.toNat < v2.toNat))

/-- All supported versions as a list. -/
def allVersions : List PythonVersion := [.py312, .py313, .py314]

/-- toNat is injective. -/
theorem toNat_injective (v1 v2 : PythonVersion) (h : v1.toNat = v2.toNat) : v1 = v2 := by
  cases v1 <;> cases v2 <;> simp [toNat] at h <;> rfl

/-- Version ordering is reflexive. -/
theorem le_refl (v : PythonVersion) : v ≤ v := by
  simp [LE.le, toNat]

/-- Version ordering is transitive. -/
theorem le_trans (v1 v2 v3 : PythonVersion) (h12 : v1 ≤ v2) (h23 : v2 ≤ v3) : v1 ≤ v3 := by
  simp [LE.le, toNat] at *
  omega

/-- Version ordering is antisymmetric. -/
theorem le_antisymm (v1 v2 : PythonVersion) (h12 : v1 ≤ v2) (h21 : v2 ≤ v1) : v1 = v2 := by
  apply toNat_injective
  simp [LE.le, toNat] at *
  omega

/-- Version ordering is total. -/
theorem le_total (v1 v2 : PythonVersion) : v1 ≤ v2 ∨ v2 ≤ v1 := by
  simp [LE.le, toNat]
  omega

/-- py312 ≤ py313. -/
theorem py312_le_py313 : PythonVersion.py312 ≤ PythonVersion.py313 := by
  simp [LE.le, toNat]

/-- py313 ≤ py314. -/
theorem py313_le_py314 : PythonVersion.py313 ≤ PythonVersion.py314 := by
  simp [LE.le, toNat]

/-- py312 ≤ py314. -/
theorem py312_le_py314 : PythonVersion.py312 ≤ PythonVersion.py314 := by
  simp [LE.le, toNat]

end PythonVersion

/-- Version configuration specifying a range of supported versions. -/
structure VersionConfig where
  minVersion : PythonVersion
  maxVersion : PythonVersion
  valid : minVersion ≤ maxVersion
  deriving Repr

namespace VersionConfig

/-- A version is within the configured range. -/
def inRange (cfg : VersionConfig) (v : PythonVersion) : Prop :=
  cfg.minVersion ≤ v ∧ v ≤ cfg.maxVersion

instance (cfg : VersionConfig) (v : PythonVersion) : Decidable (cfg.inRange v) :=
  inferInstanceAs (Decidable (cfg.minVersion ≤ v ∧ v ≤ cfg.maxVersion))

/-- Configuration covering all Molt-supported versions. -/
def allSupported : VersionConfig where
  minVersion := .py312
  maxVersion := .py314
  valid := PythonVersion.py312_le_py314

/-- All three versions are in the allSupported range. -/
theorem py312_in_allSupported : allSupported.inRange .py312 := by
  simp [inRange, allSupported, LE.le, PythonVersion.toNat]

theorem py313_in_allSupported : allSupported.inRange .py313 := by
  simp [inRange, allSupported, LE.le, PythonVersion.toNat]

theorem py314_in_allSupported : allSupported.inRange .py314 := by
  simp [inRange, allSupported, LE.le, PythonVersion.toNat]

end VersionConfig

/-- Predicate: version v is at least minVer. -/
def versionAtLeast (v : PythonVersion) (minVer : PythonVersion) : Prop :=
  minVer ≤ v

instance (v minVer : PythonVersion) : Decidable (versionAtLeast v minVer) :=
  inferInstanceAs (Decidable (minVer ≤ v))

/-- Predicate: version v is in the range [lo, hi]. -/
def versionRange (v : PythonVersion) (lo hi : PythonVersion) : Prop :=
  lo ≤ v ∧ v ≤ hi

instance (v lo hi : PythonVersion) : Decidable (versionRange v lo hi) :=
  inferInstanceAs (Decidable (lo ≤ v ∧ v ≤ hi))

/-- versionAtLeast is monotone: if v >= min and v <= v', then v' >= min. -/
theorem versionAtLeast_mono (v v' min : PythonVersion)
    (hge : versionAtLeast v min) (hle : v ≤ v') : versionAtLeast v' min := by
  simp [versionAtLeast] at *
  exact PythonVersion.le_trans min v v' hge hle

/-- versionRange inclusion: if v is in [lo, hi] and [lo, hi] ⊆ [lo', hi'], then v is in [lo', hi']. -/
theorem versionRange_subset (v lo hi lo' hi' : PythonVersion)
    (hv : versionRange v lo hi) (hlo : lo' ≤ lo) (hhi : hi ≤ hi') :
    versionRange v lo' hi' := by
  simp [versionRange] at *
  constructor
  · exact PythonVersion.le_trans lo' lo v hlo hv.1
  · exact PythonVersion.le_trans v hi hi' hv.2 hhi

-- ============================================================================
-- Version-gated features
-- ============================================================================

/-- Language features that differ across Python versions.
    Each feature has a minimum version for availability. -/
inductive VersionedFeature where
  /-- ExceptionGroup: available 3.11+, but refined semantics in 3.13.
      Since Molt targets 3.12+, the base ExceptionGroup is always available.
      The 3.13 refinements (e.g., split() behavior) are gated. -/
  | exceptionGroupRefined
  /-- type alias statement (PEP 695): available 3.12+, so always available in Molt. -/
  | typeAliasStmt
  /-- Match statement pattern matching refinements (3.12+): always available. -/
  | matchStmtRefined
  /-- Deprecation of int implicit conversion from __index__ (3.14). -/
  | intIndexDeprecation
  /-- Deferred annotation evaluation (PEP 649): 3.14+ only. -/
  | deferredAnnotations
  /-- locals() snapshot semantics (PEP 667): 3.13+ only. -/
  | localsSnapshot
  /-- Type parameter defaults (PEP 696): 3.14+ only. -/
  | typeParamDefaults
  deriving DecidableEq, Repr

namespace VersionedFeature

/-- Minimum Python version required for a feature. -/
def minVersion : VersionedFeature → PythonVersion
  | .exceptionGroupRefined => .py313
  | .typeAliasStmt         => .py312
  | .matchStmtRefined      => .py312
  | .intIndexDeprecation   => .py314
  | .deferredAnnotations   => .py314
  | .localsSnapshot        => .py313
  | .typeParamDefaults     => .py314

/-- Whether a feature is available on a given Python version. -/
def availableOn (feat : VersionedFeature) (ver : PythonVersion) : Prop :=
  versionAtLeast ver feat.minVersion

instance (feat : VersionedFeature) (ver : PythonVersion) : Decidable (feat.availableOn ver) :=
  inferInstanceAs (Decidable (versionAtLeast ver feat.minVersion))

/-- Whether a feature has a legacy fallback path (can still compile on older versions
    with different semantics). -/
def hasLegacyPath : VersionedFeature → Bool
  | .exceptionGroupRefined => true   -- falls back to base ExceptionGroup behavior
  | .typeAliasStmt         => false  -- always available
  | .matchStmtRefined      => false  -- always available
  | .intIndexDeprecation   => true   -- falls back to allowing implicit conversion
  | .deferredAnnotations   => true   -- falls back to eager annotation eval
  | .localsSnapshot        => true   -- falls back to cached dict semantics
  | .typeParamDefaults     => false  -- syntax error on older versions

/-- Features available on all Molt-supported versions (3.12+). -/
def universallyAvailable (feat : VersionedFeature) : Prop :=
  feat.availableOn .py312

/-- typeAliasStmt is universally available. -/
theorem typeAliasStmt_universal : universallyAvailable .typeAliasStmt := by
  simp [universallyAvailable, availableOn, versionAtLeast, LE.le, PythonVersion.toNat, minVersion]

/-- matchStmtRefined is universally available. -/
theorem matchStmtRefined_universal : universallyAvailable .matchStmtRefined := by
  simp [universallyAvailable, availableOn, versionAtLeast, LE.le, PythonVersion.toNat, minVersion]

/-- Feature availability is monotone: if available on v, available on all v' >= v. -/
theorem availableOn_mono (feat : VersionedFeature) (v v' : PythonVersion)
    (havail : feat.availableOn v) (hle : v ≤ v') : feat.availableOn v' := by
  simp [availableOn] at *
  exact versionAtLeast_mono v v' feat.minVersion havail hle

end VersionedFeature

-- ============================================================================
-- Version-gated expressions and statements
-- ============================================================================

/-- A version-gated expression: wraps a PyExpr with a version requirement. -/
structure VersionGatedExpr where
  expr : PyExpr
  requiredFeature : Option VersionedFeature
  deriving Repr

namespace VersionGatedExpr

/-- Whether a gated expression is valid for a given version. -/
def validFor (ge : VersionGatedExpr) (ver : PythonVersion) : Prop :=
  match ge.requiredFeature with
  | none => True
  | some feat => feat.availableOn ver

instance (ge : VersionGatedExpr) (ver : PythonVersion) : Decidable (ge.validFor ver) := by
  simp [validFor]
  cases ge.requiredFeature with
  | none => exact isTrue trivial
  | some feat => exact inferInstanceAs (Decidable (feat.availableOn ver))

/-- An ungated expression is always valid. -/
def ungated (e : PyExpr) : VersionGatedExpr :=
  { expr := e, requiredFeature := none }

/-- Ungated expressions are valid on all versions. -/
theorem ungated_always_valid (e : PyExpr) (ver : PythonVersion) :
    (ungated e).validFor ver := by
  simp [ungated, validFor]

end VersionGatedExpr

/-- A version-gated statement: wraps a PyStmt with a version requirement. -/
structure VersionGatedStmt where
  stmt : PyStmt
  requiredFeature : Option VersionedFeature
  deriving Repr

namespace VersionGatedStmt

/-- Whether a gated statement is valid for a given version. -/
def validFor (gs : VersionGatedStmt) (ver : PythonVersion) : Prop :=
  match gs.requiredFeature with
  | none => True
  | some feat => feat.availableOn ver

instance (gs : VersionGatedStmt) (ver : PythonVersion) : Decidable (gs.validFor ver) := by
  simp [validFor]
  cases gs.requiredFeature with
  | none => exact isTrue trivial
  | some feat => exact inferInstanceAs (Decidable (feat.availableOn ver))

/-- An ungated statement is always valid. -/
def ungated (s : PyStmt) : VersionGatedStmt :=
  { stmt := s, requiredFeature := none }

/-- Ungated statements are valid on all versions. -/
theorem ungated_always_valid (s : PyStmt) (ver : PythonVersion) :
    (ungated s).validFor ver := by
  simp [ungated, validFor]

end VersionGatedStmt

-- ============================================================================
-- Version-gated module
-- ============================================================================

/-- A Python module with version-gated statements. -/
structure VersionGatedModule where
  stmts : List VersionGatedStmt
  targetVersion : PythonVersion
  deriving Repr

namespace VersionGatedModule

/-- All statements in a module are valid for its target version. -/
def wellFormed (m : VersionGatedModule) : Prop :=
  ∀ gs, gs ∈ m.stmts → gs.validFor m.targetVersion

/-- Extract the plain statements (dropping version gates). -/
def toModule (m : VersionGatedModule) : PyModule :=
  m.stmts.map (·.stmt)

/-- A module with only ungated statements is always well-formed. -/
theorem ungated_module_wellFormed (stmts : List PyStmt) (ver : PythonVersion) :
    (VersionGatedModule.mk (stmts.map VersionGatedStmt.ungated) ver).wellFormed := by
  intro gs hgs
  simp [List.mem_map] at hgs
  obtain ⟨s, _, rfl⟩ := hgs
  exact VersionGatedStmt.ungated_always_valid s ver

end VersionGatedModule

-- ============================================================================
-- Compatibility transitivity
-- ============================================================================

/-- If a gated expression is valid on v1 and v1 <= v2, it is valid on v2.
    This captures forward compatibility. -/
theorem gatedExpr_forward_compat (ge : VersionGatedExpr) (v1 v2 : PythonVersion)
    (hvalid : ge.validFor v1) (hle : v1 ≤ v2) : ge.validFor v2 := by
  simp [VersionGatedExpr.validFor] at *
  cases hf : ge.requiredFeature with
  | none => trivial
  | some feat =>
    simp [hf] at hvalid
    exact VersionedFeature.availableOn_mono feat v1 v2 hvalid hle

/-- If a gated statement is valid on v1 and v1 <= v2, it is valid on v2. -/
theorem gatedStmt_forward_compat (gs : VersionGatedStmt) (v1 v2 : PythonVersion)
    (hvalid : gs.validFor v1) (hle : v1 ≤ v2) : gs.validFor v2 := by
  simp [VersionGatedStmt.validFor] at *
  cases hf : gs.requiredFeature with
  | none => trivial
  | some feat =>
    simp [hf] at hvalid
    exact VersionedFeature.availableOn_mono feat v1 v2 hvalid hle

/-- Forward compatibility for modules: if a module is well-formed on v1 and v1 <= v2,
    retargeting to v2 preserves well-formedness. -/
theorem module_forward_compat (stmts : List VersionGatedStmt) (v1 v2 : PythonVersion)
    (hwf : (VersionGatedModule.mk stmts v1).wellFormed) (hle : v1 ≤ v2) :
    (VersionGatedModule.mk stmts v2).wellFormed := by
  intro gs hgs
  exact gatedStmt_forward_compat gs v1 v2 (hwf gs hgs) hle

end MoltPython
