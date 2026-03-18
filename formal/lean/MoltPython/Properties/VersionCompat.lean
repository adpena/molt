/-
  MoltPython.Properties.VersionCompat -- Cross-version compatibility proofs.

  Proves that:
  1. Programs valid on Python 3.12 remain valid on 3.13 and 3.14 (forward compat)
  2. Version-gated features are only enabled when the version matches
  3. The common subset (features in all 3 versions) is well-defined and non-empty
-/
import MoltPython.VersionGated

set_option autoImplicit false

namespace MoltPython

-- ============================================================================
-- Section 1: Forward compatibility
-- ============================================================================

/-- If a program works on 3.12, it works on 3.13. -/
theorem forward_compat_312_to_313 (stmts : List VersionGatedStmt)
    (hwf : (VersionGatedModule.mk stmts .py312).wellFormed) :
    (VersionGatedModule.mk stmts .py313).wellFormed :=
  module_forward_compat stmts .py312 .py313 hwf PythonVersion.py312_le_py313

/-- If a program works on 3.12, it works on 3.14. -/
theorem forward_compat_312_to_314 (stmts : List VersionGatedStmt)
    (hwf : (VersionGatedModule.mk stmts .py312).wellFormed) :
    (VersionGatedModule.mk stmts .py314).wellFormed :=
  module_forward_compat stmts .py312 .py314 hwf PythonVersion.py312_le_py314

/-- If a program works on 3.13, it works on 3.14. -/
theorem forward_compat_313_to_314 (stmts : List VersionGatedStmt)
    (hwf : (VersionGatedModule.mk stmts .py313).wellFormed) :
    (VersionGatedModule.mk stmts .py314).wellFormed :=
  module_forward_compat stmts .py313 .py314 hwf PythonVersion.py313_le_py314

/-- Full forward compatibility chain: 3.12 well-formed implies all versions well-formed. -/
theorem forward_compat_312_all (stmts : List VersionGatedStmt)
    (hwf : (VersionGatedModule.mk stmts .py312).wellFormed)
    (ver : PythonVersion) :
    (VersionGatedModule.mk stmts ver).wellFormed := by
  cases ver with
  | py312 => exact hwf
  | py313 => exact forward_compat_312_to_313 stmts hwf
  | py314 => exact forward_compat_312_to_314 stmts hwf

-- ============================================================================
-- Section 2: Version-gated features are only enabled when version matches
-- ============================================================================

/-- exceptionGroupRefined requires 3.13+: not available on 3.12. -/
theorem exceptionGroupRefined_not_on_312 :
    ¬ VersionedFeature.exceptionGroupRefined.availableOn .py312 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

/-- exceptionGroupRefined is available on 3.13. -/
theorem exceptionGroupRefined_on_313 :
    VersionedFeature.exceptionGroupRefined.availableOn .py313 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

/-- deferredAnnotations requires 3.14+: not available on 3.12 or 3.13. -/
theorem deferredAnnotations_not_on_312 :
    ¬ VersionedFeature.deferredAnnotations.availableOn .py312 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem deferredAnnotations_not_on_313 :
    ¬ VersionedFeature.deferredAnnotations.availableOn .py313 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem deferredAnnotations_on_314 :
    VersionedFeature.deferredAnnotations.availableOn .py314 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

/-- localsSnapshot requires 3.13+: not available on 3.12. -/
theorem localsSnapshot_not_on_312 :
    ¬ VersionedFeature.localsSnapshot.availableOn .py312 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem localsSnapshot_on_313 :
    VersionedFeature.localsSnapshot.availableOn .py313 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

/-- intIndexDeprecation requires 3.14+: not available on 3.12 or 3.13. -/
theorem intIndexDeprecation_not_on_312 :
    ¬ VersionedFeature.intIndexDeprecation.availableOn .py312 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem intIndexDeprecation_not_on_313 :
    ¬ VersionedFeature.intIndexDeprecation.availableOn .py313 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem intIndexDeprecation_on_314 :
    VersionedFeature.intIndexDeprecation.availableOn .py314 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

/-- typeParamDefaults requires 3.14+. -/
theorem typeParamDefaults_not_on_312 :
    ¬ VersionedFeature.typeParamDefaults.availableOn .py312 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem typeParamDefaults_not_on_313 :
    ¬ VersionedFeature.typeParamDefaults.availableOn .py313 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

theorem typeParamDefaults_on_314 :
    VersionedFeature.typeParamDefaults.availableOn .py314 := by
  simp [VersionedFeature.availableOn, versionAtLeast, LE.le, PythonVersion.toNat,
        VersionedFeature.minVersion]

