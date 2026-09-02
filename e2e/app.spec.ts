import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const calls: { command: string; args: unknown }[] = [];
    let callbackId = 0;
    let catalogReady = false;
    let hexdekReady = false;
    let forgeReady = false;
    let continuedStatus: unknown;
    let competitiveSeed = false;
    let importedSeed = false;
    const simulatorStatuses = () => [
      { id: "structural", name: "Metagene structural screen", installed: true, data_ready: true, executable: "built in", supported_formats: ["all"], note: "Fast structural screen" },
      { id: "hexdek", name: "HexDek Yggdrasil", installed: hexdekReady, data_ready: hexdekReady, executable: "hexdek-tournament.exe", supported_formats: ["constructed", "commander"], note: "Bundled matchup engine" },
      { id: "forge", name: "Forge 2.0.14", installed: forgeReady, data_ready: forgeReady, executable: "forge.jar", supported_formats: ["constructed", "commander"], note: "Prebuilt cross-check engine" }
    ];
    const workspace = (args: unknown) => {
      const request = (args || {}) as { platform?: string; format?: string };
      const commander = request.format === "commander" || request.format === "brawl";
      return {
        platform: request.platform || "arena", format: request.format || "historic", simulator_coverage: hexdekReady ? "parser_complete" : "all", pool_filter: [], exclude_disputed_cards: true, exclude_unsupported_cards: true,
        pool: { cards: [] }, seed_deck: { name: "Untitled Evolution", command_zone: [], mainboard: [], sideboard: [] },
        constraints: { target_mainboard_size: commander ? 59 : 60, target_sideboard_size: 0, minimum_commanders: commander ? 1 : 0, maximum_commanders: commander ? 1 : 0, default_max_copies: commander ? 1 : 4, locked_cards: [], minimum_lands: 20, maximum_lands: 30 },
        config: { population_size: 128, generations: 50, elite_count: 12, trials_per_candidate: 256, mutation_swaps: 2, mutation_rate: 0.85, package_mutation_rate: 0.35, package_size: 3, mana_mutation_rate: 0.35, tournament_size: 4, seed: 42, turn_horizon: 6, worker_threads: 6, evolve_commanders: commander, evolve_sideboard: false, commander_mutation_rate: 0.08, learned_packages: [], evolution_mode: "rebuild", tune_policy: { strategy_retention_floor: 0.70, primary_package_floor: 0.60, maximum_nonland_edit_fraction: 0.40 }, fitness_weights: { consistency: 30, tempo: 24, synergy: 20, interaction: 12, resilience: 14, commander: 8, sideboard: 8, mana_failure_penalty: 35, legendary_dead_draw_penalty: 12, unsupported_synergy_penalty: 20, dead_action_penalty: 18, narrow_card_penalty: 12, closing_power: 18, role_balance: 10 } },
        opponent_field: { available: false, creature_rate: 0.70, artifact_or_enchantment_rate: 0.25, graveyard_rate: 0.30, stack_spell_rate: 0.75 }
      };
    };
    const evolvedCandidate = {
      rank: 1, pareto_rank: 0,
      deck: { name: "Evolved Candidate 1", command_zone: [], mainboard: [], sideboard: [] },
      fitness: { score: 0.72, consistency: 0.8, tempo: 0.7, synergy: 0.6, interaction: 0.5, resilience: 0.5, mana_failure_rate: 0.1, commander_quality: 0, sideboard_quality: 0, severe_mana_failure: false, mana_blocking_reasons: [], early_sources: [0,0,0,0,0,0], late_sources: [0,0,0,0,0,0], unrestricted_early_sources: [0,0,0,0,0,4], conditional_early_sources: [0,1,0,0,0,0], unusable_sources: [0,4,0,0,0,0], colored_pip_demand: [0,4,0,0,0,0], on_curve_castability: 0.42, turn_horizon_castability: 0.61, stranded_card_rate: 0.39, legendary_dead_draw_rate: 0.12, unmet_requirement_rate: 0.25, legendary_penalty: 0.0144, unsupported_synergy_penalty: 0.05, synergy_warnings: ["Command Tower: produces no mana without a commander"], card_castability: [{ card_id: "whoosh", name: "Whoosh!", copies: 4, on_curve_castability: 0.2, turn_horizon_castability: 0.3, unrestricted_early_sources: 0, conditional_early_sources: 1, unusable_sources: 4, source_restrictions: ["Mirrodin's Core — unrestricted"], reasons: ["Whoosh!: four copies, no eligible early blue source"] }] },
      packages: [], candidate_status: "provisional"
    };
    Object.assign(window, { __METAGENE_TEST_CALLS__: calls });
    Object.assign(window, { __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} } });
    Object.assign(window, { __TAURI_INTERNALS__: {
      transformCallback(callback: unknown) { const id = ++callbackId; Object.assign(window, { [`_${id}`]: callback }); return id; },
      unregisterCallback() {},
      async invoke(command: string, args: unknown) {
        calls.push({ command, args });
        if (command.startsWith("plugin:event|")) return 1;
        if (command === "catalog_status") return { ready: catalogReady, card_count: catalogReady ? 38184 : 0, database_bytes: catalogReady ? 94371840 : 0, catalog_version: 7 };
        if (command === "sync_catalog") { catalogReady = true; return { ready: true, card_count: 38184, database_bytes: 94371840, catalog_version: 7 }; }
        if (command === "new_workspace") return workspace(args);
        if (command === "import_workspace") {
          const input = args as { text: string; platform: string; format: string };
          if (input.text.includes("Missing Card")) throw new Error("line 2: card not found in the selected platform and format: Missing Card");
          importedSeed = true;
          competitiveSeed = input.text.includes("Competitive Seed");
          const imported = workspace(input);
          if (input.text.includes("Strategy Seed")) {
            const copyCard = { id: "copy-engine", name: "Copy Engine", set_code: "tst", collector_number: "1", type_line: "Sorcery", oracle_text: "Create a token that's a copy of target creature.", colors: ["Blue"], color_identity: ["Blue"], platforms: ["arena"], legalities: {}, features: { is_land: false, unlimited_copies: false, enters_tapped: false, mana_value: 3, mana_production: 0, ramp: 0, card_advantage: 0, interaction: 0, pressure: 0, resilience: 0, tags: [], is_legendary: false, can_be_land: false, mechanics: { graveyard_setup_supply: 0, requires_death: false }, initial_modes: [{ name: "Copy Engine", type_line: "Sorcery", colors: ["Blue"], mana_cost: { generic: 2, colored: [0,1,0,0,0,0], hybrid: [], variable: false }, is_land: false, is_legendary: false, creature_types: [], mana_source_colors: [], late_mana_source_colors: [], mana_sources: [], enters_tapped: false, abilities: [{ label: "Create a token that's a copy of target creature.", kind: "Copy", timing: "Spell", zone: "Stack", cost: { mana: { generic: 0, colored: [0,0,0,0,0,0], hybrid: [], variable: false }, uses_tap: false, sacrifices_self: false, sacrifice_permanents: 0, discards_self: false, discard_cards: 0, exiles_self: false, exile_cards: 0, life: 0, remove_counters: 0 }, target: "AnyCreature", conditions: [], effect_value: 1, narrow: false, once_per_turn: false, parser_confidence: 1 }] }] } };
            const payoffCard = { ...copyCard, id: "etb-payoff", name: "ETB Payoff", type_line: "Creature — Test", oracle_text: "When this creature enters, draw a card.", features: { ...copyCard.features, pressure: 1, initial_modes: [{ ...copyCard.features.initial_modes[0], name: "ETB Payoff", type_line: "Creature — Test", abilities: [{ ...copyCard.features.initial_modes[0].abilities[0], label: "When this creature enters, draw a card.", kind: "Draw", timing: "Triggered", zone: "Battlefield", conditions: ["when this creature enters"] }] }] } };
            return { ...imported, pool: { cards: [copyCard, payoffCard] }, seed_deck: { name: "Imported Strategy Seed", command_zone: [], mainboard: [{ card_id: "copy-engine", quantity: 30 }, { card_id: "etb-payoff", quantity: 30 }], sideboard: [] } };
          }
          return { ...imported, seed_deck: { name: input.text.includes("Start Failure") ? "Rejected Seed" : competitiveSeed ? "Competitive Seed" : "Imported Historic Deck", command_zone: [], mainboard: [{ card_id: "imported-seed", quantity: 60 }], sideboard: [] } };
        }
        if (command === "ground_evolution_from_field") {
          const input = args as { request: { evolution: ReturnType<typeof workspace>; opponents: { name: string; command_zone: never[]; mainboard: { card_id: string; quantity: number }[]; sideboard: never[] }[] } };
          return {
            ...input.request.evolution,
            seed_deck: { ...input.request.opponents[0], name: `Competitive field seed · ${input.request.opponents[0].name}` },
            pool_filter: ["Tournament seed"]
          };
        }
        if (command === "simulator_status") return simulatorStatuses();
        if (command === "install_hexdek_engine") { hexdekReady = true; return simulatorStatuses()[1]; }
        if (command === "install_forge_engine") { forgeReady = true; return simulatorStatuses()[2]; }
        if (command === "start_evolution") {
          const input = args as { request: { seed_deck: { name: string } } };
          if (input.request.seed_deck.name === "Rejected Seed") throw new Error("required seed card is outside the selected focused pool");
          return "run-color";
        }
        if (command === "acquire_competitive_field") {
          if (importedSeed && !competitiveSeed) throw new Error("fixture tournament field disabled");
          const opponent = { name: "Izzet Wizards", command_zone: [], mainboard: [{ card_id: "tournament-seed", quantity: 60 }], sideboard: [] };
          return {
            field: {
              schema_version: 1, revision_id: "field-e2e", platform: "arena", format: "historic", commander: false,
              provenance: { adapter: "spicerack-public-v1", source_url: "https://api.spicerack.gg/api/export-decklists/", retrieved_at_unix: 1788134400, event_start_unix: 1785542400, event_end_unix: 1788134400, event_count: 12, submitted_deck_count: 18, accepted_deck_count: 16, transformation_notes: ["logical cards canonicalized before clustering"] },
              clusters: [{ id: "izzet", label: "Izzet Wizards", strategy: "aggro", representative: opponent, representative_fingerprint: "izzet-1", member_fingerprints: ["izzet-1", "izzet-2"], occurrences: 9, observed_weight: 0.92, normalized_selected_weight: 1 }],
              decks: [opponent], weights: [1],
              audit: { schema_version: 1, source_tier: "tournament", competitive_claims_allowed: true, observed_weight_covered: 0.92, submitted_decks: 18, accepted_decks: 16, selected_archetypes: 8, exact_duplicates_merged: 2, near_duplicates_clustered: 3, strategy_coverage: ["aggro", "midrange", "control", "combo", "graveyard"], quarantined: [], warnings: [] }
            },
            saved: { id: "saved-field-e2e", name: "Historic tournament field", revision: 1, created_at: "2026-08-30T00:00:00Z", platform: "arena", format: "historic", commander: false, decks: [opponent], weights: [1], metadata: { source: "Spicerack", observed_at: "30 days", notes: "fixture", competitive_claims_allowed: true, field_revision_id: "field-e2e" } },
            source_attempts: ["spicerack:accepted"]
          };
        }
        if (command === "build_engine_route_plan") return {
          schema_version: 1, plan_id: "route-e2e", candidate: { schema_version: 1, engine: "hexdek", deck_name: "Competitive Seed", card_copies: 60, trusted_card_copies: 60, trusted_copy_ratio: 1, critical_actions: 4, trusted_critical_actions: 4, critical_coverage: 1, capabilities: [], blockers: [] }, candidate_by_engine: [], opponent_reports: [],
          routes: [{ opponent_index: 0, opponent_name: "Izzet Wizards", weight: 1, engine: "hexdek", eligible: true, reasons: [] }], field_weighted_coverage: 1, candidate_critical_coverage: 1, locked: true,
          claim: { tier: "single_engine_validated", promotable: true, label: "Single-engine validated", reasons: [] }
        };
        if (command === "start_hybrid_evolution") {
          const input = args as { request: { evolution: { seed_deck: { mainboard: unknown[] } } } };
          if (input.request.evolution.seed_deck.mainboard.length === 0) throw new Error("fixture rules coverage blocked");
          return "hybrid-e2e";
        }
        if (command === "prove_deck_combos") {
          const resources = { mana: 0, cards: 0, permanents: 0, tokens: 0, life: 0, graveyard: 0, counters: 0, damage: 0, mill: 0 };
          return { schema_version: 1, deck_name: "Evolved Candidate 1", corpus_revision: "spellbook-e2e", corpus_hypotheses: 42, packages: [], explored_states: 24, elapsed_ms: 3, truncated: false, warnings: [], lines: [{ id: "line-e2e", corpus_id: "42", cards: ["Copy Engine", "ETB Payoff"], actions: [{ id: "action-e2e", card_id: "copy-engine", card_name: "Copy Engine", mode_name: "Copy Engine", label: "Create a token copy", kind: "copy", timing: "sorcery", zone: "Battlefield", cost: resources, effect: { ...resources, tokens: 1 }, uses_tap: false, consumes_self: false, once_per_turn: false, parser_confidence: 1, conditions: [] }], proof: "cross_engine_verified", finite_win: false, repeatable: true, loop_output: { ...resources, tokens: 1 }, minimum_resources: { ...resources, permanents: 2 }, expected_turn: 4, setup: ["Both permanents on battlefield"], disruption_points: ["Remove Copy Engine"], dead_pieces: [], unsupported_assumptions: [], explored_states: 24, elapsed_ms: 3 }] };
        }
        if (command === "load_saved_run") return {
          request: workspace({ platform: "arena", format: "historic" }),
          status: { id: "run-finished", state: "completed", result: { generations_completed: 50, evaluated_candidates: 6400, unique_evaluations: 6000, evaluations_avoided: 400, elapsed_ms: 1000, cancelled: false, frontier: [evolvedCandidate], map_elites: [{ descriptor: { archetype: "engine", colors: "U", curve: "mid", interaction: "light", victory_path: "tokens" }, candidate: { ...evolvedCandidate, candidate_status: "niche_elite" } }], structural_model_revision: "modes-colored-mana-v3" } },
          resumable: false, remaining_generations: 0, run_kind: "structural", remaining_cycles: 0
        };
        if (command === "continue_evolution") {
          const input = args as { request: ReturnType<typeof workspace>; candidate: typeof evolvedCandidate; sourceRunId: string };
          continuedStatus = { id: input.sourceRunId, state: "running", progress: { generation: 50, total_generations: 125, evaluated_candidates: 6400, unique_evaluations: 6000, evaluations_avoided: 400, unique_in_generation: 0, duplicates_in_generation: 0, elapsed_ms: 1000, candidates_per_second: 6400, unique_evaluations_per_second: 6000, best: { ...input.candidate, rank: 1, candidate_status: "incumbent" }, top: [{ ...input.candidate, rank: 1, candidate_status: "incumbent" }], structural_model_revision: "logical-cards-continuation-v7" } };
          return { id: input.sourceRunId, request: { ...input.request, seed_deck: { ...input.candidate.deck, name: `${input.candidate.deck.name} · continued` } }, continued_from: input.sourceRunId, status: continuedStatus, generations_completed: 50 };
        }
        if (command === "evolution_status") return continuedStatus || { id: "run-color", state: "running" };
        const values: Record<string, unknown> = {
          resource_budget_status: { budgets: { cpu_threads: 8, max_engine_processes: 2, memory_mib: 4096, run_time_seconds: 3600, disk_mib: 4096 }, app_data_bytes: 1024, process_memory_bytes: 512, orphan_job_directories_removed: 0 },
          match_history: [], feedback_summary: { matches: 0, exact_revision_matches: 0, wins: 0, losses: 0, draws: 0, observed_win_rate: 0, by_archetype: [] },
          system_profile: { cpu_name: "E2E CPU", physical_cores: 4, logical_cores: 8, memory_gib: 16, recommended_workers: 6 },
          run_history: [{ id: "run-finished", created_at: "2026-08-30", platform: "arena", format: "historic", state: "completed", best_score: 0.72, evaluated_candidates: 6400, elapsed_ms: 1000, title: "", notes: "", tags: [] }], gauntlet_history: [], validation_history: [], halving_history: [],
          simulation_cache_status: { entries: 0, database_bytes: 0 }, art_cache_status: { entries: 0, bytes: 0, max_bytes: 536870912 },
          conformance_summary: { observations: 0, scenario_runs: 0, database_bytes: 0, interaction_verified_scenarios: 0, cross_engine_verified_scenarios: 0, failed_scenarios: 0, conflicting_scenarios: 0 },
          hall_of_fame_status: { entries: [], active_diverse_ids: [], maximum_entries_for_scope: 64 },
          calibration_status: { observations: 0, empirical_interval_coverage: 0, target_interval_coverage: 0.95, mean_generalization_gap: 0, mean_absolute_error: 0, recommended_risk_aversion_delta: 0, state: "collecting", database_bytes: 0 },
          surrogate_status: { labels: 5000, minimum_labels: 5000, ready: true, stale: false, gpu_training_enabled: true, gpu_decision: "WGPU enabled after throughput, calibration, quality, and parity gates", database_bytes: 4096, trustworthy_only: true, manifest: { schema_version: 2, model_revision: "burn-matchup-ensemble-v2", trained_at: 1788134400, feature_revision: "matchup-actions-v2", platform: "arena", format: "historic", commander: false, labels: 5000, lineage_field_groups: 12, validation_labels: 1000, validation_groups: 3, validation_mae: 0.08, validation_log_loss: 0.55, baseline_log_loss: 0.67, group_isolated: true, holdout_isolation: "active run excluded" }, device: { schema_version: 1, benchmarked_at: 1788134400, cpu_backend: "Burn Flex CPU", gpu_backend: "Burn WGPU", gpu_available: true, cpu_predictions_per_second: 10000, gpu_predictions_per_second: 25000, speedup: 2.5, maximum_absolute_difference: 0.00001, calibration_gate_passed: true, closed_loop_quality_per_minute_gain: 1.5, gpu_enabled: true, decision: "enabled" } },
          combo_corpus_status: { schema_version: 2, source: "Commander Spellbook", revision: "spellbook-e2e", synced_at_unix: 1788134400, hypotheses: 42, fully_resolved: 40, database_bytes: 4096, conformance_observations: 3 },
          commander_conformance: { policy_id: "test", schema_version: 1, covered_rules: [], cases: [], passed: 0, failed: 0 },
          storage_health: { schema_version: 1, databases: [{ name: "runs.sqlite", bytes: 512, healthy: true, detail: "ok" }], backups: [] },
          create_local_backup: { id: "backup-test", bytes: 128, created_at: "test" }, export_safe_diagnostics: "{\"schema_version\":1}",
          import_match_history: { imported: 1, exact_revision_matches: 0, quarantined_identity: 1, duplicates: 0, ignored_lines: 0, records: [] }
        };
        return values[command];
      }
    }});
  });
  await page.goto("/");
});

