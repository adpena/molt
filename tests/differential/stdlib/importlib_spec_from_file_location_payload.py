"""Purpose: validate intrinsic-backed spec_from_file_location package/path shaping."""

import importlib.util
import pathlib
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    pkg_dir = root / "pkgdemo"
    pkg_dir.mkdir()
    pkg_init = pkg_dir / "__init__.py"
    pkg_init.write_text("value = 7\n", encoding="utf-8")

    mod_path = root / "moddemo.py"
    mod_path.write_text("value = 9\n", encoding="utf-8")

    pkg_spec = importlib.util.spec_from_file_location("pkgdemo", pkg_init)
    print(pkg_spec is not None)
    print(bool(pkg_spec and pkg_spec.submodule_search_locations))
    print(
        tuple(
            pathlib.Path(entry).name
            for entry in (pkg_spec.submodule_search_locations or ())
        )
        if pkg_spec
        else ()
    )

    explicit_spec = importlib.util.spec_from_file_location(
        "pkgdemo_explicit",
        pkg_init,
        submodule_search_locations=("x", "y"),
    )
    print(
        tuple(explicit_spec.submodule_search_locations or ()) if explicit_spec else ()
    )

    mod_spec = importlib.util.spec_from_file_location("moddemo", mod_path)
    print(mod_spec is not None)
    print(mod_spec.submodule_search_locations if mod_spec else None)
