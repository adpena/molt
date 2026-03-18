/-
  MoltTIR.Runtime.CapabilityGate — Capability gate verification.

  Formalizes the Molt capability gate model from
  runtime/molt-runtime/src/async_rt/channels.rs and proves the key
  security properties: trusted-grants-all, default-deny, decidability,
  monotonicity, and irrevocability.

  Key results:
  - Trusted mode bypasses all capability checks.
  - An untrusted runtime with empty capability set denies everything.
  - All capability checks are decidable.
  - Capability sets are monotonic: supersets preserve access.
  - Capabilities are irrevocable within a runtime session.

  References:
  - runtime/molt-runtime/src/async_rt/channels.rs (is_trusted, has_capability)
-/

set_option autoImplicit false

namespace MoltTIR.Runtime

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Capability enumeration
-- ══════════════════════════════════════════════════════════════════

/-- Runtime capabilities matching the Rust MOLT_CAPABILITIES whitelist. -/
inductive Capability where
  | net
  | netConnect
  | netListen
  | netBind
  | netPoll
  | dbRead
  | dbWrite
  | timeWall
  | time
  | process
  | processExec
  | websocketConnect
  deriving DecidableEq, Repr, BEq

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Runtime configuration
-- ══════════════════════════════════════════════════════════════════

/-- A capability set is a predicate over capabilities.
    This mirrors the Rust `HashSet<String>` from `load_capabilities`. -/
structure CapabilitySet where
  member : Capability → Bool

/-- Runtime configuration matching the Rust `is_trusted` / `has_capability` model.
    `trusted` corresponds to `MOLT_TRUSTED=1`.
    `grantedCaps` corresponds to the parsed `MOLT_CAPABILITIES` env var. -/
structure RuntimeConfig where
  trusted : Bool
  grantedCaps : CapabilitySet

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Capability check (mirrors Rust has_capability)
-- ══════════════════════════════════════════════════════════════════

/-- Check whether a runtime configuration grants a given capability.
    Mirrors the Rust `has_capability` function:
    ```rust
    pub(crate) fn has_capability(_py: &PyToken<'_>, name: &str) -> bool {
        if is_trusted(_py) { return true; }
        caps.contains(name)
    }
    ``` -/
def hasCapability (cfg : RuntimeConfig) (c : Capability) : Bool :=
  if cfg.trusted then true else cfg.grantedCaps.member c

/-- Prop version of capability check for theorem statements. -/
def HasCapability (cfg : RuntimeConfig) (c : Capability) : Prop :=
  hasCapability cfg c = true

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Capability set operations
-- ══════════════════════════════════════════════════════════════════

/-- The empty capability set (no capabilities granted). -/
def CapabilitySet.empty : CapabilitySet :=
  ⟨fun _ => false⟩

/-- The full capability set (all capabilities granted). -/
def CapabilitySet.full : CapabilitySet :=
  ⟨fun _ => true⟩

/-- Add a single capability to a set. -/
def CapabilitySet.insert (s : CapabilitySet) (c : Capability) : CapabilitySet :=
  ⟨fun c' => if c' == c then true else s.member c'⟩

/-- Subset relation on capability sets. -/
def CapabilitySet.subset (s₁ s₂ : CapabilitySet) : Prop :=
  ∀ (c : Capability), s₁.member c = true → s₂.member c = true

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Security property — trusted grants all
-- ══════════════════════════════════════════════════════════════════

/-- A trusted runtime grants all capabilities.
    This is the core bypass: `if is_trusted(_py) { return true; }`. -/
theorem trusted_grants_all (cfg : RuntimeConfig) (h : cfg.trusted = true) :
    ∀ (c : Capability), HasCapability cfg c := by
  intro c
  unfold HasCapability hasCapability
  simp [h]

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Security property — default deny
-- ══════════════════════════════════════════════════════════════════

/-- An untrusted runtime with no capabilities grants nothing (default-deny).
    Mirrors the Rust behavior: when not trusted, `caps.contains(name)` on
    an empty set returns false. -/
theorem no_caps_grants_nothing (cfg : RuntimeConfig)
    (hNotTrusted : cfg.trusted = false)
    (hEmpty : ∀ (c : Capability), cfg.grantedCaps.member c = false) :
    ∀ (c : Capability), ¬HasCapability cfg c := by
  intro c
  unfold HasCapability hasCapability
  simp [hNotTrusted, hEmpty c]

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Security property — decidability
-- ══════════════════════════════════════════════════════════════════

/-- All capability checks are decidable — the runtime can always
    compute a yes/no answer. This is immediate because `hasCapability`
    is a Bool-valued function. -/
instance capability_check_decidable (cfg : RuntimeConfig) (c : Capability) :
    Decidable (HasCapability cfg c) :=
  inferInstanceAs (Decidable (hasCapability cfg c = true))

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Security property — monotonicity (superset preserves access)
-- ══════════════════════════════════════════════════════════════════

/-- If a capability set grants access, any superset also grants access.
    This ensures that adding capabilities never removes existing ones. -/
