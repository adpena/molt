"""Shared source programs for reconstructed-function semantic oracles."""


def _activation_publication_sources(route, protected):
    import textwrap

    sources = {
        "alternatives": (
            "def replacement():\n    import replacement_provider\n"
            "def give():\n    return replacement\n"
        ),
    }
    consumer = (
        "from alternatives import replacement, give\n"
        "class Outer:\n"
        "    def __enter__(self): return self\n"
        "    def __exit__(self, *args): return True\n"
        "def original():\n    import original_provider\n"
        "selected = original\n"
    )
    if route.startswith("function-"):
        consumer += "from types import FunctionType\n"
        invocation = "selected()"
        if protected:
            invocation = "with Outer():\n    selected()"
        consumer += "def template():\n" + textwrap.indent(invocation, "    ") + "\n"
        if route == "function-dict":
            namespace = (
                "{'Outer': Outer, 'selected': replacement, "
                "'__builtins__': __builtins__}"
            )
        else:
            selection = (
                "give()" if route == "function-helper-missing" else "replacement"
            )
            consumer += (
                "class Activation(dict):\n"
                "    def __missing__(self, key):\n"
                "        if key == 'selected':\n"
                f"            return {selection}\n"
                "        raise KeyError(key)\n"
            )
            namespace = "Activation(Outer=Outer, __builtins__=__builtins__)"
        consumer += (
            "def entry():\n"
            f"    activation = {namespace}\n"
            "    rebound = FunctionType(template.__code__, activation)\n"
            "    rebound()\n"
        )
        expected_provider = "replacement_provider"
    else:
        if route.endswith("foreign"):
            sources["writer"] = (
                "import consumer\n"
                "from alternatives import replacement\n"
                "def update():\n    consumer.selected = replacement\n"
            )
            consumer += "from writer import update\n"
        else:
            consumer += (
                "def update():\n    global selected\n    selected = replacement\n"
            )
        alias = route.startswith("alias-")
        invocation = "retained()" if alias else "selected()"
        if protected:
            invocation = "with Outer():\n    " + invocation
        consumer += (
            "def entry():\n"
            + ("    retained = selected\n" if alias else "")
            + "    update()\n"
            + textwrap.indent(invocation, "    ")
            + "\n"
        )
        expected_provider = "original_provider" if alias else "replacement_provider"
    sources["consumer"] = consumer
    return sources, expected_provider


def _activation_context_sources(route):
    if route == "code-read":
        return {
            "consumer": (
                "def dormant():\n    import left_provider\n"
                "seen = []\n"
                "def entry():\n"
                "    try:\n        return dormant.__code__\n"
                "    except ImportError as error:\n"
                "        seen.append(error)\n"
            ),
        }
    alternatives = (
        "def left():\n    import left_provider\n"
        "def right():\n    import right_provider\n"
        "selected = right\n"
    )
    consumer = (
        "from types import FunctionType\n"
        "from alternatives import left, right\n"
        "selected = left\n"
        "seen = []\n"
    )
    if route.startswith("replace-"):
        alternatives += (
            "def make_codes():\n"
            "    retained = None\n"
            "    def initial():\n"
            "        'immutable declaration doc'\n"
            "        return retained\n"
            "    def replacement():\n"
            "        'replacement declaration doc'\n"
            "        retained\n        return selected\n"
            "    initial.__name__ = 'mutated source name'\n"
            "    initial.__qualname__ = 'mutated source qualname'\n"
            "    initial.__doc__ = 'mutated source doc'\n"
            "    return initial.__code__, replacement.__code__, initial.__closure__\n"
        )
        consumer += (
            "from alternatives import make_codes\n"
            "first_code, replacement_code, retained_closure = make_codes()\n"
        )
        if route == "replace-builtins":
            consumer += (
                "left_builtins = {'selected': left}\n"
                "right_builtins = {'selected': right}\n"
                "left_namespace = {'__builtins__': left_builtins}\n"
                "right_namespace = {'__builtins__': right_builtins}\n"
            )
        else:
            consumer += (
                "left_namespace = {'selected': left}\n"
                "right_namespace = {'selected': right}\n"
            )
        consumer += (
            "a = FunctionType(first_code, left_namespace, 'class name', "
            "closure=retained_closure)\n"
            "b = FunctionType.__new__(FunctionType, first_code, right_namespace, "
            "'direct name', closure=retained_closure)\n"
            "metadata_checks = [\n"
            "    a.__name__ == 'class name', b.__name__ == 'direct name',\n"
            "    first_code.co_qualname == 'make_codes.<locals>.initial',\n"
            "    a.__qualname__ == 'make_codes.<locals>.initial',\n"
            "    b.__qualname__ == 'make_codes.<locals>.initial',\n"
            "    a.__doc__ == 'immutable declaration doc',\n"
            "    b.__doc__ == 'immutable declaration doc',\n"
            "]\n"
            "a.__qualname__ = 'mutated first owner'\n"
            "a.__doc__ = 'mutated first owner doc'\n"
            "metadata_checks.extend([\n"
            "    first_code.co_qualname == 'make_codes.<locals>.initial',\n"
            "    b.__qualname__ == 'make_codes.<locals>.initial',\n"
            "    b.__doc__ == 'immutable declaration doc',\n"
            "])\n"
        )
        if route == "replace-builtins":
            consumer += (
                "left_namespace['__builtins__'] = right_builtins\n"
                "right_namespace['__builtins__'] = left_builtins\n"
            )
        consumer += "a.__code__ = replacement_code\nb.__code__ = replacement_code\n"
        consumer += (
            "c = FunctionType(replacement_code, left_namespace, 'new code owner', "
            "closure=retained_closure)\n"
            "metadata_checks.extend([\n"
            "    a.__qualname__ == 'mutated first owner',\n"
            "    a.__doc__ == 'mutated first owner doc',\n"
            "    b.__qualname__ == 'make_codes.<locals>.initial',\n"
            "    b.__doc__ == 'immutable declaration doc',\n"
            "    replacement_code.co_qualname == 'make_codes.<locals>.replacement',\n"
            "    c.__qualname__ == 'make_codes.<locals>.replacement',\n"
            "    c.__doc__ == 'replacement declaration doc',\n"
            "    c.__name__ == 'new code owner',\n"
            "])\n"
        )
    else:
        body = {
            "shared-return": "    return selected\n",
            "nested-def": (
                "    def nested():\n        return selected()\n    return nested\n"
            ),
            "nested-lambda": "    return lambda: selected()\n",
        }[route]
        consumer += (
            "def template():\n" + body + "left_namespace = {'selected': left}\n"
            "right_namespace = {'selected': right}\n"
            "a = FunctionType(template.__code__, left_namespace)\n"
            "b = FunctionType(template.__code__, right_namespace)\n"
        )
    consumer += (
        "def entry():\n"
        "    try:\n        a()()\n"
        "    except ImportError as error:\n        seen.append(error)\n"
        "    b()()\n"
    )
    return {"consumer": consumer, "alternatives": alternatives}


