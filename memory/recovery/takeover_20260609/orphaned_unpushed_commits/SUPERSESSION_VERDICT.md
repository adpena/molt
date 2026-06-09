# Supersession verdict (git cherry vs origin/main fb57f16fa, 2026-06-09)

All 6 unpushed orphaned commits preserved as patches in this dir. git-cherry verdict:

| worktree | commit | verdict | disposition |
|---|---|---|---|
| wt_laneB2 | 087d5926e split-field read deforestation (perf) | `-` equivalent on origin | SUPERSEDED — safe |
| wt_p59 | ad14533af #59 IC marker-transmute SIGSEGV fix | `-` equivalent on origin | SUPERSEDED — confirms #59 shipped |
| wt_round10 | 13ecbdb16 structured-loop polarity-from-CFG | `-` equivalent on origin | SUPERSEDED — safe |
| wt_round10 | 6a305f94c round-11 baton (doc) | `-` equivalent on origin | SUPERSEDED — safe |
| **wt_77** | 48896724e #77 per-raise-leak DIAGNOSIS + RED regression + zero-cost EXC_RC trace | `+` NOT on origin | UNIQUE (diagnostic). #77 cycle-attribution finding is COMPLETED; this is the instrumentation/RED-test behind it. Patch retained; integrate the EXC_RC trace + RED test if the exception_heavy perf arc resumes. |
| **wt_genthrow** | 8e6a0b63a #38 gen.throw resumption state-machine WIP (TIR+LLVM) | `+` NOT on origin | UNIQUE, UNFINISHED. Rate-limit-orphaned mid-implementation of generator `.throw()` resumption (#38 bare-raise-across-yield). Patch retained; resume from here when #38 is scheduled. |

No signal lost: superseded commits' content is on origin; the 2 unique commits are preserved
verbatim as format-patch. The orphaned worktrees can now be pruned safely.
