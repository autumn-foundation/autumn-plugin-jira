# Acceptance criteria: evidence (DRAFT)

This file lists each acceptance criterion (AC) of [plan.md](plan.md)
section 16, and each example criterion (EX1 to EX8) of
[examples-plan.md](examples-plan.md), with its status and its evidence. It
also records the spikes, the Verus result, the TDD steps, the review
rounds and the mutation tests.

Status: DRAFT. Wave 4 (WP7 ecosystem, WP8a guards and README) is in
progress. The rows of EX5 to EX8, AC38 to AC40 and each guard test say so.

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
| ACs done | 30 of 40 done. 7 done except their guard test, which is in progress (wave 4): AC2, AC7, AC19, AC21, AC22, AC30 and AC33. 3 in progress (wave 4): AC38, AC39 and AC40. |
| Examples | EX1 to EX4 done. EX5 to EX8 in progress (wave 4). |
| Tests | 345 pass, 0 fail in the main workspace (`cargo test --workspace --all-features --locked`, with Postgres) at the end of wave 3. The five live cases in `core/t/live.rs` are ignored and never run in CI. |
| Line coverage (`src/`) | `autumn-jira` 97.63%, `autumn-jira-fake` 98.96%, `autumn-plugin-jira` 94.97%, `autumn-jira-harvest` 96.36%, `autumn-jira-camel` 97.02% (the wave reports; `cargo llvm-cov`, tests and examples left out). The CI gate is 85% for each crate. |
| Verus | `verus/policy.rs`: 31 verified, 0 errors (CI run 41387106070, job 125090259261, head `59dc5d4`). |
| MSRV | 1.89: `cargo +1.89.0 check --workspace --all-targets --all-features --locked` passed in CI (run 41387106070, job `msrv`, head `59dc5d4`). Verified in CI only (plan R4). |
| CI | `lint`, `test` (with coverage), `examples`, `msrv`, `verus`, `deps`, `example-ecosystem`. The full run on the PR head is due at C5. |

## Acceptance criteria: library

