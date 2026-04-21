# Falcon-OCR Artifact Boundary

This directory is the canonical Molt-side landing zone for Falcon-OCR
artifact metadata and manifests.

The first tranche intentionally keeps implementation ownership unchanged:

- Molt owns the compiled runtime and `src/molt/stdlib/tinygrad/examples/falcon_ocr.py`.
- enjoice remains the current source for the local experiment artifact tree.
- Large weight blobs are external artifacts and must not be committed here.

Tests should depend on `tests.helpers.falcon_ocr_paths` instead of hardcoded
absolute paths so the artifact root can move to this directory in one future
change.
