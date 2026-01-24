"""Purpose: differential coverage for set methods."""


def main() -> None:
    s = {1, 2}
    u = s.union([2, 3])
    print(len(u))
    print(1 in u, 3 in u, 4 in u)
    u_multi = s.union({3}, [4, 1])
    print(len(u_multi))
    print(1 in u_multi, 4 in u_multi, 5 in u_multi)
    i = s.intersection({2, 4})
    print(len(i))
    print(2 in i, 1 in i)
    i_multi = s.intersection({1, 2, 3}, [2, 5])
    print(len(i_multi))
    print(2 in i_multi, 1 in i_multi)
    d = s.difference([2, 5])
    print(len(d))
    print(1 in d, 2 in d)
    d_multi = s.difference({2}, [1])
    print(len(d_multi))
    print(1 in d_multi, 2 in d_multi)
    x = s.symmetric_difference({2, 3})
    print(len(x))
    print(1 in x, 2 in x, 3 in x)

    fs = frozenset([1, 2])
    fu = fs.union({2, 3})
    print(len(fu))
    print(1 in fu, 3 in fu)
    fu_multi = fs.union({2, 3}, [4])
    print(len(fu_multi))
    print(1 in fu_multi, 4 in fu_multi)
    fi = fs.intersection({2, 4})
    print(len(fi))
    print(2 in fi, 1 in fi)
    fi_multi = fs.intersection({1, 2, 3}, [2])
    print(len(fi_multi))
    print(2 in fi_multi, 1 in fi_multi)
    fd = fs.difference({2})
    print(len(fd))
    print(1 in fd, 2 in fd)
    fd_multi = fs.difference({2}, [1])
    print(len(fd_multi))
    print(1 in fd_multi, 2 in fd_multi)
    fx = fs.symmetric_difference({2, 3})
    print(len(fx))
    print(1 in fx, 2 in fx, 3 in fx)

    s_update = {1, 2}
    s_update.update([2, 3], {4})
    print(len(s_update))
    print(1 in s_update, 3 in s_update, 4 in s_update)
    s_update.update()
    print(len(s_update))

    s_inter = {1, 2, 3}
    s_inter.intersection_update({2, 3}, [3, 4])
    print(len(s_inter))
    print(3 in s_inter, 2 in s_inter)

    s_diff = {1, 2, 3}
    s_diff.difference_update({2}, [3])
    print(len(s_diff))
    print(1 in s_diff, 2 in s_diff, 3 in s_diff)

    s_sym = {1, 2, 3}
    s_sym.symmetric_difference_update({2, 4})
    print(len(s_sym))
    print(1 in s_sym, 2 in s_sym, 4 in s_sym)


if __name__ == "__main__":
    main()
