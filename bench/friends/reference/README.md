# Reference Lane Scaffold

This directory is the canonical home for the reference-lane scaffolding used by
the first Falcon model lane.

Layout:

- `pins.toml` is the editable source of truth for models, lanes, and local
  lane paths.
- `tinygrad/` and `tinygpu/` are lane-local workspaces. They are intentionally
  empty apart from documentation so the directory layout is stable from the
  start.
- `bench/results/reference_manifest.json` is the generated, repo-checked
  manifest emitted from `tools/reference_fetch.py`.

The first concrete model is `falcon-1`, but the schema is intentionally
model-agnostic so additional models can be added without changing the layout.

Run the placeholder harnesses from the repo root:

```bash
python3 tools/reference_fetch.py --json
python3 tools/reference_compare.py --json
python3 tools/bench_reference.py --json
```

These commands only validate the manifest and lane paths today. They are
designed to grow into real fetch/compare/bench flows without changing the
directory structure.