| AC | Status | Tests and evidence |
|---|---|---|
| AC1 Newtypes | Done | u: `ids::{strict_rules_table, deserialize_is_strict, display_round_trip_proptest, no_panic_proptest, issue_key_parts, issue_ref_picks_by_shape, idempotency_markers}`; u: `model::response_ids_are_lenient`; `core/t/pure.rs::{idempotency_key_derive_is_stable, root_re_exports_work}` |
| AC2 Fixed hosts, no redirects | Done (WP1, WP3). The guard is in progress (wave 4). | `core/t/client.rs::{a_redirect_is_not_followed, path_segments_are_encoded}`; u: `site::{base_urls, site_name_rejects_hosts}`; `core/t/guards.rs::{one_base_url_builder, one_base_url_builder_sees_a_second_builder}` |
| AC3 API token auth | Done | `core/t/client.rs::{basic_header_is_sent, unauthorized_is_not_retried, captcha_is_typed, debug_has_no_token}`; u: `backend::direct::{transport_error_map, debug_hides_the_token, one_attempt_with_basic_auth}`; u: `site::{basic_auth_rules, debug_hides_the_email_and_the_token}` |
| AC4 san-mateo backend | Done | u: `backend::san_mateo::{error_map_covers_every_variant, prefix_by_provider}`; u: `backend::{resolve_cloud_id_picks_jira, resolve_cloud_id_needs_a_site_when_many, requests_and_replies}`; `core/t/san_mateo.rs::{discovery_stores_the_cloud_id, metadata_cloud_id_skips_discovery, one_send_for_a_503_and_for_a_429, a_401_refreshes_once, backend_errors_are_typed}` |
| AC5 One suite, three setups | Done. 20 suite cases on each setup. The OAuth setup expires the first token, so each case does one refresh and one cloud id discovery. | `core/t/conformance.rs::{suite_on_api_token, suite_on_san_mateo_basic, suite_on_san_mateo_oauth}` |
| AC6 Issue CRUD | Done | suite cases `{create_get_edit_delete, notify_users_only_when_set, delete_gone_after_retry, properties_round_trip, myself_and_server_info, projects_and_one_project}`; `core/t/client.rs::{moved_issue_key_is_kept, empty_204_is_not_parsed_and_statuses_are_checked}`; u: `error::op_expected_table` |
| AC7 Lists | Done (WP3). The guard is in progress (wave 4). | suite cases `{search_pages_to_the_end, search_stalls_on_a_repeated_token, search_page_limit, comments_offset_pages, projects_list_all_pages, issue_types_of_a_project, approximate_count}`; `core/t/client.rs::{short_non_last_comment_page, one_search_page_at_a_token}`; `core/t/guards.rs::{no_legacy_search_path, no_legacy_search_path_sees_the_old_path}` |
| AC8 Transitions | Done | suite cases `{transition_by_category, already_there_sends_no_post, no_transition_lists_names, done_needs_resolution_fields}` |
| AC9 Comments | Done | suite cases `{add_and_list_comments, all_comments_is_bounded, idempotent_writes_round_trip}` |
| AC10 JQL builder | Done | u: `jql::{builds_clauses, injection_stays_one_literal, control_chars_rejected, field_names_validated, raw_text_is_wrapped_before_and}`; u: `policy::spec_as_test_jql_quote_round_trip` |
| AC11 ADF | Done | u: `adf::{text_splits_lines, no_empty_text_nodes, builder_nodes, plain_text_round_trip_proptest, links_need_https, from_value_checks_the_root, plain_text_of_unknown_and_inline_nodes}` |
| AC12 Timestamps | Done | u: `time::{parse_table, epoch_round_trip_and_serde}` |
| AC13 Typed errors | Done | `core/t/errors.rs::{status_table_over_memory_transport, display_has_no_secret_or_query}`; u: `error::{class_table_covers_every_variant, codes_are_unique, accessors, display_holds_no_messages, may_have_run_table}`; u: `model::decode_errors_hold_no_body_value`; u: `client::call::{classify_table, retry_after_reads_integer_seconds_only}` |
| AC14 Retry policy | Done. Each test runs with `start_paused`. | `core/t/retry.rs::{reads_retry_server_errors, retry_after_is_the_floor, retry_after_above_the_cap_gives_up, beta_retry_after_is_ignored, jitter_uses_entropy, not_sent_transient_is_retried}` |
| AC15 Unsafe writes | Done | `core/t/retry.rs::{plain_create_is_sent_once_after_503, plain_create_is_sent_once_after_a_reset, plain_create_is_not_retried_after_429, plain_transition_is_sent_once}` |
| AC16 Idempotent create | Done. The chaos proptest runs 64 cases on a paused current-thread runtime. | `core/t/idempotency.rs::{marker_goes_in_the_create_body, prelookup_hit_sends_no_post, lost_reply_is_found, lag_beyond_lookups_is_outcome_unknown, label_only_hit_is_skipped, two_hits_pick_the_lowest_id, failed_lookup_is_not_not_found, labels_not_on_screen_has_a_fix, chaos_at_most_one_issue_per_key}` |
| AC17 Idempotent comment, checked transition | Done | `core/t/idempotency.rs::{lost_comment_reply_is_found, transition_after_lost_reply_is_already_there}` |
| AC18 Verified core | Done. CI job `verus`: 31 verified, 0 errors (see "Verus"). | u: `policy::{verus_bodies_match, every_const_fn_is_proved_or_listed, the_drift_check_sees_changes, backoff_matches_san_mateo, spec_as_test_decide_no_resend_after_maybe_sent, spec_as_test_decide_attempt_bound, spec_as_test_decide_done_and_stop, spec_as_test_attempt_loop_is_bounded, spec_as_test_classify_status, spec_as_test_backoff, spec_as_test_retry_wait, spec_as_test_scale_permille, spec_as_test_jitter_bounds, spec_as_test_host_may_send, spec_as_test_token_page_step, spec_as_test_token_pager_terminates, spec_as_test_offset_next, spec_as_test_freshness, spec_as_test_webhook_status, spec_as_test_is_lower_hex, spec_as_test_signature_shape, spec_as_test_jql_quote_round_trip}`; CI job `verus` |
| AC19 Webhook types | Done (WP1). The guard is in progress (wave 4). The code compares MAC bytes only with `Mac::verify_slice`. | u: `webhook::{parses_all_events, unknown_event_is_other, reduced_issue_parses, changelog_id_string_or_number, status_change_from_changelog, atlassian_test_vector, rejects_bad_shapes, flow_and_delivery}`; `core/t/webhook.rs::{signed_delivery_reaches_a_sink, header_names_are_pinned}`; `core/t/guards.rs::{no_eq_on_mac_bytes, no_eq_on_mac_bytes_sees_an_eq}` |
| AC20 Event seam | Done | u: `sink::{handler_error_from_jira_error, default_sink_name}` |
| AC21 No secrets in logs | Done (WP3). The guard is in progress (wave 4). The log test passed at RED: it guards code that already existed. | `core/t/logs.rs::no_secret_or_body_in_logs`; `core/t/guards.rs::{instrument_skips_all, instrument_skips_all_sees_a_skip_list}` |
| AC22 FakeJira behaviour | Done (WP2). The guard is in progress (wave 4). The fake `Cargo.toml` has no `autumn-jira` dependency. | `fake/t/fake.rs::{every_endpoint, jql_subset, auth_rules, oauth_refresh_rotates, accessible_resources, faults_table, index_lag_in_virtual_time, workflow_needs_resolution, deliveries_are_signed, own_hmac_matches_the_vector, fallback_serves_other_hosts, snapshot_shape, edit_paths_and_validation, delayed_writes_on_other_ops, no_secret_in_debug_or_logs}`; u: `wire::{urls_split, decoding, auth_headers}`; u: `jql::lexer_and_parser_edges`; `core/t/guards.rs::{fake_does_not_depend_on_the_client, fake_does_not_depend_on_the_client_sees_a_dependency}` |
| AC23 FakeJira matches the spec | Done. The test walks more than 100 objects. A mutation in a scratch clone (an unknown key and a wrong type in Project) gave 10 shape errors. | `fake/t/spec_shapes.rs::fake_replies_match_the_spec_shapes` |
| AC24 Plugin install | Done | `plugin/t/install.rs::{plugin_installs_with_one_call, name_contract_and_config_section, plugin_passes_the_autumn_conformance_checks, webhook_route_is_public_and_attributed, signed_delivery_reaches_the_handler_not_500, duplicate_plugin_is_a_no_op}`; u: `plugin::{contract_matches_cargo_toml, prefix_and_webhook_path}`; `plugin/t/spike_mount.rs::nest_strips_the_prefix_and_breaks_the_lookup` |
| AC25 Plugin config | Done | u: `config::{layers, mode_from_table, both_tables_fail, token_key_names_token_env, debug_hides_values, validation_rules}`; `plugin/t/lifecycle.rs::{client_and_mode_table_conflict, transport_in_san_mateo_mode_conflicts}` |
| AC26 Service and lifecycle | Done | `plugin/t/lifecycle.rs::{service_is_503_before_start, startup_makes_no_network_call, san_mateo_missing_has_a_fix, san_mateo_after_jira_has_a_fix, for_connection_caches_clients, handler_uses_the_service, a_missing_secret_names_the_variable, autumn_error_keeps_the_status_and_hides_internal_text, the_runtime_and_the_role, debug_hides_secret_values}`; u: `secrets::overrides_win_and_empty_values_count_as_missing` |
| AC27 Health | Done | `plugin/t/health.rs::{health_only_group, cached_for_cache_secs, jira_down_is_down_not_unready, details_have_no_pii}`; u: `health::down_before_start_and_after_a_failed_start` |
| AC28 Webhook boot check | Done | `plugin/t/webhook_config.rs::{each_wrong_field_fails_with_the_fix, small_body_cap_only_warns, helper_endpoint_passes, worker_role_skips_the_check, replay_off_needs_the_opt_in, a_bad_jira_config_fails_the_start}`; `plugin/t/lifecycle.rs::the_runtime_and_the_role` |
| AC29 Webhook route | Done | `plugin/t/webhook.rs::{signed_is_200, missing_signature_is_400, bad_signature_is_401, duplicate_is_409_without_dispatch, ignored_cases_are_200_and_counted, transient_is_503_then_redelivery_succeeds, timeout_is_503, permanent_is_200, two_mib_body_passes, over_cap_is_400, csrf_on_still_passes, the_route_reads_the_retry_and_flow_headers, a_fake_delivery_reaches_the_sink, deliveries_in_one_millisecond_are_all_accepted}` |
| AC30 Dispatch | Done (WP4). The guard is in progress (wave 4). | `plugin/t/webhook.rs::{sinks_run_in_order, timed_out_sink_still_finishes, transient_stops_later_sinks}`; `core/t/guards.rs::{no_exactly_once_claim, no_exactly_once_claim_sees_a_claim}` |
| AC31 Testing helpers | Done | `plugin/t/testing.rs::app_handler_test_in_ten_lines` |
| AC32 Harvest activities | Done | `harvest/t/activities.rs::{names_are_pinned, client_ref_named_and_connection, failure_for_table, lost_create_is_pending_then_found, maybe_sent_attempt_looks_up_only, opt_in_resends_after_negative_lookup, missing_state_is_config, key_is_stable_across_attempts, start_to_close_covers_the_client_budget, no_secret_in_outputs_or_history, handler_call_parses_typed_failure, every_activity_without_state_is_config, create_with_a_route_tracks_the_issue_in_one_call, reads_and_idempotent_writes, outcome_codes_have_their_constants}`; u: `failure::previous_attempt_table`; u: `clients::serde_shape_is_tagged`; u: `activities::get_issue_input_shape` |
| AC33 Durable wait | Done (WP5). The guard is in progress (wave 4). Spike S2 passes on 24 seeds. | `harvest/t/wait.rs::{already_there_returns_at_once, signal_ends_the_wait, poll_ends_the_wait_without_webhooks, ignored_signals_do_not_extend_the_deadline, other_issue_and_old_timestamp_are_skipped, timeout_over_max_wait_fails, every_case_replays, the_last_read_at_the_deadline_can_reach_the_target, a_late_wait_keeps_its_full_timeout, a_failed_read_keeps_its_error_type}`; u: `wait::{no_ctx_now_in_src, helpers}`; `core/t/guards.rs::{no_ctx_now_in_harvest, no_ctx_now_in_harvest_sees_ctx_now}` |
| AC34 Signal bridge | Done. With no database URL, a Postgres test asserts that `CI` is not set before it skips. | u: `bridge::{wants_only_status_changes, routes_need_tag_and_allowlist, tag_is_constant_time, harvest_errors_map_to_classes}`; u: `route::{route_key_needs_32_bytes_and_hides_itself, route_parts_follow_the_rule, serde_checks_the_rule, tag_is_hmac_of_the_versioned_input, verify_rejects_bad_tags, property_entry_has_the_tag, hex_helpers_round_trip}`; `harvest/t/bridge_pg.rs::{duplicate_delivery_gives_one_signal, missing_run_is_skipped, finished_run_is_skipped, db_error_is_transient, forged_tag_makes_no_db_call}` |
| AC35 Watch an issue | Done | `harvest/t/activities.rs::watch_is_idempotent`; `harvest/t/wait.rs::wait_on_a_human_issue` |
| AC36 camel component | Done | `camel/t/route.rs::{create_get_edit_transition_comment_search, consumer_is_rejected, unknown_option_lists_names, project_and_jql_conflict, override_header_without_flag_fails, override_header_with_flag_works, errors_hold_no_body, a_named_connection_picks_its_client}`; u: `options::{uri_options_match_the_parser, unknown_option_lists_names, project_and_jql_conflict, defaults, parses_all_options_of_each_operation, operations_match_the_constant, option_of_another_operation_fails, the_first_error_is_stable}`; u: `input::{read_takes_json_text_and_bytes, create_reads_each_key, edit_needs_one_change, transition_takes_nothing_or_fields, comment_takes_text_or_a_string}`; u: `headers::{marked_reads_the_key_and_the_attempt, a_header_equal_to_the_option_passes}` |
| AC37 camel idempotency and errors | Done | `camel/t/route.rs::{key_header_uses_the_idempotent_path, retry_attempt_looks_up_only, opt_in_resends}`; `camel/t/retry_rule.rs::{map_keeps_the_retry_class_for_every_error, the_samples_cover_every_code, each_camel_kind_gives_its_camel_variant, messages_hold_the_code_and_no_body, outcome_unknown_with_a_key_is_io_and_without_is_validation, http_errors_have_the_plan_shape}`; u: `error::{http_errors_name_the_method_of_the_failed_call, reason_phrases}` |
| AC38 Quality gates | In progress (wave 4). Clippy pedantic and nursery `-D warnings` are clean at the end of wave 3. MSRV passed in CI. Coverage of each crate is over 85% (see "Summary"). | CI jobs `lint`, `test`, `msrv`; `core/t/guards.rs::{no_unwrap_or_todo_in_src, no_unwrap_or_todo_in_src_sees_each_kind, every_allow_has_a_reason, every_allow_has_a_reason_sees_a_bare_allow, no_continue_on_error, no_continue_on_error_sees_hidden_failures}` |
| AC39 Dependency rules | In progress (wave 4) | CI job `deps`; `core/t/guards.rs::{root_lock_is_free_of_c_builds, root_lock_is_free_of_c_builds_sees_one, ecosystem_lints_equal_the_root, the_lint_compare_sees_a_change}` |
| AC40 Docs | In progress (wave 4). This file, `docs/plan.md` (design v3, as built) and `docs/examples-plan.md` are written. The review angle R7 is due after C4. | `core/t/guards.rs::{readme_writes_table_equals_writes, the_writes_compare_sees_a_change, evidence_names_real_tests, the_evidence_check_sees_a_missing_test, adr_files_exist, adr_files_exist_sees_a_gap, the_lexer_drops_comments_and_test_items}`; u: `writes::{writes_table, writes_match_the_markers}` |