theorem capability_subset_preserved (s₁ s₂ : CapabilitySet)
    (hSub : CapabilitySet.subset s₁ s₂)
    (trusted : Bool) (c : Capability)
    (hHas : HasCapability ⟨trusted, s₁⟩ c) :
    HasCapability ⟨trusted, s₂⟩ c := by
  unfold HasCapability hasCapability at *
  cases ht : trusted
  · simp_all; exact hSub c hHas
  · simp_all

/-- Adding a capability to a set produces a superset. -/
theorem insert_is_superset (s : CapabilitySet) (c : Capability) :
    CapabilitySet.subset s (s.insert c) := by
  intro c' hMem
  unfold CapabilitySet.insert
  simp only
  split
  · rfl
  · exact hMem

/-- Monotonicity: adding a capability cannot remove others. -/
theorem capability_monotonic (cfg : RuntimeConfig) (c cNew : Capability)
    (hHas : HasCapability cfg c) :
    HasCapability ⟨cfg.trusted, cfg.grantedCaps.insert cNew⟩ c := by
  exact capability_subset_preserved cfg.grantedCaps (cfg.grantedCaps.insert cNew)
    (insert_is_superset cfg.grantedCaps cNew) cfg.trusted c hHas

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Security property — irrevocability (state invariant)
-- ══════════════════════════════════════════════════════════════════

/-- A runtime session is a sequence of configurations where capabilities
    can only grow (never be revoked). This models the Rust runtime where
    capabilities are loaded once from env vars and cached immutably. -/
structure RuntimeSession where
  /-- Configuration at each step (indexed by natural number). -/
  configAt : Nat → RuntimeConfig
  /-- Trust is monotonic: once trusted, always trusted. -/
  trust_monotone : ∀ (i : Nat), (configAt i).trusted = true →
    (configAt (i + 1)).trusted = true
  /-- Capabilities are monotonic: once granted, never revoked. -/
  caps_monotone : ∀ (i : Nat), CapabilitySet.subset
    (configAt i).grantedCaps (configAt (i + 1)).grantedCaps

/-- Capabilities are irrevocable: if a capability is granted at step i,
    it is granted at step i + 1. -/
theorem capability_irrevocable (session : RuntimeSession) (i : Nat)
    (c : Capability) (hHas : HasCapability (session.configAt i) c) :
    HasCapability (session.configAt (i + 1)) c := by
  unfold HasCapability hasCapability at *
  cases ht : (session.configAt i).trusted
  · -- untrusted case: capability must be in the set
    simp [ht] at hHas
    cases ht' : (session.configAt (i + 1)).trusted
    · simp
      exact session.caps_monotone i c hHas
    · simp
  · -- trusted case: trust is preserved
    have := session.trust_monotone i ht
    simp [this]

/-- Irrevocability extends transitively: if granted at step i, granted at
    any later step j ≥ i. -/
theorem capability_irrevocable_transitive (session : RuntimeSession)
    (i j : Nat) (hle : i ≤ j)
    (c : Capability) (hHas : HasCapability (session.configAt i) c) :
    HasCapability (session.configAt j) c := by
  induction hle with
  | refl => exact hHas
  | step _ ih => exact capability_irrevocable session _ c ih

-- ══════════════════════════════════════════════════════════════════
-- Section 10: Security property — determinism
-- ══════════════════════════════════════════════════════════════════

/-- Capability checks are deterministic: the same config and capability
    always produce the same result. This is trivially true because
    `hasCapability` is a pure function, but we state it explicitly. -/
theorem capability_deterministic (cfg : RuntimeConfig) (c : Capability) :
    hasCapability cfg c = hasCapability cfg c :=
  rfl

/-- Stronger determinism: two configs with the same fields yield the same result. -/
theorem capability_deterministic_ext (cfg₁ cfg₂ : RuntimeConfig) (c : Capability)
    (hTrust : cfg₁.trusted = cfg₂.trusted)
    (hCaps : cfg₁.grantedCaps.member c = cfg₂.grantedCaps.member c) :
    hasCapability cfg₁ c = hasCapability cfg₂ c := by
  unfold hasCapability
  cases ht : cfg₁.trusted
  · simp [ht, ← hTrust, hCaps]
  · simp [ht, ← hTrust]

-- ══════════════════════════════════════════════════════════════════
-- Section 11: Concrete validation
-- ══════════════════════════════════════════════════════════════════

/-- A trusted config with empty caps grants net. -/
theorem trusted_empty_grants_net :
    hasCapability ⟨true, CapabilitySet.empty⟩ Capability.net = true := by native_decide

/-- An untrusted config with empty caps denies net. -/
theorem untrusted_empty_denies_net :
    hasCapability ⟨false, CapabilitySet.empty⟩ Capability.net = false := by native_decide

/-- An untrusted config with net in the set grants net. -/
theorem untrusted_with_net_grants_net :
    hasCapability ⟨false, ⟨fun c => match c with | Capability.net => true | _ => false⟩⟩
      Capability.net = true := by native_decide

/-- An untrusted config with net in the set denies dbRead. -/
theorem untrusted_with_net_denies_dbRead :
    hasCapability ⟨false, ⟨fun c => match c with | Capability.net => true | _ => false⟩⟩
      Capability.dbRead = false := by native_decide

end MoltTIR.Runtime
