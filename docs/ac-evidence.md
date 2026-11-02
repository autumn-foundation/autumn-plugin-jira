# Acceptance criteria: evidence

This file lists each acceptance criterion (AC) of [plan.md](plan.md)
section 16, and each example criterion (EX1 to EX8) of
[examples-plan.md](examples-plan.md), with its status and its evidence. It
also records the spikes, the Verus result, the TDD steps, the review
rounds and the mutation tests.

Status: final, after review round 2. Wave 4 (WP7 ecosystem, WP8a guards
and README) is done. Each row below names its tests. The mutation table is
the one open item: see "Mutation tests".

Paths: `core/` is `crates/autumn-jira`, `fake/` is
`crates/autumn-jira-fake`, `plugin/` is `crates/autumn-plugin-jira`,
`harvest/` is `crates/autumn-jira-harvest`, `camel/` is
`crates/autumn-jira-camel`, `eco/` is `examples/ecosystem`. `t/` is a
`tests/` folder. `u:` is a unit test in `src/`. `ex:` is a test in an
example file. `eco u:` is a test in `examples/ecosystem/src`. "suite cases"
are cases in `core/t/common/suite.rs`; the three tests of AC5 run each of
them. The guard `evidence_names_real_tests` (AC40) checks that each name in
a column with "test" in its header is a `fn` in the named file or module.

## Summary

| Item | Result |
|---|---|
| ACs done | 40 of 40 done. |
| Examples | EX1 to EX8 done. |
| Tests | After review round 2: 517 pass, 0 fail, 18 ignored in the main workspace (`cargo test --workspace --all-features --locked --no-fail-fast`, with Postgres, 41 test binaries and doc-test runs). The ignored ones are the five live cases in `core/t/live.rs`, which never run in CI, the boot child `boot_child_server`, which runs only as a child process, and 12 doc snippets marked `ignore`. The ecosystem workspace: 19 pass, 0 fail, 1 ignored (`cargo test --locked --manifest-path examples/ecosystem/Cargo.toml`, with Postgres). |
| Line coverage (`src/`) | `autumn-jira` 97.63%, `autumn-jira-fake` 98.96%, `autumn-plugin-jira` 94.97%, `autumn-jira-harvest` 96.36%, `autumn-jira-camel` 97.02% (the wave reports; `cargo llvm-cov`, tests and examples left out). The CI gate is 85% for each crate. |
| Verus | `verus/policy.rs`: 31 verified, 0 errors (CI run 41422883903, job `verus` 125202423323, head `8c278e7`). |
| MSRV | 1.89: `cargo +1.89.0 check --workspace --all-targets --all-features --locked` passed in CI (run 41422883903, job `msrv` 125202423505, head `8c278e7`). Verified in CI only (plan R4). |
| CI | Run 41422883903 on head `8c278e7`: all 7 jobs pass. The jobs are `lint`, `test` (with coverage), `examples`, `msrv`, `verus`, `deps` and `example-ecosystem` (job 125202423939). This record has no CI run of a later head. After review round 2, the local `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` and both test commands pass. |

## Acceptance criteria: library

