"""MIME-type utilities for Molt — CPython 3.12 parity (no external DB).

Provides ``guess_type()``, ``guess_extension()``, ``guess_all_extensions()``,
``add_type()``, ``init()``, and the ``MimeTypes`` class.  The built-in type
map covers the most common web and data-science MIME types; no OS-specific
database is consulted (molt restriction: no subprocess, no filesystem probing).
"""

from __future__ import annotations

import posixpath
import urllib.parse

__all__ = [
    "MimeTypes",
    "add_type",
    "guess_all_extensions",
    "guess_extension",
    "guess_file",
    "guess_type",
    "init",
    "inited",
    "knownfiles",
    "read_mime_types",
    "suffix_map",
    "encodings_map",
    "types_map",
    "common_types",
]

# ---------------------------------------------------------------------------
# Decompression aliases: these extensions imply a content-encoding wrapper
# ---------------------------------------------------------------------------
suffix_map: dict[str, str] = {
    ".svgz": ".svg.gz",
    ".tgz": ".tar.gz",
    ".taz": ".tar.gz",
    ".tz": ".tar.gz",
    ".tbz2": ".tar.bz2",
    ".txz": ".tar.xz",
}

encodings_map: dict[str, str] = {
    ".gz": "gzip",
    ".Z": "compress",
    ".br": "br",
    ".bz2": "bzip2",
    ".xz": "xz",
    ".zst": "zstd",
}

# (strict, non-strict) types — same split as CPython.  Non-strict entries are
# in *common_types*; strict entries are in *types_map[True]*.  We build a
# flat dict for each and combine at lookup time.

# strict=True: registered IANA types only
_types_strict: dict[str, str] = {
    # Text
    ".css": "text/css",
    ".csv": "text/csv",
    ".htm": "text/html",
    ".html": "text/html",
    ".ics": "text/calendar",
    ".js": "text/javascript",
    ".mjs": "text/javascript",
    ".txt": "text/plain",
    ".tsv": "text/tab-separated-values",
    ".vcard": "text/vcard",
    ".vcf": "text/vcard",
    ".xml": "text/xml",
    # Application
    ".bin": "application/octet-stream",
    ".bz2": "application/x-bzip2",
    ".doc": "application/msword",
    ".docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ".epub": "application/epub+zip",
    ".gz": "application/gzip",
    ".jar": "application/java-archive",
    ".json": "application/json",
    ".jsonld": "application/ld+json",
    ".mp4": "video/mp4",
    ".mpkg": "application/vnd.apple.installer+xml",
    ".odp": "application/vnd.oasis.opendocument.presentation",
    ".ods": "application/vnd.oasis.opendocument.spreadsheet",
    ".odt": "application/vnd.oasis.opendocument.text",
    ".ogx": "application/ogg",
    ".pdf": "application/pdf",
    ".php": "application/x-httpd-php",
    ".ppt": "application/vnd.ms-powerpoint",
    ".pptx": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ".rar": "application/vnd.rar",
    ".rtf": "application/rtf",
    ".sh": "application/x-sh",
    ".tar": "application/x-tar",
    ".toml": "application/toml",
    ".wasm": "application/wasm",
    ".xhtml": "application/xhtml+xml",
    ".xls": "application/vnd.ms-excel",
    ".xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ".xul": "application/vnd.mozilla.xul+xml",
    ".zip": "application/zip",
    ".7z": "application/x-7z-compressed",
    # Audio
    ".aac": "audio/aac",
    ".flac": "audio/flac",
    ".mid": "audio/midi",
    ".midi": "audio/midi",
    ".mp3": "audio/mpeg",
    ".oga": "audio/ogg",
    ".opus": "audio/opus",
    ".wav": "audio/wav",
    ".weba": "audio/webm",
    # Image
    ".apng": "image/apng",
    ".avif": "image/avif",
    ".bmp": "image/bmp",
    ".gif": "image/gif",
    ".ico": "image/x-icon",
    ".jpeg": "image/jpeg",
    ".jpg": "image/jpeg",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".tif": "image/tiff",
    ".tiff": "image/tiff",
    ".webp": "image/webp",
    # Video
    ".avi": "video/x-msvideo",
    ".mov": "video/quicktime",
    ".mpeg": "video/mpeg",
    ".mpg": "video/mpeg",
    ".ogv": "video/ogg",
    ".ts": "video/mp2t",
    ".webm": "video/webm",
    # Font
    ".otf": "font/otf",
    ".ttf": "font/ttf",
    ".woff": "font/woff",
    ".woff2": "font/woff2",
    # Multipart
    ".eot": "application/vnd.ms-fontobject",
}

