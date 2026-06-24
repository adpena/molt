# Supersession verdict (git cherry vs origin/main fb57f16fa, 2026-06-09)

All 6 unpushed orphaned commits preserved as patches in this dir. git-cherry verdict:

| worktree | commit | verdict | disposition |
|---|---|---|---|
| wt_laneB2 | 087d5926e split-field read deforestation (perf) | `-` equivalent on origin | SUPERSEDED — safe |
| wt_p59 | ad14533af #59 IC marker-transmute SIGSEGV fix | `-` equivalent on origin | SUPERSEDED — confirms #59 shipped |
| wt_round10 | 13ecbdb16 structured-loop polarity-from-CFG | `-` equivalent on origin | SUPERSEDED — safe |
| wt_round10 | 6a305f94c round-11 baton (doc) | `-` equivalent on origin | SUPERSEDED — safe |
| **wt_77** | 48896724e #77 per-raise-leak DIAGNOSIS + RED regression + zero-cost EXC_RC trace | `+` NOT on origin | UNIQUE (diagnostic). #77 cycle-attribution finding is COMPLETED; this is the instrumentation/RED-test behind it. Patch retained; integrate the EXC_RC trace + RED test if the exception_heavy perf arc resumes. |
| **wt_genthrow** | 8e6a0b63a former #38 generator `.throw()` recovery patch | resolved in current main | SUPERSEDED by landed StateDispatch/generator `.throw()` resumption support; the recovery matrix is promoted to `tests/differential/basic/generator_throw_resumption.py` and the old patch is deleted. |

No signal lost: superseded commits' content is on origin. `wt_77` remains the
only unique diagnostic patch in this verdict; `wt_genthrow` is now covered by a
tracked differential regression and its recovery patch can be pruned safely.
