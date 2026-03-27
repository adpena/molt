"""Purpose: keep missing-module failure shape aligned across import paths."""

import importlib


def _capture(call):
    try:
        call()
    except BaseException as exc:
        return exc.__class__.__name__, str(exc)
    return "no-error", ""


missing_dunder = _capture(
    lambda: importlib.import_module("molt_missing_module_for_import_regression")
)
missing_importlib = _capture(
    lambda: importlib.import_module(
        "molt_missing_package_for_import_regression.submodule"
    )
)

print(missing_dunder[0])
print(missing_dunder[1].startswith("No module named "))
print("molt_missing_module_for_import_regression" in missing_dunder[1])
print(missing_importlib[0])
print(missing_importlib[1].startswith("No module named "))
print("molt_missing_package_for_import_regression" in missing_importlib[1])
