# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: stdlib import smoke for fs-capability modules (B)."""

import plistlib
import py_compile
import pyclbr
import pydoc
import pydoc_data
import runpy
import shelve
import tabnanny
import trace
import zipapp
import zipimport
import ensurepip
import venv
import wave
import mmap

modules = [
    plistlib,
    py_compile,
    pyclbr,
    pydoc,
    pydoc_data,
    runpy,
    shelve,
    tabnanny,
    trace,
    zipapp,
    zipimport,
    ensurepip,
    venv,
    wave,
    mmap,
]
print([mod.__name__ for mod in modules])
