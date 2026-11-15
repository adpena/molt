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

### 2025–2026 work and adjacent prior art (researched 2026-11-14)

Each entry names what it changes in this design.

- **Metaproductivity (Huxley-Gödel Machine, Wang, ..., Schmidhuber, ICLR 2026) [10].** A
  self-modification's benchmark score is a poor proxy for the gains of its descendants; HGM ranks
  a change by its clade's metaproductivity (CMP), the aggregated scores of the changes built on it.
  Molt has the same mismatch: typed `FunctionId` (HF-847 slice 1) changes instructions by zero by
  design, yet slices 2–6 depend on it. *Change:* an anchor records the typed `enables` edges to the
  changes that consume its facts, and the lifecycle credits a change with its clade's measured
  gains, not only its own.
- **Empirical in place of formal proof (Darwin Gödel Machine, Zhang, Hu, Lu, Lange, Clune, 2025)
  [11]; the Gödel Machine (Schmidhuber 2003–2007) [12].** The Gödel Machine rewrites itself only
  with a proof of benefit; DGM replaces the proof with measured benchmark evidence and keeps an
  archive of all variants, so a weak variant can seed a later strong one. *Change:* correctness
  keeps the proof discipline (translation validation, below), benefit is measured; rejected
  variants stay in the registry as reopenable anchors, never deleted.
- **Airtight metrics (AlphaEvolve, Google DeepMind, 2025) [13].** An evolutionary coding agent
  found a 23% faster matmul tiling heuristic and up to 32.5% on XLA-generated FlashAttention IR,
  because every candidate was scored by an automatic evaluator. *Change:* the deterministic axis
  (instructions retired, HF-459) is the registry's evaluator; wall time is a measured axis with its
  spread, never a verdict alone.
- **Many good models (Rudin et al., ICML 2024 position paper) [14]; Rashomon-set enumeration
  (SORTeD anytime enumeration, 2025 [15]; Rashomon ensemble active learning, 2026 [16]).** Where
  near-optimal sparse models disagree, the data does not decide; the disagreement is the next
  measurement. *Change:* the Rashomon queue (Phase 3) uses anytime enumeration, so a partial set
  can already rank the queue.
- **Sparse recovery and multiscale analysis (Daubechies) [17, 18, 19].** Iterative soft
  thresholding recovers a sparse coefficient vector from few linear measurements; iteratively
  reweighted least squares does the same with guarantees; wavelets separate a signal by scale.
  Performance work has the same shape: a few context features drive most of an outcome, and a
  measurement series mixes slow drift, step regressions and noise. *Change:* fit each family's
  feature weights with an ℓ1-sparse solver before SLIM rounds them to integers, so the score
  names few features; analyze each benchmark's per-commit instruction series by scale, so a step
  regression is told apart from drift. Pact's `wavelet_multi_scale_ranker` and
  `compressive_sensing_lattice_recovery` are the local precedents.
- **Performance-influence models (Siegmund et al., ESEC/FSE 2015; Fourier learning, ASE 2015; Ha
  and Zhang, Fourier and Lasso, ICSME 2019) [20, 21, 22].** Configurable systems' performance is
  learned as a sparse sum of option and interaction terms from few sampled configurations.
  *Change:* pass and representation choices are the options; the registry samples configurations
  by expected learning (item 5), not exhaustively.
- **Correctness apart from heuristics (MLGO, Trofin et al., LLVM) [23].** LLVM's learned inlining
  and eviction policies decide only among correctness-preserving choices. *Change:* a model in
  this registry chooses among proven-correct options (a threshold, an order, a representation);
  it never decides a semantic fact. A bound it learns replaces a hand-picked constant and states
  its derivation.
- **Rewrite only with proof (translation validation).** Alive2 checks LLVM's peephole rewrites
  against their specifications [24]; egg produces proofs for equality-saturation rewrites [25];
  Cranelift's acyclic e-graphs apply rewrites as they match [26]. *Change:* an anchor's
  correctness verdict cites the formal-model check of its rewrite (AGENTS.md: rewrites prove
  themselves against the formal model), not only a passing differential suite.
- **Typed refusals as labeled data.** A pass that refuses an optimization records a closed-enum
  reason with the site's typed context (for example `GuardedCallRefusal`, call_anatomy.md 4.5a).
  *Change:* refusals are anchors with outcome "refused" and their reason as a feature, so a
  context change that removes a reason reopens exactly the sites it unblocks.

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
   instruction-signal records (`tools/perf_instructions.py`: commit and run identity), landing
   receipts for gate wall time, and `perf_causality` attributions.
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
   compiler PGO verdict (de0c1b22b: about 0% while the backend was 1.5% of compile wall clock). Verdicts
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
10. Wang, Piękos, Li, Laakom, Chen, Ostaszewski, Zhuge, Schmidhuber. Huxley-Gödel Machine:
    Human-Level Coding Agent Development by an Approximation of the Optimal Self-Improving
    Machine. ICLR 2026; arXiv:2510.21614. Code: https://github.com/metauto-ai/HGM
11. Zhang, Hu, Lu, Lange, Clune. Darwin Gödel Machine: Open-Ended Evolution of Self-Improving
    Agents. arXiv:2505.22954, 2025.
12. Schmidhuber. Gödel Machines: Fully Self-Referential Optimal Universal Self-Improvers. In
    Artificial General Intelligence, Springer, 2007 (arXiv:cs/0309048, 2003).
13. Novikov et al. (Google DeepMind). AlphaEvolve: A coding agent for scientific and algorithmic
    discovery. arXiv:2506.13131, 2025.
14. Rudin et al. Position: Amazing Things Come From Having Many Good Models. ICML 2024, PMLR
    235:42783–42795.
15. SORTeD Rashomon Sets of Sparse Decision Trees: Anytime Enumeration. arXiv:2511.03344, 2025.
16. REALITrees: Rashomon Ensemble Active Learning for Interpretable Trees. arXiv:2603.22750, 2026.
17. Daubechies, Defrise, De Mol. An iterative thresholding algorithm for linear inverse problems
    with a sparsity constraint. Communications on Pure and Applied Mathematics 57(11):1413–1457,
    2004.
18. Daubechies, DeVore, Fornasier, Güntürk. Iteratively reweighted least squares minimization for
    sparse recovery. Communications on Pure and Applied Mathematics 63(1):1–38, 2010.
19. Daubechies. Ten Lectures on Wavelets. SIAM, 1992.
20. Siegmund, Grebhahn, Apel, Kästner. Performance-influence models for highly configurable
    systems. ESEC/FSE 2015.
21. Zhang, Guo, Blais, Czarnecki. Performance prediction of configurable software systems by
    Fourier learning. ASE 2015.
22. Ha, Zhang. Performance-influence model for highly configurable software with Fourier learning
    and Lasso regression. ICSME 2019.
23. Trofin, Qian, Brevdo, Lin, Choromanski, Li. MLGO: a Machine Learning Guided Compiler
    Optimizations Framework. arXiv:2101.04808, 2021; https://llvm.org/docs/MLGO.html
24. Lopes, Lee, Hur, Liu, Regehr. Alive2: Bounded Translation Validation for LLVM. PLDI 2021.
25. Willsey, Nandi, Wang, Flatt, Tatlock, Panchekha. egg: Fast and Extensible Equality
    Saturation. POPL 2021.
26. Cranelift `cranelift-egraph` (acyclic e-graphs): https://docs.rs/cranelift-egraph
