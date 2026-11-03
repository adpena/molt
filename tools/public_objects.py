"""Read the single declaration of public native Python object identities."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
import math
from pathlib import Path
import re
import tomllib
from typing import Literal, overload


class PublicObjectKind(Enum):
    FUNCTION = "function"
    BUILTIN_FUNCTION_OR_METHOD = "builtin_function_or_method"
    TYPE = "type"


class PublicObjectProducer(Enum):
    INTRINSIC = "intrinsic"
    COMPILED = "compiled"


@dataclass(frozen=True)
class PublicSignature:
    posonly: tuple[str, ...]
    positional: tuple[str, ...]
    defaults: tuple[str, ...]
    kwonly: tuple[str, ...]
    kwdefaults: tuple[str, ...]
    vararg: str | None
    varkw: str | None

    @property
    def arity(self) -> int:
        return (
            len(self.posonly)
            + len(self.positional)
            + len(self.kwonly)
            + (self.vararg is not None)
            + (self.varkw is not None)
        )


@dataclass(frozen=True)
class PublicConstructor(PublicSignature):
    callback: str


@overload
def _signature(row: dict, *, constructor: Literal[True]) -> PublicConstructor: ...


@overload
def _signature(
    row: dict, *, constructor: Literal[False] = False
) -> PublicSignature: ...


def _signature(row: dict, *, constructor: bool = False) -> PublicSignature:
    if not isinstance(row, dict):
        raise ValueError("native signature requires a table")
    fields = {
        "callback",
        "posonly",
        "positional",
        "defaults",
        "kwonly",
        "kwdefaults",
        "vararg",
        "varkw",
    }
    if (
        set(row) - fields
        or (not constructor and "callback" in row)
        or (
            constructor
            and (
                not isinstance(row.get("callback"), str)
                or not row["callback"].isascii()
                or not row["callback"].isidentifier()
            )
        )
    ):
        raise ValueError("invalid public constructor declaration")
    for field in ("posonly", "positional", "kwonly"):
        if not isinstance(row.get(field, []), list) or any(
            not isinstance(name, str) or not name.isascii() or not name.isidentifier()
            for name in row.get(field, [])
        ):
            raise ValueError(f"invalid constructor {field}")
    for field in ("vararg", "varkw"):
        if field in row and (
            not isinstance(row[field], str)
            or not row[field].isascii()
            or not row[field].isidentifier()
        ):
            raise ValueError(f"invalid constructor {field}")
    for field in ("defaults", "kwdefaults"):
        if not isinstance(row.get(field, []), list) or any(
            not isinstance(value, str)
            or (
                value not in ("None", "True", "False", "Missing")
                and re.fullmatch(r"-?[0-9]+", value) is None
                and re.fullmatch(r"-?(?:[0-9]+\.[0-9]+|[0-9]+[eE][+-]?[0-9]+)", value)
                is None
            )
            for value in row.get(field, [])
        ):
            raise ValueError(f"invalid constructor {field}")
        if any(
            value not in ("None", "True", "False", "Missing")
            and re.fullmatch(r"-?[0-9]+", value) is None
            and not math.isfinite(float(value))
            for value in row.get(field, [])
        ):
            raise ValueError(f"non-finite native signature {field}")
    values = (
        tuple(row.get("posonly", [])),
        tuple(row.get("positional", [])),
        tuple(row.get("defaults", [])),
        tuple(row.get("kwonly", [])),
        tuple(row.get("kwdefaults", [])),
        row.get("vararg"),
        row.get("varkw"),
    )
    result = (
        PublicConstructor(*values, row["callback"])
        if constructor
        else PublicSignature(*values)
    )
    names = [
        *result.posonly,
        *result.positional,
        *result.kwonly,
        *([result.vararg] if result.vararg else []),
        *([result.varkw] if result.varkw else []),
    ]
    if (
        len(names) != len(set(names))
        or len(result.defaults) > len(result.posonly) + len(result.positional)
        or len(result.kwonly) != len(result.kwdefaults)
        or (constructor and (not result.posonly or result.posonly[0] != "cls"))
    ):
        raise ValueError("invalid constructor binding shape")
    return result


@dataclass(frozen=True)
class PublicObjectIdentity:
    kind: PublicObjectKind
    name: str
    qualname: str
    module: str
    bindings: tuple[str, ...]
    signature: str | None = None
    arguments: str | None = None
    class_factory: str | None = None
    instance_shape: str | None = None
    constructor: PublicConstructor | None = None
    constructor_312: PublicConstructor | None = None
    call: PublicSignature | None = None
    call_before_314: PublicSignature | None = None
    publisher: str | None = None
    producer: PublicObjectProducer = PublicObjectProducer.INTRINSIC


DECLARATIONS = (
    Path(__file__).resolve().parents[1]
    / "runtime/molt-runtime/src/intrinsics/public_objects.toml"
)


def public_object_id(runtime: str) -> str:
    return "".join(word.title() for word in runtime.removeprefix("molt_").split("_"))


def load_public_objects(path: Path = DECLARATIONS) -> dict[str, PublicObjectIdentity]:
    with path.open("rb") as source:
        document = tomllib.load(source)
    if set(document) != {"object"}:
        raise ValueError("public objects declaration requires one object table")
    identities = {}
    bindings = set()
    for runtime, row in document["object"].items():
        required = {"kind", "name", "qualname", "module", "bindings"}
        if not required <= set(row) or set(row) - required - {
            "signature",
            "arguments",
            "class_factory",
            "instance_shape",
            "constructor",
            "constructor_312",
            "call",
            "call_before_314",
            "publisher",
            "producer",
        }:
            raise ValueError(f"{runtime}: incomplete or unknown public identity field")
        if not runtime.isidentifier() or not runtime.startswith("molt_"):
            raise ValueError(f"{runtime}: invalid runtime declaration")
        for field in ("name", "qualname", "module"):
            value = row[field]
            if (
                not isinstance(value, str)
                or not value.isascii()
                or not all(part.isidentifier() for part in value.split("."))
            ):
                raise ValueError(f"{runtime}: invalid {field}")
        if "." in row["name"]:
            raise ValueError(f"{runtime}: name must be unqualified")
        if not isinstance(row["bindings"], list) or not row["bindings"]:
            raise ValueError(f"{runtime}: public object needs a publication site")
        for binding in row["bindings"]:
            if not isinstance(binding, str) or not all(
                part.isidentifier() for part in binding.split(".")
            ):
                raise ValueError(f"{runtime}: invalid public binding")
            if binding in bindings:
                raise ValueError(f"duplicate public binding {binding}")
            bindings.add(binding)
        identities[runtime] = PublicObjectIdentity(
            PublicObjectKind(row["kind"]),
            row["name"],
            row["qualname"],
            row["module"],
            tuple(row["bindings"]),
            row.get("signature"),
            row.get("arguments"),
            row.get("class_factory"),
            row.get("instance_shape"),
            _signature(row["constructor"], constructor=True)
            if "constructor" in row
            else None,
            _signature(row["constructor_312"], constructor=True)
            if "constructor_312" in row
            else None,
            _signature(row["call"]) if "call" in row else None,
            _signature(row["call_before_314"]) if "call_before_314" in row else None,
            row.get("publisher"),
            PublicObjectProducer(row.get("producer", "intrinsic")),
        )
        if identities[runtime].producer is PublicObjectProducer.COMPILED and (
            identities[runtime].kind is not PublicObjectKind.BUILTIN_FUNCTION_OR_METHOD
            or any(
                field in row
                for field in ("call", "call_before_314", "constructor", "class_factory")
            )
        ):
            raise ValueError(
                f"{runtime}: compiled builtin owns its source argument binder"
            )
        for field in ("class_factory", "instance_shape"):
            if field in row and (
                not isinstance(row[field], str)
                or not row[field].isascii()
                or not row[field].isidentifier()
            ):
                raise ValueError(f"{runtime}: invalid {field}")
        if (
            row["kind"] == "type"
            and any(not binding.startswith("builtins.") for binding in row["bindings"])
            and not row.get("class_factory")
        ):
            raise ValueError(f"{runtime}: native type requires its class producer")
        if row.get("class_factory") and not row.get("constructor"):
            raise ValueError(f"{runtime}: native type requires its constructor")
        if "call" in row and row["kind"] != "builtin_function_or_method":
            raise ValueError(
                f"{runtime}: native call signature requires a builtin function"
            )
        if "call_before_314" in row and "call" not in row:
            raise ValueError(
                f"{runtime}: versioned signature requires its current signature"
            )
        if "publisher" in row and (
            row["publisher"] not in row["bindings"]
            or row["kind"] != "builtin_function_or_method"
        ):
            raise ValueError(
                f"{runtime}: publisher must be a declared native function binding"
            )
        if identities[runtime].kind == PublicObjectKind.FUNCTION and (
            not isinstance(row.get("signature"), str)
            or not isinstance(row.get("arguments"), str)
        ):
            raise ValueError(
                f"{runtime}: Python function requires its source signature and arguments"
            )
    return identities