# strict=False: common/non-registered types
_types_non_strict: dict[str, str] = {
    ".py": "text/x-python",
    ".pyc": "application/x-python-code",
    ".pyo": "application/x-python-code",
    ".rb": "application/x-ruby",
    ".md": "text/markdown",
    ".markdown": "text/markdown",
    ".rst": "text/x-rst",
    ".yaml": "application/x-yaml",
    ".yml": "application/x-yaml",
    ".ini": "text/plain",
    ".cfg": "text/plain",
    ".conf": "text/plain",
    ".log": "text/plain",
    ".bat": "text/plain",
    ".c": "text/x-csrc",
    ".cc": "text/x-c++src",
    ".cpp": "text/x-c++src",
    ".cxx": "text/x-c++src",
    ".h": "text/x-chdr",
    ".hpp": "text/x-c++hdr",
    ".java": "text/x-java",
    ".swift": "text/x-swift",
    ".go": "text/x-go",
    ".rs": "text/x-rustsrc",
    ".sql": "application/x-sql",
    ".db": "application/x-sqlite3",
    ".sqlite": "application/x-sqlite3",
    ".sqlite3": "application/x-sqlite3",
    ".npy": "application/x-npy",
    ".npz": "application/x-npz",
    ".pkl": "application/x-pickle",
    ".pickle": "application/x-pickle",
    ".parquet": "application/x-parquet",
    ".arrow": "application/x-apache-arrow",
    ".feather": "application/x-feather",
    ".h5": "application/x-hdf5",
    ".hdf5": "application/x-hdf5",
    ".nc": "application/x-netcdf",
    ".geojson": "application/geo+json",
    ".mp4a": "audio/mp4",
    ".m4a": "audio/mp4",
    ".m4v": "video/mp4",
    ".3gp": "video/3gpp",
    ".3g2": "video/3gpp2",
    ".ps": "application/postscript",
    ".ai": "application/postscript",
    ".eps": "application/postscript",
    ".fig": "application/x-xfig",
    ".psd": "image/vnd.adobe.photoshop",
    ".xcf": "image/x-xcf",
    ".kml": "application/vnd.google-earth.kml+xml",
    ".kmz": "application/vnd.google-earth.kmz",
    ".swf": "application/x-shockwave-flash",
    ".torrent": "application/x-bittorrent",
    ".deb": "application/x-deb",
    ".rpm": "application/x-rpm",
    ".dmg": "application/x-apple-diskimage",
    ".iso": "application/x-iso9660-image",
    ".img": "application/x-raw-disk-image",
    ".msi": "application/x-msi",
    ".exe": "application/x-msdownload",
    ".dll": "application/x-msdownload",
    ".com": "application/x-msdownload",
    ".scr": "application/x-msdownload",
    ".apk": "application/vnd.android.package-archive",
    ".ipa": "application/octet-stream",
    ".dex": "application/vnd.android.dex",
    ".whl": "application/zip",
    ".egg": "application/zip",
    ".pem": "application/x-pem-file",
    ".crt": "application/x-x509-ca-cert",
    ".cer": "application/x-x509-ca-cert",
    ".p12": "application/x-pkcs12",
    ".pfx": "application/x-pkcs12",
    ".csr": "application/pkcs10",
}

# types_map mirrors CPython: types_map[True] = strict, types_map[False] = non-strict
types_map: dict[bool, dict[str, str]] = {
    True: _types_strict,
    False: _types_non_strict,
}

# common_types = non-strict map (kept for API compat)
common_types: dict[str, str] = _types_non_strict

# Lazily-built reverse maps: type -> list[ext]
_types_map_inv: dict[bool, dict[str, list[str]]] = {True: {}, False: {}}

