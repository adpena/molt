# AI Agent Operating Assumptions (Binding)

AI agents working on Molt may assume the following environment:

## Execution model
- Long‑running processes are allowed and expected
- tmux sessions persist across disconnects
- The agent may detach and resume work later
- Output will not be lost on network failure

## Planning implications
Agents are encouraged to:
- Plan multi‑stage tasks
- Emit intermediate checkpoints
- Write logs and artifacts to disk
- Run benchmarks asynchronously
- Resume from partial state instead of restarting

## Prohibited assumptions
Agents must NOT assume:
- Single‑shot execution
- Ephemeral terminals
- Immediate human supervision
- That work must finish in one interaction

## Recommended practices
- Use clear window names in tmux
- Write progress summaries periodically
- Leave instructions for resumption
- Prefer idempotent scripts

These assumptions are part of Molt’s engineering culture.
