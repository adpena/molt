# Agent Instruction Design

`AGENTS.md` is Molt's single always-loaded project constitution. `CLAUDE.md`
imports it with Claude Code's supported `@AGENTS.md` syntax so the two clients
cannot drift.

## Design rules

1. Keep the root constitution below 200 lines and limited to facts or boundaries
   that matter in almost every session.
2. State outcomes and invariants at the altitude a capable engineer can apply.
   Do not encode brittle step-by-step behavior, thought-policing phrases, or a
   catalogue of every historical failure.
3. Put subsystem rules next to the subsystem, repeatable procedures in skills or
   runbooks, mechanically enforced policy in tests/hooks, and changing lane state
   in the orchestration board.
4. Add a root instruction only after a repeated error demonstrates that the
   model cannot reliably infer it from code, tests, or a nearer authority.
5. Review instructions when models, tools, workflows, targets, or architecture
   change. Delete rules whose motivating behavior is no longer present.
6. Prefer one clear boundary over several overlapping prohibitions. Contradictory
   instructions reduce adherence and must be reconciled immediately.
7. Do not force-load referenced documents. Pointers support just-in-time context;
   imports are reserved for genuinely universal material.

## Maintenance test

For every proposed always-loaded sentence, ask:

- Would a capable new maintainer need this in most tasks?
- Is it current, verifiable, and owned here?
- Does it describe the result or invariant rather than micromanage the method?
- Would a test, hook, nested rule, skill, design, or live board be a better home?
- Does it enable judgment, or merely encode anxiety about an older model?

If the answer points elsewhere, move or delete the sentence instead of growing
the constitution.

## Rationale

Current OpenAI guidance describes a short, accurate `AGENTS.md` as more useful
than a long file of vague or repeated rules and recommends moving task-specific
guidance to referenced files. Current Claude Code guidance targets fewer than
200 lines, recommends path-scoped rules or skills for narrower procedures, and
explicitly supports importing `AGENTS.md` from `CLAUDE.md`. Anthropic's Fable 5
guidance recommends re-evaluating old scaffolding because stronger instruction
following makes prior-model prompts unnecessarily prescriptive; it also favors
evidence-grounded progress, asynchronous subagents, durable memory, and pausing
only for genuine user dependencies.

External guidance informs this design but does not supersede Molt's engineering
requirements. The repository's live architecture and measured agent behavior
remain the deciding evidence.