inited: bool = False
knownfiles: list[str] = []  # no filesystem DB in molt


def _invert(mapping: dict[str, str]) -> dict[str, list[str]]:
    inv: dict[str, list[str]] = {}
    for ext, mime in mapping.items():
        inv.setdefault(mime, []).append(ext)
    # Sort for determinism; prefer shorter/simpler extensions
    for exts in inv.values():
        exts.sort(key=lambda e: (len(e), e))
    return inv


def init(files=None) -> None:
    """(Re)initialise the module-level maps.

    In molt, files are ignored — there is no OS MIME database.  The
    built-in maps are always the authoritative source.
    """
    global inited, _types_map_inv
    _types_map_inv = {
        True: _invert(_types_strict),
        False: _invert(_types_non_strict),
    }
    inited = True


def read_mime_types(filename: str):
    """Read a ``mime.types`` file and return a dict.

    In molt, filesystem access is not available at import time, so this
    always returns ``None``.
    """
    return None


def add_type(type: str, ext: str, strict: bool = True) -> None:
    """Add a mapping between *type* and *ext* in the module-level tables."""
    if not ext.startswith("."):
        ext = "." + ext
    types_map[strict][ext] = type
    if not inited:
        init()
    _types_map_inv[strict].setdefault(type, [])
    if ext not in _types_map_inv[strict][type]:
        _types_map_inv[strict][type].append(ext)


def guess_type(url: str, strict: bool = True) -> tuple[str | None, str | None]:
    """Guess the type of a file based on its URL.

    Returns a ``(type, encoding)`` tuple where *type* is a MIME type string
    such as ``'text/plain'`` and *encoding* is the encoding for the file
    (``'gzip'``, ``'br'``, …) or ``None`` if the file is not encoded.

    If the type cannot be guessed ``(None, None)`` is returned.
    """
    if not inited:
        init()
    scheme, url = urllib.parse.splittype(url)
    if scheme == "data":
        # data:[<mediatype>][;base64],<data>
        comma = url.find(",")
        if comma < 0:
            return (None, None)
        semi = url.find(";", 0, comma)
        if semi < 0:
            mime = url[:comma]
        else:
            mime = url[:semi]
        mime = mime.strip()
        if not mime:
            mime = "text/plain"
        return (mime, None)

    base, _, _ = url.partition("?")
    base, _, _ = base.partition("#")
    # Strip query/fragment the standard way
    _scheme, netloc, path, _query, _frag = urllib.parse.urlsplit(url)
    base = posixpath.basename(path)

    # Walk through the suffix_map to unfold compound extensions
    while True:
        root, ext = posixpath.splitext(base)
        if ext in suffix_map:
            base = root + suffix_map[ext]
        else:
            break

    root, ext = posixpath.splitext(base)
    ext = ext.lower()

    # Check encoding map first
    encoding: str | None = None
    if ext in encodings_map:
        encoding = encodings_map[ext]
        root2, ext2 = posixpath.splitext(root)
        ext2 = ext2.lower()
        if ext2:
            ext = ext2

    # Look up MIME type
    mime: str | None = None
    if strict:
        mime = _types_strict.get(ext) or _types_non_strict.get(ext)
    else:
        mime = _types_non_strict.get(ext) or _types_strict.get(ext)

    return (mime, encoding)


def guess_all_extensions(type: str, strict: bool = True) -> list[str]:
    """Guess the extensions for a file based on its MIME type.

    Return value is a list of strings giving all possible filename
    extensions, including the leading dot.  The extension is not
    guaranteed to have been associated with any particular data stream.
    """
    if not inited:
        init()
    type = type.lower()
    if strict:
        exts = list(_types_map_inv[True].get(type, []))
        for ext in _types_map_inv[False].get(type, []):
            if ext not in exts:
                exts.append(ext)
    else:
        exts = list(_types_map_inv[False].get(type, []))
        for ext in _types_map_inv[True].get(type, []):
            if ext not in exts:
                exts.append(ext)
    return exts


