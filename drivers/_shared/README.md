# Shared Driver Helpers

Shared driver helpers live here.

Rules:
- Keep target-agnostic manifest, hashing, verification, and config parsing here.
- Keep host/runtime-specific behavior in the appropriate target namespace.
- Keep model-specific overrides under `drivers/<model>/...`, not here.
