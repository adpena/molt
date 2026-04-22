"""Intrinsic-backed adapters for ``importlib.metadata`` message handling."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORTLIB_IMPORT_REQUIRED = _require_intrinsic("molt_importlib_import_required")

email = _MOLT_IMPORTLIB_IMPORT_REQUIRED("email")
functools = _MOLT_IMPORTLIB_IMPORT_REQUIRED("functools")
re = _MOLT_IMPORTLIB_IMPORT_REQUIRED("re")
textwrap = _MOLT_IMPORTLIB_IMPORT_REQUIRED("textwrap")
warnings = _MOLT_IMPORTLIB_IMPORT_REQUIRED("warnings")

_email_message = _MOLT_IMPORTLIB_IMPORT_REQUIRED("email.message")


def _method_cache(method):
    """Simple per-instance method cache."""
    cache_attr = "_cache_" + method.__name__

    @functools.wraps(method)
    def wrapper(self):
        try:
            return getattr(self, cache_attr)
        except AttributeError:
            result = method(self)
            try:
                object.__setattr__(self, cache_attr, result)
            except (AttributeError, TypeError):
                pass
            return result

    return wrapper


class FoldedCase(str):
    """A case insensitive string class; behaves just like str
    except compares equal when the only variation is case.
    """

    def __lt__(self, other: object) -> bool:
        return self.lower() < other.lower()  # type: ignore[union-attr]

    def __gt__(self, other: object) -> bool:
        return self.lower() > other.lower()  # type: ignore[union-attr]

    def __eq__(self, other: object) -> bool:
        return self.lower() == other.lower()  # type: ignore[union-attr]

    def __ne__(self, other: object) -> bool:
        return self.lower() != other.lower()  # type: ignore[union-attr]

    def __hash__(self) -> int:
        return hash(self.lower())

    def __contains__(self, other: object) -> bool:
        return super().lower().__contains__(other.lower())  # type: ignore[union-attr]

    def in_(self, other: str) -> bool:
        """Does self appear in other?"""
        return self in FoldedCase(other)

    @_method_cache
    def lower(self) -> str:
        return super().lower()

    def index(self, sub: str, *args: object) -> int:  # type: ignore[override]
        return self.lower().index(sub.lower())

    def split(self, splitter: str = " ", maxsplit: int = 0) -> list[str]:  # type: ignore[override]
        pattern = re.compile(re.escape(splitter), re.IGNORECASE)
        return pattern.split(self, maxsplit)


def _warn() -> None:
    msg = (
        "Accessing unset metadata may raise KeyError in a future version. "
        "Use .get() or check 'in' to avoid."
    )
    warnings.warn(msg, DeprecationWarning, stacklevel=3)


class Message(_email_message.Message):
    multiple_use_keys = set(
        map(
            FoldedCase,
            [
                "Classifier",
                "Obsoletes-Dist",
                "Platform",
                "Project-URL",
                "Provides-Dist",
                "Provides-Extra",
                "Requires-Dist",
                "Requires-External",
                "Supported-Platform",
                "Dynamic",
            ],
        )
    )

    def __new__(cls, orig: object) -> "Message":
        res = super().__new__(cls)
        vars(res).update(vars(orig))
        return res

    def __init__(self, *args: object, **kwargs: object) -> None:
        self._headers = self._repair_headers()

    def __iter__(self):  # type: ignore[override]
        return super().__iter__()

    def __getitem__(self, item: str) -> str | None:
        res = super().__getitem__(item)
        if res is None:
            _warn()
        return res

    def _repair_headers(self) -> list[tuple[str, str]]:
        def redent(value: str) -> str:
            if not value or "\n" not in value:
                return value
            return textwrap.dedent(" " * 8 + value)

        headers = [(key, redent(value)) for key, value in vars(self)["_headers"]]
        if self._payload:
            headers.append(("Description", self.get_payload()))
        return headers

    @property
    def json(self) -> dict[str, object]:
        """Convert PackageMetadata to a JSON-compatible format per PEP 566."""

        def transform(key: FoldedCase) -> tuple[str, object]:
            value: object
            if key in self.multiple_use_keys:
                value = self.get_all(key)
            else:
                value = self[key]
            if key == "Keywords":
                value = re.split(r"\s+", value)  # type: ignore[arg-type]
            tk = key.lower().replace("-", "_")
            return tk, value

        return dict(map(transform, map(FoldedCase, self)))

    def raw_items(self):  # type: ignore[override]
        return vars(self)["_headers"]

    def set_raw(self, name: str, value: str) -> None:
        super().__setitem__(name, value)


globals().pop("_require_intrinsic", None)