test("consolidates the product into four distinct primary views", async ({ page }) => {
  const expected: Record<string, string> = { Build: "Build a deck", Results: "Leading candidates", "Card Library": "Format-legal card library", Settings: "Local engines and storage" };
  for (const [view, heading] of Object.entries(expected)) {
    await page.getByRole("button", { name: view, exact: true }).click();
    await expect(page.getByRole("heading", { name: heading })).toBeVisible();
  }
});

test("automatically provisions a clean install and keeps every setup control responsive", async ({ page }) => {
  await expect(page.getByText("Ready · 0 legal cards · no compiler setup required")).toBeVisible();
  await page.getByText("Advanced optimizer controls").click();
  const population = page.getByLabel("Population");
  await expect(population).toHaveValue("128");
  await population.fill("256");
  await expect(population).toHaveValue("256");

  const mutation = page.locator('.range-row input[type="range"]').first();
  await expect(mutation).toHaveValue("85");
  await mutation.fill("50");
  await expect(mutation).toHaveValue("50");

  const unsupported = page.getByLabel("Exclude unsupported cards");
  await expect(unsupported).toBeChecked();
  await unsupported.uncheck();
  await expect(unsupported).not.toBeChecked();

  await page.getByRole("button", { name: "Card Library" }).click();
  const search = page.getByPlaceholder("Name, rules text, or card type");
  await search.fill("Parallel Lives");
  await expect(search).toHaveValue("Parallel Lives");

  await page.getByRole("button", { name: "Build", exact: true }).click();
  const format = page.locator(".format-controls select").nth(1);
  await format.selectOption("brawl");
  await expect(format).toHaveValue("brawl");

  const startupCommands = await page.evaluate(() => (window as unknown as { __METAGENE_TEST_CALLS__: { command: string }[] }).__METAGENE_TEST_CALLS__.map((call) => call.command));
  expect(startupCommands).toEqual(expect.arrayContaining(["sync_catalog", "install_hexdek_engine", "install_forge_engine"]));
  expect(startupCommands).not.toContain("storage_health");
});