ACTIVATION_PUBLICATION_ROUTES = (
    "function-dict",
    "function-missing",
    "function-helper-missing",
)
ACTIVATION_CONTEXT_ROUTES = (
    "shared-return",
    "nested-def",
    "nested-lambda",
    "replace-globals",
    "replace-builtins",
)
ACTIVATION_TARGET_ROUTES = (*ACTIVATION_PUBLICATION_ROUTES, *ACTIVATION_CONTEXT_ROUTES)


def activation_target_sources(route: str) -> dict[str, str]:
    """Project the reference programs into ordinary importable target inputs.

    Providers record real module execution and raise retained marker objects.
    The main program prints observations, so the differential runner compares
    actual behavior even when an optimizer removes assertions.
    """
    if route in ACTIVATION_PUBLICATION_ROUTES:
        sources, _ = _activation_publication_sources(route, True)
        observation = "print(escaped is None)\n"
    elif route in ACTIVATION_CONTEXT_ROUTES:
        sources = _activation_context_sources(route)
        observation = (
            "print(escaped is markers['right_provider'])\n"
            "print([error is markers['left_provider'] for error in consumer.seen])\n"
            "checks = [consumer.a.__globals__ is consumer.left_namespace, "
            "consumer.b.__globals__ is consumer.right_namespace]\n"
        )
        if route.startswith("replace-"):
            observation += (
                "checks.extend([len(consumer.replacement_code.co_freevars) == 1, "
                "consumer.a.__closure__ is consumer.retained_closure, "
                "consumer.b.__closure__ is consumer.retained_closure, "
                "consumer.a.__code__ is consumer.replacement_code, "
                "consumer.b.__code__ is consumer.replacement_code])\n"
            )
        if route == "replace-builtins":
            observation += (
                "checks.extend([consumer.a.__builtins__ is consumer.left_builtins, "
                "consumer.b.__builtins__ is consumer.right_builtins, "
                "consumer.a.__globals__['__builtins__'] is consumer.right_builtins, "
                "consumer.b.__globals__['__builtins__'] is consumer.left_builtins])\n"
            )
        observation += "print(checks)\n"
        if route.startswith("replace-"):
            observation += "print(consumer.metadata_checks)\n"
    else:
        raise ValueError(f"unknown reconstructed-function case: {route}")
    providers = (
        ("original_provider", "replacement_provider")
        if route in ACTIVATION_PUBLICATION_ROUTES
        else ("left_provider", "right_provider")
    )
    sources["observations"] = (
        "attempts = []\nmarkers = {\n"
        + "".join(f"    {name!r}: ImportError({name!r}),\n" for name in providers)
        + "}\n"
    )
    for name in providers:
        sources[name] = (
            "from observations import attempts, markers\n"
            f"attempts.append({name!r})\nraise markers[{name!r}]\n"
        )
    sources["main"] = (
        "import consumer\nfrom observations import attempts, markers\n"
        "escaped = None\ntry:\n    consumer.entry()\n"
        "except ImportError as error:\n    escaped = error\n"
        f"print({route!r})\nprint(attempts)\n" + observation
    )
    return sources
