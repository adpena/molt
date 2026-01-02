# Molt Tasks, Channels, and Native Concurrency
**Status:** Priority / Core Feature  
**Audience:** Compiler engineers, runtime engineers, AI coding agents  

## Executive Summary
Native concurrency is foundational to Molt. This document defines a goroutine-class task model with channels, designed for Python ergonomics and Go-like performance.

## Goals
- Millions of cheap tasks
- No GIL
- M:N scheduling
- Structured cancellation
- Typed channels
- Production-grade semantics

## User Model
```python
from molt import task, spawn, chan

@task
def worker(jobs, results):
    for j in jobs:
        results.send(process(j))

def main():
    jobs = chan[int](buffer=1024)
    results = chan[int]()
    spawn(worker, jobs, results)
```

## Runtime
- Work-stealing scheduler
- OS threads
- Cooperative yields at channel and I/O ops

## Safety
- Immutable sharing allowed
- Mutable state via channels only (Tier 0)
- Compile-time rejection of unsafe sharing

## I/O
- epoll / kqueue integration
- One task per connection/request

## Partner Library
`molt_accel` provides CPython integration via a Molt worker binary and IPC.

## Acceptance
- 10× CPython throughput for I/O services
- Linear core scaling
- No global lock
