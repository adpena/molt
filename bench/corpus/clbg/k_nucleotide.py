# CLBG-derived kernel, vendored verbatim for molt's perf corpus (doc 69 / 69A, task S4).
# Upstream: The Computer Language Benchmarks Game
#   https://salsa.debian.org/benchmarksgame-team/benchmarksgame
#   repo_ref 40296663ed350d5fe4a6ab5e367bab61cb77c219  (site 25.03)
#   zip entry: knucleotide/knucleotide.python3
# Revised BSD (3-clause) -- see ./LICENSE. Original contributor credits retained below.
# Per BSD clause 3, do NOT use the CLBG / "Benchmarks Game" names to promote molt.
# Codon tier: equivalent. Reads FASTA from stdin (downstream of fasta.py).

# The Computer Language Benchmarks Game
# https://salsa.debian.org/benchmarksgame-team/benchmarksgame/
#
# submitted by Ian Osgood
# modified by Sokolov Yura
# modified by bearophile
# 2to3

from sys import stdin

def gen_freq(seq, frame, frequences):
    ns = len(seq) + 1 - frame
    frequences.clear()
    for ii in range(ns):
        nucleo = seq[ii:ii + frame]
        if nucleo in frequences:
            frequences[nucleo] += 1
        else:
            frequences[nucleo] = 1
    return ns, frequences


def sort_seq(seq, length, frequences):
    n, frequences = gen_freq(seq, length, frequences)

    l = sorted(list(frequences.items()), reverse=True, key=lambda seq_freq: (seq_freq[1],seq_freq[0]))

    print('\n'.join("%s %.3f" % (st, 100.0*fr/n) for st,fr in l))
    print()


def find_seq(seq, s, frequences):
    n,t = gen_freq(seq, len(s), frequences)
    print("%d\t%s" % (t.get(s, 0), s))


def main():
    frequences = {}
    for line in stdin:
        if line[0:3] == ">TH":
            break

    seq = []
    for line in stdin:
        if line[0] in ">;":
            break
        seq.append( line[:-1] )
    sequence = "".join(seq).upper()

    for nl in 1,2:
        sort_seq(sequence, nl, frequences)

    for se in "GGT GGTA GGTATT GGTATTTTAATT GGTATTTTAATTTATAGT".split():
        find_seq(sequence, se, frequences)

main()
