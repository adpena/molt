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

## GPT-6 Astra and GPT-5.6 review (2026-09-14)

Official sources read for this review:

- Eric Provencher, [Rethinking skills and prompts for GPT-6 Astra](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra).
- [GPT-6 Astra prompting best practices](https://developers.openai.com/api/docs/guides/latest-model/gpt-6-astra#prompting-best-practices).
- [Prompting guidance for GPT-5.6 Sol](https://developers.openai.com/api/docs/guides/prompt-guidance-gpt-5p6).

Both model families benefit from outcome, evidence, permission boundaries, and
completion criteria without repeated process instructions. Astra's stronger
instruction following makes obsolete stop rules, broad skill triggers, and
mandatory document itineraries especially costly. Its thorough testing and
tentative stopping behavior call for task-appropriate proof and an explicit
integration outcome. Sol guidance likewise favors trimming one instruction
group at a time and evaluating the same representative tasks.

Apply those findings through the existing hierarchy:

- Keep `CLAUDE.md` as `@AGENTS.md`; do not copy model-specific rule sets into it.
- Route by task. The historical orchestration board is supporting context;
  live commands and registered ownership establish current work state.
- Name when delegation helps and its write boundaries. The existing registry
  is discovery, not source locking; the handoff protocol owns reconciliation.
- Preserve compiler matrix requirements, source custody, and process safety.
  Prompt simplification cannot turn a unit result into native/WASM acceptance.
- Stop repeating successful checks when inputs and claims have not changed.
  Carry authorized implementation through its named integration outcome.
- Test-authoring policy and its research sources live in
  [the testing strategy](../spec/areas/testing/0007-testing.md#test-quality-and-agent-written-tests).
  The root constitution routes test work there; `CLAUDE.md` inherits the same
  route through its import, without another prompt body or forced document load.
- Audit a skill when its trigger, permission rule, or procedure actually affects
  the task. Keep descriptions narrow and route supporting material on demand;
  do not modify installed third-party skills as part of repository maintenance.

The observed regression was missing live registration despite active workers,
not lack of more emphatic instructions. Registration and content-bound handoff
are the relevant correction. Validate the hierarchy with the existing checker;
evaluate behavioral changes on representative continuation, conflicting-WIP,
and failed-proof recoveries. A green document check alone does not demonstrate
better model behavior, lower latency, or complete consolidation. Model defaults
and reasoning effort remain unchanged by this instruction review.
