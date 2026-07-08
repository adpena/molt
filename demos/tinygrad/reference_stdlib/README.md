# Quarantined Tinygrad Reference

This directory preserves the former Molt-owned `tinygrad` compatibility package
for research, science, regression archaeology, and reference comparison only.
It is intentionally outside `src/` and is not a compiler stdlib/runtime package.
The `.molt-research-quarantine` marker makes that status machine-readable for
`tools/fail_closed_gate.py`.

Production tinygrad support must compile upstream tinygrad Python and extensions
through package/import custody with automatic toolchain provisioning. Molt GPU
may use tinygrad's primitive model as the model for Molt-owned compiler/runtime
primitives, but Molt must not ship this reference clone as the implementation of
the third-party package.

Tests that need the old reference behavior load it explicitly through
`tests/helpers/tinygrad_stdlib_loader.py`; production imports must not depend on
this directory.
