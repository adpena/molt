"""Purpose: csv and logging must link into one full-stdlib binary together.

Regression for a duplicate ``#[no_mangle]`` C symbol: ``molt_csv_runtime_ready``
was defined both in the serial/csv runtime leaf (its canonical owner, referenced
by the csv intrinsic resolver) and, by a stray copy-paste, in the http/logging
runtime leaf (``molt-runtime-http/src/functions_logging.rs``). Both leaves link
into the full-stdlib runtime, so the duplicate definition raised a hard linker
error (MSVC LNK2005 / "multiply defined symbols") and the native full-stdlib
link failed for *every* program that pulled both leaves in.

Importing ``csv`` and ``logging`` together forces both the serial/csv leaf and
the http/logging leaf to be linked into one binary, which is exactly the
configuration that collided. Exercising both modules then proves their runtime
behavior is byte-identical to CPython once the binary links.
"""

import csv
import io
import logging

# csv leaf: canonical owner of molt_csv_runtime_ready. Exercise the writer
# (which round-trips quoting through the same leaf) and read the rendered text
# back as a plain string; this keeps the regression focused on link coexistence
# rather than depending on unrelated csv-reader iteration behavior.
buf = io.StringIO()
writer = csv.writer(buf)
writer.writerow(["name", "score"])
writer.writerow(["a,b", 7])
print(repr(buf.getvalue()))

# logging leaf: the surface whose functions_logging.rs held the stray duplicate.
logger = logging.getLogger("demo")
logger.setLevel(logging.INFO)
print(logging.getLevelName(logging.WARNING))
print(logging.getLevelName(logging.INFO))
print(logger.isEnabledFor(logging.INFO))
print(logger.isEnabledFor(logging.DEBUG))
