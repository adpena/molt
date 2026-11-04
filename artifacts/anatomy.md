# HF-685 construction anatomy before implementation

Base: `b8eea1a219b4d37d9187b1451331729e2c0356e3`, clean assigned checkout and retained detached base checkout. macOS arm64; native, guest/compiler release, runtime release-output. Frontend Python 3.12.15; independent CPython oracle 3.12.16 (executable SHA256 `7a427de7a538c033f821bae69d18dba1fcf01e1947f8fdfece0d9654f680a787`).

Retained observations were inspected before code changes. Five paired N=0/N=K observations use the canonical primitive programs and scoreboard counter/normalization (`macos-ri_instructions`, `/usr/bin/time -l`, startup subtracted). Guest streams agree. Each retained binary SHA256 matches its build observation. This is diagnostic evidence, not release acceptance; compiled_with_verified=false. The earlier full scoreboard stopped on SIGINT building unrelated dynamic_call and is not a completed board. The focused three-program counts completed.

| Primitive | Molt instructions/op | CPython instructions/op | Median paired ratio | Ratio spread |
| --- | ---: | ---: | ---: | ---: |
| plain_construction | 16,909.21 | 1,964.96 | 8.60324 | 0.4414% |
| new_and_update | 28,826.88 | 2,952.37 | 9.75999 | 0.4571% |
| dataclass_construction | 29,879.82 | 1,969.10 | 15.18361 | 0.2546% |

`opt_explorer` self-time buckets below are sampled attribution hints, not measured instruction counts for individual mechanisms. Multiplying these percentages by instructions/op would not establish causal instruction attribution. Startup is present, especially in the shorter new_and_update profile. Exact instruction subdivision remains unknown.

| Mechanism | plain self samples | new/update self samples | Independent runtime-counter delta/op and source attribution |
| --- | ---: | ---: | --- |
| Allocation / release | 384 (10.0%) | 59 (6.7%) | Plain: 1 instance + 1 callargs, 0 dicts, 80.001 bytes; new/update: 1 instance + 1 dict, 96.007 bytes. heap_lifecycle bucket includes release traversal, not only allocation. |
| __new__/__init__ dispatch | 464 (12.1%) | 66 (7.5%) | Plain emitted loop calls callargs_new, two push_pos and call_bind_ic_owned; Point.__init__ has 53 self samples. New/update: one runtime call dispatch and three attribute lookups/op. |
| Attribute storage / layout / dict exposure | 381 (9.9%) | 192 (21.9%) | Plain has two struct_field_store events, no dict allocation. class_layout::for_each_field has 61/12 additional other-runtime self samples; object/class shape reads and instance_dict_bits_ptr also recur. New/update materializes one dict/op by explicit __dict__ access. |
| Reference counting | 255 (6.6%) | 47 (5.4%) | Emitted inner loop has 7 dec_ref_obj + 3 inc_ref_obj sites for plain; 12 + 6 for new/update. Counts are static sites, not dynamic call totals. |
| Runtime entry | 186 (4.8%) | 34 (3.9%) | TLS and runtime guards; tlv_get_addr 129/29 self samples. |
| Exceptions | 30 (0.8%) | 15 (1.7%) | Both allocate zero exceptions/op. Pending checks and guards are not exception allocations. |
| Other runtime | 1,575 (40.9%) | 258 (29.5%) | Includes deduplicated symbols (348/26), hashing (212 plain), GC (track/untrack), sealed-layout traversal and namespace/frame lifetime work. |
| System | 478 (12.4%) | 198 (22.6%) | Mutexes, memset, loader/startup; not all attributable to allocation. |
| Compiled code | 94 (2.4%) | 6 (0.7%) | Inner static loop: 212 instructions plain, 441 new/update; different range paths also occur. |

Plain profile: 3,847 self samples; new/update: 875. Dataclass counters: one instance, one dict, four allocations and 192 bytes/op; no exception allocation. Nested runtime feedback counters were recomputed from baseline/full records; the previous helper's top-level numeric delta omitted nested counters.

The canonical owners already implement sealed fixed layouts (`class_layout`, `alloc_class_instance`) and lazy dictionaries (`field_storage`). Replacing these with another layout or cache would duplicate authority. The first structural candidate is removing repeated representation/layout projection from field traversal and dictionary ownership, preserving native-subtype payload tails and declared/inherited slots. Dispatch remains a separate measured contributor; choose further work from actual A/B results.

Prior art to follow: CPython's managed inline values and demand dictionary materialization, with existing Molt field_storage as the transition authority; https://github.com/python/cpython/blob/v3.12.0/Objects/object.c and https://github.com/python/cpython/blob/v3.12.0/Objects/dictobject.c . Existing findings_prior_art.md section X supplies early-binding context. No copied code.

Retained evidence root: `/Volumes/VertigoDataTier/Molt/tmp/scratch-24def3725233/`:
- `hf685-base-counts/axes.json`, per-program paired JSON, raw counter and stream files, identity.json.
- `hf685-base-profile-direct/`: canonical source bytes, identities, binaries, anatomy JSON, profiles and raw sample call trees.
- `hf685-base-counters/counters.json`: baseline and full runtime feedback.
- Drivers: hf685-profile-driver.py, hf685-count-retained.py, hf685-profile-counters.py.

Binary SHA256: plain `65fcd0749f3aa92543b5594c8ce6186cdf10d55da4a517385dc4d6059572c0fd`; new/update `b3f93d9b501fd361b761bde022af1081ca5e3b3b5820e0c2b1a795d597279db9`; dataclass `f294d4bf9f0c870d27f0dbf5fca0f786109adbfb576142d46c94d72697decf11`.

This file is committed before any code change as explicitly required by the live brief's 2026-11-04 06:06 CST correction. No backend differential, performance improvement, precheck, landing or retirement is claimed by this baseline artifact.