## Acceptance criteria: examples

| EX | Status | Tests and evidence |
|---|---|---|
| EX1 `embedded` | Done. `cargo run --locked -p autumn-jira --example embedded` prints the summary and `check: ok`. | ex: `embedded::{the_example_runs_and_checks, a_bad_field_is_a_validation_error, a_wrong_value_fails_the_check}`; CI step "example: embedded" in `.github/workflows/ci.yml` |
| EX2 `autumn_app` | Done. `scripts/smoke-autumn-example.sh` passed locally: dev and prod, `GET /issues/OPS-1` 200, 404, 400, a signed openssl-HMAC POST 200, the copy 409, a bad signature 401, `jira` in `/actuator/health` in both profiles, SIGTERM exit 0, no secret in the log, and a missing `JIRA_API_TOKEN` exits non-zero and names it. | ex: `autumn_app::{the_app_serves_issues_and_webhooks, a_wrong_value_fails_the_check, the_demo_fake_has_the_issue}`; `plugin/t/boot.rs::{missing_secret_exits_and_names_the_variable, boot_child_server, the_app_boots_and_takes_a_signed_post}`; `scripts/smoke-autumn-example.sh` |
| EX3 `harvest_workflow` | Done. `cargo run --example harvest_workflow` on Postgres gave `check: ok`. Two copies at the same time, in two rounds: all four runs exited 0. | ex: `harvest_workflow::{lifecycle_resolves_by_webhook, watch_resolves_by_poll, a_400_fails_with_one_post, a_wrong_value_fails_the_check}`; CI steps "example: harvest_workflow" and "two runs at the same time" |
| EX4 `camel_route` | Done. `cargo run --locked -p autumn-jira-camel --example camel_route` prints the summary and exits 0. | ex: `camel_route::{the_route_creates_and_moves, unknown_option_fails_at_route_add, a_wrong_value_fails_the_check}`; CI step "example: camel_route" |
| EX5 Ecosystem happy path | In progress (wave 4, WP7). RED commit `c439a8f`. | eco u: `tests::happy_path_timeline_is_exact`; CI job `example-ecosystem` |
| EX6 Ecosystem failures | In progress (wave 4, WP7) | eco u: `tests::{lost_create_fails_closed, bad_webhook_secret_resolves_by_poll, no_webhook_no_poll_times_out, a_wrong_value_fails_the_check, the_line_format_has_an_eight_wide_tag}` |
| EX7 Ecosystem edges | In progress (wave 4, WP7) | eco u: `tests::{webhook_negatives, two_tenants_stay_apart, is_retryable_copy_matches_harvest_camel, boot_three_plugins_real_http}` |
| EX8 Examples wired | In progress (wave 4, WP8a). RED commit `9250bd7`. | `core/t/examples_wired.rs::{each_example_is_wired, no_example_turns_on_the_dev_only_proxy_flags, each_example_file_is_in_the_list, the_ecosystem_workspace_stays_apart}` |