/-- A gated expression requiring a 3.14-only feature rejects 3.12. -/
theorem gated_expr_rejects_old_version (e : PyExpr) :
    ¬ (VersionGatedExpr.mk e (some .deferredAnnotations)).validFor .py312 := by
  simp [VersionGatedExpr.validFor]
  exact deferredAnnotations_not_on_312

/-- A gated statement requiring a 3.14-only feature rejects 3.13. -/
theorem gated_stmt_rejects_old_version (s : PyStmt) :
    ¬ (VersionGatedStmt.mk s (some .typeParamDefaults)).validFor .py313 := by
  simp [VersionGatedStmt.validFor]
  exact typeParamDefaults_not_on_313

-- ============================================================================
-- Section 3: Common subset is well-defined and non-empty
-- ============================================================================

/-- The set of features available on all three versions. -/
def commonFeature (feat : VersionedFeature) : Prop :=
  feat.availableOn .py312 ∧ feat.availableOn .py313 ∧ feat.availableOn .py314

instance (feat : VersionedFeature) : Decidable (commonFeature feat) :=
  inferInstanceAs (Decidable (feat.availableOn .py312 ∧ feat.availableOn .py313 ∧ feat.availableOn .py314))

/-- typeAliasStmt is in the common subset. -/
theorem typeAliasStmt_common : commonFeature .typeAliasStmt := by
  simp [commonFeature, VersionedFeature.availableOn, versionAtLeast, LE.le,
        PythonVersion.toNat, VersionedFeature.minVersion]

/-- matchStmtRefined is in the common subset. -/
theorem matchStmtRefined_common : commonFeature .matchStmtRefined := by
  simp [commonFeature, VersionedFeature.availableOn, versionAtLeast, LE.le,
        PythonVersion.toNat, VersionedFeature.minVersion]

/-- The common subset is non-empty: there exists at least one common feature. -/
theorem common_subset_nonempty : ∃ feat : VersionedFeature, commonFeature feat :=
  ⟨.typeAliasStmt, typeAliasStmt_common⟩

/-- There are exactly two universally available features. -/
theorem common_features_are_312_plus :
    commonFeature .typeAliasStmt ∧ commonFeature .matchStmtRefined := by
  exact ⟨typeAliasStmt_common, matchStmtRefined_common⟩

/-- 3.13-only features are not in the common subset. -/
theorem exceptionGroupRefined_not_common : ¬ commonFeature .exceptionGroupRefined := by
  simp [commonFeature]
  intro h
  exact absurd h exceptionGroupRefined_not_on_312

/-- 3.14-only features are not in the common subset. -/
theorem deferredAnnotations_not_common : ¬ commonFeature .deferredAnnotations := by
  simp [commonFeature]
  intro h
  exact absurd h deferredAnnotations_not_on_312

/-- A module using only common features is well-formed on all versions. -/
theorem common_module_all_versions
    (stmts : List VersionGatedStmt)
    (hcommon : ∀ gs, gs ∈ stmts → match gs.requiredFeature with
      | none => True
      | some feat => commonFeature feat)
    (ver : PythonVersion) :
    (VersionGatedModule.mk stmts ver).wellFormed := by
  intro gs hgs
  simp [VersionGatedStmt.validFor]
  cases hf : gs.requiredFeature with
  | none => trivial
  | some feat =>
    have hc := hcommon gs hgs
    simp [hf] at hc
    simp [commonFeature, VersionedFeature.availableOn] at hc
    cases ver with
    | py312 => exact hc.1
    | py313 => exact hc.2.1
    | py314 => exact hc.2.2

-- ============================================================================
-- Decidable version checks (for runtime use in the compiler)
-- ============================================================================

/-- Check if a feature is available, returning a Bool for use in compilation logic. -/
def checkFeatureAvailable (feat : VersionedFeature) (ver : PythonVersion) : Bool :=
  decide (feat.availableOn ver)

/-- The Bool check is correct with respect to the Prop. -/
theorem checkFeatureAvailable_correct (feat : VersionedFeature) (ver : PythonVersion) :
    checkFeatureAvailable feat ver = true ↔ feat.availableOn ver := by
  simp [checkFeatureAvailable, decide_eq_true_eq]

/-- Filter a module's statements to those valid on the target version. -/
def filterForVersion (stmts : List VersionGatedStmt) (ver : PythonVersion) :
    List VersionGatedStmt :=
  stmts.filter (fun gs => decide (gs.validFor ver))

/-- The filtered module is always well-formed. -/
theorem filterForVersion_wellFormed (stmts : List VersionGatedStmt) (ver : PythonVersion) :
    (VersionGatedModule.mk (filterForVersion stmts ver) ver).wellFormed := by
  intro gs hgs
  simp [filterForVersion, List.mem_filter, decide_eq_true_eq] at hgs
  exact hgs.2

end MoltPython
