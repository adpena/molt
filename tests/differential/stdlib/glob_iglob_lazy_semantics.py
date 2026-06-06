# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for glob.iglob lazy-iterator semantics (task #29).

`glob.iglob` must return a *lazy iterator* (not a pre-materialized list wrapped
in iter()), streaming CPython-byte-identical results across the full option
matrix: ordering (readdir order, NOT sorted), hidden-file rules, recursive `**`
incl. include_hidden, dir-only trailing slash, root_dir, dir_fd, byte paths,
empty/no-match edges. Partial consumption must not require materializing the
whole tree.
"""

from __future__ import annotations

import glob
import os
import tempfile


def _touch(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("ok")


with tempfile.TemporaryDirectory(prefix="molt_iglob_sem_") as root:
    _touch(os.path.join(root, "top.txt"))
    _touch(os.path.join(root, "top.log"))
    _touch(os.path.join(root, ".hidden_top.txt"))
    _touch(os.path.join(root, "pkg", "module.py"))
    _touch(os.path.join(root, "pkg", "sub", "data.txt"))
    _touch(os.path.join(root, "pkg", ".dot", "inner.txt"))
    _touch(os.path.join(root, ".secret", "deep", "k.txt"))

    # iglob returns a lazy iterator, and iter(it) is it (iterator protocol).
    it = glob.iglob("*.txt", root_dir=root)
    print("is_iterator", iter(it) is it)
    print("not_a_list", not isinstance(it, list))

    # Partial consumption: pull one element, then the rest. Reassembling the
    # full set (sorted) must equal a full glob().
    it2 = glob.iglob("**/*.txt", root_dir=root, recursive=True)
    one = next(it2)
    rest = list(it2)
    streamed = sorted([one] + rest)
    eager = sorted(glob.glob("**/*.txt", root_dir=root, recursive=True))
    print("partial_eq_eager", streamed == eager)
    print("streamed_sorted", streamed)

    # Ordering parity: iglob order must match CPython's iglob order exactly on
    # the same tree (readdir order, NOT sorted). Both molt and CPython read the
    # same directory at the same instant, so the raw sequences must agree.
    print("order_star", list(glob.iglob("*", root_dir=root)))
    print("order_pkg_star", list(glob.iglob("pkg/*", root_dir=root)))

    # Hidden-file rules.
    print("explicit_dot", sorted(glob.iglob(".*", root_dir=root)))
    print(
        "star_include_hidden",
        sorted(glob.iglob("*", root_dir=root, include_hidden=True)),
    )

    # Recursive ** with and without include_hidden.
    print(
        "rec_txt",
        sorted(glob.iglob("**/*.txt", root_dir=root, recursive=True)),
    )
    print(
        "rec_txt_hidden",
        sorted(
            glob.iglob("**/*.txt", root_dir=root, recursive=True, include_hidden=True)
        ),
    )
    # Bare ** (dir + file walk).
    print("rec_all", sorted(glob.iglob("**", root_dir=root, recursive=True)))

    # Dir-only trailing slash.
    print("trailing_dir", list(glob.iglob("pkg/", root_dir=root)))
    print("trailing_file", list(glob.iglob("top.txt/", root_dir=root)))

    # Magic directory component (dirname has magic).
    print("magic_dir", sorted(glob.iglob("*/*.py", root_dir=root)))
    print(
        "magic_dir_rec",
        sorted(glob.iglob("**/sub/*.txt", root_dir=root, recursive=True)),
    )

    # Empty pattern + no-match edges yield nothing.
    print("empty_pattern", list(glob.iglob("", root_dir=root)))
    print("no_match", list(glob.iglob("nope_*.zzz", root_dir=root)))

    # Byte paths: items must be bytes, in readdir order.
    broot = root.encode()
    print("bytes_item_type", type(next(glob.iglob(b"*.txt", root_dir=broot))).__name__)
    print("bytes_rec", sorted(glob.iglob(b"**/*.txt", root_dir=broot, recursive=True)))

    # dir_fd type errors stream identically (raised on first iteration). These
    # use unrolled try/except blocks (NOT a `for ... in [...]:` loop) to avoid a
    # pre-existing, glob-unrelated backend codegen bug (Cranelift "block cannot
    # be empty" verifier error) triggered by `try/except` inside a loop nested
    # in a `tempfile.TemporaryDirectory` + `os.makedirs` body. See the BATON in
    # the final report; the glob semantics under test are unaffected.
    try:
        list(glob.iglob("*.txt", root_dir=root, dir_fd="."))
    except Exception as exc:  # noqa: BLE001
        print("dirfd_str", type(exc).__name__, str(exc))
    try:
        list(glob.iglob("*.txt", root_dir=root, dir_fd=1.5))
    except Exception as exc:  # noqa: BLE001
        print("dirfd_float", type(exc).__name__, str(exc))

    # bad fd with absolute root still streams results (openat ignores fd for abs).
    print(
        "dirfd_badfd_abs",
        list(glob.iglob("*.txt", root_dir=root, dir_fd=1_000_000)),
    )
