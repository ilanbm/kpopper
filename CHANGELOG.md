# Changelog

## 1.8.0 — 2026-09-19

- Guide readers through three expandable README paths (#142) — patch
- Run focused CI for the research example instead of the full matrix (#141) — patch
- Refresh README capabilities and demonstrate research query replay (#140) — patch
- Show how GROUNDING.yaml connects sessions, checks and evidence (#139) — patch
- Separate optional HTML applications from the core (#137) — minor
- Fix session attribution and repeated Stop reminders (#135) — patch

Decisions recorded: d.application_names_are_nouns, d.html_applications_are_optional, d.plugin_descriptions_lead_with_reasoning, d.readme_illustration_reading_order, d.readme_images_use_markdown, d.readme_reasoning_overview, d.startup_checks_only_record_dependencies

## Unreleased

- Plugin users upgrading from the earlier HTML-dependent runtime should run
  `plugin_runtime.py setup` once to prepare the new private environment.

- Probe only PyYAML before opening a session; normal setup retains timezone data for followups.
- Separate the optional experimental hub and document applications from the core CLI.
  Install their runtime with `kpopper[html]` or plugin setup's `--applications html`,
  then use `kpop experimental hub` or `kpop experimental annotated-doc`. Existing command
  names (`page` and `document`) remain compatibility aliases, including under `experimental`.
  The canonical skills are `hub` and `annotated-doc`; old skill names forward to them.
- Keep record checks and ordinary session hooks independent of HTML rendering and
  layouts. Projects using the page should also run `kpop experimental hub --verify`.
  Explicit presentation authoring and layout proposals retain their application checks.
- Make application selection explicit in agent guidance; mapping or requesting generic
  HTML no longer automatically selects a kpopper HTML application.

## 1.7.0 — 2026-09-19

- Default new records to history-backed reasoning and complete temporal observers (#134) — minor
- Retain captured context in history fold previews (#133) — minor
- Name the kpopper plugin when describing skill use (#105) — patch
- Support core observers and verify prepared history folds (#132) — minor
- Use captured core findings for consolidation (#131) — minor
- Build the page from the record the reader already read (#127) — patch
- Read core and history assessments in followup scans (#128) — minor
- Infer roles once per document the overlay compares, not once per entry (#126) — patch
- Add reasoning runtime and versioned knowledge badges (#129) — patch
- Keep remeasure tests stable across UTC midnight (#122) — patch
- Reduce CI latency with parallel tests and verified native caches (#121) — patch
- Connect query verification to its original request (#124) — patch
- Record finite-query release candidate verification (#123) — patch
- Add finite scoped query reasoning (#120) — minor
- Check Darwin where a change lands, and bound every job's runtime (#119) — patch
- Unify core assessment across consumers (#118) — minor
- Add composable typed conditions to core/v1 (#117) — minor
- Add immutable history and recoverable record writes (#116) — minor
- Verify OpenClaw without native plugin fallback and clarify writes (#115) — patch
- Add contribution guides, issue forms and repository status badges (#114) — patch
- Fix plugin Python setup and hook runtime selection (#113) — patch
- Document OpenClaw and OpenCode setup and fix host hook contracts (#111) — minor
- Record the intent behind selective CI (#112) — patch
- Keep the insertion test independent of timezone differences (#110) — patch
- Run CI by changed input families and avoid unchanged native builds (#107) — patch
- Make kpop the command; kpopper stays the project and package name (#106) — minor
- Make a page's source links and its own address reach their files (#100) — patch
- Add the versions core under examination, with its scenarios, counterexamples and cost as tests (#103) — minor
- Take the two re-decided rules by name, and ask the person before a take rather than hand them the command (#104) — patch
- Fold a verdict over a standing judgment only by its own condition or by name, and keep what it replaced (#101) — minor
- Add an experimental portable deterministic reasoning core (#86) — minor
- Wait for the worker lease in the batch capture test before cleanup (#97) — patch
- Clarify working modes and show consolidation in README (#85) — patch

Decisions recorded: d.a_name_becomes_a_url_by_escaping, d.ci_execution_reuses_verified_work, d.ci_selects_families, d.community_entrypoints, d.darwin_checked_where_it_lands, d.every_worker_test_waits_for_the_lease, d.history_authority_is_explicit, d.link_ends_share_one_spelling, d.native_workflow_is_corresponding_source, d.overlay_infers_once_per_document, d.page_build_reads_the_loaded_record, d.plugin_runtime_is_explicit, d.readme_working_modes, d.remeasure_tests_share_one_day, d.replacement_leaves_a_trail

## 1.6.0 — 2026-09-14

- Document focused exports and recording scope in README (#78) — patch
- Publish a GitHub release when a version lands (#82) — patch
- Record that the record's form is read, its lines are judgments, and its storage stays in the reader (#61) — patch
- Add Simple and Advanced modes with durable pending grounding (#84) — minor
- Add shared assessment findings and scoped attention (#83) — minor
- Scope capture exemptions and document reader compatibility (#81) — patch
- Allow contradiction discovery and contextual followup gaps (#80) — patch
- Preserve qualitative rules and existing document citations (#79) — minor
- Compute readable formulas and record source reports atomically (#75) — minor
- Clarify README reasoning and selective notifications (#74) — patch
- Add focused graph exports and clarify knowledge recording scope (#76) — minor

Decisions recorded: d.assessment_separates_findings_and_attention, d.export_preserves_recorded_context, d.github_packages_is_not_a_channel, d.merge_publishes_the_release, d.record_scope_preserves_learning, d.release_is_one_tree, d.shape_is_read_not_declared, d.storage_line_is_a_judgment, d.storage_stays_in_the_reader

Reader compatibility for 1.6.0: records containing structured expressions or computed
snapshots require a 1.6.0-or-later CLI/plugin/CI reader and writer. Upgrade every entrypoint
before writing or reviewing such records with another installation. Expression evaluation
also requires a matching built Lean core; native Windows uses individual writes instead
of durable report batching. See [compatibility details](skills/kpopper/EXPRESSIONS.md#reader-compatibility).

## 1.5.2 — 2026-09-13

- Remove introductory caption and illustration-production guide (#73) — patch
- Illustrate more deterministic reasoning and refresh the knowledge map (#71) — patch

## 1.5.1 — 2026-09-13

- Refresh README introduction and visual identity (#69) — patch
- Fix concurrent record creation and layout migration regressions (#70) — patch
- Record the entry-file naming measurement on the Codex host (#66) — patch

Decisions recorded: d.detached_worker_test_cleanup, d.knowledge_map_keeps_its_detail, d.readme_layered_explanation, d.readme_main_tagline, d.readme_opens_with_what_it_does, d.readme_story_series, d.readme_visual_identity

## 1.5.0 — 2026-09-11

- Say when a record was moved by half, and name the directory a plain listing skips (#65) — minor
- Name the record's entry file GROUNDING.yaml and keep its files in .kpopper beside it (#64) — minor
- Record the untouched-record question's measurement on the Codex host (#63) — patch
- Ask for a word as a prefix, and show a legend for the letters a record keeps (#62) — minor
- Read a record file once per change, and write where its subject is (#59) — minor

Decisions recorded: d.a_half_move_is_said, d.a_write_joins_its_own_shard, d.brand_on_the_directory, d.entry_file_is_grounding, d.home_beside_the_entry, d.in_the_tree_only_what_is_reviewed, d.parse_kept_by_file_identity, d.prefix_is_a_word, d.unasked_recording_is_the_opener

## 1.4.0 — 2026-09-10

- Split the method into skills per occasion and pass every session through the record (#56) — minor

Decisions recorded: d.first_add_is_the_birth, d.gate_asks_once_after_work, d.hooks_carry_pointers, d.named_until_read, d.no_record_opener_is_two_lines, d.skill_is_an_occasion

## 1.3.0 — 2026-09-10

- Add resumable search and evidence context to checked sessions (#50) — minor
- Add asynchronous branch watch and scoped shared facts (#55) — minor
- Lead with the third brain and show standalone document reasoning (#54) — patch
- Add standalone HTML documents with offline evidence and review (#51) — minor
- Add graph-linked followups and daily review (#53) — minor
- Strengthen falsification guidance and speed up dependency reach (#52) — patch
- Add optional first-use guidance and workspace mapping (#47) — minor
- Rewrite README around agent reasoning and practical adoption (#48) — patch
- Reduce the README diagram size by 86% (#46) — patch
- Document background capture within the README workflow (#43) — patch

Decisions recorded: d.context_is_an_explicit_read, d.followups_installation_is_not_execution, d.followups_readiness_is_not_authority, d.standalone_html_is_a_copy, d.watch_async_compatibility, d.watch_shared_scope, d.workspace_operations

## 1.2.0 — 2026-09-09

- Add experimental Lean-checked session grounding (#44) — minor
- Add background ingestion with selective attention delivery (#41) — minor

Decisions recorded: d.checked_session_is_optional, d.capture_is_not_clearance, d.ingestion_attention_is_selective, d.native_delivery_scope

## 1.1.0 — 2026-09-07

- Keep update citations accurate and allow flagged judgments at stop (#39) — minor

## 1.0.0 — 2026-09-04

- Add grouped page components, output lint, and localized chrome (#27) — minor
- Admit a rewrite of a standing judgment only on its own sign (#37) — major
- Read a falsifier's shape once, and say when nothing decides it (#34) — minor
- Show where two readings part, not where they agree (#35) — patch
- Name a hypothesis for the claim it carries, not only the id it contradicts (#36) — minor
- Give the record's own counts a recipe (#33) — patch
- Hold a text to the prose it places, and let prose cover without silencing (#32) — minor
- Read also: by absence, and say where that reading does not hold (#31) — minor
- Correct why a merge driver is set aside, and stop hedging the CI step (#30) — patch
- Let the pull request re-measure the record against the tree (#29) — minor
- Teach the method its forks: hypotheses, the consolidation walk, how a merge goes (#28) — minor
- Count the share of high-confidence judgments refuted by consolidation (#25) — minor
- Score the reach rule against ground, not against itself (#26) — patch
- Judge sameness: the nearest at add, then same or distinct (#24) — minor
- Consolidate hypotheses: test the union, fold it, refute it, read a branch's record (#23) — minor
- Hold the page's arrangements to their signs, and re-decide them in place (#22) — minor
- Say how much of a record stands on a session's own confidence (#21) — minor
- Arrange this record by when it is read (#20) — minor
- Let the record fork on a contradiction (#19) — minor
- Admit a session's prior as a source, and say what re-opens a judgment (#16) — minor
- Say that the page can be opened, and where looking still beats checking (#18) — patch
- Record that both names are published (#17) — patch
- Say which files the release moves, and the shorter way to run the checks (#14) — patch
- Draw every tab, and hold the page against what its sessions were for (#11) — minor
- Record that the npm name is held (#12) — patch

Decisions recorded: d.a_comparison_shows_where_it_parts, d.a_line_is_clipped_by_its_parts, d.also_is_read_by_absence, d.another_branch_reads_as_hypotheses, d.arrangement_link_is_earned, d.arrangement_sign_is_one_comparison, d.candidate_lists_are_a_floor, d.chrome_follows_language, d.components_follow_shape, d.contested_is_a_rival_claim, d.contradiction_by_id_and_day, d.coverage_printed_never_acted, d.distinct_is_a_bare_id, d.drift_from_newest_born, d.dry_run_is_check_on_the_union, d.fold_passes_the_one_door, d.gate_and_opener_against_the_mark, d.gate_by_consequence, d.hypotheses_read_beside_the_base, d.hypothesis_files_meet_on_one_name, d.local_claims_want_a_run, d.measure_names_a_recipe, d.mention_covers_never_silences, d.names_rank_never_decide, d.page_lint_reads_output, d.predicate_shape_is_one_reading, d.prior_carries_its_own_falsifier, d.prior_confidence_survives_refutation, d.priors_counted_at_the_matrix_line, d.purpose_gaps_warn, d.record_is_code, d.record_measured_from_its_text, d.refutations_keep_judgment_ids, d.refuted_hypothesis_leaves_one_finding, d.reopened_by_is_not_blocked_on, d.request_is_a_visible_claim, d.requested_words_are_quotes, d.reversal_is_a_redecision, d.same_asks_the_one_door, d.serving_earned_by_picks, d.shape_move_read_by_its_sign, d.spill_on_every_tab, d.stood_is_derived, d.text_holds_the_prose_it_places, d.threshold_read_inside_its_function, d.tree_reads_as_a_hypothesis, d.truth_value_is_matched

## 0.21.0 — 2026-09-03

- Install the browser checks with the command line, and hold the name on npm (#5) — minor
- Draw the written layer and open the record's write path (#9) — minor
- Keep the page's code in the page's languages (#3) — patch
- Release from a pull request of its own, with the bump each change declares (#7) — patch

Decisions recorded: d.add_places_by_prefix, d.browser_code_in_browser_files, d.one_reference_resolver, d.review_names_what_it_reads, d.npm_holds_the_name, d.checks_ship_with_the_reader, d.release_is_its_own_pull_request, d.write_path_returns_reach, d.written_layer_drawn_by_build