## Spikes

| Spike | Result | Test |
|---|---|---|
| S1 webhook mount | PASS. A route merged at the full path: 200 for a good signature, 401 for a bad one. With `nest`: 500 (`EndpointMissing`), because `nest` strips the prefix and `SignedWebhook` looks up the endpoint by the full path. Commits `016d549`, `8c75f73`, `97166de`. | `plugin/t/install.rs::signed_delivery_reaches_the_handler_not_500`; `plugin/t/spike_mount.rs::nest_strips_the_prefix_and_breaks_the_lookup` |
| S2 durable wait | PASS. 24 seeds. Ten ignored signals land at seeded times, some after the 60 s poll. Each seed gives `TimedOut` at exactly 120 s. The deadline tests run in the seeded `Simulation`; "advancing clock off" is `WorkflowReplayer::new()` (plan 9.3, V43). | `harvest/t/wait.rs::ignored_signals_do_not_extend_the_deadline` |
| S3 ecosystem graph | In progress (wave 4, WP7) | eco u: `tests::boot_three_plugins_real_http` |
| S4 Verus in CI | PASS. Run 41382418046 (head `8e3f4a7`): Verus builds, `vstd` 2060 verified; the job then failed only on the missing spec file, as expected for the spike. | CI job `verus` |
| S5 one attempt | PASS. The san-mateo part passed at RED. The backend part failed at RED, then passed at GREEN (`ef41c71`, `4a6364d`). | `core/t/san_mateo.rs::one_send_for_a_503_and_for_a_429` |

