# Optimization evidence registry (HF-460)

Status: design, 2026-10-22. Not implemented.

## Problem

Molt repeats measured work. Its verdicts on optimizations live in prose: doc 75 §4, ledger rows
such as HF-263's measured test-result cache, lane notes and session memory. A prose verdict cannot
say when it stops holding.

An optimization's outcome depends on its whole context: the code around it, the representations,
carriers and protocols it touches, the pass pipeline, the target and the workload. When one of
those changes, a rejected idea can start to pay, and a landed one can stop paying. A static
"do not redo" list cannot see that.

## Prior art

- tsrs keeps a "measured and rejected (do not redo)" list in `notes/perf-round2-followups.md` and
  tells contributors to read it before an experiment [1]. An idea returns only with new evidence.
  The list is free text, and its revisit conditions are prose.
- Interpretable machine learning as the Pact project applies it [2]:
  - a SLIM scorer whose `explain()` reads as arithmetic [3];
  - falling rule lists: the autopilot's rules each carry a predicted score band, and the preflight
    gate's rules each carry a recommended fix [4];
  - a Rashomon set of near-optimal models whose disagreement chooses the next measurement [5];
  - GOSDT decision trees for dispatch [6];
  - continual updates from new anchors.

  Pact's rule "preflight failure messages must cite the rule chain" makes each explanation a
  contract.
- Schmidhuber's Success-Story Algorithm keeps a self-modification only while the reward rate
  measured since that modification stays higher than before it; it undoes the others [7]. His
  curiosity work rewards compression progress, so the next experiment is the one the model expects
  to learn most from [8]. History compression passes on only what a model failed to predict [9].

## Design

1. **Anchors.** One typed record per measured outcome:
   - the optimization, as a typed family: pass rule, representation, carrier, scheduler, build
     setting, or test selection;
   - the change: its Change-Id, commit or bundle;
   - the correctness verdict from an independent oracle: the differential suite, or byte-identical
     emitted programs;
   - the outcome per target cell: on deterministic axes, single-threaded instructions retired
     (HF-459) and code size; on measured axes, peak RSS and wall time, each with its run count and
     spread.

   Anchors cite existing evidence and add no measurement store of their own. They cite
   `perf_history` board identities (revision, methodology, suite, host class), landing receipts
   for gate wall time, and `perf_causality` attributions.
2. **Context fingerprint.** Typed features drawn from Molt's generated authorities, never prose:
   - the op-kind table and its facts;
   - the heap-kind, ABI, carrier and intrinsic manifests;
   - the pass pipeline's order and settings (doc 76);
   - the target cell: OS, architecture, backend, profile and Python version;
   - the toolchain identity;
   - the workload suite hash;
   - the digests of the code the optimization touches and reads.
3. **Models, interpretable only.** Each family has three models:
   - a SLIM score of expected gain over the context features, with an `explain()` that reads as
     arithmetic;
   - a falling rule list for the verdict (keep, reject or reopen), where each rule carries its
     action: the command that re-measures it, and its owner;
   - a Rashomon set of near-optimal models.

   No black-box model decides a verdict.
4. **Lifecycle (success story).** A landed optimization keeps its place only while its measured
   gain in the current context clears the landing bar. When a feature that drove its verdict
   changes, the registry queues a re-measurement. A gain that no longer pays reopens the entry
   with its evidence. A rejected optimization reopens in two cases: its predicted gain in the new
   context crosses the bar, or the Rashomon set disagrees about it.
5. **Experiment choice (curiosity).** A fixed re-measurement budget goes to the experiments with
   the highest expected learning progress per unit of measurement cost. Outcomes the models
   predict with confidence are not re-run. Each anchor stores its residual against the
   prediction, so surprises surface first.
6. **Contracts.** Every verdict cites its rule chain and its action. Agents query the registry
   before they design. A performance landing cites its entries. The ledger links to entries and
   does not restate them.
7. **Self-test.** A gate that cannot fail proves nothing. A planted context change must reopen a
   planted rejection, and a planted surprise must be flagged.

## Cost

| Path | Cost | Scaling |
|---|---|---|
| Anchor write | about 1 KiB of JSON per measured change (identifiers and digests) | linear in measurements |
| Fingerprint | digests the generated authorities already compute | constant per run |
| Model fit | sparse models over thousands of anchors, offline; an estimate of milliseconds to seconds that Phase 2 measures (SLIM is an integer program, and Rashomon enumeration grows with depth) | per family |
| Query | an index lookup by family or path | constant |
| Re-measurement | the scoreboard's existing cost, spent where expected learning is highest | bounded by the budget |

None of this enters the user's compile path.

## Phases

1. Anchors, the fingerprint and `query`, seeded from doc 75 §4, HF-263's measured rejection, the
   ledger's measured rejections, the gate-speed measurements (HF-457, HF-334), and the April
   compiler PGO verdict (0c587cd8e: about 0% while the backend was 1.5% of compile wall clock). Verdicts
   start as hand-written falling rules, each with its evidence.
2. Fit SLIM scores and learned rule lists for each family that has enough anchors. Add the
   reopen trigger on fingerprint changes.
3. Add the Rashomon disagreement queue and the curiosity budget.

## Sources

1. tsrs at commit 89ea430, `notes/README.md` and `notes/perf-round2-followups.md`:
   https://github.com/oxc-project/tsrs
2. Pact, `src/tac/autopilot_rudin_daubechies/`, `src/tac/preflight_rudin_daubechies/`
   (`PreflightFallingRule.recommended_fix`) and `AGENTS.md` ("Preflight failure messages must cite
   the rule chain"): https://github.com/adpena/comma-lab
3. Ustun, Rudin. Supersparse linear integer models for optimized medical scoring systems. Machine
   Learning 102, 2016.
4. Wang, Rudin. Falling Rule Lists. AISTATS 2015.
5. Semenova, Rudin, Parr. On the Existence of Simpler Machine Learning Models (the Rashomon
   ratio). ACM FAccT 2022.
6. Lin, Zhong, Hu, Rudin, Seltzer. Generalized and Scalable Optimal Sparse Decision Trees. ICML
   2020.
7. Schmidhuber, Zhao, Wiering. Shifting Inductive Bias with Success-Story Algorithm, Adaptive
   Levin Search, and Incremental Self-Improvement. Machine Learning 28(1):105–130, 1997.
8. Schmidhuber. Formal Theory of Creativity, Fun, and Intrinsic Motivation (1990–2010). IEEE
   Transactions on Autonomous Mental Development 2(3):230–247, 2010.
9. Schmidhuber. Learning Complex, Extended Sequences Using the Principle of History Compression.
   Neural Computation 4(2):234–242, 1992.