test("supports first-run, recovery, diagnostics, and Arena feedback controls", async ({ page }) => {
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.getByText("Primary workflow ready")).toBeVisible();
  await expect(page.getByText("runs.sqlite")).toBeVisible();
  await page.getByRole("button", { name: "Create backup" }).click();
  await page.getByRole("button", { name: "Copy safe diagnostics" }).click();
  await page.getByRole("button", { name: "Results" }).click();
  await page.locator(".feedback-lab > summary").click();
  await page.getByPlaceholder("Paste Arena Player.log JSON lines…").fill('METAGENE_MATCH {"result":"Win"}');
  await page.getByRole("button", { name: "Import locally" }).click();
  const commands = await page.evaluate(() => (window as unknown as { __METAGENE_TEST_CALLS__: { command: string }[] }).__METAGENE_TEST_CALLS__.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(["create_local_backup", "export_safe_diagnostics", "import_match_history"]));
});

test("switches constructed and Commander format setup", async ({ page }) => {
  const format = page.locator(".format-controls select").nth(1);
  await expect(format).toHaveValue("historic");
  await page.locator(".format-controls select").first().selectOption("paper");
  await expect(format).toHaveValue("standard");
  await format.selectOption("commander");
  await expect(format).toHaveValue("commander");
});

test("continues a completed candidate for an additional generation budget", async ({ page }) => {
  await page.getByRole("button", { name: "Results" }).click();
  await page.getByText("Run history", { exact: true }).click();
  await page.getByRole("button", { name: "Open" }).click();
  await page.getByRole("button", { name: "Build", exact: true }).click();
  await page.getByText("Advanced optimizer controls").click();
  await page.locator(".setup-panel").getByLabel("Generations", { exact: true }).fill("75");
  await page.getByRole("button", { name: "Results" }).click();
  await expect(page.getByRole("heading", { name: "Evolved Candidate 1" })).toBeVisible();
  await page.getByRole("button", { name: "Continue +75 generations" }).click();
  const continuation = await page.evaluate(() => (window as unknown as { __METAGENE_TEST_CALLS__: { command: string; args: unknown }[] }).__METAGENE_TEST_CALLS__.find((call) => call.command === "continue_evolution"));
  expect(continuation).toBeTruthy();
  expect(continuation?.args).toMatchObject({ sourceRunId: "run-finished", request: { config: { generations: 75 } }, candidate: { rank: 1 } });
  await expect(page.getByText("Continuing candidate #1 from cumulative generation 50")).toBeVisible();
  await expect(page.getByRole("button", { name: /incumbent/ })).toBeVisible();
});

