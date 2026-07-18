use super::*;

#[test]
fn epoch_delta_separates_monotonic_counters_from_signed_gauges() {
    let baseline = RuntimeProfileEpochBaseline {
        generation: 7,
        label: "steady_state".to_owned(),
        payload: serde_json::json!({
            "profile": {
                "alloc_count": 10,
                "alloc_bytes_total": 320,
                "dealloc_count": 8,
                "live_objects": 5,
                "expected_live": 4,
            },
            "aux": {"aux_sidecar_live_count": 2},
            "gc": {
                "gc_track_count": 9,
                "gc_tracked_live": 3,
                "gc_tracked_high_water": 6,
            },
            "hot_paths": {"call_bind_ic_hit": 1},
            "deopt_reasons": {"guard_tag_type_mismatch": 0},
            "memory": {
                "source": "proc-self-status",
                "available": true,
                "peak_rss_bytes": 4096,
                "current_rss_bytes": 3072,
            },
        }),
    };
    let end = serde_json::json!({
        "profile": {
            "alloc_count": 12,
            "alloc_bytes_total": 384,
            "dealloc_count": 7,
            "live_objects": 4,
            "expected_live": 4,
        },
        "aux": {"aux_sidecar_live_count": 1},
        "gc": {
            "gc_track_count": 13,
            "gc_tracked_live": 2,
            "gc_tracked_high_water": 8,
        },
        "hot_paths": {"call_bind_ic_hit": 6},
        "deopt_reasons": {"guard_tag_type_mismatch": 1},
        "memory": {
            "source": "proc-self-status",
            "available": true,
            "peak_rss_bytes": 8192,
            "current_rss_bytes": 2048,
        },
    });

    let Ok(delta) = runtime_profile_epoch_delta_payload(&baseline, &end) else {
        panic!("valid profile payload rejected");
    };

    assert_eq!(delta["generation"], 7);
    assert_eq!(delta["label"], "steady_state");
    assert_eq!(delta["claimable"], false);
    assert_eq!(delta["delta"]["profile"]["alloc_count"], 2);
    assert_eq!(delta["delta"]["profile"]["alloc_bytes_total"], 64);
    assert_eq!(
        delta["delta"]["profile"]["dealloc_count"],
        serde_json::Value::Null
    );
    assert_eq!(
        delta["counter_regressions"]["profile"]["dealloc_count"],
        serde_json::json!({"start": 8, "end": 7})
    );
    assert_eq!(delta["delta"]["gc"]["gc_track_count"], 4);
    assert_eq!(delta["delta"]["hot_paths"]["call_bind_ic_hit"], 5);
    assert_eq!(delta["gauges"]["profile"]["live_objects"]["delta"], -1);
    assert_eq!(delta["gauges"]["gc"]["gc_tracked_high_water"]["delta"], 2);
    assert_eq!(delta["memory"]["current_rss_delta_bytes"], -1024);
    assert_eq!(delta["memory"]["end"]["peak_rss_bytes"], 8192);
}

#[test]
fn epoch_delta_keeps_unavailable_rss_explicit() {
    let baseline = RuntimeProfileEpochBaseline {
        generation: 8,
        label: "wasm".to_owned(),
        payload: serde_json::json!({
            "profile": {"alloc_count": 1},
            "memory": {
                "source": "unsupported-wasm",
                "available": false,
                "peak_rss_bytes": null,
                "current_rss_bytes": null,
            },
        }),
    };
    let end = serde_json::json!({
        "profile": {"alloc_count": 2},
        "memory": {
            "source": "unsupported-wasm",
            "available": false,
            "peak_rss_bytes": null,
            "current_rss_bytes": null,
        },
    });

    let delta = runtime_profile_epoch_delta_payload(&baseline, &end)
        .expect("explicitly unavailable RSS must remain valid epoch evidence");
    assert_eq!(delta["claimable"], true);
    assert_eq!(
        delta["memory"]["current_rss_delta_bytes"],
        serde_json::Value::Null
    );
    assert_eq!(delta["memory"]["start"]["available"], false);
    assert_eq!(delta["memory"]["end"]["source"], "unsupported-wasm");
}