| AC | Status | Tests and evidence |
|---|---|---|
| AC1 Newtypes | Done | u: `ids::{strict_rules_table, deserialize_is_strict, display_round_trip_proptest, no_panic_proptest, issue_key_parts, issue_ref_picks_by_shape, idempotency_markers}`; u: `model::response_ids_are_lenient`; `core/t/pure.rs::{idempotency_key_derive_is_stable, root_re_exports_work}` |
| AC2 Fixed hosts, no redirects | Done (WP1, WP3). The guard is done (WP8a). | `core/t/client.rs::{a_redirect_is_not_followed, path_segments_are_encoded}`; u: `site::{base_urls, site_name_rejects_hosts}`; `core/t/guards.rs::{one_base_url_builder, one_base_url_builder_sees_a_second_builder}` |
| AC3 API token auth | Done | `core/t/client.rs::{basic_header_is_sent, unauthorized_is_not_retried, captcha_is_typed, debug_has_no_token}`; u: `backend::direct::{transport_error_map, debug_hides_the_token, one_attempt_with_basic_auth}`; u: `site::{basic_auth_rules, debug_hides_the_email_and_the_token}` |
| AC4 san-mateo backend | Done | u: `backend::san_mateo::{error_map_covers_every_variant, prefix_by_provider}`; u: `backend::{resolve_cloud_id_picks_jira, resolve_cloud_id_needs_a_site_when_many, requests_and_replies}`; `core/t/san_mateo.rs::{discovery_stores_the_cloud_id, metadata_cloud_id_skips_discovery, one_send_for_a_503_and_for_a_429, a_401_refreshes_once, backend_errors_are_typed}`; `core/t/san_mateo.rs::{a_reconnect_after_confirmation_is_seen, a_reconnect_after_discovery_is_seen}` (round 2) |
| AC5 One suite, three setups | Done. 20 suite cases on each setup. The OAuth setup expires the first token, so each case does one refresh and one cloud id discovery. | `core/t/conformance.rs::{suite_on_api_token, suite_on_san_mateo_basic, suite_on_san_mateo_oauth}` |
| AC6 Issue CRUD | Done | suite cases `{create_get_edit_delete, notify_users_only_when_set, delete_gone_after_retry, properties_round_trip, myself_and_server_info, projects_and_one_project}`; `core/t/client.rs::{moved_issue_key_is_kept, empty_204_is_not_parsed_and_statuses_are_checked}`; u: `error::op_expected_table` |
| AC7 Lists | Done (WP3). The guard is done (WP8a). | suite cases `{search_pages_to_the_end, search_stalls_on_a_repeated_token, search_page_limit, comments_offset_pages, projects_list_all_pages, issue_types_of_a_project, approximate_count}`; `core/t/client.rs::{short_non_last_comment_page, one_search_page_at_a_token}`; `core/t/guards.rs::{no_legacy_search_path, no_legacy_search_path_sees_the_old_path}` |
| AC8 Transitions | Done | suite cases `{transition_by_category, already_there_sends_no_post, no_transition_lists_names, done_needs_resolution_fields}` |
| AC9 Comments | Done | suite cases `{add_and_list_comments, all_comments_is_bounded, idempotent_writes_round_trip}` |
| AC10 JQL builder | Done | u: `jql::{builds_clauses, injection_stays_one_literal, control_chars_rejected, field_names_validated, raw_text_is_wrapped_before_and}`; u: `policy::spec_as_test_jql_quote_round_trip` |
| AC11 ADF | Done | u: `adf::{text_splits_lines, no_empty_text_nodes, builder_nodes, plain_text_round_trip_proptest, links_need_https, from_value_checks_the_root, plain_text_of_unknown_and_inline_nodes}` |
| AC12 Timestamps | Done | u: `time::{parse_table, epoch_round_trip_and_serde}` |
| AC13 Typed errors | Done | `core/t/errors.rs::{status_table_over_memory_transport, display_has_no_secret_or_query}`; u: `error::{class_table_covers_every_variant, codes_are_unique, accessors, display_holds_no_messages, may_have_run_table}`; u: `model::decode_errors_hold_no_body_value`; u: `client::call::{classify_table, retry_after_reads_integer_seconds_only}` |
| AC14 Retry policy | Done. Each test runs with `start_paused`. | `core/t/retry.rs::{reads_retry_server_errors, retry_after_is_the_floor, retry_after_above_the_cap_gives_up, beta_retry_after_is_ignored, jitter_uses_entropy, not_sent_transient_is_retried}` |
| AC15 Unsafe writes | Done | `core/t/retry.rs::{plain_create_is_sent_once_after_503, plain_create_is_sent_once_after_a_reset, plain_create_is_not_retried_after_429, plain_transition_is_sent_once}`; `core/t/retry.rs::unsafe_writes_send_once_table` (round 1) |
| AC16 Idempotent create | Done. The chaos proptest runs 64 cases on a paused current-thread runtime. | `core/t/idempotency.rs::{marker_goes_in_the_create_body, prelookup_hit_sends_no_post, lost_reply_is_found, lag_beyond_lookups_is_outcome_unknown, label_only_hit_is_skipped, two_hits_pick_the_lowest_id, failed_lookup_is_not_not_found, labels_not_on_screen_has_a_fix, chaos_at_most_one_issue_per_key}`; `core/t/idempotency.rs::{failed_prelookup_sends_no_post, marked_write_with_an_unexpected_2xx_looks_up_the_marker, a_bad_label_value_is_a_validation_error}` (round 1) |
| AC17 Idempotent comment, checked transition | Done | `core/t/idempotency.rs::{lost_comment_reply_is_found, transition_after_lost_reply_is_already_there}`; `core/t/idempotency.rs::{comment_lookup_finds_marker_beyond_100, a_marker_from_another_author_is_not_found, failed_comment_lookup_after_a_lost_reply_is_outcome_unknown, transition_after_maybe_sent_stops_when_the_issue_moved_on, global_transition_after_maybe_sent_is_not_sent_again, transition_400_after_lost_reply_reads_the_status}` (round 1); `core/t/idempotency.rs::{transition_maybe_sent_on_the_last_attempt_is_outcome_unknown, a_rate_limit_after_a_maybe_sent_attempt_is_outcome_unknown, a_failed_status_read_after_a_maybe_sent_attempt_is_outcome_unknown, a_resumed_transition_at_the_target_is_already_there, a_resumed_transition_off_the_target_sends_nothing, transition_400_after_maybe_sent_when_moved_on_is_outcome_unknown, transition_with_no_is_global_after_maybe_sent_is_not_resent, comment_lookup_survives_a_delete_between_pages, a_first_attempt_comments_on_an_issue_with_many_comments}` (round 2) |
| AC18 Verified core | Done. CI job `verus`: 31 verified, 0 errors (see "Verus"). | u: `policy::{verus_bodies_match, every_const_fn_is_proved_or_listed, the_drift_check_sees_changes, backoff_matches_san_mateo, spec_as_test_decide_no_resend_after_maybe_sent, spec_as_test_decide_attempt_bound, spec_as_test_decide_done_and_stop, spec_as_test_attempt_loop_is_bounded, spec_as_test_classify_status, spec_as_test_backoff, spec_as_test_retry_wait, spec_as_test_scale_permille, spec_as_test_jitter_bounds, spec_as_test_host_may_send, spec_as_test_token_page_step, spec_as_test_token_pager_terminates, spec_as_test_offset_next, spec_as_test_freshness, spec_as_test_webhook_status, spec_as_test_is_lower_hex, spec_as_test_signature_shape, spec_as_test_jql_quote_round_trip}`; CI job `verus`; u: `policy::{each_fn_is_in_exactly_one_list, the_spec_has_no_proof_escape, spec_as_test_scale_permille_in_range, spec_as_test_scale_permille_out_of_range, decide_examples}` (round 1); u: `policy::{the_spec_has_no_proof_escape, the_drift_check_sees_changes}` (round 2) |
| AC19 Webhook types | Done (WP1). The guard is done (WP8a). The code compares MAC bytes only with `Mac::verify_slice`. | u: `webhook::{parses_all_events, unknown_event_is_other, reduced_issue_parses, changelog_id_string_or_number, status_change_from_changelog, atlassian_test_vector, rejects_bad_shapes, flow_and_delivery}`; `core/t/webhook.rs::{signed_delivery_reaches_a_sink, header_names_are_pinned}`; `core/t/guards.rs::{no_eq_on_mac_bytes, no_eq_on_mac_bytes_sees_an_eq}`; u: `webhook::{v2_string_comment_body_is_read, issue_commented_update_keeps_the_comment, comment_actor_follows_the_event, only_a_full_last_line_marker_counts}` (round 1); `core/t/webhook.rs::{a_delivery_keeps_the_host_of_its_site, a_site_host_has_a_strict_rule, each_fake_delivery_names_its_site}` (round 2) |
| AC20 Event seam | Done | u: `sink::{handler_error_from_jira_error, default_sink_name}`; u: `sink::handler_error_from_jira_error` (round 2) |
| AC21 No secrets in logs | Done (WP3). The guard is done (WP8a). The log test passed at RED: it guards code that already existed. | `core/t/logs.rs::no_secret_or_body_in_logs`; `core/t/guards.rs::{instrument_skips_all, instrument_skips_all_sees_a_skip_list}` |
| AC22 FakeJira behaviour | Done (WP2). The guard is done (WP8a). The fake `Cargo.toml` has no `autumn-jira` dependency. | `fake/t/fake.rs::{every_endpoint, jql_subset, auth_rules, oauth_refresh_rotates, accessible_resources, faults_table, index_lag_in_virtual_time, workflow_needs_resolution, deliveries_are_signed, own_hmac_matches_the_vector, fallback_serves_other_hosts, snapshot_shape, edit_paths_and_validation, delayed_writes_on_other_ops, no_secret_in_debug_or_logs}`; u: `wire::{urls_split, decoding, auth_headers}`; u: `jql::lexer_and_parser_edges`; `core/t/guards.rs::{fake_does_not_depend_on_the_client, fake_does_not_depend_on_the_client_sees_a_dependency}` |
| AC23 FakeJira matches the spec | Done. The test walks more than 100 objects. A mutation in a scratch clone (an unknown key and a wrong type in Project) gave 10 shape errors. | `fake/t/spec_shapes.rs::fake_replies_match_the_spec_shapes` |
| AC24 Plugin install | Done | `plugin/t/install.rs::{plugin_installs_with_one_call, name_contract_and_config_section, plugin_passes_the_autumn_conformance_checks, webhook_route_is_public_and_attributed, signed_delivery_reaches_the_handler_not_500, duplicate_plugin_is_a_no_op}`; u: `plugin::{contract_matches_cargo_toml, prefix_and_webhook_path}`; `plugin/t/spike_mount.rs::nest_strips_the_prefix_and_breaks_the_lookup`; u: `plugin::contract_is_compatible` (round 1) |
| AC25 Plugin config | Done | u: `config::{layers, mode_from_table, both_tables_fail, token_key_names_token_env, debug_hides_values, validation_rules}`; `plugin/t/lifecycle.rs::{client_and_mode_table_conflict, transport_in_san_mateo_mode_conflicts}`; u: `config::the_timeout_cap_applies_in_api_token_mode_only` (round 2) |
| AC26 Service and lifecycle | Done | `plugin/t/lifecycle.rs::{service_is_503_before_start, startup_makes_no_network_call, san_mateo_missing_has_a_fix, san_mateo_after_jira_has_a_fix, for_connection_caches_clients, handler_uses_the_service, a_missing_secret_names_the_variable, autumn_error_keeps_the_status_and_hides_internal_text, the_runtime_and_the_role, debug_hides_secret_values}`; u: `secrets::overrides_win_and_empty_values_count_as_missing`; `plugin/t/lifecycle.rs::{san_mateo_startup_makes_no_network_call, the_configured_cloud_id_is_for_the_default_connection_only, a_prebuilt_client_with_a_jira_http_table_conflicts}`; u: `service::{the_cache_has_a_bound_and_drops_the_least_recently_used, forget_drops_one_client}` (round 1) |
| AC27 Health | Done | `plugin/t/health.rs::{health_only_group, cached_for_cache_secs, jira_down_is_down_not_unready, details_have_no_pii}`; u: `health::down_before_start_and_after_a_failed_start`; `plugin/t/health.rs::no_default_connection_is_not_down` (round 1) |
| AC28 Webhook boot check | Done | `plugin/t/webhook_config.rs::{each_wrong_field_fails_with_the_fix, small_body_cap_only_warns, helper_endpoint_passes, worker_role_skips_the_check, replay_off_needs_the_opt_in, a_bad_jira_config_fails_the_start}`; `plugin/t/lifecycle.rs::the_runtime_and_the_role`; `plugin/t/webhook_config.rs::{max_age_and_skew_stay_inside_the_replay_window, a_max_age_below_the_jira_retry_horizon_warns, max_future_skew_has_the_policy_bound, the_fix_never_prints_a_secret_env_value_that_is_not_a_name}` (round 1) |
| AC29 Webhook route | Done | `plugin/t/webhook.rs::{signed_is_200, missing_signature_is_400, bad_signature_is_401, duplicate_is_409_without_dispatch, ignored_cases_are_200_and_counted, transient_is_503_then_redelivery_succeeds, timeout_is_503, permanent_is_200, two_mib_body_passes, over_cap_is_400, csrf_on_still_passes, the_route_reads_the_retry_and_flow_headers, a_fake_delivery_reaches_the_sink, deliveries_in_one_millisecond_are_all_accepted}`; `plugin/t/webhook.rs::{hung_handlers_have_a_bound, a_hung_task_stops_at_abandon_after_ms, a_request_timeout_still_counts_the_delivery, a_sink_for_all_actors_gets_the_ignored_actor, a_panicking_sink_is_503}` (round 1); `plugin/t/webhook.rs::a_full_slot_starts_no_task`; u: `runtime::the_slot_count_stays_at_the_semaphore_cap` (round 2) |
| AC30 Dispatch | Done (WP4). The guard is done (WP8a). | `plugin/t/webhook.rs::{sinks_run_in_order, timed_out_sink_still_finishes, transient_stops_later_sinks}`; `core/t/guards.rs::{no_exactly_once_claim, no_exactly_once_claim_sees_a_claim}` |
| AC31 Testing helpers | Done | `plugin/t/testing.rs::app_handler_test_in_ten_lines` |
| AC32 Harvest activities | Done | `harvest/t/activities.rs::{names_are_pinned, client_ref_named_and_connection, failure_for_table, lost_create_is_pending_then_found, maybe_sent_attempt_looks_up_only, opt_in_resends_after_negative_lookup, missing_state_is_config, key_is_stable_across_attempts, start_to_close_covers_the_client_budget, no_secret_in_outputs_or_history, handler_call_parses_typed_failure, every_activity_without_state_is_config, create_with_a_route_tracks_the_issue_in_one_call, reads_and_idempotent_writes, outcome_codes_have_their_constants}`; u: `failure::previous_attempt_table`; u: `clients::serde_shape_is_tagged`; u: `activities::get_issue_input_shape`; `harvest/t/activities.rs::{a_failed_pre_lookup_is_not_sent, a_delete_404_after_any_failed_attempt_is_gone}` (round 1); `harvest/t/activities.rs::{add_comment_on_an_issue_with_301_comments_works_on_attempt_1, a_transition_after_a_maybe_sent_attempt_sends_nothing}` (round 2) |
| AC33 Durable wait | Done (WP5). The guard is done (WP8a). Spike S2 passes on 24 seeds. | `harvest/t/wait.rs::{already_there_returns_at_once, signal_ends_the_wait, poll_ends_the_wait_without_webhooks, ignored_signals_do_not_extend_the_deadline, other_issue_and_old_timestamp_are_skipped, timeout_over_max_wait_fails, every_case_replays, the_last_read_at_the_deadline_can_reach_the_target, a_late_wait_keeps_its_full_timeout, a_failed_read_keeps_its_error_type}`; u: `wait::{no_ctx_now_in_src, helpers}`; `core/t/guards.rs::{no_ctx_now_in_harvest, no_ctx_now_in_harvest_sees_ctx_now}`; `harvest/t/wait.rs::{a_failed_poll_is_a_missed_poll, a_failed_last_read_times_out, a_signal_older_than_a_read_is_skipped, too_many_poll_rounds_fail_at_once, the_round_bound_fits_the_history_cap, two_waits_at_the_same_time_run_one_after_the_other, a_signal_after_the_wait_needs_the_drain}`; u: `wait::the_deadline_fits_the_run` (round 1); `harvest/t/wait.rs::{a_wait_past_the_signal_cap_still_polls, missed_polls_fit_the_history_cap, one_poll_round_over_the_bound_fails}`; u: `wait::{the_signal_cap_falls_back_to_polls, the_wait_reads_do_not_wait_for_a_retry_after, a_poll_at_the_deadline_is_the_last_read}` (round 2) |
| AC34 Signal bridge | Done. With no database URL, a Postgres test asserts that `CI` is not set before it skips. | u: `bridge::{wants_only_status_changes, routes_need_tag_and_allowlist, tag_is_constant_time, harvest_errors_map_to_classes}`; u: `route::{route_key_needs_32_bytes_and_hides_itself, route_parts_follow_the_rule, serde_checks_the_rule, tag_is_hmac_of_the_versioned_input, verify_rejects_bad_tags, property_entry_has_the_tag, hex_helpers_round_trip}`; `harvest/t/bridge_pg.rs::{duplicate_delivery_gives_one_signal, missing_run_is_skipped, finished_run_is_skipped, db_error_is_transient, forged_tag_makes_no_db_call}`; `harvest/t/bridge_pg.rs::a_delivery_of_another_site_gives_the_status_of_this_site` (round 1); `harvest/t/bridge_pg.rs::{a_delivery_of_another_site_is_refused, a_copied_route_does_not_signal, a_late_delivery_of_this_site_keeps_its_reported_status}`; u: `route::v1_tags_verify_only_as_v1` (round 2) |
| AC35 Watch an issue | Done | `harvest/t/activities.rs::watch_is_idempotent`; `harvest/t/wait.rs::wait_on_a_human_issue` |
| AC36 camel component | Done | `camel/t/route.rs::{create_get_edit_transition_comment_search, consumer_is_rejected, unknown_option_lists_names, project_and_jql_conflict, override_header_without_flag_fails, override_header_with_flag_works, errors_hold_no_body, a_named_connection_picks_its_client}`; u: `options::{uri_options_match_the_parser, unknown_option_lists_names, project_and_jql_conflict, defaults, parses_all_options_of_each_operation, operations_match_the_constant, option_of_another_operation_fails, the_first_error_is_stable}`; u: `input::{read_takes_json_text_and_bytes, create_reads_each_key, edit_needs_one_change, transition_takes_nothing_or_fields, comment_takes_text_or_a_string}`; u: `headers::{marked_reads_the_key_and_the_attempt, a_header_equal_to_the_option_passes}`; `camel/t/route.rs::the_out_header_of_an_earlier_step_is_not_an_override` (round 1) |
| AC37 camel idempotency and errors | Done | `camel/t/route.rs::{key_header_uses_the_idempotent_path, retry_attempt_looks_up_only, opt_in_resends}`; `camel/t/retry_rule.rs::{map_keeps_the_retry_class_for_every_error, the_samples_cover_every_code, each_camel_kind_gives_its_camel_variant, messages_hold_the_code_and_no_body, outcome_unknown_with_a_key_is_io_and_without_is_validation, http_errors_have_the_plan_shape}`; u: `error::{http_errors_name_the_method_of_the_failed_call, reason_phrases}`; `camel/t/route.rs::{camel_redelivery_of_marked_create_looks_up_only, camel_redelivery_never_resends_unmarked_writes, retry_attempt_lookup_waits_for_index_lag}` (round 1); `camel/t/route.rs::{redelivered_transition_does_not_resend_after_the_issue_moved_on, a_harvest_retry_of_a_transition_reads_only}` (round 2) |
| AC38 Quality gates | Done. Clippy pedantic and nursery `-D warnings` are clean after review round 2. MSRV passed in CI. Coverage of each crate is over 85% (see "Summary"). | CI jobs `lint`, `test`, `msrv`; `core/t/guards.rs::{no_unwrap_or_todo_in_src, no_unwrap_or_todo_in_src_sees_each_kind, every_allow_has_a_reason, every_allow_has_a_reason_sees_a_bare_allow, no_continue_on_error, no_continue_on_error_sees_hidden_failures, no_session_paths, no_session_paths_sees_one}` |
| AC39 Dependency rules | Done | CI job `deps`; `core/t/guards.rs::{root_lock_is_free_of_c_builds, root_lock_is_free_of_c_builds_sees_one, ecosystem_lints_equal_the_root, the_lint_compare_sees_a_change, weekly_updates_get_the_one_version_check, weekly_updates_get_the_one_version_check_sees_a_gap, published_crates_have_metadata, published_crates_have_metadata_sees_a_gap}` |
| AC40 Docs | Done. This file, `docs/plan.md` (design v3, as built) and `docs/examples-plan.md` are final. The review angle R7 ran in round 1, and R2E checked the guards in round 2. | `core/t/guards.rs::{readme_writes_table_equals_writes, the_writes_compare_sees_a_change, evidence_names_real_tests, the_evidence_check_sees_a_missing_test, adr_files_exist, adr_files_exist_sees_a_gap, the_lexer_drops_comments_and_test_items, docs_are_ste_prose, docs_are_ste_prose_sees_a_long_sentence, docs_make_no_stale_claims, docs_make_no_stale_claims_sees_each_claim, adrs_cite_real_doc_text, adrs_cite_real_doc_text_sees_a_missing_quote, adr_0005_names_each_policy_fn, adr_0005_names_each_policy_fn_sees_a_gap, doc_comments_are_ste_prose, doc_comments_are_ste_prose_sees_each_kind, adrs_cite_real_doc_text_reads_no_adr, ex_status_agrees, ex_status_agrees_sees_a_gap}`; u: `writes::{writes_table, writes_match_the_markers}` |

