"""Purpose: validate intrinsic optional/fallback import behavior."""

from _intrinsics import require_intrinsic

_import_optional = require_intrinsic("molt_importlib_import_optional", globals())
_import_or_fallback = require_intrinsic("molt_importlib_import_or_fallback", globals())

frozen = _import_optional("_frozen_importlib")
print(
    "optional_existing",
    frozen is None or getattr(frozen, "__name__", "") == "_frozen_importlib",
)

missing = _import_optional("molt_missing_module_for_optional_import_test")
print("optional_missing", missing is None)

fallback = object()
out = _import_or_fallback("molt_missing_module_for_optional_import_test", fallback)
print("fallback_missing", out is fallback)

sys_mod = _import_or_fallback("sys", fallback)
print("fallback_existing", getattr(sys_mod, "__name__", "") == "sys")
