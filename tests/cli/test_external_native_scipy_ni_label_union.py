"""Resolver-union teeth for the pact-witness scipy.ndimage native closure.

The pact Kernel A witness links source-recompiled SciPy ``.molt.wasm`` static
artifacts through several sealed extension roots that are collected together into
``MOLT_MODULE_ROOTS`` (see ``tools/proof_queue.py::_pact_witness_native_roots``).
``scipy.ndimage.label`` (used by ``field_solve.py``) triggers a module-level
``from . import _ni_label`` in ``scipy/ndimage/_measurements.py`` and then calls
``_ni_label._label(...)`` / catches ``_ni_label.NeedMoreBits`` (a C-defined
exception type). So the ``_ni_label`` C-extension must be resolvable as the
native module ``scipy.ndimage._ni_label`` or scipy.ndimage import fails closed.

``_ni_label`` is staged as its own witness root
(``tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi``) rather than physically
inside the primary ndimage seal
(``tmp/pact_scipy_ndimage_sealed_for_witness_next``, which carries
``_nd_image``). The external-native resolver is expected to *union* native
artifacts across every root under one package, so both ``_nd_image`` and
``_ni_label`` present as ``scipy.ndimage.*`` submodules. Root registration is
already gated
(``tests/tools/test_proof_queue.py::
test_proof_queue_pact_witness_acceptance_admits_staged_native_roots``); this
module gives the *resolution* path teeth so a regression in the union walk (or a
silent drop of ``_ni_label`` from the witness scipy closure) fails loudly here
rather than only at a full witness build.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from molt.cli.extension_manifest import _default_molt_c_api_version
from molt.cli.external_native import (
    _resolve_external_package_native_artifact_plan,
)
from tests.cli.test_cli_extension_commands import _wasm_exporting_i64_unary_symbol

REPO_ROOT = Path(__file__).resolve().parents[2]


def _write_ndimage_native_ext(
    root: Path,
    *,
    module: str,
    init_symbol: str,
    wasm_name: str,
    abi_version: str,
) -> None:
    """Stage a minimal-but-valid scipy.ndimage native artifact under ``root``.

    The artifact is a real WASM object exporting ``init_symbol`` and a manifest
    that passes the external-native artifact validator; only the package/module
    topology and the init symbol matter for the resolver-union assertions.
    """

    package_dir = root / "scipy" / "ndimage"
    package_dir.mkdir(parents=True, exist_ok=True)
    (root / "scipy" / "__init__.py").write_text("", encoding="utf-8")
    (package_dir / "__init__.py").write_text("", encoding="utf-8")

    artifact_bytes = _wasm_exporting_i64_unary_symbol(init_symbol)
    (package_dir / wasm_name).write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()

    manifest = {
        "schema_version": 1,
        "name": "scipy",
        "version": "0.1.0",
        "module": module,
        "molt_c_api_version": abi_version,
        "abi_tag": f"molt_abi{abi_version}",
        "python_tag": "py3",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": init_symbol,
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["module.extension.exec"],
        "extension": wasm_name,
        "extension_sha256": extension_sha256,
        "python_exports": ["scipy"],
        # Sealed custody markers so re-derivation tolerates absent sources.
        "sealed_from_manifest_sha256": extension_sha256,
        "sealed_from_extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": 1,
            "root_symbol": init_symbol,
            "init_symbol_owner": "0.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0.o",
                    "source_sha256": extension_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": [init_symbol],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    (package_dir / f"{wasm_name}.extension_manifest.json").write_text(
        json.dumps(manifest), encoding="utf-8"
    )


def _resolved_ndimage_modules(
    roots: list[Path],
) -> dict[str, str]:
    plan, errors = _resolve_external_package_native_artifact_plan(
        external_module_roots=roots,
        admitted_packages={"scipy"},
        required_modules={
            "scipy.ndimage._nd_image",
            "scipy.ndimage._ni_label",
        },
    )
    assert errors == [], errors
    assert plan is not None
    return {artifact.module: artifact.init_symbol for artifact in plan.artifacts}


def test_resolver_unions_ni_label_with_nd_image_across_roots(
    tmp_path: Path,
) -> None:
    """``_ni_label`` in a sibling root unions with ``_nd_image`` as scipy.ndimage.*."""

    abi_version = _default_molt_c_api_version(REPO_ROOT)
    ndimage_seal = tmp_path / "pact_scipy_ndimage_sealed_for_witness_next"
    ni_label_root = tmp_path / "pact_scipy_ni_label_molt_ext_wasm_cpython_abi"
    _write_ndimage_native_ext(
        ndimage_seal,
        module="scipy.ndimage._nd_image",
        init_symbol="PyInit__nd_image",
        wasm_name="_nd_image.molt.wasm",
        abi_version=abi_version,
    )
    _write_ndimage_native_ext(
        ni_label_root,
        module="scipy.ndimage._ni_label",
        init_symbol="PyInit__ni_label",
        wasm_name="_ni_label.molt.wasm",
        abi_version=abi_version,
    )

    modules = _resolved_ndimage_modules([ndimage_seal, ni_label_root])

    assert modules.get("scipy.ndimage._nd_image") == "PyInit__nd_image"
    assert modules.get("scipy.ndimage._ni_label") == "PyInit__ni_label"


def test_ni_label_absent_root_drops_it_from_scipy_closure(
    tmp_path: Path,
) -> None:
    """Teeth: without the ``_ni_label`` root, it is absent from the closure.

    Proves the previous test asserts a real union (``_ni_label`` is provided by
    its own root, not incidentally by the ndimage seal). A regression that fails
    to union the sibling root would drop ``scipy.ndimage._ni_label`` and break
    ``scipy.ndimage.label``'s ``from . import _ni_label``.
    """

    abi_version = _default_molt_c_api_version(REPO_ROOT)
    ndimage_seal = tmp_path / "pact_scipy_ndimage_sealed_for_witness_next"
    _write_ndimage_native_ext(
        ndimage_seal,
        module="scipy.ndimage._nd_image",
        init_symbol="PyInit__nd_image",
        wasm_name="_nd_image.molt.wasm",
        abi_version=abi_version,
    )

    modules = _resolved_ndimage_modules([ndimage_seal])

    assert modules.get("scipy.ndimage._nd_image") == "PyInit__nd_image"
    assert "scipy.ndimage._ni_label" not in modules
