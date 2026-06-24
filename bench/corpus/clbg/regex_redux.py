# CLBG-derived kernel, vendored verbatim for molt's perf corpus (doc 69 / 69A, task S4).
# Upstream: The Computer Language Benchmarks Game
#   https://salsa.debian.org/benchmarksgame-team/benchmarksgame
#   repo_ref 40296663ed350d5fe4a6ab5e367bab61cb77c219  (site 25.03)
#   zip entry: regexredux/regexredux.python3
# Revised BSD (3-clause) -- see ./LICENSE. Original contributor credits retained below.
# Per BSD clause 3, do NOT use the CLBG / "Benchmarks Game" names to promote molt.
# Codon tier: NON_EQUIVALENT. Engine-dependent (re); uses multiprocessing.Pool as a work distributor -- the adapter must support it or run var_find single-process; NEVER hand-edit this source. Reads FASTA from stdin.

# The Computer Language Benchmarks Game
# https://salsa.debian.org/benchmarksgame-team/benchmarksgame/
#
# regex-dna program contributed by Dominique Wahli
# 2to3
# mp by Ahmad Syukri
# modified by Justin Peel
# converted from regex-dna program

from sys import stdin
from re import sub, findall
from multiprocessing import Pool

def init(arg):
    global seq
    seq = arg

def var_find(f):
    return len(findall(f, seq))

def main():
    seq = stdin.read()
    ilen = len(seq)

    seq = sub('>.*\n|\n', '', seq)
    clen = len(seq)

    pool = Pool(initializer = init, initargs = (seq,))

    variants = (
          'agggtaaa|tttaccct',
          '[cgt]gggtaaa|tttaccc[acg]',
          'a[act]ggtaaa|tttacc[agt]t',
          'ag[act]gtaaa|tttac[agt]ct',
          'agg[act]taaa|ttta[agt]cct',
          'aggg[acg]aaa|ttt[cgt]ccct',
          'agggt[cgt]aa|tt[acg]accct',
          'agggta[cgt]a|t[acg]taccct',
          'agggtaa[cgt]|[acg]ttaccct')
    for f in zip(variants, pool.imap(var_find, variants)):
        print(f[0], f[1])

    subst = {
          'tHa[Nt]' : '<4>', 'aND|caN|Ha[DS]|WaS' : '<3>', 'a[NSt]|BY' : '<2>',
          '<[^>]*>' : '|', '\\|[^|][^|]*\\|' : '-'}
    for f, r in list(subst.items()):
        seq = sub(f, r, seq)

    print()
    print(ilen)
    print(clen)
    print(len(seq))

if __name__=="__main__":
    main()
