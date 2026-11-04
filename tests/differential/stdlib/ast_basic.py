"""Purpose: differential coverage for ast basics."""

import ast

source = """
def foo(x):
    \"\"\"doc\"\"\"
    return x + 1
""".strip()

tree = ast.parse(source)
func = tree.body[0]
print(ast.get_docstring(func))
node_names = {type(node).__name__ for node in ast.walk(tree)}
print("FunctionDef" in node_names, "Return" in node_names, "Name" in node_names)
print(len(node_names))
expr = ast.parse("1+2", mode="eval")
print(isinstance(expr, ast.Expression))
print(hasattr(ast, "PyCF_ALLOW_TOP_LEVEL_AWAIT"))


class BrokenField(ast.AST):
    _fields = ("value",)

    @property
    def value(self):
        raise ValueError("AST field failure")


for operation in (ast.iter_fields, ast.iter_child_nodes):
    try:
        list(operation(BrokenField()))
    except ValueError as error:
        print("field-error", operation.__name__, str(error))


class FieldsFailure(ast.AST):
    _fields = ()
    failure = None

    def __getattribute__(self, name):
        if name == "_fields":
            failure = object.__getattribute__(self, "failure")
            if failure is not None:
                raise failure
        return object.__getattribute__(self, name)


node = FieldsFailure()
for error_type in (AttributeError, ValueError):
    node.failure = error_type("AST fields failure")
    for operation in (ast.iter_fields, ast.iter_child_nodes):
        try:
            list(operation(node))
        except Exception as error:
            print("fields-error", operation.__name__, type(error).__name__, str(error))
