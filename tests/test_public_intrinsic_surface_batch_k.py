from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import types as _types
import sys
import types
from tests.stdlib_intrinsic_registry import install_registry


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


BACKSLASH = chr(92)
WINDOWS_ARCHIVE = 'C:' + BACKSLASH + 'cache.zip' + BACKSLASH + 'app.zip'
archive_calls = []
read_calls = []


def _zip_archive_path(path):
    archive_calls.append(path)
    return {{
        'pkg.zip': ('pkg.zip', ''),
        WINDOWS_ARCHIVE + BACKSLASH + 'nested': (WINDOWS_ARCHIVE, 'nested'),
    }}.get(path)


def _zip_read_entry(archive, inner):
    read_calls.append((archive, inner))
    return b'payload'


install_registry({{
    "molt_token_payload": lambda: {{
        "ENDMARKER": 0, "NAME": 1, "NUMBER": 2, "NEWLINE": 4,
        "PLUS": 14, "EQUAL": 22, "OP": 55, "COMMENT": 64, "NL": 65,
        "ENCODING": 67, "N_TOKENS": 68, "NT_OFFSET": 256,
        "tok_name": {{0: "ENDMARKER", 1: "NAME", 2: "NUMBER", 4: "NEWLINE",
                     14: "PLUS", 22: "EQUAL", 55: "OP", 64: "COMMENT", 65: "NL",
                     67: "ENCODING", 68: "N_TOKENS", 256: "NT_OFFSET"}},
        "EXACT_TOKEN_TYPES": {{"+": 14, "=": 22}},
        "__all__": ["tok_name", "ISTERMINAL", "ISNONTERMINAL", "ISEOF",
                    "EXACT_TOKEN_TYPES", "ENDMARKER", "NAME", "NUMBER", "NEWLINE",
                    "PLUS", "EQUAL", "OP", "COMMENT", "NL", "ENCODING",
                    "N_TOKENS", "NT_OFFSET"],
    }},
    "molt_this_payload": lambda: ("enc", {{"a": "b"}}, "Zen line", 1, 2),
    "molt_importlib_find_in_path_package_context": lambda fullname, roots: {{
        "loader_kind": "zip_source",
        "zip_archive": "pkg.zip",
        "zip_inner_path": "pkg/mod.py",
        "is_package": False,
        "origin": "pkg.zip/pkg/mod.py",
    }},
    "molt_importlib_zip_source_exec_payload": lambda fullname, archive, inner, is_package: {{
        "source": "VALUE = 7\\n"
    }},
    "molt_importlib_zip_read_entry": _zip_read_entry,
    "molt_importlib_zip_archive_path": _zip_archive_path,
}})

machinery_mod = _types.ModuleType("importlib.machinery")


class ModuleSpec:
    def __init__(self, name, loader=None, origin=None, is_package=False):
        self.name = name
        self.loader = loader
        self.origin = origin
        self.submodule_search_locations = [] if is_package else None


class _ZipSourceLoader:
    def __init__(self, fullname, archive, inner):
        self.fullname = fullname
        self.archive = archive
        self.inner = inner

    def exec_module(self, module):
        module.VALUE = 7


machinery_mod.ModuleSpec = ModuleSpec
machinery_mod._ZipSourceLoader = _ZipSourceLoader
sys.modules["importlib.machinery"] = machinery_mod


token_mod = _load_module("molt_test_token", {str(STDLIB_ROOT / "token.py")!r})
this_mod = _load_module("molt_test_this", {str(STDLIB_ROOT / "this.py")!r})
zipimport_mod = _load_module("molt_test_zipimport", {str(STDLIB_ROOT / "zipimport.py")!r})

imported = zipimport_mod.zipimporter("pkg.zip")
spec = imported.find_spec("pkg.mod")
data = imported.get_data("pkg.zip/pkg/mod.py")

try:
    zipimport_mod.zipimporter("missing.zip")
except zipimport_mod.ZipImportError:
    rejected_archive = True
else:
    rejected_archive = False

# Project the Windows facade constants without changing the host's os module.
zipimport_mod.path_sep = BACKSLASH
zipimport_mod.alt_path_sep = '/'
windows_importer = zipimport_mod.zipimporter('C:/cache.zip/app.zip/nested')
windows_mixed_data = windows_importer.get_data(WINDOWS_ARCHIVE + '/nested/pkg/mod.py')
windows_native_data = windows_importer.get_data(
    WINDOWS_ARCHIVE + BACKSLASH + 'nested' + BACKSLASH + 'pkg' + BACKSLASH + 'mod.py'
)

checks = {{
    "token": (
        token_mod.ENDMARKER == 0
        and token_mod.EXACT_TOKEN_TYPES["+"] == token_mod.PLUS == 14
        and token_mod.tok_name[55] == "OP"
        and token_mod.OP == 55
        and token_mod.ISTERMINAL(255) and not token_mod.ISTERMINAL(256)
        and token_mod.ISNONTERMINAL(256) and not token_mod.ISNONTERMINAL(0)
        and token_mod.ISEOF(0) and not token_mod.ISEOF(1)
        and token_mod.__all__[:5] == ["tok_name", "ISTERMINAL", "ISNONTERMINAL",
                                     "ISEOF", "EXACT_TOKEN_TYPES"]
        and "PLUS" in token_mod.__all__
        and "molt_token_payload" not in token_mod.__dict__
        and "_require_intrinsic" not in token_mod.__dict__
    ),
    "this": (
        getattr(this_mod, "s") == "enc"
        and "molt_this_payload" not in this_mod.__dict__
    ),
    "zipimport": (
        spec is not None
        and imported.get_source("pkg.mod") == "VALUE = 7\\n"
        and data == b"payload"
        and "molt_importlib_zip_read_entry" not in zipimport_mod.__dict__
        and imported.archive == 'pkg.zip'
        and imported.prefix == ''
        and rejected_archive
        and windows_importer.archive == WINDOWS_ARCHIVE
        and windows_importer.prefix == 'nested' + BACKSLASH
        and windows_mixed_data == b'payload'
        and windows_native_data == b'payload'
        and archive_calls == [
            'pkg.zip', 'missing.zip', WINDOWS_ARCHIVE + BACKSLASH + 'nested'
        ]
        and read_calls == [
            ('pkg.zip', 'pkg/mod.py'),
            (WINDOWS_ARCHIVE, 'nested/pkg/mod.py'),
            (WINDOWS_ARCHIVE, 'nested/pkg/mod.py'),
        ]
        and "molt_importlib_zip_archive_path" not in zipimport_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_k() -> None:
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if line.startswith("CHECK|"):
            _, key, value = line.split("|", 2)
            checks[key] = value
    assert checks == {
        "this": "True",
        "token": "True",
        "zipimport": "True",
    }