def guess_extension(type: str, strict: bool = True) -> str | None:
    """Guess the extension for a file based on its MIME type.

    There may be more than one extension for a given MIME type, and the
    first one found in the tables is returned.  If no extension can be
    determined, ``None`` is returned.
    """
    exts = guess_all_extensions(type, strict)
    if not exts:
        return None
    return exts[0]


def guess_file(filename: str, strict: bool = True) -> tuple[str | None, str | None]:
    """Guess the type of *filename*.

    Like ``guess_type()`` but takes a plain filename (not a URL).
    """
    return guess_type("file://" + filename, strict=strict)


class MimeTypes:
    """MIME-types datastore.

    This class offers an interface compatible with CPython's MimeTypes.
    It keeps instance-level overrides layered on top of the module-level maps.
    """

    def __init__(self, filenames: tuple[str, ...] = (), strict: bool = True) -> None:
        if not inited:
            init()
        # Instance-level maps: types_map[strict][ext] -> mime
        self.types_map: tuple[dict[str, str], dict[str, str]] = (
            dict(_types_non_strict),
            dict(_types_strict),
        )
        self.types_map_inv: tuple[dict[str, list[str]], dict[str, list[str]]] = (
            _invert(self.types_map[0]),
            _invert(self.types_map[1]),
        )
        self.suffix_map: dict[str, str] = dict(suffix_map)
        self.encodings_map: dict[str, str] = dict(encodings_map)
        # filenames are silently ignored (no filesystem in molt)

    def add_type(self, type: str, ext: str, strict: bool = True) -> None:
        """Add a mapping from *ext* to *type*."""
        if not ext.startswith("."):
            ext = "." + ext
        idx = 1 if strict else 0
        self.types_map[idx][ext] = type
        self.types_map_inv[idx].setdefault(type, [])
        if ext not in self.types_map_inv[idx][type]:
            self.types_map_inv[idx][type].append(ext)

    def guess_type(
        self, url: str, strict: bool = True
    ) -> tuple[str | None, str | None]:
        """Guess the type of a file at *url*."""
        scheme, url2 = urllib.parse.splittype(url)
        if scheme == "data":
            return guess_type(url, strict=strict)

        _scheme, netloc, path, _query, _frag = urllib.parse.urlsplit(url)
        base = posixpath.basename(path)

        while True:
            root, ext = posixpath.splitext(base)
            if ext in self.suffix_map:
                base = root + self.suffix_map[ext]
            else:
                break

        root, ext = posixpath.splitext(base)
        ext = ext.lower()

        encoding: str | None = None
        if ext in self.encodings_map:
            encoding = self.encodings_map[ext]
            _root2, ext2 = posixpath.splitext(root)
            if ext2:
                ext = ext2.lower()

        idx = 1 if strict else 0
        alt_idx = 0 if strict else 1
        mime = self.types_map[idx].get(ext) or self.types_map[alt_idx].get(ext)
        return (mime, encoding)

    def guess_all_extensions(self, type: str, strict: bool = True) -> list[str]:
        """Return a list of extensions for *type*."""
        type = type.lower()
        idx = 1 if strict else 0
        alt_idx = 0 if strict else 1
        exts: list[str] = list(self.types_map_inv[idx].get(type, []))
        for ext in self.types_map_inv[alt_idx].get(type, []):
            if ext not in exts:
                exts.append(ext)
        return exts

    def guess_extension(self, type: str, strict: bool = True) -> str | None:
        """Return a single extension for *type* or None."""
        exts = self.guess_all_extensions(type, strict)
        if not exts:
            return None
        return exts[0]

    def read(self, filename: str, strict: bool = True) -> None:
        """Read a mime.types file.  Silently ignored in molt."""

    def readfp(self, fp, strict: bool = True) -> None:
        """Read MIME info from a file-like object.

        Parses lines of the form: ``type/subtype  ext1 ext2 ...``
        """
        for line in fp:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split()
            if len(parts) < 2:
                continue
            mime = parts[0]
            for ext in parts[1:]:
                if not ext.startswith("."):
                    ext = "." + ext
                self.add_type(mime, ext, strict=strict)

    def read_windows_registry(self, strict: bool = True) -> None:
        """Read MIME types from Windows registry.  No-op in molt."""


# Initialise on import (populates reverse maps)
init()
