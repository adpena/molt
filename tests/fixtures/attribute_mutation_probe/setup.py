from setuptools import Extension, setup

setup(
    name="attribute_mutation_probe",
    version="0.0.0",
    ext_modules=[
        Extension("attribute_mutation_probe", ["src/attribute_mutation_probe.c"])
    ],
)