test("shows functional source restrictions, deductions, and legacy rescore warning", async ({ page }) => {
  await page.getByRole("button", { name: "Results" }).click();
  await page.getByText("Run history", { exact: true }).click();
  await page.getByRole("button", { name: "Open" }).click();
  await page.getByRole("button", { name: "Build", exact: true }).click();
  await expect(page.getByText(/Legacy structural model/)).toBeVisible();
  await page.getByRole("button", { name: "Results" }).click();
  await expect(page.getByText("Command Tower: produces no mana without a commander")).toBeVisible();
  await expect(page.getByText("Screen deductions")).toBeVisible();
  await page.getByText("Per-card castability and eligible sources").click();
  await expect(page.getByText(/4× Whoosh!/)).toBeVisible();
  await expect(page.getByText(/4× Whoosh!.*unusable 4/)).toBeVisible();
});

test("supports exact mono, multicolor, and explicit colorless deck palettes", async ({ page }) => {
  const red = page.getByRole("button", { name: "Red", exact: true });
  const colorless = page.getByRole("button", { name: "Colorless", exact: true });
  await colorless.click();
  await expect(page.getByText("Colorless", { exact: true })).toBeVisible();
  await red.click();
  await expect(red).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("Red + Colorless", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Build competitive deck", exact: true }).click();
  const run = await page.evaluate(() => (window as unknown as { __METAGENE_TEST_CALLS__: { command: string; args: unknown }[] }).__METAGENE_TEST_CALLS__.find((call) => call.command === "start_evolution"));
  expect(run?.args).toMatchObject({ request: { constraints: { allowed_colors: ["Red", "Colorless"] } } });
  expect(run?.args).toMatchObject({ request: { seed_deck: { name: "Competitive field seed · Izzet Wizards", mainboard: [{ card_id: "tournament-seed", quantity: 60 }] } } });
  await expect(page.getByText(/Evolution running · Provisional—field-grounded structural screening/)).toBeVisible();
});

test("blocks failed imports, preserves generations, and lets the imported deck establish its palette", async ({ page }) => {
  await page.getByText("Advanced optimizer controls").click();
  const generations = page.locator(".setup-panel").getByLabel("Generations", { exact: true });
  await generations.fill("1");
  await page.getByRole("button", { name: "Red", exact: true }).click();
  await page.getByText("Import an existing deck", { exact: true }).click();
  const deckText = page.locator(".deck-import textarea");
  const rebuild = page.getByRole("button", { name: "Build competitive deck", exact: true });

  await deckText.fill("Deck\n4 Missing Card");
  await expect(rebuild).toBeDisabled();
  await page.getByRole("button", { name: "Import and validate" }).click();
  await expect(page.getByRole("alert")).toContainText("Import failed");
  await expect(rebuild).toBeDisabled();

  await deckText.fill("Deck\n3 Glasspool Mimic (ZNR) 60\n4 Bala Ged Recovery (ZNR) 180");
  await page.getByRole("button", { name: "Import and validate" }).click();
  await expect(page.getByText("60 cards active", { exact: true })).toBeVisible();
  await expect(page.getByText("This 60-card deck is the active evolutionary seed.")).toBeVisible();
  await expect(generations).toHaveValue("1");
  await expect(page.getByRole("button", { name: "Auto", exact: true })).toHaveClass(/selected/);
  await expect(page.getByRole("button", { name: "Red", exact: true })).toHaveAttribute("aria-pressed", "false");

  await page.getByRole("button", { name: "Tune deck" }).click();
  const run = await page.evaluate(() => (window as unknown as { __METAGENE_TEST_CALLS__: { command: string; args: unknown }[] }).__METAGENE_TEST_CALLS__.findLast((call) => call.command === "start_evolution"));
  expect(run?.args).toMatchObject({ request: { config: { generations: 1 }, seed_deck: { mainboard: [{ card_id: "imported-seed", quantity: 60 }] } } });
  expect(run?.args).toMatchObject({ request: { config: { evolution_mode: "tune", tune_policy: { strategy_retention_floor: 0.70, primary_package_floor: 0.60, maximum_nonland_edit_fraction: 0.40, root_deck: { name: "Imported Historic Deck" } } } } });
  expect((run?.args as { request: { constraints: { allowed_colors?: string[] } } }).request.constraints.allowed_colors).toBeUndefined();
});

test("detects a typed strategy after import and allows an explicit Tune or Rebuild choice", async ({ page }) => {
  await page.getByText("Import an existing deck", { exact: true }).click();
  await page.locator(".deck-import textarea").fill("Deck\n60 Strategy Seed");
  await page.getByRole("button", { name: "Import and validate" }).click();
  await expect(page.getByText("copy", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Tune this strategy" })).toHaveClass(/selected/);
  await expect(page.getByRole("button", { name: "Tune deck" })).toBeVisible();
  await page.getByRole("button", { name: "Rebuild", exact: true }).click();
  await expect(page.getByRole("button", { name: "Rebuild", exact: true })).toHaveClass(/selected/);
});

test("keeps a backend start failure visible beside the launch control", async ({ page }) => {
  await page.getByText("Import an existing deck", { exact: true }).click();
  await page.locator(".deck-import textarea").fill("Deck\n60 Start Failure");
  await page.getByRole("button", { name: "Import and validate" }).click();
  await page.getByRole("button", { name: "Tune deck" }).click();

  const alert = page.getByRole("alert");
  await expect(alert).toContainText("Evolution did not start");
  await expect(alert).toContainText("required seed card is outside the selected focused pool");
  await expect(alert).toContainText("Your imported seed is still active");
});

test("surfaces competitive truth, combo proof, MAP niches, and learned-device evidence", async ({ page }) => {
  await page.getByRole("button", { name: "Results" }).click();
  await page.getByText("Run history", { exact: true }).click();
  await page.getByRole("button", { name: "Open" }).click();
  await expect(page.getByText("1 preserved strategy niches")).toBeVisible();
  await expect(page.getByText("42 hypotheses")).toBeVisible();
  await page.getByText("Combo intelligence", { exact: true }).click();
  await page.getByRole("button", { name: "Scan candidate #1" }).click();
  await expect(page.getByText("Copy Engine + ETB Payoff")).toBeVisible();
  await expect(page.getByText("cross engine verified", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Build", exact: true }).click();
  await page.getByText("Import an existing deck", { exact: true }).click();
  await page.locator(".deck-import textarea").fill("Deck\n60 Competitive Seed");
  await page.getByRole("button", { name: "Import and validate" }).click();
  await page.getByRole("button", { name: "Tune deck" }).click();
  await page.locator("details.validation-lab > summary").click();
  await expect(page.getByText(/Competitive field audit · tournament/)).toBeVisible();
  await expect(page.getByText("Tournament eligible")).toBeVisible();
  await expect(page.getByText(/Locked engine route · Single-engine validated/)).toBeVisible();
  await expect(page.getByText(/5,000 \/ 5,000 completed, fully covered holdout labels/)).toBeVisible();
  await expect(page.getByText(/Burn WGPU 25,000 predictions\/s \(2.50×/)).toBeVisible();

  const commands = await page.evaluate(() => (window as unknown as { __METAGENE_TEST_CALLS__: { command: string }[] }).__METAGENE_TEST_CALLS__.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(["prove_deck_combos", "acquire_competitive_field", "build_engine_route_plan", "start_hybrid_evolution"]));
});
