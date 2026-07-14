# Harvested Pact evidence

This directory preserves unique evidence recovered before the Pact collaboration
worktrees and branches were retired. It contains evidence only, not executable
product authority.

## 2026-07-10 DTypeMeta witness run

- Run: `manual-20260710T125349.921684Z-23156`
- Guard artifact: `20260710_dtypemeta_witness_memguard.json`
- Guard SHA-256: `ca4653734d5f6e65a629177b46b827a181ff92ba3403ab43e60ab1c0f07c6534`
- Result: return code 1 after 1026.313 seconds; no timeout, resource violation,
  incident, or orphaned process group.
- Peak RSS: 1.945133 GiB for the build process and 2.891800 GiB for the
  guarded process tree.
- Frontier artifact SHA-256:
  `e778e4faa4579fb629416dea069b9911fd25f4cbce9695f9ffbc5b0d8e3d02be`.
- Frontier: `_multiarray_umath` built and linked, then its static-link
  `Py_mod_exec` slot returned non-zero without setting an exception.

The original worktree also contained ad hoc launch scripts, replay logs, build
logs, and keyed runtime checksums. Their semantic results are already recorded
in `docs/agent/CLAIMS.md`; they contained no additional production source or
durable authority and were not retained.
