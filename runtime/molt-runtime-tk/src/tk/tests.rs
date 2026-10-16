#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::callbacks::filehandler_event_name;
use super::callbacks::{
    after_callback_name_from_token, filehandler_command_name, lookup_after_command_for_token,
    lookup_after_kind_for_token, next_after_token, register_after_command_token,
    remove_after_events_for_tokens, schedule_after_timer_token, sort_after_info_tokens,
    tokens_for_after_command, unregister_after_command_token,
};
#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
use super::callbacks::{filehandler_poll_events, filehandler_revents_to_mask};
use super::commands::{tkwait_visibility_reached_in_app, tkwait_window_exists};
use super::dialogs::{
    apply_default_extension, clamp_dialog_selection, commondialog_allowed_options,
    commondialog_is_supported_command, commondialog_supports_parent,
    filedialog_is_supported_command, join_dialog_path, messagebox_icon_is_supported,
    normalize_color_literal, resolve_messagebox_selection,
};
use super::dispatch::pop_next_ready_event;
use super::event_commands::{
    event_generate_binding_sequences, parse_bind_script_commands,
    remove_bind_script_command_invocations, treeview_event_target_item,
};
use super::parsing::{
    first_missing_treeview_item, parse_bool_text, parse_expr_literal, parse_notebook_index_strict,
    parse_tcl_script_commands, parse_treeview_column_offset, parse_treeview_index_strict,
    parse_ttk_insert_index_strict, treeview_hit_item_by_y, treeview_item_is_descendant_of,
    treeview_visible_items,
};
#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
use super::state::TkFileHandlerRegistration;
use super::state::{
    TK_FILE_EVENT_EXCEPTION, TK_FILE_EVENT_READABLE, TK_FILE_EVENT_WRITABLE, TkAppState, TkEvent,
    TkExprLiteral, TkGateState, TkOperation, TkRegistry, TkTraceRegistration, TkTreeviewItem,
    TkTreeviewState, TkWidgetState, TkWmState, format_permission_error_message,
    format_tk_unavailable_message, has_platform_preflight_blockers, wm_state_for_path,
    wm_state_for_path_mut,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::tcl_find_executable_arg;
use super::trace_commands::{
    bump_variable_version, collect_trace_callbacks_for_operation, normalize_trace_mode_name,
    split_array_variable_reference, trace_callback_command_words, trace_mode_matches,
    variable_version,
};
use std::collections::HashMap;

#[test]
fn permission_message_single_capability_stays_stable() {
    let state = TkGateState {
        missing_gui_window: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_permission_error_message(&state),
        "missing gui.window capability"
    );
}

#[test]
fn permission_message_multi_capability_stays_ordered() {
    let state = TkGateState {
        missing_gui_window: true,
        missing_process_spawn: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_permission_error_message(&state),
        "missing capabilities: gui.window, process.spawn"
    );
}

#[test]
fn unavailable_message_native_blockers_exclude_backend_not_implemented() {
    let state = TkGateState {
        wasm_unsupported: false,
        backend_unimplemented: false,
        missing_gui_window: true,
        missing_process_spawn: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_tk_unavailable_message(TkOperation::AppNew, &state),
        "tkinter runtime support is not implemented yet (molt_tk_app_new) [blockers: capability.gui.window, capability.process.spawn]"
    );
}

#[test]
fn unavailable_message_includes_platform_preflight_blockers() {
    let state = TkGateState {
        missing_linux_display: true,
        missing_macos_main_thread: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_tk_unavailable_message(TkOperation::AppNew, &state),
        "tkinter runtime support is not implemented yet (molt_tk_app_new) [blockers: platform.linux.display, platform.macos.main_thread]"
    );
}

#[test]
fn platform_preflight_blockers_helper_matches_state() {
    let mut state = TkGateState::default();
    assert!(!has_platform_preflight_blockers(&state));
    state.missing_linux_display = true;
    assert!(has_platform_preflight_blockers(&state));
    state.missing_linux_display = false;
    state.missing_macos_main_thread = true;
    assert!(has_platform_preflight_blockers(&state));
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[test]
fn tcl_find_executable_arg_prefers_non_empty_path() {
    let arg = tcl_find_executable_arg();
    assert!(!arg.as_bytes().is_empty());
}

#[test]
fn tcl_script_parser_handles_quotes_braces_and_commands() {
    assert_eq!(
        parse_tcl_script_commands("  set   answer   42  "),
        vec![vec![
            "set".to_string(),
            "answer".to_string(),
            "42".to_string()
        ]]
    );
    assert_eq!(
        parse_tcl_script_commands("set a {x y}; set b \"quoted value\""),
        vec![
            vec!["set".to_string(), "a".to_string(), "x y".to_string()],
            vec![
                "set".to_string(),
                "b".to_string(),
                "quoted value".to_string()
            ],
        ]
    );
    assert!(parse_tcl_script_commands(" \t\n ").is_empty());
}

#[test]
fn trace_callback_words_parses_command_prefix_words() {
    assert_eq!(
        trace_callback_command_words("::__molt_trace_cb arg1 arg2"),
        vec![
            "::__molt_trace_cb".to_string(),
            "arg1".to_string(),
            "arg2".to_string(),
        ]
    );
}

#[test]
fn trace_callback_words_preserves_single_braced_word() {
    assert_eq!(
        trace_callback_command_words("{::__molt trace cb}"),
        vec!["::__molt trace cb".to_string()]
    );
}

#[test]
fn bind_script_parser_extracts_if_wrapper_command_words() {
    let script = "if {\"[::__molt_cb %# %x %y]\" == \"break\"} break";
    assert_eq!(
        parse_bind_script_commands(script),
        vec![vec![
            "::__molt_cb".to_string(),
            "%#".to_string(),
            "%x".to_string(),
            "%y".to_string(),
        ]]
    );
}

#[test]
fn bind_script_remove_command_drops_matching_if_wrapper_lines() {
    let script = concat!(
        "if {\"[::__molt_keep %#]\" == \"break\"} break\n",
        "if {\"[::__molt_drop %#]\" == \"break\"} break\n",
    );
    assert_eq!(
        remove_bind_script_command_invocations(script, "::__molt_drop"),
        "if {\"[::__molt_keep %#]\" == \"break\"} break\n"
    );
}

#[test]
fn bind_script_remove_command_preserves_non_matching_commands() {
    let script = "+::__molt_drop %x %y\n+::__molt_keep %x %y";
    assert_eq!(
        remove_bind_script_command_invocations(script, "::__molt_drop"),
        "+::__molt_keep %x %y"
    );
}

#[test]
fn bind_script_remove_command_handles_plus_prefixed_if_wrapper() {
    let script = concat!(
        "+if {\"[::__molt_drop %#]\" == \"break\"} break\n",
        "+if {\"[::__molt_keep %#]\" == \"break\"} break\n",
    );
    assert_eq!(
        remove_bind_script_command_invocations(script, "::__molt_drop"),
        "+if {\"[::__molt_keep %#]\" == \"break\"} break\n"
    );
}

#[test]
fn expr_literal_parsing_handles_int_and_float() {
    assert_eq!(parse_expr_literal("123"), Some(TkExprLiteral::Int(123)));
    assert_eq!(parse_expr_literal("3.5"), Some(TkExprLiteral::Float(3.5)));
    assert_eq!(parse_expr_literal("x + 1"), None);
}

#[test]
fn after_token_generation_is_deterministic() {
    let mut next_after_id = 0;
    assert_eq!(next_after_token(&mut next_after_id), "after#1");
    assert_eq!(next_after_token(&mut next_after_id), "after#2");
    assert_eq!(next_after_token(&mut next_after_id), "after#3");
}

#[test]
fn after_callback_name_derivation_is_deterministic() {
    assert_eq!(
        after_callback_name_from_token("after#7"),
        "::__molt_after_callback_7"
    );
    assert_eq!(
        after_callback_name_from_token("custom_token"),
        "::__molt_after_callback_custom_token"
    );
}

#[test]
fn after_helpers_remove_expected_tokens() {
    let mut app = TkAppState::default();
    register_after_command_token(&mut app, "after#1", "cmd_one", "timer");
    register_after_command_token(&mut app, "after#2", "cmd_one", "idle");
    register_after_command_token(&mut app, "after#3", "cmd_two", "timer");
    schedule_after_timer_token(&mut app, "after#1", 5);

    app.event_queue.push_back(TkEvent::Callback {
        token: "after#1".to_string(),
    });
    app.event_queue.push_back(TkEvent::Script {
        token: "after#2".to_string(),
        commands: vec![vec![
            "set".to_string(),
            "value".to_string(),
            "1".to_string(),
        ]],
    });
    app.event_queue.push_back(TkEvent::Callback {
        token: "after#3".to_string(),
    });

    assert_eq!(
        lookup_after_command_for_token(&app, "after#1").as_deref(),
        Some("cmd_one")
    );
    let tokens = tokens_for_after_command(&app, "cmd_one");
    assert_eq!(tokens.len(), 2);
    remove_after_events_for_tokens(&mut app, &tokens);

    assert_eq!(app.event_queue.len(), 1);
    let Some(TkEvent::Callback { token }) = app.event_queue.front() else {
        panic!("expected callback event");
    };
    assert_eq!(token, "after#3");

    unregister_after_command_token(&mut app, "after#3");
    assert!(lookup_after_kind_for_token(&app, "after#3").is_none());
    assert!(!app.after_due_at_ms.contains_key("after#3"));
}

#[test]
fn after_scheduler_waits_for_due_tokens_and_prefers_non_idle() {
    let mut app = TkAppState::default();
    register_after_command_token(&mut app, "after#timer", "timer_cmd", "timer");
    register_after_command_token(&mut app, "after#idle", "idle_cmd", "idle");
    schedule_after_timer_token(&mut app, "after#timer", 3);

    app.event_queue.push_back(TkEvent::Script {
        token: "after#idle".to_string(),
        commands: vec![vec!["set".to_string(), "idle".to_string(), "1".to_string()]],
    });
    app.event_queue.push_back(TkEvent::Script {
        token: "after#timer".to_string(),
        commands: vec![vec![
            "set".to_string(),
            "timer".to_string(),
            "1".to_string(),
        ]],
    });

    let first = pop_next_ready_event(&mut app).expect("first event");
    assert!(matches!(first, TkEvent::Script { token, .. } if token == "after#idle"));

    let second = pop_next_ready_event(&mut app);
    assert!(second.is_none());

    let third = pop_next_ready_event(&mut app).expect("third event");
    assert!(matches!(third, TkEvent::Script { token, .. } if token == "after#timer"));
}

#[test]
fn after_info_token_sorting_prefers_newest_after_ids() {
    let mut tokens = vec![
        "after#2".to_string(),
        "after#10".to_string(),
        "after#1".to_string(),
        "custom".to_string(),
    ];
    sort_after_info_tokens(&mut tokens);
    assert_eq!(
        tokens,
        vec![
            "after#10".to_string(),
            "after#2".to_string(),
            "after#1".to_string(),
            "custom".to_string(),
        ]
    );
}

#[test]
fn tkwait_window_exists_handles_root_handle_and_widget_paths() {
    let mut registry = TkRegistry::default();
    registry.apps.insert(7, TkAppState::default());
    assert!(tkwait_window_exists(&registry, 7, "."));
    assert!(!tkwait_window_exists(&registry, 8, "."));
    {
        let app = registry.apps.get_mut(&7).expect("app handle");
        app.widgets.insert(
            ".w".to_string(),
            TkWidgetState {
                widget_command: "frame".to_string(),
                ..TkWidgetState::default()
            },
        );
    }
    assert!(tkwait_window_exists(&registry, 7, ".w"));
    assert!(!tkwait_window_exists(&registry, 7, ".missing"));
}

#[test]
fn tkwait_visibility_tracks_root_wm_state_and_widget_manager() {
    let mut app = TkAppState::default();
    assert!(tkwait_visibility_reached_in_app(&app, "."));

    app.wm.state = "withdrawn".to_string();
    assert!(!tkwait_visibility_reached_in_app(&app, "."));

    app.wm.state = "normal".to_string();
    app.widgets.insert(
        ".w".to_string(),
        TkWidgetState {
            widget_command: "frame".to_string(),
            manager: Some("pack".to_string()),
            ..TkWidgetState::default()
        },
    );
    assert!(tkwait_visibility_reached_in_app(&app, ".w"));
    assert!(!tkwait_visibility_reached_in_app(&app, ".missing"));
}

#[test]
fn wm_state_is_isolated_per_toplevel_path() {
    let mut app = TkAppState::default();
    app.wm.title = "root".to_string();
    app.wm
        .protocols
        .insert("WM_DELETE_WINDOW".to_string(), "root_cb".to_string());
    app.widgets.insert(
        ".dialog".to_string(),
        TkWidgetState {
            widget_command: "toplevel".to_string(),
            wm: Some(TkWmState::default()),
            ..TkWidgetState::default()
        },
    );

    let dialog_wm = wm_state_for_path_mut(&mut app, ".dialog").expect("dialog wm");
    dialog_wm.title = "dialog".to_string();
    dialog_wm.transient = Some(".".to_string());
    dialog_wm
        .protocols
        .insert("WM_DELETE_WINDOW".to_string(), "dialog_cb".to_string());

    let root_wm = wm_state_for_path(&app, ".").expect("root wm");
    assert_eq!(root_wm.title, "root");
    assert_eq!(
        root_wm
            .protocols
            .get("WM_DELETE_WINDOW")
            .map(String::as_str),
        Some("root_cb")
    );
    assert!(root_wm.transient.is_none());

    let dialog_wm = wm_state_for_path(&app, ".dialog").expect("dialog wm");
    assert_eq!(dialog_wm.title, "dialog");
    assert_eq!(dialog_wm.transient.as_deref(), Some("."));
    assert_eq!(
        dialog_wm
            .protocols
            .get("WM_DELETE_WINDOW")
            .map(String::as_str),
        Some("dialog_cb")
    );
}

#[test]
fn split_array_variable_reference_handles_array_element_names() {
    assert_eq!(
        split_array_variable_reference("name"),
        ("name".to_string(), None)
    );
    assert_eq!(
        split_array_variable_reference("arr(key)"),
        ("arr".to_string(), Some("key".to_string()))
    );
    assert_eq!(
        split_array_variable_reference("(broken)"),
        ("(broken)".to_string(), None)
    );
}

#[test]
fn trace_mode_normalization_and_matching_are_stable() {
    assert_eq!(normalize_trace_mode_name("write").as_deref(), Ok("write"));
    assert_eq!(
        normalize_trace_mode_name("write read").as_deref(),
        Ok("read write")
    );
    assert_eq!(
        normalize_trace_mode_name("w, read, read, u").as_deref(),
        Ok("read write unset")
    );
    assert!(normalize_trace_mode_name("bogus").is_err());
    assert!(normalize_trace_mode_name("").is_err());

    assert!(trace_mode_matches("write", "write"));
    assert!(trace_mode_matches("read write", "read"));
    assert!(trace_mode_matches("read write", "write"));
    assert!(trace_mode_matches("unset", "unset"));
    assert!(!trace_mode_matches("read", "write"));
    assert!(!trace_mode_matches("", "read"));
}

#[test]
fn trace_callbacks_preserve_registration_order() {
    let mut app = TkAppState::default();
    app.traces.insert(
        "trace_var".to_string(),
        vec![
            TkTraceRegistration {
                mode_name: "write".to_string(),
                callback_name: "cb_write".to_string(),
                order: 20,
            },
            TkTraceRegistration {
                mode_name: "write".to_string(),
                callback_name: "cb_w".to_string(),
                order: 10,
            },
            TkTraceRegistration {
                mode_name: "read".to_string(),
                callback_name: "cb_read".to_string(),
                order: 30,
            },
        ],
    );

    let write_callbacks = collect_trace_callbacks_for_operation(&app, "trace_var", "write", None);
    assert_eq!(
        write_callbacks,
        vec![
            ("cb_w".to_string(), "write".to_string()),
            ("cb_write".to_string(), "write".to_string())
        ]
    );
    let read_callbacks = collect_trace_callbacks_for_operation(&app, "trace_var", "read", None);
    assert_eq!(
        read_callbacks,
        vec![("cb_read".to_string(), "read".to_string())]
    );
}

#[test]
fn trace_callbacks_include_array_mode_for_element_access() {
    let mut app = TkAppState::default();
    app.traces.insert(
        "arr".to_string(),
        vec![
            TkTraceRegistration {
                mode_name: "array".to_string(),
                callback_name: "cb_array".to_string(),
                order: 2,
            },
            TkTraceRegistration {
                mode_name: "write".to_string(),
                callback_name: "cb_write".to_string(),
                order: 1,
            },
        ],
    );

    let callbacks =
        collect_trace_callbacks_for_operation(&app, "arr(index)", "write", Some("index"));
    assert_eq!(
        callbacks,
        vec![
            ("cb_write".to_string(), "write".to_string()),
            ("cb_array".to_string(), "array".to_string()),
        ]
    );
}

#[test]
fn event_generate_binding_sequences_include_virtual_aliases() {
    let mut app = TkAppState::default();
    app.virtual_events.insert(
        "<<ProbeVirtual>>".to_string(),
        vec!["<KeyPress>".to_string(), "<Button-1>".to_string()],
    );
    let key_sequences = event_generate_binding_sequences(&app, "<KeyPress>");
    assert_eq!(
        key_sequences,
        vec!["<KeyPress>".to_string(), "<<ProbeVirtual>>".to_string()]
    );
    let virtual_sequences = event_generate_binding_sequences(&app, "<<ProbeVirtual>>");
    assert_eq!(virtual_sequences, vec!["<<ProbeVirtual>>".to_string()]);
}

#[test]
fn treeview_descendant_detection_handles_parent_chains() {
    let mut treeview = TkTreeviewState::default();
    treeview.items.insert(
        "root_child".to_string(),
        TkTreeviewItem {
            parent: "".to_string(),
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "leaf".to_string(),
        TkTreeviewItem {
            parent: "root_child".to_string(),
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "deep_leaf".to_string(),
        TkTreeviewItem {
            parent: "leaf".to_string(),
            ..TkTreeviewItem::default()
        },
    );

    assert!(treeview_item_is_descendant_of(
        &treeview,
        "deep_leaf",
        "root_child"
    ));
    assert!(treeview_item_is_descendant_of(
        &treeview,
        "leaf",
        "root_child"
    ));
    assert!(!treeview_item_is_descendant_of(
        &treeview,
        "root_child",
        "deep_leaf"
    ));
}

#[test]
fn treeview_event_target_item_priority_is_stable() {
    let mut treeview = TkTreeviewState::default();
    treeview
        .items
        .insert("i1".to_string(), TkTreeviewItem::default());
    treeview
        .items
        .insert("i2".to_string(), TkTreeviewItem::default());
    treeview
        .items
        .insert("i3".to_string(), TkTreeviewItem::default());
    treeview.focus = Some("i2".to_string());
    treeview.selection = vec!["i3".to_string()];

    let mut options = HashMap::new();
    options.insert("-item".to_string(), "i1".to_string());
    assert_eq!(
        treeview_event_target_item(&treeview, &options).as_deref(),
        Some("i1")
    );

    options.clear();
    assert_eq!(
        treeview_event_target_item(&treeview, &options).as_deref(),
        Some("i2")
    );

    treeview.focus = None;
    assert_eq!(
        treeview_event_target_item(&treeview, &options).as_deref(),
        Some("i3")
    );
}

#[test]
fn treeview_visible_order_and_hit_testing_are_deterministic() {
    let mut treeview = TkTreeviewState {
        root_children: vec!["r1".to_string(), "r2".to_string()],
        ..TkTreeviewState::default()
    };
    treeview.items.insert(
        "r1".to_string(),
        TkTreeviewItem {
            parent: String::new(),
            children: vec!["c1".to_string()],
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "c1".to_string(),
        TkTreeviewItem {
            parent: "r1".to_string(),
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "r2".to_string(),
        TkTreeviewItem {
            parent: String::new(),
            ..TkTreeviewItem::default()
        },
    );

    assert_eq!(
        treeview_visible_items(&treeview),
        vec!["r1".to_string(), "c1".to_string(), "r2".to_string()]
    );
    assert_eq!(treeview_hit_item_by_y(&treeview, 0).as_deref(), Some("r1"));
    assert_eq!(treeview_hit_item_by_y(&treeview, 20).as_deref(), Some("c1"));
    assert_eq!(treeview_hit_item_by_y(&treeview, 40).as_deref(), Some("r2"));
    assert_eq!(treeview_hit_item_by_y(&treeview, 60).as_deref(), None);
}

#[test]
fn treeview_column_offset_parser_accepts_hash_indices() {
    assert_eq!(parse_treeview_column_offset("#0"), Some(0));
    assert_eq!(parse_treeview_column_offset("#1"), Some(120));
    assert_eq!(parse_treeview_column_offset("#2"), Some(240));
    assert_eq!(parse_treeview_column_offset("#-1"), None);
    assert_eq!(parse_treeview_column_offset("1"), None);
    assert_eq!(parse_treeview_column_offset("bad"), None);
}

#[test]
fn treeview_strict_index_parser_rejects_non_integer_tokens() {
    assert_eq!(parse_treeview_index_strict("end", 4), Some(4));
    assert_eq!(parse_treeview_index_strict("2", 4), Some(2));
    assert_eq!(parse_treeview_index_strict("-7", 4), Some(0));
    assert_eq!(parse_treeview_index_strict("oops", 4), None);
}

#[test]
fn ttk_insert_index_parser_rejects_non_integer_tokens() {
    assert_eq!(parse_ttk_insert_index_strict("end", 3), Some(3));
    assert_eq!(parse_ttk_insert_index_strict("1", 3), Some(1));
    assert_eq!(parse_ttk_insert_index_strict("-2", 3), Some(0));
    assert_eq!(parse_ttk_insert_index_strict("bad", 3), None);
}

#[test]
fn notebook_index_parser_enforces_bounds() {
    assert_eq!(parse_notebook_index_strict("end", 2), Ok(2));
    assert_eq!(parse_notebook_index_strict("0", 2), Ok(0));
    assert_eq!(parse_notebook_index_strict("1", 2), Ok(1));
    assert!(parse_notebook_index_strict("-1", 2).is_err());
    assert!(parse_notebook_index_strict("2", 2).is_err());
    assert!(parse_notebook_index_strict("tabx", 2).is_err());
}

#[test]
fn treeview_missing_item_detection_reports_first_missing_id() {
    let mut treeview = TkTreeviewState::default();
    treeview
        .items
        .insert("i1".to_string(), TkTreeviewItem::default());
    treeview
        .items
        .insert("i2".to_string(), TkTreeviewItem::default());

    let items = vec!["i1".to_string(), "missing".to_string(), "i2".to_string()];
    assert_eq!(
        first_missing_treeview_item(&treeview, &items),
        Some("missing")
    );

    let existing = vec!["i1".to_string(), "i2".to_string()];
    assert_eq!(first_missing_treeview_item(&treeview, &existing), None);
}

#[test]
fn variable_version_progress_is_monotonic() {
    let mut app = TkAppState::default();
    assert_eq!(variable_version(&app, "name"), 0);
    bump_variable_version(&mut app, "name");
    assert_eq!(variable_version(&app, "name"), 1);
    bump_variable_version(&mut app, "name");
    assert_eq!(variable_version(&app, "name"), 2);
}

#[test]
fn commondialog_supported_command_allowlist_is_stable() {
    for command in [
        "tk_messageBox",
        "tk_getOpenFile",
        "tk_getSaveFile",
        "tk_chooseDirectory",
        "tk_chooseColor",
    ] {
        assert!(commondialog_is_supported_command(command));
        assert!(commondialog_supports_parent(command));
    }
    assert!(!commondialog_is_supported_command("tk_chooseFont"));
    assert!(!commondialog_supports_parent("tk_chooseFont"));
}

#[test]
fn commondialog_option_allowlists_cover_core_dialog_flags() {
    assert!(
        commondialog_allowed_options("tk_messageBox")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-type"))
    );
    assert!(
        commondialog_allowed_options("tk_getOpenFile")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-multiple"))
    );
    assert!(
        commondialog_allowed_options("tk_chooseDirectory")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-mustexist"))
    );
    assert!(
        !commondialog_allowed_options("tk_chooseColor")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-multiple"))
    );
}

#[test]
fn bool_text_parser_accepts_tcl_prefix_forms() {
    assert_eq!(parse_bool_text("t"), Some(true));
    assert_eq!(parse_bool_text("tr"), Some(true));
    assert_eq!(parse_bool_text("y"), Some(true));
    assert_eq!(parse_bool_text("f"), Some(false));
    assert_eq!(parse_bool_text("fa"), Some(false));
    assert_eq!(parse_bool_text("n"), Some(false));
    assert_eq!(parse_bool_text("o"), None);
    assert_eq!(parse_bool_text(""), None);
}

#[test]
fn filedialog_supported_command_allowlist_is_stable() {
    assert!(filedialog_is_supported_command("tk_getOpenFile"));
    assert!(filedialog_is_supported_command("tk_getSaveFile"));
    assert!(filedialog_is_supported_command("tk_chooseDirectory"));
    assert!(!filedialog_is_supported_command("tk_chooseColor"));
    assert!(!filedialog_is_supported_command("tk_chooseFont"));
}

#[test]
fn messagebox_selection_defaults_are_deterministic() {
    assert_eq!(
        resolve_messagebox_selection("ok", None).as_deref(),
        Ok("ok")
    );
    assert_eq!(
        resolve_messagebox_selection("yesnocancel", None).as_deref(),
        Ok("yes")
    );
    assert_eq!(
        resolve_messagebox_selection("yesno", Some("no")).as_deref(),
        Ok("no")
    );
    assert!(resolve_messagebox_selection("bogus", None).is_err());
    assert!(resolve_messagebox_selection("ok", Some("cancel")).is_err());
}

#[test]
fn messagebox_icon_validation_is_stable() {
    for icon in ["error", "info", "question", "warning", "ERROR"] {
        assert!(messagebox_icon_is_supported(icon));
    }
    assert!(!messagebox_icon_is_supported("bogus"));
}

#[test]
fn dialog_path_joining_handles_common_forms() {
    assert_eq!(join_dialog_path("", ""), "");
    assert_eq!(join_dialog_path("/tmp", "out.txt"), "/tmp/out.txt");
    assert_eq!(join_dialog_path("/tmp/", "out.txt"), "/tmp/out.txt");
    assert_eq!(
        join_dialog_path("C:\\Users\\me", "out.txt"),
        "C:\\Users\\me\\out.txt"
    );
}

#[test]
fn default_extension_application_is_stable() {
    assert_eq!(apply_default_extension("", ".txt"), "");
    assert_eq!(
        apply_default_extension("/tmp/output", ".txt"),
        "/tmp/output.txt"
    );
    assert_eq!(
        apply_default_extension("/tmp/output", "log"),
        "/tmp/output.log"
    );
    assert_eq!(
        apply_default_extension("/tmp/output.txt", ".log"),
        "/tmp/output.txt"
    );
}

#[test]
fn color_literal_normalization_supports_short_and_long_hex() {
    assert_eq!(normalize_color_literal("#abc").as_deref(), Some("#aabbcc"));
    assert_eq!(
        normalize_color_literal("#A1b2C3").as_deref(),
        Some("#A1b2C3")
    );
    assert_eq!(normalize_color_literal("red").as_deref(), Some("red"));
    assert!(normalize_color_literal("#zzzzzz").is_none());
}

#[test]
fn dialog_selection_clamping_is_stable() {
    assert_eq!(clamp_dialog_selection(-2, 0), 0);
    assert_eq!(clamp_dialog_selection(-2, 3), 0);
    assert_eq!(clamp_dialog_selection(1, 3), 1);
    assert_eq!(clamp_dialog_selection(9, 3), 2);
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[test]
fn filehandler_event_name_mapping_is_stable() {
    assert_eq!(
        filehandler_event_name(TK_FILE_EVENT_READABLE),
        Some("readable")
    );
    assert_eq!(
        filehandler_event_name(TK_FILE_EVENT_WRITABLE),
        Some("writable")
    );
    assert_eq!(
        filehandler_event_name(TK_FILE_EVENT_EXCEPTION),
        Some("exception")
    );
    assert_eq!(filehandler_event_name(0), None);
}

#[test]
fn filehandler_command_name_is_deterministic() {
    assert_eq!(
        filehandler_command_name(41, "readable"),
        "::__molt_filehandler_41_readable"
    );
    assert_eq!(
        filehandler_command_name(7, "exception"),
        "::__molt_filehandler_7_exception"
    );
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
#[test]
fn filehandler_poll_event_mask_is_stable() {
    let mut registration = TkFileHandlerRegistration {
        callback_bits: 0,
        file_obj_bits: 0,
        commands: HashMap::new(),
    };
    registration
        .commands
        .insert(TK_FILE_EVENT_READABLE, "r".to_string());
    registration
        .commands
        .insert(TK_FILE_EVENT_WRITABLE, "w".to_string());
    assert_eq!(
        filehandler_poll_events(&registration),
        libc::POLLIN | libc::POLLOUT
    );
    registration
        .commands
        .insert(TK_FILE_EVENT_EXCEPTION, "x".to_string());
    assert_eq!(
        filehandler_poll_events(&registration),
        libc::POLLIN | libc::POLLOUT | libc::POLLPRI
    );
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
#[test]
fn filehandler_revents_translation_is_stable() {
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLIN),
        TK_FILE_EVENT_READABLE
    );
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLOUT),
        TK_FILE_EVENT_WRITABLE
    );
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLERR | libc::POLLNVAL),
        TK_FILE_EVENT_EXCEPTION
    );
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLHUP | libc::POLLPRI),
        TK_FILE_EVENT_READABLE | TK_FILE_EVENT_EXCEPTION
    );
}

// Each test owns process-global OnceLocks and CWD: run one exact ignored test
// per fresh process. The parent proof guard supplies TEMP/TMPDIR and removes its
// scratch after exit; Windows keeps the production-owned DLL mapped until then.
#[cfg(all(
    feature = "native-tcl",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod library_policy_tests {
    use crate::tk::tcl::{load_tcl_api, tcl_library_candidates};
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    fn set_operator(key: &str, value: Option<&OsStr>) {
        molt_runtime_platform::env_support::process_env_set(OsStr::new(key), value).unwrap();
    }

    fn installed_tcl() -> PathBuf {
        let path =
            PathBuf::from(std::env::var_os("MOLT_TCL_LIB").expect(
                "fresh-process proof requires MOLT_TCL_LIB naming an installed Tcl library",
            ));
        assert!(
            path.is_absolute(),
            "proof prerequisite must be an absolute file"
        );
        assert!(path.is_file(), "installed Tcl library is missing: {path:?}");
        path
    }

    struct TestDirectories {
        original_cwd: PathBuf,
        startup: PathBuf,
        guest: PathBuf,
    }

    impl TestDirectories {
        fn new() -> Self {
            let root = std::path::absolute(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "molt-tcl-policy-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            std::fs::create_dir(&root).unwrap();
            let startup = root.join("startup");
            let guest = root.join("guest");
            std::fs::create_dir(&startup).unwrap();
            std::fs::create_dir(&guest).unwrap();
            eprintln!(
                "Tcl policy proof scratch (retire after process exit): {}",
                root.display()
            );
            let original_cwd = std::env::current_dir().unwrap();
            std::env::set_current_dir(&startup).unwrap();
            // Match the OS startup basis even when TEMP itself is a symlink.
            let startup = std::env::current_dir().unwrap();
            let guest = startup.parent().unwrap().join("guest");
            Self {
                original_cwd,
                startup,
                guest,
            }
        }
    }

    impl Drop for TestDirectories {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original_cwd).unwrap();
        }
    }

    // Inspect the actual function supplied by the OS loader, not the policy's
    // candidate list or a test replacement for Library::new.
    #[cfg(unix)]
    fn loaded_image(address: *const std::ffi::c_void) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        let mut info = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
        assert_ne!(unsafe { libc::dladdr(address, info.as_mut_ptr()) }, 0);
        let info = unsafe { info.assume_init() };
        assert!(!info.dli_fname.is_null());
        let name = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
        PathBuf::from(OsStr::from_bytes(name.to_bytes()))
    }

    #[cfg(windows)]
    fn loaded_image(address: *const std::ffi::c_void) -> PathBuf {
        use std::os::windows::ffi::OsStringExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            #[link_name = "GetModuleHandleExW"]
            fn module_from_address(
                flags: u32,
                address: *const u16,
                module: *mut *mut std::ffi::c_void,
            ) -> i32;
            #[link_name = "GetModuleFileNameW"]
            fn module_filename(module: *mut std::ffi::c_void, name: *mut u16, capacity: u32)
            -> u32;
        }
        let mut module = std::ptr::null_mut();
        // FROM_ADDRESS | UNCHANGED_REFCOUNT: production already pins the image.
        assert_ne!(
            unsafe { module_from_address(0x00000004 | 0x00000002, address.cast(), &mut module) },
            0
        );
        let mut name = vec![0_u16; 32768];
        let length =
            unsafe { module_filename(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        assert!(
            length > 0 && length < name.len(),
            "module filename must not truncate"
        );
        PathBuf::from(std::ffi::OsString::from_wide(&name[..length]))
    }

    // Exercise the real selected library, not a symbol-shaped fixture. Each
    // count output is checked against literal contents; Tcl 9's wide writes
    // cannot be hidden by a successful loader-only test.
    fn assert_loaded_value_abi(api: &'static crate::tk::tcl::TclApi) {
        use crate::tk::tcl::{TCL_OK, tcl_merge_args, tcl_result_string, tcl_split_list};
        use std::ffi::c_void;
        use std::ptr;

        let executable = super::tcl_find_executable_arg();
        unsafe { api.find_executable(executable.as_ptr()) };
        let interp = unsafe { (api.create_interp)() };
        assert!(!interp.is_null());
        // Tcl_CreateInterp installs core commands without requiring Tk/display
        // or an init.tcl discovery policy unrelated to this ABI witness.
        unsafe {
            // Impossible sizes must fail before the foreign function sees the
            // null payload, for either admitted ABI and without large buffers.
            assert!(api.new_string_obj(ptr::null(), usize::MAX).is_err());
            assert!(api.new_byte_array_obj(ptr::null(), usize::MAX).is_err());
            assert!(api.new_list_obj(usize::MAX, ptr::null()).is_err());
            assert!(api.merge(usize::MAX, ptr::null()).is_err());
            assert!(api.eval_objv(interp, usize::MAX, ptr::null(), 0).is_err());
            let text = api.new_string_obj(b"a\0b".as_ptr().cast(), 3).unwrap();
            assert!(!text.is_null());
            api.incr_ref_count_obj(text);
            let mut length = -1;
            let bytes = api.get_string_from_obj(text, &mut length);
            assert_eq!(length, 3);
            assert_eq!(std::slice::from_raw_parts(bytes.cast::<u8>(), 3), b"a\0b");
            api.decr_ref_count_obj(text);

            let data = api.new_byte_array_obj([0_u8, 255, 42].as_ptr(), 3).unwrap();
            assert!(!data.is_null());
            api.incr_ref_count_obj(data);
            let mut length = -1;
            let bytes = api.get_byte_array_from_obj(data, &mut length);
            assert_eq!(length, 3);
            assert_eq!(std::slice::from_raw_parts(bytes, 3), &[0, 255, 42]);
            api.decr_ref_count_obj(data);

            let empty = api.new_byte_array_obj(ptr::null(), 0).unwrap();
            assert!(!empty.is_null());
            api.incr_ref_count_obj(empty);
            let mut length = -1;
            let bytes = api.get_byte_array_from_obj(empty, &mut length);
            assert!(!bytes.is_null());
            assert_eq!(length, 0);
            api.decr_ref_count_obj(empty);

            // The shared exported boolean wrapper must parse Tcl spellings,
            // including false, and write a full C int on either ABI.
            for (text, expected) in [(c"yes", 1), (c"off", 0)] {
                let value = api.new_string_obj(text.as_ptr(), 3).unwrap();
                assert!(!value.is_null());
                api.incr_ref_count_obj(value);
                let mut truth = -1;
                assert_eq!(
                    (api.get_boolean_from_obj)(interp, value, &mut truth),
                    TCL_OK
                );
                assert_eq!(truth, expected);
                api.decr_ref_count_obj(value);
            }

            let word = api.new_string_obj(c"two words".as_ptr(), 9).unwrap();
            let boolean = (api.new_wide_int_obj)(1);
            let list = api.new_list_obj(2, [word, boolean].as_ptr()).unwrap();
            assert!(!list.is_null());
            api.incr_ref_count_obj(list);
            let list_type = (api.get_obj_type)(c"list".as_ptr());
            assert!(!list_type.is_null());
            assert_eq!(
                (*list.cast::<crate::tk::tcl::TclObjHeader>()).type_ptr,
                list_type
            );
            let mut count = -1;
            let mut elements: *mut *mut c_void = ptr::null_mut();
            assert_eq!(
                api.list_obj_get_elements(interp, list, &mut count, &mut elements),
                TCL_OK
            );
            assert_eq!(count, 2);
            assert_eq!(*elements, word);
            let mut truth = -1;
            assert_eq!(
                (api.get_boolean_from_obj)(interp, *elements.add(1), &mut truth),
                TCL_OK
            );
            assert_eq!(truth, 1);
            let last = api.new_string_obj(c"tail".as_ptr(), 4).unwrap();
            assert_eq!((api.list_obj_append_element)(interp, list, last), TCL_OK);
            let command = api.new_string_obj(c"llength".as_ptr(), 7).unwrap();
            api.incr_ref_count_obj(command);
            assert_eq!(
                api.eval_objv(interp, 2, [command, list].as_ptr(), 0)
                    .unwrap(),
                TCL_OK
            );
            assert_eq!(tcl_result_string(api, interp), "3");
            api.decr_ref_count_obj(command);
            api.decr_ref_count_obj(list);

            let args = vec![
                "two words".to_string(),
                "{braces}".to_string(),
                "".to_string(),
            ];
            let merged = String::from_utf8(tcl_merge_args(api, &args).unwrap()).unwrap();
            assert_eq!(tcl_split_list(api, interp, &merged).unwrap(), args);

            let command = api.new_string_obj(c"error".as_ptr(), 5).unwrap();
            let message = api
                .new_string_obj(c"ABI error witness".as_ptr(), 17)
                .unwrap();
            api.incr_ref_count_obj(command);
            api.incr_ref_count_obj(message);
            assert_eq!(
                api.eval_objv(interp, 2, [command, message].as_ptr(), 0)
                    .unwrap(),
                1
            );
            assert_eq!(tcl_result_string(api, interp), "ABI error witness");
            api.decr_ref_count_obj(command);
            api.decr_ref_count_obj(message);
            (api.delete_interp)(interp);
        }
    }

    fn assert_first_load_is(expected: &Path) {
        let api = load_tcl_api().expect("real Tcl first-use load must succeed");
        let actual = loaded_image(api.create_interp as *const () as *const std::ffi::c_void);
        // Filesystem identity is an independent test oracle only. Production
        // capture never canonicalizes, stats, discovers or loads the path.
        assert_eq!(
            actual.canonicalize().unwrap(),
            expected.canonicalize().unwrap()
        );
        assert_loaded_value_abi(api);
    }

    fn path_load_after_chdir(make_override: impl FnOnce(&Path) -> PathBuf) {
        let source = installed_tcl();
        let dirs = TestDirectories::new();
        let preferred = dirs.startup.join("preferred");
        let selected = preferred.join(source.file_name().unwrap());
        let configured = make_override(&selected);
        set_operator("MOLT_TCL_LIB", Some(configured.as_os_str()));
        set_operator("MOLT_TCL_LIB_DIR", Some(OsStr::new("preferred")));
        crate::initialize_library_policy();
        // Neither the file nor its parent exists at capture: eager loading,
        // canonicalization and filesystem discovery would reject this case.
        std::fs::create_dir(&preferred).unwrap();
        std::fs::copy(&source, &selected).unwrap();
        std::env::set_current_dir(&dirs.guest).unwrap();
        set_operator("MOLT_TCL_LIB", Some(OsStr::new("./guest-selected-library")));
        set_operator(
            "MOLT_TCL_LIB_DIR",
            Some(OsStr::new("guest-selected-directory")),
        );
        crate::initialize_library_policy();
        assert_first_load_is(&selected);

        // MOLT_TCL_LIB_DIR follows system defaults. This assertion proves its
        // captured route/order, not that a system Tcl loaded from this directory.
        let candidates = tcl_library_candidates().unwrap();
        assert!(candidates[0].is_absolute());
        assert_eq!(
            candidates[0].canonicalize().unwrap(),
            selected.canonicalize().unwrap()
        );
        let directory_start = candidates
            .iter()
            .skip(1)
            .position(|path| path.parent() == Some(preferred.as_path()))
            .map(|position| position + 1)
            .expect("captured directory candidates");
        assert!(
            candidates[directory_start..]
                .iter()
                .all(|path| path.parent() == Some(preferred.as_path()))
        );
        assert!(!candidates.iter().any(|path| path.starts_with(&dirs.guest)));
    }

    #[test]
    #[ignore = "requires installed Tcl and fresh process with guarded scratch; owns CWD and loader OnceLocks"]
    fn relative_library_is_loaded_from_startup_after_guest_chdir() {
        path_load_after_chdir(|selected| {
            Path::new(".")
                .join("preferred")
                .join(selected.file_name().unwrap())
        });
    }

    #[test]
    #[ignore = "requires installed Tcl and fresh process with guarded scratch; owns CWD and loader OnceLocks"]
    fn absolute_library_is_unchanged_after_guest_chdir() {
        path_load_after_chdir(Path::to_path_buf);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires installed Tcl and fresh Windows process with guarded scratch; owns CWD and loader OnceLocks"]
    fn rooted_library_keeps_startup_drive() {
        path_load_after_chdir(|selected| {
            let rooted: PathBuf = selected.components().skip(1).collect();
            assert!(rooted.has_root() && !rooted.is_absolute());
            rooted
        });
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires installed Tcl and fresh Windows process with guarded scratch; owns CWD and loader OnceLocks"]
    fn drive_relative_library_keeps_startup_directory() {
        path_load_after_chdir(|selected| {
            let std::path::Component::Prefix(prefix) = selected.components().next().unwrap() else {
                panic!("native Windows fixture must have a drive prefix");
            };
            let drive = match prefix.kind() {
                std::path::Prefix::Disk(drive) | std::path::Prefix::VerbatimDisk(drive) => drive,
                _ => panic!("drive-relative proof requires a local-drive scratch directory"),
            };
            PathBuf::from(format!("{}:preferred", char::from(drive)))
                .join(selected.file_name().unwrap())
        });
    }

    #[test]
    #[ignore = "requires installed Tcl discoverable by bare loader name and a fresh process with guarded scratch"]
    fn bare_library_name_preserves_native_loader_search() {
        let source = installed_tcl();
        let bare = source.file_name().unwrap();
        let dirs = TestDirectories::new();
        set_operator("MOLT_TCL_LIB", Some(bare));
        set_operator("MOLT_TCL_LIB_DIR", None);
        crate::initialize_library_policy();
        std::env::set_current_dir(&dirs.guest).unwrap();
        assert_first_load_is(&source);
        assert_eq!(
            tcl_library_candidates().unwrap()[0].as_path(),
            Path::new(bare)
        );
    }

    #[cfg(unix)]
    fn unavailable_startup_basis_is_retained(setting: &str) {
        let source = installed_tcl();
        let dirs = TestDirectories::new();
        set_operator("MOLT_TCL_LIB", None);
        set_operator("MOLT_TCL_LIB_DIR", None);
        set_operator(setting, Some(OsStr::new("./relative-operator-path")));
        std::fs::remove_dir(&dirs.startup).unwrap();
        crate::initialize_library_policy();
        std::env::set_current_dir(&dirs.guest).unwrap();
        // Even a valid late override must not clear the startup failure or load
        // a fallback. This exercises the actual first-use API after CWD recovery.
        set_operator("MOLT_TCL_LIB", Some(source.as_os_str()));
        set_operator("MOLT_TCL_LIB_DIR", None);
        crate::initialize_library_policy();
        let error = match load_tcl_api() {
            Ok(_) => panic!("startup path capture failure admitted a library"),
            Err(error) => error,
        };
        assert!(
            error.starts_with(&format!("cannot capture startup {setting} path:")),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires installed Tcl and fresh process with guarded scratch; removes its CWD before init"]
    fn library_capture_error_survives_until_first_use() {
        unavailable_startup_basis_is_retained("MOLT_TCL_LIB");
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires installed Tcl and fresh process with guarded scratch; removes its CWD before init"]
    fn directory_capture_error_blocks_first_use_before_fallback() {
        unavailable_startup_basis_is_retained("MOLT_TCL_LIB_DIR");
    }
}
