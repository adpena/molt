"""Generic custom loaders must not acquire extension policy from their name."""
import importlib.machinery
import importlib.util
import sys


class ExtensionFileLoader:
    @property
    def __class__(self):
        raise AssertionError("loader __class__ descriptor was evaluated")

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.answer = 6 * 7


class CustomExtensionFileLoaderSuffix(ExtensionFileLoader):
    pass


class Finder:
    def __init__(self, name, loader, origin):
        self.name = name
        self.loader = loader
        self.origin = origin

    def find_spec(self, fullname, path=None, target=None):
        if fullname == self.name:
            return importlib.machinery.ModuleSpec(
                fullname, self.loader, origin=self.origin
            )
        return None


for index, loader_type in enumerate(
    (ExtensionFileLoader, CustomExtensionFileLoaderSuffix)
):
    for origin in (None, "memory:custom-loader"):
        name = "custom_loader_name_" + str(index)
        loader = loader_type()
        finder = Finder(name, loader, origin)
        sys.meta_path.insert(0, finder)
        try:
            spec = importlib.util.find_spec(name)
            print("spec", index, spec.name == name, spec.loader is loader, spec.origin)
            module = importlib.util.module_from_spec(spec)
            loader.exec_module(module)
            print("execute", module.answer)
        finally:
            sys.meta_path.remove(finder)