## Acceptance criteria: examples

| EX | Status | Tests and evidence |
|---|---|---|
| EX1 `embedded` | Done. `cargo run --locked -p autumn-jira --example embedded` prints the summary and `check: ok`. | ex: `embedded::{the_example_runs_and_checks, a_bad_field_is_a_validation_error, a_wrong_value_fails_the_check}`; CI step "example: embedded" in `.github/workflows/ci.yml` |
| EX2 `autumn_app` | Done. `scripts/smoke-autumn-example.sh` passed locally: dev and prod, `GET /issues/OPS-1` 200, 404, 400, a signed openssl-HMAC POST 200, the copy 409, a bad signature 401, `jira` in `/actuator/health` in both profiles, SIGTERM exit 0, no secret in the log, and a missing `JIRA_API_TOKEN` exits non-zero and names it. | ex: `autumn_app::{the_app_serves_issues_and_webhooks, a_wrong_value_fails_the_check, the_demo_fake_has_the_issue}`; `plugin/t/boot.rs::{missing_secret_exits_and_names_the_variable, boot_child_server, the_app_boots_and_takes_a_signed_post}`; `scripts/smoke-autumn-example.sh`; ex: `autumn_app::the_selector_selects_the_profile_of_the_real_path` (round 2) |
| EX3 `harvest_workflow` | Done. `cargo run --example harvest_workflow` on Postgres gave `check: ok`. Two copies at the same time, in two rounds: all four runs exited 0. | ex: `harvest_workflow::{lifecycle_resolves_by_webhook, watch_resolves_by_poll, a_400_fails_with_one_post, a_wrong_value_fails_the_check}`; CI steps "example: harvest_workflow" and "two runs at the same time" |
| EX4 `camel_route` | Done. `cargo run --locked -p autumn-jira-camel --example camel_route` prints the summary and exits 0. | ex: `camel_route::{the_route_creates_and_moves, unknown_option_fails_at_route_add, a_wrong_value_fails_the_check}`; CI step "example: camel_route" |
| EX5 Ecosystem happy path | Done (WP7: RED `c439a8f`, GREEN `46da54b`). `cargo run --locked` of the ecosystem prints the nine timeline lines and `check: ok`. | eco u: `tests::happy_path_timeline_is_exact`; CI job `example-ecosystem` |
| EX6 Ecosystem failures | Done (WP7) | eco u: `tests::{lost_create_fails_closed, bad_webhook_secret_resolves_by_poll, no_webhook_no_poll_times_out, a_wrong_value_fails_the_check, the_line_format_has_an_eight_wide_tag}`; eco u: `tests::{a_run_timeout_still_stops_everything, no_worker_records_no_worker_stop}` (rounds 1 and 2) |
| EX7 Ecosystem edges | Done (WP7). The bot copy goes to the `sink_for_all_actors` bridge: the bridge gets 2 deliveries, and `ignored_self` is 0. | eco u: `tests::{webhook_negatives, two_tenants_stay_apart, is_retryable_copy_matches_harvest_camel, boot_three_plugins_real_http}` |
| EX8 Examples wired | Done (WP8a: RED `9250bd7`, GREEN `ad9dfb3`). | `core/t/examples_wired.rs::{each_example_is_wired, no_example_turns_on_the_dev_only_proxy_flags, each_example_file_is_in_the_list, the_ecosystem_workspace_stays_apart, readme_snippets_copy_their_examples, docs_give_the_ci_commands_of_the_separate_workspace, a_comment_or_a_string_is_not_code, a_time_bound_in_the_tests_is_for_a_server_only, a_step_name_or_an_if_is_not_a_run, a_readme_command_needs_its_env_vars, a_snippet_must_copy_its_example, a_doc_must_give_the_ci_commands}` |

## Spikes