## Verus

- `verus verus/policy.rs`: "verification results:: 31 verified, 0 errors".
  CI run 41387106070, job 125090259261, head `59dc5d4`.
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
| WP8a Guards (in progress) | guard tests with negative self-tests | `1dee13a` | `d7f15ba` | |
| WP8a (in progress) | `examples_wired` (EX8), README | `9250bd7` | `ad9dfb3` | `eb2dce2` |
| WP7 Ecosystem (in progress) | ecosystem example, EX5 to EX7 | `c439a8f` | | |

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

The lead fills this section after C4.

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
- The comment lookup reads only the 100 newest comments. A lost comment
  reply on an issue that gets more than 100 comments before the lookup
  gives `outcome_unknown`.
- The `comment_created` webhook shape (no top-level `user`) follows the
  Jira webhook page. No live test confirms it yet (plan V18).
- `jira_watch_issue` does a read-modify-write of the routes property. Two
  watchers at the same moment can lose one route (ADR 0006).
- The wait drains each `jira.event` signal at its end. A workflow that
  reads `jira.event` itself after the wait does not get them (ADR 0006).
- The bridge returns 503 at the first transient route error. Jira sends the
  delivery again, and the routes that already have their signal give
  `Duplicate` (ADR 0006).
- The `for_connection` client cache has no size bound.
- The plugin does not add its own account id to the self filter. Use
  `ignore_actor_account_ids` with a dedicated service account.
- `throwExceptionOnFailure=false` does not soften the `outcome_unknown` of a
  negative lookup. That error always fails the exchange (plan V48).
- camel operation names match exactly: `createissue` fails.
