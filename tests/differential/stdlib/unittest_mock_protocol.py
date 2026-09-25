"""Deterministic public unittest.mock protocol corpus, shared with source tests."""

import asyncio
import copy
import inspect
import json
import pickle
import types
import unittest.mock as mock


def rejects(exception, operation):
    try:
        operation()
    except exception:
        return True
    return False


def function(required, *, flag=False):
    return required, flag


class Service:
    def __init__(self, label):
        self.label = label

    def method(self, value, *, enabled=False):
        return value, enabled


async def remote(value):
    return value


def identities():
    token = mock.sentinel.protocol_token
    names = (
        "Mock",
        "MagicMock",
        "NonCallableMock",
        "NonCallableMagicMock",
        "AsyncMock",
        "create_autospec",
        "patch",
    )
    return {
        "default": mock.DEFAULT is mock.sentinel.DEFAULT,
        "patch_default": inspect.signature(mock.patch).parameters["new"].default
        is mock.DEFAULT,
        "object_default": inspect.signature(mock.patch.object).parameters["new"].default
        is mock.DEFAULT,
        "public_modules": all(
            getattr(mock, name).__module__ == "unittest.mock" for name in names
        ),
        "sentinel_type_module": type(token).__module__ == "unittest.mock",
        "sentinel_pickle": pickle.loads(pickle.dumps(token)) is token,
        "default_pickle": pickle.loads(pickle.dumps(mock.DEFAULT)) is mock.DEFAULT,
        "sentinel_copy": copy.copy(token) is token and copy.deepcopy(token) is token,
        "class_pickle": pickle.loads(pickle.dumps(mock.MagicMock)) is mock.MagicMock,
        "function_pickle": pickle.loads(pickle.dumps(mock.create_autospec))
        is mock.create_autospec,
    }


def autospec():
    wrapped = mock.create_autospec(function, return_value="result")
    result = wrapped(7, flag=True)
    wrapped.assert_called_once_with(7, flag=True)
    constructor = mock.create_autospec(Service, spec_set=True)
    instance = constructor("example")
    instance.method(8, enabled=True)
    instance.method.assert_called_once_with(8, enabled=True)
    instance_only = mock.create_autospec(Service, instance=True, spec_set=True)
    instance_only.method(9)
    instance_only.method.assert_called_once_with(9)
    return {
        "function_return": result == "result",
        "function_signature": inspect.signature(wrapped) == inspect.signature(function),
        "missing_argument": rejects(TypeError, lambda: wrapped()),
        "unknown_keyword": rejects(TypeError, lambda: wrapped(1, unknown=True)),
        "class_signature": rejects(TypeError, lambda: constructor()),
        "method_signature": rejects(TypeError, lambda: instance.method()),
        "class_identity": isinstance(constructor, mock.MagicMock),
        "instance_identity": isinstance(instance, mock.NonCallableMagicMock),
        "spec_identity": isinstance(instance, Service),
        "instance_only": not callable(instance_only),
        "spec_set": rejects(
            AttributeError, lambda: setattr(instance_only, "not_in_spec", 1)
        ),
    }


def mutable_public_settings():
    original_filter = mock.FILTER_DIR
    original_class = mock.MagicMock
    target = types.SimpleNamespace(function=function)
    item = mock.Mock()

    class Replacement(original_class):
        pass

    try:
        mock.FILTER_DIR = True
        filtered = "_mock_parent" not in dir(item)
        mock.FILTER_DIR = False
        unfiltered = "_mock_parent" in dir(item)
        mock.MagicMock = Replacement
        generated = mock.create_autospec(Service)
        with mock.patch.object(target, "function") as patched:
            patch_uses_public_class = isinstance(patched, Replacement)
        return {
            "filter_on": filtered,
            "filter_off": unfiltered,
            "autospec_public_class": isinstance(generated, Replacement),
            "patch_public_class": patch_uses_public_class,
        }
    finally:
        mock.FILTER_DIR = original_filter
        mock.MagicMock = original_class


def patch_lifecycle():
    target = types.SimpleNamespace(left="left", right="right")
    with mock.patch.object(target, "left") as replacement:
        default_replacement = (
            target.left is replacement and isinstance(replacement, mock.MagicMock)
        )
    default_restored = target.left == "left"
    with mock.patch.multiple(
        target, left=mock.DEFAULT, right=mock.DEFAULT
    ) as replacements:
        multiple_identity = (
            target.left is replacements["left"]
            and target.right is replacements["right"]
            and isinstance(target.left, mock.MagicMock)
        )
    multiple_restored = (target.left, target.right) == ("left", "right")
    first = mock.patch.object(target, "left", "changed-left")
    second = mock.patch.object(target, "right", "changed-right")
    try:
        first.start()
        second.start()
        started = (target.left, target.right) == ("changed-left", "changed-right")
    finally:
        mock.patch.stopall()
    stopped = (target.left, target.right) == ("left", "right")
    mapping = {"before": 1}
    with mock.patch.dict(mapping, {"during": 2}, clear=True):
        dict_during = mapping == {"during": 2}
    try:
        with mock.patch.object(target, "left", "temporary"):
            raise ValueError("restore on exit")
    except ValueError:
        pass
    with mock.patch.object(target, "created", create=True) as created:
        created_identity = target.created is created

    @mock.patch.object(target, "left")
    def decorated(value):
        return value is target.left

    return {
        "default_replacement": default_replacement,
        "default_restored": default_restored,
        "multiple_identity": multiple_identity,
        "multiple_restored": multiple_restored,
        "started": started,
        "stopall": stopped,
        "dict_during": dict_during,
        "dict_restored": mapping == {"before": 1},
        "exception_restored": target.left == "left",
        "created_identity": created_identity,
        "created_removed": not hasattr(target, "created"),
        "decorator_identity": decorated(),
    }


async def async_selection():
    target = types.SimpleNamespace(remote=remote)
    with mock.patch.object(target, "remote") as replacement:
        replacement.return_value = "awaited"
        result = await target.remote(5)
        replacement.assert_awaited_once_with(5)
        selected = isinstance(replacement, mock.AsyncMock)
    autospecced = mock.create_autospec(remote, return_value="autospecced")
    auto_result = await autospecced(6)
    autospecced.assert_awaited_once_with(6)
    return {
        "async_class": selected,
        "awaited_result": result == "awaited",
        "target_restored": target.remote is remote,
        "autospec_result": auto_result == "autospecced",
        "autospec_signature": rejects(TypeError, lambda: autospecced()),
    }


def main():
    results = {
        "identities": identities(),
        "autospec": autospec(),
        "mutable_public_settings": mutable_public_settings(),
        "patch_lifecycle": patch_lifecycle(),
        "async_selection": asyncio.run(async_selection()),
    }
    failures = [
        group_name + "." + name
        for group_name, group in results.items()
        for name, value in group.items()
        if value is not True
    ]
    assert not failures, failures
    print(json.dumps(results, sort_keys=True))


if __name__ == "__main__":
    main()
