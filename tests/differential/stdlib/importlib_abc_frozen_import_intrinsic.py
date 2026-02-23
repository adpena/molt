"""Purpose: validate importlib.abc frozen bootstrap wiring through import intrinsics."""

import importlib.abc as abc

print("has_loader", hasattr(abc, "Loader"))
frozen = getattr(abc, "_frozen_importlib", None)
print(
    "frozen_name_ok",
    frozen is None or getattr(frozen, "__name__", "") == "_frozen_importlib",
)
frozen_external = getattr(abc, "_frozen_importlib_external", None)
print(
    "frozen_external_name_ok",
    getattr(frozen_external, "__name__", "")
    in {"_frozen_importlib_external", "importlib._bootstrap_external"},
)
print("source_loader_subclass", issubclass(abc.SourceLoader, abc.ExecutionLoader))
