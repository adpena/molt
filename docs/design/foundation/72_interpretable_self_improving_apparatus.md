# 72 — The Interpretable, Self-Improving Apparatus

**Status:** executable plan. **Owner:** orchestrator. **Date:** 2026-07-04.

The Molt "apparatus" — the orchestration control plane: `tools/proof_queue.py`
(proof/build custody + deterministic diagnosis), `tools/memory_guard.py` +
`tools/win_job.py` + `tools/orphan_reaper.py` (process custody), `tools/
structural_audit.py` + `tools/canonicalization_contract.py` (structural
ratchets), the multi-agent board (`docs/agent/ORCHESTRATION.md`), and the
persistent memory — is a **high-stakes automated decision system**. It kills
processes, quarantines proofs, gates landings, classifies failures, and routes
lanes. High-stakes automated decisions are exactly the regime three research
programs speak to. This plan grounds their principles in primary sources and
maps each to a concrete apparatus mechanism.

Guardrail (binding, from `instrumental-serves-outcomes`): every mechanism here
is INSTRUMENTAL. It earns its place only by accelerating the 100-year OUTCOMES
(perf > CPython, canonicalization shipped, memory-safety floor, CPython ≥3.12
parity, the numpy/field_solve/WASM path). Meta-work that does not cash into
outcomes is deleted.

## Grounding (primary sources)

- **Cynthia Rudin — interpretability by construction.** "Stop Explaining Black
  Box Machine Learning Models for High Stakes Decisions and Use Interpretable
  Models Instead" (Nat. Mach. Intell. 1, 206–215, 2019; arXiv:1811.10154). For
  high-stakes decisions, do NOT post-hoc "explain" an opaque scorer — use a
  model whose reasoning is transparent BY CONSTRUCTION (sparse decision rules,
  scoring systems with explicit point provenance, prototypes). Later: optimal
  sparse decision trees, scoring systems, and the "Rashomon set" of equally-good
  interpretable models.
- **Ingrid Daubechies — multi-resolution.** Wavelets give a signal a
  coarse-to-fine multi-scale representation; with Rudin (e.g. "Adaptive Wavelet
  Distillation," arXiv:2107.09145; sparse GAM Rashomon-set work, AISTATS 2024)
  the theme is compact, multi-scale, human-legible structure.
- **Jürgen Schmidhuber — compression progress & provable self-improvement.**
  "Formal Theory of Creativity, Fun, and Intrinsic Motivation" (IEEE TAMD 2010):
  interestingness = the FIRST DERIVATIVE of the observer's compression progress —
  data is interesting exactly when a previously-unexplained regularity suddenly
  becomes explainable/compressible. Two modules: an adaptive compressor of the
  history, and a learner rewarded by the compressor's LEARNING PROGRESS. Gödel
  machine (arXiv:cs/0309048): rewrite your own code only once you have PROVEN the
  rewrite raises future utility. PowerPlay: continually seek the simplest still-
  unsolvable problem, growing a verified repertoire ordered by difficulty.

## The mapping — principle → apparatus mechanism

| Research principle | Apparatus mechanism (nascent today → target) |
|---|---|
| Rudin: high-stakes decisions must be interpretable-by-construction, not black-box scores | proof_queue `_diagnostic(signal_id, summary, evidence, next_action, scopes)` is already a transparent rule. TARGET: an **interpretability contract** — every automated decision (kill, quarantine, gate, classify) must be produced by a named rule citing its evidence; any bare numeric score (`structural_god_score`, severity) must expose its sparse, additive provenance (a Rudin scoring system), never an opaque threshold. |
| Schmidhuber: interestingness = compression progress; reward learning-progress | proof_queue's `unclassified-failed-proof` → "add a deterministic diagnosis rule before this becomes tribal knowledge" is the compression loop by hand. TARGET: **measure** it — the fraction of distinct failure signatures covered by a rule is the apparatus's compression ratio; its rise over time is compression PROGRESS; the highest recurrence×cost UNCLASSIFIED signature is the most-interesting next self-improvement target (the curiosity queue). |
| Schmidhuber: Gödel machine — rewrite only once PROVEN useful | The memory `world-class-rigor-no-fakes` already demands "prove the gate FAILS on a synthetic violation" before landing a gate. TARGET: make it a **contract** — every new diagnosis rule / gate ships with a positive proof (fires on the real signature) AND a negative control (silent on clean input). No rule lands without both. |
| Schmidhuber: PowerPlay — grow a verified repertoire, simplest-unsolved first | The gate portfolio (structural_audit, canonicalization_contract, op_family, wasm-triple) is a growing repertoire. TARGET: order the curiosity queue by difficulty and always compress the cheapest still-uncompressed failure class next. |
| Daubechies: multi-resolution, coarse→fine | The `coupled-analysis` infra (trace ONE value AST→TIR-repr→alloc→binary) is multi-scale by value; structural_audit/board are the coarse view. TARGET: a uniform **zoom** — portfolio → lane → row → diagnosis, and suite → op → binary — so any signal is legible at every scale without a bespoke report. |

## Concrete integrations (build order)

1. **Compression-progress ledger** (`tools/apparatus_ledger.py`, this arc):
   scans the proof-queue run history, normalizes each failure into a stable
   SIGNATURE, and reports (a) the compression ratio = classified / total
   distinct signatures, (b) the curiosity queue = recurring `unclassified`
   signatures ranked by recurrence × cost (the Schmidhuber "most interesting"
   next rule to write), and (c) an interpretability audit = any failed row whose
   only signal is a bare status with no named rule. This is the apparatus's
   self-model + intrinsic-reward signal, in Rudin-interpretable form.
2. **Interpretability contract** (extends `canonicalization_contract.py`): a
   check that every apparatus decision path emits a named `_diagnostic`/rule with
   evidence + next_action; flag black-box thresholds lacking a sparse-additive
   provenance.
3. **Rule-falsifiability contract** (extends the gate-authoring flow): a new
   diagnosis rule / structural gate must land with a positive fixture and a
   negative control; CI proves the gate fails-closed on the synthetic violation.
4. **Uniform multi-resolution zoom** (extends the board + coupled-analysis): one
   drill-down grammar across portfolio → lane → row → op → binary.

## Why this serves the outcomes

Recurring un-compressed failures are pure drag on the 100-year work: every
`unclassified-failed-proof` re-paid as manual log archaeology is time not spent
on perf/parity/decomposition. Turning the ad-hoc "should become a rule" note
into a MEASURED, curiosity-ranked loop makes the apparatus learn from its own
mistakes at a measured rate — Schmidhuber's compression progress in service of
Rudin-interpretable, Gödel-disciplined self-improvement. The ledger's rising
compression ratio is a direct, honest readout of how fast the apparatus is
retiring its own drag.