| Spike | Result | Test |
|---|---|---|
| S1 webhook mount | PASS. A route merged at the full path: 200 for a good signature, 401 for a bad one. With `nest`: 500 (`EndpointMissing`), because `nest` strips the prefix and `SignedWebhook` looks up the endpoint by the full path. Commits `016d549`, `8c75f73`, `97166de`. | `plugin/t/install.rs::signed_delivery_reaches_the_handler_not_500`; `plugin/t/spike_mount.rs::nest_strips_the_prefix_and_breaks_the_lookup` |
| S2 durable wait | PASS. 24 seeds. Ten ignored signals land at seeded times, some after the 60 s poll. Each seed gives `TimedOut` at exactly 120 s. The deadline tests run in the seeded `Simulation`; "advancing clock off" is `WorkflowReplayer::new()` (plan 9.3, V43). | `harvest/t/wait.rs::ignored_signals_do_not_extend_the_deadline` |
| S3 ecosystem graph | PASS. The app with `SanMateoPlugin`, `HarvestPlugin` and `JiraPlugin` boots with `run()` and takes one signed POST over real HTTP. | eco u: `tests::boot_three_plugins_real_http` |
| S4 Verus in CI | PASS. Run 41382418046 (head `8e3f4a7`): Verus builds, `vstd` 2060 verified; the job then failed only on the missing spec file, as expected for the spike. | CI job `verus` |
| S5 one attempt | PASS. The san-mateo part passed at RED. The backend part failed at RED, then passed at GREEN (`ef41c71`, `4a6364d`). | `core/t/san_mateo.rs::one_send_for_a_503_and_for_a_429` |

## Verus

- `verus verus/policy.rs`: "verification results:: 31 verified, 0 errors".
  CI run 41422883903, job 125202423323, head `8c278e7`.
- The proofs have no `assume` and no `admit`. The proof style copies the
  san-mateo `verus/policy.rs`. The `nonlinear_arith` form of
  `lemma_scale_fits` copies autumn-harvest `formal`.
- `scale_permille` has proof code in its Verus body, so a proptest
  (`spec_as_test_scale_permille`) checks it in place of a body match
  (plan V4).
- Local stand-in: one `spec_as_test_*` proptest for each `ensures` clause
  (AC18).

## TDD record

Each step has a RED commit (failing tests), a GREEN commit (tests pass)
and, when needed, a REFACTOR commit. A "RED/GREEN" commit adds tests for
code that already existed; they passed at once, and the commit message
says so.

| WP | Step | RED | GREEN | REFACTOR |
|---|---|---|---|---|
| WP1 Core pure | policy spec and proptests | `c8aceab` | `623a263` | `0ae4975` |
| WP1 | ids, site, time, error class table | `20754cc` | `bc0a64e` | `8615b98` |
| WP1 | adf and jql | `6c76be5` | `b42f342` | |
| WP1 | model, wire types, request types | `1f92c56` | `28ea658` | `5549eb8` |
| WP1 | webhook, sink, writes | `33bd706` | `40f159a` | |
| WP2 Fake | sign (Atlassian vector) | `20de2db` | `ff50bfa` | |
| WP2 | router, auth, issues, workflow, comments, JQL, faults, lag, OAuth, deliveries, snapshot | `762c6da` | `d08b700`, `76c965d` | `1bb2f5f` |
| WP2 | spec shape test (AC23) | `b430dbe` (RED/GREEN) | `b430dbe` | |
| WP4 P0 Plugin | spike S1, config, webhook boot check | `016d549` | `8c75f73` | `97166de` |
| WP3 Client | spike S5 and the backend seam | `ef41c71` | `4a6364d` | |
| WP3 | reads, retries, errors, writes, pagers, conformance suite | `aa2456b` | `2014c21` | `8949ed7` |
| WP3 | comment actor, logs, live cases, `embedded` | `01090df` | `841d361` | |
| WP5 H0 Harvest | spike S2 and the durable wait | `60a7617` | `9b6aa9d` | `87e670b` |
| WP5 H0 | `failure_for` and `previous_attempt` | `218f863` | `397b538` | |
| WP5 H0 | route tags and the pure bridge parts | `5335407` | `543631f` | |
| WP5 H0 | `JiraClients` and `client_for` | `1eca814` | `29ce234` | |
| WP5 H0 | a late wait keeps its full timeout (mutation M8) | `3074d96` (RED/GREEN) | `3074d96` | |
| WP6 K0 Camel | URI options, error map, pinned retry rule | `66652ff` | `136f813` | `b92993c` |
| WP4 P1 Plugin | service and lifecycle | `68da7f6` | `0921baa` | `aa06768` |
| WP4 P1 | health indicator | `4a43120` | `41cfbc0` | |
| WP4 P2 | route pipeline and dispatch | `3d3bc63` | `81aacd1` | |
| WP4 P2 | `testing` feature | `6b4581b` | `5588438` | |
| WP4 P2 | boot child test (EX2) | `322f20b` (RED/GREEN) | `322f20b` | |
| WP4 P2 | `autumn_app` example and smoke script (EX2) | `b529c99` | `8690a4d` | |
| WP4 | deliveries in one millisecond are all accepted | `09b4557` | `59f4787` | |
| WP5 H1 Harvest | core `may_have_run`, `NOT_SENT_NETWORK_CODES`, `search_page_at` | `02e77db` | `87106b6` | |
| WP5 H1 | activities, watch, bridge on Postgres, `harvest_workflow` | `e5b05e7` | `7c3ddbb` | `026ff8e`, `80707dc`, `ac27dc5` |
| WP6 K1 Camel | component and producer per operation | `3c785cd` | `f3435ec` | `f97bddb` |
| WP6 K1 | idempotency and the attempt rule | `024b961` | `82faca9` | |
| WP6 K1 | header overrides | `0680afa` | `ddef50b` | |
| WP6 K1 | `camel_route` example (EX4) | `bfe3c00` | `2730a8e` | |
| WP6 K1 | unit tests for the body read and the in headers | `3976ff2` (RED/GREEN) | `3976ff2` | |
| WP8a Guards | guard tests with negative self-tests | `1dee13a` | `d7f15ba` | |
| WP8a | `examples_wired` (EX8), README | `9250bd7` | `ad9dfb3` | `eb2dce2` |
| WP7 Ecosystem | ecosystem example, EX5 to EX7 | `c439a8f` | `46da54b` | `8c278e7` |

Notes on the record:

- WP3: three RED commits had tests that passed at once (`logs.rs`, the
  settings and call-glue unit tests, two example tests, and
  `chaos_at_most_one_issue_per_key`). They guard code that already existed.
  Each commit message says so.
- WP4: in the RED of the pipeline, `missing_signature_is_400`,
  `bad_signature_is_401` and `over_cap_is_400` passed at once: autumn
  answers them before the route. Two GREEN commits fixed test bugs, and
  the messages name them.
- WP6: the example RED commit used a `start()` stub, because the library
  already existed. In `82faca9` one test expectation changed (a lost marked
  create maps to `Io` with the code `outcome_pending`), and the message
  says so. The REFACTOR commit `f97bddb` changed no test file.
- WP4 changed the TDD order of plan 18 (plan V33).

## Review rounds

### Round 1

Round 1 ran after C4 over 10 angles (plan 17.4):

| Angle | Topic |
|---|---|
| R1 | write safety |
| R2 | webhook security |
| R3 | secrets and SSRF |
| R4 | harvest determinism |
| R5 | spec and proof |
| R6 | fake fidelity |
| R7 | API and docs |
| R8 | build and CI |
| R9 | test quality |
| R10 | plugin and ecosystem |

The reviewers gave 91 findings. Adversarial verifiers refuted 12 of them,
confirmed 64 and rated 15 as plausible. Five fixers took the 79 findings
that were not refuted. Each code fix has a RED commit with the failing test
and a GREEN commit. A doc-only fix has a DOCS commit. Plan section 2.1
(V54 to V95) gives the design change of each fix, with its finding id.

The severity is the severity after the verifier. The status comes from the
fixer reports and the follow-up commits after them: 77 fixed, 1
partially fixed, 1 skipped. "none" in the tests
column means a doc fix with no test of its own.

