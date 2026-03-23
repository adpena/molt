import decorator

print("decorator", decorator.__version__)

@decorator.decorator
def my_decorator(func, *args, **kwargs):
    return func(*args, **kwargs)

@my_decorator
def greet(name):
    return "hello " + name

print("result:", greet("molt"))
print("func name:", greet.__name__)
