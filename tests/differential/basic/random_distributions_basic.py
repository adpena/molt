"""Purpose: differential coverage for random distributions."""

import random


def show(label, value):
    print(label, value)


r = random.Random(12345)
show("uniform", r.uniform(1.25, 9.5))
show("triangular_default", r.triangular())
show("triangular_mode", r.triangular(2.0, 12.0, 7.0))
show("normalvariate", r.normalvariate(0.0, 2.5))
show("gauss_1", r.gauss(1.5, 0.75))
show("gauss_2", r.gauss(1.5, 0.75))
show("lognormvariate", r.lognormvariate(0.25, 0.5))
show("expovariate", r.expovariate(1.25))
show("vonmisesvariate", r.vonmisesvariate(0.75, 1.1))
show("vonmisesvariate_flat", r.vonmisesvariate(0.75, 0.0))
show("gammavariate_gt1", r.gammavariate(3.5, 1.1))
show("gammavariate_eq1", r.gammavariate(1.0, 2.0))
show("gammavariate_lt1", r.gammavariate(0.6, 1.8))
show("betavariate", r.betavariate(1.5, 0.8))
show("paretovariate", r.paretovariate(1.2))
show("weibullvariate", r.weibullvariate(1.25, 2.75))
show("binomial_single", r.binomialvariate(1, 0.33))
show("binomial_sym", r.binomialvariate(20, 0.6))
show("binomial_geom", r.binomialvariate(12, 0.2))
show("binomial_btrs", r.binomialvariate(500, 0.2))

r2 = random.Random(777)
show("method_uniform", r2.uniform(-5.0, 5.0))
show("method_expovariate", r2.expovariate(0.75))

random.seed(4242)
show("global_uniform", random.uniform(0.5, 2.5))
show("global_triangular", random.triangular(0.5, 1.5, 1.25))
show("global_gauss", random.gauss(0.0, 1.0))
show("global_binomial", random.binomialvariate(10, 0.25))