| Id | Severity | Finding | Status | Tests | Commits |
|---|---|---|---|---|---|
| R1-write-safety-1 | medium | A camel redelivery of a marked write sent the create again. | fixed | `camel/t/route.rs::camel_redelivery_of_marked_create_looks_up_only`; u: `headers::a_camel_redelivery_looks_up_only` | 768f1f3 (RED), 686364d (GREEN), a458324 (REFACTOR clippy) |
| R1-write-safety-2 | low | A camel redelivery of an unmarked write sent it again. | fixed | `camel/t/route.rs::camel_redelivery_never_resends_unmarked_writes`; u: `headers::a_camel_redelivery_looks_up_only` | 768f1f3 (RED), 686364d (GREEN) |
| R1-write-safety-3 | medium | A reset after the request bytes counted as not sent. | fixed | `core/t/client.rs::reset_after_write_is_maybe_sent`; u: `backend::direct::only_a_check_before_the_send_is_not_sent`; u: `backend::san_mateo::a_send_error_of_the_transport_is_maybe_sent` | c013326, 96bc793 |
| R1-write-safety-4 | low | A transition was sent again after a maybe-sent attempt, also when the issue moved on. | fixed | `core/t/idempotency.rs::{transition_after_maybe_sent_stops_when_the_issue_moved_on, global_transition_after_maybe_sent_is_not_sent_again}` | 055794b, 773b709 |
| R1-write-safety-5 | low | A 400 or 409 after a maybe-sent transition did not read the status again. | fixed | `core/t/idempotency.rs::transition_400_after_lost_reply_reads_the_status` | 055794b, 773b709 |
| R1-write-safety-6 | low | A delete 404 after an earlier failed attempt was a terminal not_found. | fixed | `harvest/t/activities.rs::{a_delete_404_after_any_failed_attempt_is_gone, reads_and_idempotent_writes}` | b717f55 (RED), ba5e3df (GREEN) |
| R1-write-safety-7 | low | The docs did not say that a retry after outcome_pending is a lookup only. | fixed | none (doc only) | 4551219 |
| R1-write-safety-8 | low | An unexpected 2xx on a marked write skipped the marker lookup. | fixed | `core/t/idempotency.rs::marked_write_with_an_unexpected_2xx_looks_up_the_marker` | 4f4dc75, 57c043c |
| R1-write-safety-9 | low | The comment lookup read only the 100 newest comments. | fixed | `core/t/idempotency.rs::comment_lookup_finds_marker_beyond_100` | 04705d4, fa5fb0d, d158c70 |
| R1-write-safety-10 | low | The camel lookup-only create had no settle wait for the search index. | fixed | `camel/t/route.rs::retry_attempt_lookup_waits_for_index_lag`; u: `options::{settle_ms_takes_0_to_the_max, uri_options_match_the_parser}`; u: `settle::the_wait_ends_after_the_time` | 2c132cb (RED), dd5d031 (GREEN) |
| R2-webhook-security-1 | low | A delivery of another site with the same issue id could move a wait. | partially fixed | `harvest/t/bridge_pg.rs::a_delivery_of_another_site_gives_the_status_of_this_site`; ex: `harvest_workflow::lifecycle_resolves_by_webhook` | faf1fcb (RED), 9541505 (GREEN) |
| R2-webhook-security-2 | low | The comment actor filter used the author for an update or a delete. | fixed | u: `webhook::{comment_actor_follows_the_event, parses_all_events}` | e638c6b, 2824822 |
| R2-webhook-security-3 | low | The boot check left out max_future_skew_secs. | fixed | `plugin/t/webhook_config.rs::max_age_and_skew_stay_inside_the_replay_window` | f6bd05c, 5d16fa0 |
| R2-webhook-security-4 | low | ADR 0004 did not name the crash case of the replay store. | fixed | `core/t/guards.rs::docs_make_no_stale_claims` | 193a4af (RED), 28c5d04 (GREEN) |
| R2-webhook-security-5 | low | The self filter hid the own status changes from the bridge. | fixed | `plugin/t/webhook.rs::a_sink_for_all_actors_gets_the_ignored_actor`; eco u: `tests::webhook_negatives` | 29184b4 (RED), 3fc3cbf (GREEN), 578b33b (RED), 57f861c (GREEN, the ecosystem bridge) |
| R2-webhook-security-6 | low | The route tag does not bind the issue, so a copied route entry works. | skipped | none (limit documented) | e68504a (DOCS) |
| R2-webhook-security-7 | low | A typed or quoted comment marker counted, and the lookup did not check the author. | fixed | u: `webhook::only_a_full_last_line_marker_counts`; `core/t/idempotency.rs::a_marker_from_another_author_is_not_found` | e638c6b, 2824822, 04705d4, fa5fb0d, 4931958 |
| R2-webhook-security-8 | low | A max_age_secs under the Jira retry horizon dropped the retries. | fixed | `plugin/t/webhook_config.rs::a_max_age_below_the_jira_retry_horizon_warns` | f6bd05c, 5d16fa0 |
| R2-webhook-security-9 | low | The body cap is memory that an unsigned request can use. | fixed | none (doc only) | 5c13065 (DOCS), 1fea9b3 (DOCS, README and ADR 0004) |
| R3-secrets-ssrf-1 | medium | The property keys . and .. moved the request to another path. | fixed | `core/t/client.rs::dot_segment_property_keys_are_rejected` | d94f304, 13f4557 |
| R3-secrets-ssrf-2 | low | One configured cloud id pinned each tenant token to one site. | fixed | `plugin/t/lifecycle.rs::the_configured_cloud_id_is_for_the_default_connection_only` | e36f5e7, d8d9d1a |
| R3-secrets-ssrf-3 | low | A stored cloud id was trusted for ever. | fixed | `core/t/san_mateo.rs::a_stale_metadata_cloud_id_is_checked_after_a_404` | 76c9731, f8849ae (RED fixed: my first stale id was the fake's own default id), 8e2a3d4 |
| R3-secrets-ssrf-4 | low | A secret in email_env or token_env showed in the error. | fixed | u: `config::env_keys_hold_env_var_names`; `plugin/t/webhook_config.rs::the_fix_never_prints_a_secret_env_value_that_is_not_a_name` | b717333 (RED), 0385b6e (GREEN) |
| R4-harvest-determinism-1 | high | One failed poll read ended the durable wait. | fixed | `harvest/t/wait.rs::{a_failed_poll_is_a_missed_poll, a_failed_last_read_times_out, a_signal_after_a_failed_poll_ends_the_wait, a_non_retryable_poll_failure_ends_the_wait}`; `harvest/t/activities.rs::failure_for_table` | 9479c97 (RED), 284bdaf (GREEN) |
| R4-harvest-determinism-2 | medium | A signal after the wait broke a strict replay. | fixed | `harvest/t/wait.rs::a_signal_after_the_wait_needs_the_drain`; eco u: `tests::a_late_jira_signal_replays` | f509647 (RED), 71d1053 (GREEN), a77491c (DOCS), 578b33b (RED), 57f861c (GREEN, the ecosystem drain) |
| R4-harvest-determinism-3 | medium | The reads did not move the stale-signal time. | fixed | `harvest/t/wait.rs::a_signal_older_than_a_read_is_skipped` | 9479c97 (RED), 284bdaf (GREEN) |
| R4-harvest-determinism-4 | medium | No bound on the rounds of a wait, so the history could reach its cap. | fixed | `harvest/t/wait.rs::{too_many_poll_rounds_fail_at_once, the_round_bound_fits_the_history_cap}` | 9479c97 (RED), 284bdaf (GREEN) |
| R4-harvest-determinism-5 | low | The wait deadline did not fit the execution deadline of the run. | fixed | u: `wait::the_deadline_fits_the_run` | 51ef333 (RED), 3c26e47 (GREEN) |
| R4-harvest-determinism-6 | low | The start_to_close budgets did not cover a san-mateo send. | fixed | `harvest/t/activities.rs::start_to_close_covers_the_client_budget` | 3424c78 (RED), b066c73 + 945c3226 (GREEN) |
| R4-harvest-determinism-7 | low | Two waits of one run took the signals of each other. | fixed | `harvest/t/wait.rs::two_waits_at_the_same_time_run_one_after_the_other` | 9479c97 (RED), 284bdaf (GREEN) |
| R5-spec-proof-1 | medium | A failed pre-lookup counted as a write that can have run. | fixed | `harvest/t/activities.rs::a_failed_pre_lookup_is_not_sent`; u: `failure::previous_attempt_table` | b717f55 (RED), ba5e3df (GREEN) |
| R5-spec-proof-2 | low | The scale_permille proptest was vacuous and it had no body match. | fixed | u: `policy::{verus_bodies_match, spec_as_test_scale_permille_in_range, spec_as_test_scale_permille_out_of_range, spec_as_test_jitter_bounds_small, wait_and_jitter_examples}` | 5eebb9d, ebedb95, ecd58ed |
| R5-spec-proof-3 | low | A function in both files could skip the body match. | fixed | u: `policy::{every_const_fn_is_proved_or_listed, each_fn_is_in_exactly_one_list}` | 5eebb9d, ebedb95 |
| R5-spec-proof-4 | low | No test rejected a proof escape in the Verus file. | fixed | u: `policy::the_spec_has_no_proof_escape` | 5eebb9d, ebedb95 |
| R5-spec-proof-5 | low | The decide proptests never checked the Retry side. | fixed | u: `policy::{spec_as_test_decide_attempt_bound_small, spec_as_test_decide_no_resend_after_maybe_sent_small, decide_examples}` | ecd58ed |
| R5-spec-proof-6 | low | An unexpected 2xx on a marked write skipped Confirm. | fixed | `core/t/idempotency.rs::marked_write_with_an_unexpected_2xx_looks_up_the_marker` | 4f4dc75, 57c043c |
| R5-spec-proof-7 | low | The offset pager and the token pager disagreed at max_pages = 0. | fixed | `core/t/client.rs::offset_pager_reads_the_first_page_like_the_token_pager` | 925becb, d158c70 |
| R5-spec-proof-8 | low | The signature shape tests missed a bad byte after the right prefix. | fixed | u: `policy::{spec_as_test_signature_shape_mixed_case, spec_as_test_signature_shape_one_bad_byte, signature_and_quote_examples}` | ecd58ed |
| R5-spec-proof-9 | low | No code bounded max_future_skew_secs by MAX_FUTURE_SKEW_MS. | fixed | `plugin/t/webhook_config.rs::max_future_skew_has_the_policy_bound` | f6bd05c, 5d16fa0 |
| R5-spec-proof-10 | low | The docs of the verified core did not match the lists. | fixed | `core/t/guards.rs::adr_0005_names_each_policy_fn` | 193a4af (RED), d8b3bbd (GREEN), 1b45ebc (the `verus/policy.rs` header and the `PROPTEST_ONLY` doc) |
| R6-fake-fidelity-1 | high | The parser and the fake used ADF where Jira webhooks send REST v2 strings. | fixed | u: `webhook::v2_string_comment_body_is_read`; `fake/t/fake.rs::deliveries_use_the_v2_shape` | e638c6b (RED), 2824822 (GREEN) |
| R6-fake-fidelity-2 | medium | The parser dropped the comment of an issue_commented update. | fixed | u: `webhook::issue_commented_update_keeps_the_comment`; `fake/t/fake.rs::{deliveries_use_the_v2_shape, deliveries_are_signed}` | e638c6b, 2824822 |
| R6-fake-fidelity-3 | medium | A bad label value became LabelsNotOnScreen. | fixed | `core/t/idempotency.rs::{a_bad_label_value_is_a_validation_error, labels_not_on_screen_has_a_fix}` | 4f4dc75, 57c043c |
| R7-api-docs-1 | medium | Health was always DOWN with no connection_id. | fixed | `plugin/t/health.rs::no_default_connection_is_not_down` | 9584752, 247ce80, 1e4dec5 |
| R7-api-docs-2 | medium | The JiraIssueKey out header of one step failed the next step. | fixed | `camel/t/route.rs::the_out_header_of_an_earlier_step_is_not_an_override`; u: `headers::the_own_out_header_is_not_an_override` | 632fdbc (RED), c65f35a (GREEN) |
| R7-api-docs-3 | low | The README camel snippet was not a copy of its example. | fixed | `core/t/examples_wired.rs::{readme_snippets_copy_their_examples, a_snippet_must_copy_its_example}` | 570bdfd (RED), 6ce8c1f (GREEN) |
| R7-api-docs-4 | low | The README said that a transition is never sent a second time. | fixed | `core/t/guards.rs::{docs_make_no_stale_claims, docs_make_no_stale_claims_sees_each_claim}` | 193a4af (RED), 28c5d04 (GREEN), adddc02 (DOCS, aligned with 773b709) |
| R7-api-docs-5 | low | The README said that a failed sink gets 503, and named a client opt-in that does not exist. | fixed | `core/t/guards.rs::docs_make_no_stale_claims` | 193a4af (RED), 28c5d04 (GREEN) |
| R7-api-docs-6 | low | client(prebuilt) dropped [jira.http] with no sign. | fixed | `plugin/t/lifecycle.rs::a_prebuilt_client_with_a_jira_http_table_conflicts` | a404a3b (RED), c2f43a7 (GREEN) |
| R7-api-docs-7 | low | The docs did not say that with_config reads no file and no env var. | fixed | none (doc only) | c767487 (DOCS) |
| R7-api-docs-8 | low | The README gave allow_memory_in_production with no table path. | fixed | `core/t/guards.rs::docs_make_no_stale_claims` | 193a4af (RED), 28c5d04 (GREEN) |
| R7-api-docs-9 | low | The for_connection cache had no bound. | fixed | u: `service::the_cache_has_a_bound_and_drops_the_least_recently_used` | 7c4af9c, e9b79ee |
| R7-api-docs-11 | low | CLAUDE.md gave a short lock command and left out ecosystem CI commands. | fixed | `core/t/examples_wired.rs::{docs_give_the_ci_commands_of_the_separate_workspace, a_doc_must_give_the_ci_commands}` | 570bdfd (RED), 6ce8c1f (GREEN) |
| R7-api-docs-12 | low | ADRs, examples-plan and plan 8 had stale text. | fixed | `core/t/guards.rs::{adrs_cite_real_doc_text, adrs_cite_real_doc_text_sees_a_missing_quote, adr_0005_names_each_policy_fn, adr_0005_names_each_policy_fn_sees_a_gap}` | 193a4af (RED), 28c5d04 and d8b3bbd (GREEN) |
| R7-api-docs-14 | low | Prose docs broke ASD-STE100. | fixed | `core/t/guards.rs::{docs_are_ste_prose, docs_are_ste_prose_sees_a_long_sentence}` | 193a4af (RED), 3b900b8 (GREEN), 1fea9b3 (DOCS, item 7) |
| R7-api-docs-15 | low | Code comments broke ASD-STE100. | fixed | `core/t/guards.rs::{doc_comments_are_ste_prose, doc_comments_are_ste_prose_sees_each_kind}` | 873f0cc (DOCS); the camel lib.rs rewrite (2) is in 686364d, 578b33b (RED), 1b45ebc (GREEN) |
| R8-build-ci-1 | medium | The README example rows left out the env vars. | fixed | `core/t/examples_wired.rs::{each_example_is_wired, a_readme_command_needs_its_env_vars}` | 570bdfd (RED), 6ce8c1f (GREEN) |
| R8-build-ci-2 | low | The weekly cargo update got no one-version check. | fixed | `core/t/guards.rs::{weekly_updates_get_the_one_version_check, weekly_updates_get_the_one_version_check_sees_a_gap}` | 193a4af (RED), b60d833 (GREEN) |
| R8-build-ci-3 | low | examples_wired took comments, strings and step names as evidence. | fixed | `core/t/examples_wired.rs::{a_comment_or_a_string_is_not_code, a_time_bound_in_the_tests_is_for_a_server_only, a_step_name_or_an_if_is_not_a_run}` | 570bdfd (RED), 6ce8c1f (GREEN), b35580f (DOCS) |
| R8-build-ci-4 | low | The CI guard missed set +e, ; true and a pipe with no pipefail. | fixed | `core/t/guards.rs::{no_continue_on_error, no_continue_on_error_sees_hidden_failures}` | 193a4af (RED+GREEN in the guard) |
| R8-build-ci-5 | low | The schema script had a session path as its default. | fixed | `core/t/guards.rs::{no_session_paths, no_session_paths_sees_one}` | 193a4af (RED), ffe8c49 (GREEN) |
| R8-build-ci-7 | low | The README doctest path broke in the packaged crate. | fixed | u: `plugin::{contract_matches_cargo_toml, contract_is_compatible}` | 4743bb2 |
| R8-build-ci-8 | low | autumn-jira-fake had no readme. | fixed | `core/t/guards.rs::{published_crates_have_metadata, published_crates_have_metadata_sees_a_gap}` | 193a4af (RED), da720f5 (GREEN) |
| R8-build-ci-9 | low | The CI evidence cited an old failed run. | fixed | `core/t/guards.rs::evidence_names_real_tests` | 37c7947 (DOCS) |
| R9-test-quality-1 | medium | No test made the pre-lookup fail. | fixed | `core/t/idempotency.rs::{failed_prelookup_sends_no_post, failed_comment_lookup_after_a_lost_reply_is_outcome_unknown}` | 5eb281a |
| R9-test-quality-2 | low | The chaos proptest could not fail a lookup. | fixed | `core/t/idempotency.rs::chaos_at_most_one_issue_per_key` | 5eb281a |
| R9-test-quality-4 | low | The boot child blocked on a full stderr pipe. | fixed | `plugin/t/boot.rs::{the_app_boots_and_takes_a_signed_post, missing_secret_exits_and_names_the_variable}` | 79911f4 (RED), aa461e8 (GREEN) |
| R9-test-quality-5 | low | Two 503 paths of the route had no test. | fixed | `plugin/t/webhook.rs::a_panicking_sink_is_503`; u: `webhook::a_delivery_before_the_start_is_503` | 27cd236 |
| R9-test-quality-6 | low | The budget test fixed the transport timeout at 60 s. | fixed | `harvest/t/activities.rs::start_to_close_covers_the_client_budget`; u: `config::validation_rules`; `harvest/t/activities.rs::the_comment_lookup_reads_at_most_its_page_cap` | 3424c78 (RED), b066c73 + 945c3226 (GREEN), 578b33b (RED), 57f861c (GREEN) |
| R9-test-quality-11 | low | AC15 did not test each unsafe write with each trigger. | fixed | `core/t/retry.rs::unsafe_writes_send_once_table` | d2f9f1e, ebedb95 (clippy) |
| R9-test-quality-12 | low | No test counted the upstream calls of a san_mateo start. | fixed | `plugin/t/lifecycle.rs::san_mateo_startup_makes_no_network_call` | 27cd236 |
| R10-plugin-ecosystem-1 | medium | for_connection copied the default cloud id to each tenant. | fixed | `plugin/t/lifecycle.rs::the_configured_cloud_id_is_for_the_default_connection_only`; u: `config::validation_rules` | e36f5e7 (RED), d8d9d1a (GREEN) |
| R10-plugin-ecosystem-2 | medium | Multi-tenant health was always DOWN. | fixed | `plugin/t/health.rs::no_default_connection_is_not_down` | 9584752 (RED), 247ce80 (GREEN), 1e4dec5 (REFACTOR) |
| R10-plugin-ecosystem-3 | low | A hung handler task had no end and no limit. | fixed | `plugin/t/webhook.rs::{hung_handlers_have_a_bound, a_hung_task_stops_at_abandon_after_ms}` | e83bc61 (RED), b6967b4 (GREEN) |
| R10-plugin-ecosystem-4 | low | The for_connection cache never dropped a client. | fixed | u: `service::{the_cache_has_a_bound_and_drops_the_least_recently_used, forget_drops_one_client}` | 7c4af9c (RED), e9b79ee (GREEN) |
| R10-plugin-ecosystem-5 | low | The real path of autumn_app skipped profiles and env overrides. | fixed | `scripts/smoke-autumn-example.sh` | 9adc790 (RED), 3f1fc5d (GREEN) |
| R10-plugin-ecosystem-6 | low | A max_age_secs under the Jira retry delay made each retry stale. | fixed | `plugin/t/webhook_config.rs::a_max_age_below_the_jira_retry_horizon_warns` | f6bd05c (RED), 5d16fa0 (GREEN) |
| R10-plugin-ecosystem-7 | low | The handlers could run past the autumn request timeout with no count. | fixed | `plugin/t/webhook.rs::a_request_timeout_still_counts_the_delivery`; u: `plugin::handler_budget_warns_at_the_request_timeout` | 6b9e83f (RED), 1472465 (GREEN) |
| R10-plugin-ecosystem-8 | low | A run timeout of the ecosystem stopped nothing. | fixed | eco u: `tests::a_run_timeout_still_stops_everything` | dcac054 (RED), b1d8194 (GREEN) |

Notes on the open items of round 1:

- R2-webhook-security-1: the bridge reads the status from its own site.
  `Delivery` has no site, because `parse_event` drops `issue.self`. So a
  bridge cannot refuse a delivery of another site. The docs say that one
  endpoint, secret and bridge serve one site (ADR 0004, ADR 0006).
- R2-webhook-security-6: skipped. A tag with the issue id needs a property
  PUT after the create. That breaks the one-call create
  (`create_with_a_route_tracks_the_issue_in_one_call`). The limit and its
  bounds are in plan 9.4 and ADR 0006. The lead decides on a v2 tag.
- R2-webhook-security-9: fixed. The plugin fixer changed the rustdoc. The
  docs sync `1fea9b3` added the same note to the README and ADR 0004.
- R5-spec-proof-10: fixed. ADR 0005 names each policy function, and a
  guard checks it. The follow-up `1b45ebc` fixed the header of
  `verus/policy.rs` and the `PROPTEST_ONLY` doc.
- R7-api-docs-14: fixed. The guard `docs_are_ste_prose` covers the root
  docs. The docs sync `1fea9b3` rewrote item 7 in
  `examples/ecosystem/README.md`.
- R7-api-docs-15: fixed. The fixers rewrote the comments in their own
  paths. The follow-up RED commit `578b33b` adds the guard
  `doc_comments_are_ste_prose` over `crates/*/src` and the ecosystem, and
  `1b45ebc` makes it pass.
- R9-test-quality-6: fixed. The budget test covers transport timeouts up
  to `MAX_TRANSPORT_TIMEOUT` (60 s). The follow-up `57f861c` caps
  `[jira.http] timeout_secs` at `MAX_TRANSPORT_TIMEOUT_SECS` (60) and caps
  the comment lookup of `jira_add_comment` at 3 pages (plan V64, V96).
- R2-webhook-security-5: the fix is the builder `sink_for_all_actors`. An
  app that ignores its own account registers the bridge with it. The
  ecosystem does so since `57f861c`.
- R4-harvest-determinism-2: the ecosystem `incident_flow` calls
  `drain_jira_signals` since `57f861c`.

### Round 2

Round 2 ran after the fixes of round 1, over 7 angles:

| Angle | Topic |
|---|---|
| R2A | write safety |
| R2B | webhooks and secrets |
| R2C | harvest |
| R2D | integration |
| R2E | tests and guards |
| R2F | fix check of round 1, part a |
| R2G | fix check of round 1, part b |

The reviewers gave 39 findings. Adversarial verifiers refuted 2 of them,
confirmed 29 and rated 8 as plausible. After the verifiers, 6 are medium,
31 are low, and none is high. Five fixers took the 37 findings that were
not refuted. Each code fix has a RED commit with the failing test and a
GREEN commit. A doc-only fix has a DOCS commit. Plan section 2.1 (V97 to
V115) gives the design change of each fix, with its finding id.

The status comes from the fixer reports: 36 fixed, 1 partially fixed.
"none" in the tests column means a doc fix with no test of its own.

| Id | Severity | Finding | Status | Tests | Commits |
|---|---|---|---|---|---|
| R2A-write-safety-1 | medium | A stop after a maybe-sent transition was retryable, so a host retry sent the transition again. | fixed | `core/t/idempotency.rs::{transition_maybe_sent_on_the_last_attempt_is_outcome_unknown, a_rate_limit_after_a_maybe_sent_attempt_is_outcome_unknown, a_failed_status_read_after_a_maybe_sent_attempt_is_outcome_unknown, a_resumed_transition_at_the_target_is_already_there, a_resumed_transition_off_the_target_sends_nothing}` | 31dc933 (RED), 1c5ea8a (GREEN) |
| R2A-write-safety-2 | low | A 400 or 409 after a maybe-sent transition gave the first error when the issue moved on. | fixed | `core/t/idempotency.rs::{transition_400_after_maybe_sent_when_moved_on_is_outcome_unknown, a_rate_limit_after_a_maybe_sent_attempt_is_outcome_unknown}` | 31dc933 (RED), 1c5ea8a (GREEN) |
| R2A-write-safety-3 | medium | The capped pre-lookup stopped attempt 1 of a comment on a busy issue, and a harvest retry sent a transition again. | fixed | `harvest/t/activities.rs::{add_comment_on_an_issue_with_301_comments_works_on_attempt_1, a_transition_after_a_maybe_sent_attempt_sends_nothing}` | 3c9c7e6 (RED), 9cc7222 (GREEN) |
| R2A-write-safety-4 | medium | A camel redelivery of transitionToStatus sent the transition again after the issue moved on. | fixed | `camel/t/route.rs::{redelivered_transition_does_not_resend_after_the_issue_moved_on, a_harvest_retry_of_a_transition_reads_only}` | 4af9642 (RED), bbef20c (GREEN) |
| R2A-write-safety-5 | low | A delete between two comment pages hid the marker from the lookup. | fixed | `core/t/idempotency.rs::comment_lookup_survives_a_delete_between_pages` | f69b66d (RED), 1f483c2 (GREEN) |
| R2A-write-safety-6 | low | A transition with no isGlobal or looped flag counted as safe to send again. | fixed | `core/t/idempotency.rs::transition_with_no_is_global_after_maybe_sent_is_not_resent` | e900ad9 (RED), b8f321e (GREEN) |
| R2B-webhooks-secrets-1 | medium | The stored cloud id check ran one time for the life of the backend. | fixed | `core/t/san_mateo.rs::{a_reconnect_after_confirmation_is_seen, a_reconnect_after_discovery_is_seen}` | e63753e (RED), 3f6d3d5 (GREEN) |
| R2B-webhooks-secrets-2 | low | One comment add gives the same comment in two deliveries with different delivery ids. | fixed | none (doc only) | 19a412b (DOCS), 48b3df6 (DOCS, README and ADR 0004) |
| R2B-webhooks-secrets-3 | low | A large max_handler_tasks passed the semaphore cap. | fixed | u: `runtime::the_slot_count_stays_at_the_semaphore_cap` | 1f63545 (RED), 666b9be (GREEN) |
| R2C-harvest-1 | low | After the signal cap, a wait with no poll_every went blind until its deadline. | fixed | `harvest/t/wait.rs::a_wait_past_the_signal_cap_still_polls`; u: `wait::the_signal_cap_falls_back_to_polls` | b3f4392 (RED), 6c75e71 (GREEN) |
| R2C-harvest-2 | medium | A read near the deadline could wait for Retry-After past LAST_READ_MARGIN. | fixed | u: `wait::{the_wait_reads_do_not_wait_for_a_retry_after, a_poll_at_the_deadline_is_the_last_read}` | b3f4392 (RED), 6c75e71 (GREEN) |
| R2C-harvest-3 | low | The history budget claims did not hold for missed polls and stored signals. | fixed | `harvest/t/wait.rs::missed_polls_fit_the_history_cap` | b3f4392 (RED), 6c75e71 (GREEN) |
| R2C-harvest-4 | low | A late delivery gave the status of a later read, so a short status was lost. | fixed | `harvest/t/bridge_pg.rs::a_late_delivery_of_this_site_keeps_its_reported_status` | 4b52a53 (RED), 643caa6 (GREEN) |
| R2D-integration-1 | low | The camel redelivery headers stay on the exchange after a step recovers. | fixed | none (doc only) | bbef20c (crate docs), 48b3df6 (DOCS, README) |
| R2D-integration-2 | low | The docs said to give each site its own endpoint and bridge, and the plugin cannot do that. | fixed | none (doc only) | 48b3df6 (DOCS) |
| R2D-integration-3 | low | AUTUMN_ENV did not select the profile file of the real autumn_app path. | fixed | ex: `autumn_app::the_selector_selects_the_profile_of_the_real_path`; `scripts/smoke-autumn-example.sh` | 00d4068 (RED), a51b75b (GREEN) |
| R2D-integration-4 | low | The 60 s timeout cap stopped a san_mateo start, where the plugin does not use the value. | fixed | u: `config::the_timeout_cap_applies_in_api_token_mode_only` | cc14e18 (RED), 528f89a (GREEN) |
| R2D-integration-6 | low | Semaphore::new could panic at the start on a 32-bit target. | fixed | u: `runtime::the_slot_count_stays_at_the_semaphore_cap` | 1f63545 (RED), 666b9be (GREEN) |
| R2D-integration-7 | low | The stop log recorded a worker stop with no worker. | fixed | eco u: `tests::{no_worker_records_no_worker_stop, a_run_timeout_still_stops_everything}` | 6c0c80e (RED), ba5422a (GREEN) |
| R2D-integration-8 | low | The ecosystem README and plan EX7 said that the bot delivery is ignored. | fixed | none (doc only) | 3fd347a (DOCS), 48b3df6 (DOCS, plan EX7) |
| R2E-tests-guards-1 | low | The abandon test depended on a 100 ms wall-clock window two times. | fixed | `plugin/t/webhook.rs::{a_full_slot_starts_no_task, a_hung_task_stops_at_abandon_after_ms}` | 6d4f71c (RED), 27f5790 (GREEN) |
| R2E-tests-guards-2 | low | An ADR quote passed when another ADR had the same quote. | fixed | `core/t/guards.rs::{adrs_cite_real_doc_text, adrs_cite_real_doc_text_sees_a_missing_quote, adrs_cite_real_doc_text_reads_no_adr}` | 1769040 (RED), 839368a (GREEN) |
| R2E-tests-guards-3 | low | The settle test did not check that the first lookup missed. | fixed | `camel/t/route.rs::retry_attempt_lookup_waits_for_index_lag` | d3074e5 (RED), a8ea132 (GREEN) |
| R2E-tests-guards-4 | low | A proof block opener in a line comment hid code from the body compare. | fixed | u: `policy::the_drift_check_sees_changes` | cb1a7c3 (RED), 253c139 (GREEN) |
| R2E-tests-guards-5 | low | A proof escape with a space passed the escape scan. | fixed | u: `policy::the_spec_has_no_proof_escape` | cb1a7c3 (RED), 253c139 (GREEN) |
| R2E-tests-guards-6 | low | No test checked the poll round bound at its edge. | fixed | `harvest/t/wait.rs::one_poll_round_over_the_bound_fails` | b3f4392 (RED/GREEN: the code was right, and the test passed at once) |
| R2E-tests-guards-7 | low | The metadata check misread workspace keys and multi-line arrays, and passed empty values. | fixed | `core/t/guards.rs::{published_crates_have_metadata, published_crates_have_metadata_sees_a_gap}` | d334ae0 (RED), 824b2b5 (GREEN) |
| R2E-tests-guards-8 | low | An update after cd or with --manifest-path= escaped the one-version check. | fixed | `core/t/guards.rs::{weekly_updates_get_the_one_version_check, weekly_updates_get_the_one_version_check_sees_a_gap}` | 947122a (RED), d29de99 (GREEN), 2c97622 (clippy) |
| R2E-tests-guards-9 | low | A README snippet passed in the wrong order, or with a line of a test or a comment. | fixed | `core/t/examples_wired.rs::{readme_snippets_copy_their_examples, a_snippet_must_copy_its_example}` | c33766f (RED), 2c97622 (GREEN) |
| R2F-fixcheck-a-1 | medium | A first comment attempt could not comment on an issue with more comments than the page cap. | fixed | `core/t/idempotency.rs::a_first_attempt_comments_on_an_issue_with_many_comments`; `harvest/t/activities.rs::add_comment_on_an_issue_with_301_comments_works_on_attempt_1` | f69b66d (RED), 1f483c2 (GREEN), 3c9c7e6 (RED), 9cc7222 (GREEN, harvest) |
| R2F-fixcheck-a-2 | low | A delivery had no site, so a bridge could not refuse a delivery of another site. | fixed | `core/t/webhook.rs::{a_delivery_keeps_the_host_of_its_site, a_site_host_has_a_strict_rule, each_fake_delivery_names_its_site}`; `harvest/t/bridge_pg.rs::a_delivery_of_another_site_is_refused` | 2193479 (RED), 66042f6 (GREEN), f63a94d (RED), 9c30715 (GREEN, harvest) |
| R2F-fixcheck-a-3 | low | The route tag did not bind the issue, so a copied route entry signalled the run. | fixed | `harvest/t/bridge_pg.rs::a_copied_route_does_not_signal`; u: `route::{tag_is_hmac_of_the_versioned_input, v1_tags_verify_only_as_v1}`; u: `bridge::routes_need_tag_and_allowlist` | f0da7f2 (RED), 62e6e42 (GREEN) |
| R2F-fixcheck-a-4 | low | The stored cloud id was checked again only on the first 401 or 404. | fixed | `core/t/san_mateo.rs::a_reconnect_after_confirmation_is_seen` | e63753e (RED), 3f6d3d5 (GREEN) |
| R2F-fixcheck-a-5 | low | outcome_pending made a sink answer 503, so a redelivery called the write again. | fixed | u: `sink::handler_error_from_jira_error` | c8407c2 (RED), 28b9066 (GREEN) |
| R2F-fixcheck-a-6 | low | The escape scan matched raw text, so a spaced escape passed. | fixed | u: `policy::the_spec_has_no_proof_escape` | cb1a7c3 (RED), 253c139 (GREEN) |
| R2G-fixcheck-b-1 | low | A tenant that moved to another site kept the stale site in the default client. | partially fixed | `core/t/san_mateo.rs::{a_reconnect_after_discovery_is_seen, a_reconnect_after_confirmation_is_seen}` | e63753e (RED), 3f6d3d5 (GREEN), 48b3df6 (DOCS, README and ADR 0002) |
| R2G-fixcheck-b-2 | low | examples-plan and this file gave EX5 to EX8 a stale status, while the work was done. | fixed | `core/t/guards.rs::{ex_status_agrees, ex_status_agrees_sees_a_gap}` | 42c4592 (RED), GREEN_COMMIT (GREEN) |

Notes on the open items of round 2:

- R2G-fixcheck-b-1: partially fixed. The core checks a stored cloud id
  again after a 401 or a 404, at most one time in each
  `CLOUD_ID_RECHECK_INTERVAL` (300 s). So the default client finds a new
  site within that time. The plugin has no manual refresh for the default
  client: `JiraService::forget` acts on `for_connection` clients only.
- R2D-integration-2: the docs now say that an app takes the webhooks of
  one Jira site (plan V114). The code of `crates/autumn-jira-harvest`
  still names one endpoint for each site in the `bridge.rs` module doc.
- R2F-fixcheck-a-2 and R2C-harvest-4: the site check and the payload
  status apply only when the bridge knows its site host. The ecosystem
  builds its bridge with a San Mateo client and no `site_host`, so it
  keeps the read status.
- R2E-tests-guards-7 and R2E-tests-guards-8 were plausible. The fixes
  close the gaps in the guards, also where the old gap failed closed.

## Mutation tests

Each mutation runs in a `git clone` in the scratchpad, never in the repo,
with its own target dir. Each one must make a named test fail.

| Id | Mutation | Test that must fail | Status |
|---|---|---|---|
| M1 | retry of an unsafe write after a 5xx | `core/t/retry.rs::plain_create_is_sent_once_after_503`; u: `policy::spec_as_test_decide_no_resend_after_maybe_sent` | pending |
| M2 | a second send after `Confirm` finds nothing | `core/t/idempotency.rs::{lag_beyond_lookups_is_outcome_unknown, chaos_at_most_one_issue_per_key}` | pending |
| M3 | `==` in place of the constant-time compare | `core/t/guards.rs::no_eq_on_mac_bytes` | pending |
| M4 | `webhook_status` gives a 4xx | u: `policy::spec_as_test_webhook_status`; `plugin/t/webhook.rs::transient_is_503_then_redelivery_succeeds` | pending |
| M5 | `retries: 0` removed | `core/t/san_mateo.rs::one_send_for_a_503_and_for_a_429` | pending |
| M6 | `jql_quote` without the backslash escape | u: `jql::injection_stays_one_literal`; u: `policy::spec_as_test_jql_quote_round_trip` | pending |
| M7 | the bridge maps a finished run to 503 | `harvest/t/bridge_pg.rs::finished_run_is_skipped` | pending |
| M8 | the wait deadline from `ctx.now()` | `harvest/t/wait.rs::{ignored_signals_do_not_extend_the_deadline, a_late_wait_keeps_its_full_timeout}`; `core/t/guards.rs::no_ctx_now_in_harvest` | pending (see below) |
| M9 | `host_may_send` ignores `MaybeSent` | u: `policy::spec_as_test_host_may_send`; `harvest/t/activities.rs::maybe_sent_attempt_looks_up_only` | pending |
| M10 | the route mounted with `nest` | `plugin/t/install.rs::signed_delivery_reaches_the_handler_not_500` | pending |

`cargo mutants -f crates/autumn-jira/src/policy.rs` with zero survivors is
also pending (when the tool installs).

The final table did not run in review round 1 or round 2. A scratch clone
needs its own target dir, and the disk had about 5 GB free. Each finding
of round 2 has a RED commit that shows its test fail on the old code.

Earlier runs in the waves (not the final table):

- WP2: a mutation of the `Project` reply (an unknown key and a wrong type)
  gave 10 shape errors in `fake_replies_match_the_spec_shapes`.
- WP5 H0: 9 of 11 mutants of the wait and the route tags were killed. The
  single-site mutant `deadline = ctx.now() + timeout` survived the first
  run. The test `a_late_wait_keeps_its_full_timeout` (`3074d96`) now
  targets it. The second pass did not run, because the disk was full. The
  "no final read" mutant was not applied, because rustfmt had changed the
  pattern text.

## Known limits

- `SearchPager::next_page` gives `PaginationStalled` or `PageLimit` on the
  page where the stop happens, and the issues of that page are lost
  (plan V16).
- `get_issue_property` gives `None` for a missing issue and for a missing
  property: Jira gives 404 for both (plan V15).
- The comment lookup reads at most `max_pages` pages of 100 comments.
  When the marker is not in them and the issue has more comments, it gives
  `PageLimit`, so a lost comment reply then fails closed (plan V62).
- The `comment_created` webhook shape (no top-level `user`) and the REST
  v2 payload shape follow the Jira webhook page. No live test confirms
  them yet (plan V18, V56).
- `jira_watch_issue` does a read-modify-write of the routes property. Two
  watchers at the same moment can lose one route (ADR 0006).
- The wait drains each `jira.event` signal at its end. A workflow that
  reads `jira.event` itself after the wait does not get them (ADR 0006).
  A workflow that does work after the wait calls `drain_jira_signals`
  last (plan V84).
- The bridge returns 503 at the first transient route error. Jira sends the
  delivery again, and the routes that already have their signal give
  `Duplicate` (ADR 0006).
- The `for_connection` client cache keeps at most 1024 clients (LRU). A
  workload with more tenants makes new clients more often (plan V72).
- The v2 route tag binds the run and the issue. A person who copies a
  route entry together with the marker label to another issue is not
  seen. The wait checks the issue id of each signal (plan V107).
- An app takes the webhooks of one Jira site: autumn skips a second
  `JiraPlugin`. The waits on other sites end by poll (plan V114).
- The bridge checks the site of a delivery only when it knows its site
  host. A San Mateo client needs `site_host` for that (plan V106).
- After a lost reply on a transition, an automation that moves the issue
  on makes the call end with `OutcomeUnknown`. A person or a later step
  must check (plan V97).
- When the token covers the old and the new site and no `site_url` is
  set, `SanMateoBackend` keeps a stale stored cloud id (plan V55). In the
  other cases it finds the new site within `CLOUD_ID_RECHECK_INTERVAL`
  (plan V101).
- The plugin does not add its own account id to the self filter. Use
  `ignore_actor_account_ids` with a dedicated service account.
- `throwExceptionOnFailure=false` does not soften the `outcome_unknown` of a
  negative lookup. That error always fails the exchange (plan V48).
- camel operation names match exactly: `createissue` fails.
