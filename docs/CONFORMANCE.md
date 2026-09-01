# Simulator conformance

Metagene separates four kinds of evidence that must not be conflated:

1. **Present** — the simulator corpus has a definition for the card.
2. **Parser-complete** — the simulator's Oracle parser reported no parse error.
3. **Interaction-verified** — a deterministic scenario proves the relevant state transition inside that engine.
4. **Cross-engine verified** — the same canonical scenario agrees with an independent rules engine or the live Arena client.

The first two levels are generated from HexDek's corpus. The desktop preflight also flags cards whose Oracle text contains high-risk mechanics—replacement effects, copying, control changes, turn changes, or Arena-only digital mechanics—even when the parser calls them complete. These warnings do not claim the card is wrong and do not block a run; they identify where matchup evidence is most vulnerable to simulator exploits.

## Canonical scenario contract

The checked-in suite lives in `data/conformance/scenarios.schema1.json` for path compatibility and now declares schema 2. It is parsed by `src-tauri/src/conformance_contract.rs`. Every scenario declares its own version, deterministic seed, constructed or Commander seat count, complete player state, aliased objects in explicit zones, ordered actions and choices, JSON Pointer assertions, the event kinds and payload fields that matter, and optional exact clause/action claims. All objects must use a card named in the scenario manifest. Unknown action verbs, aliases, zones, operators, versions, or non-contiguous action sequences are rejected before an engine runs. Stored schema-1 observations remain legacy card-level evidence.

Both adapters must return the same normalized final-state JSON. Metagene—not either simulator—evaluates the assertions and builds the normalized outcome map. Supported operators are `equals`, `not_equals`, `contains`, `count_equals`, `at_least`, and `at_most`; a missing JSON Pointer is a protocol error rather than a failed game rule.

Canonical events use schema 1 fields `sequence`, `kind`, optional `actor`, optional stable `source` alias, stable target aliases, and a JSON payload. Projection removes unrequested event kinds and engine-private payload fields, then renumbers the retained stream from one. Object keys are recursively sorted before SHA-256 hashing, while array and event order remain significant. The hash domain is `metagene.canonical-events.v1`, preventing a future schema from accidentally comparing equal to schema 1.

The first checked-in scenario, `tokens.parallel-lives-copy` version 1, specifies Mythos of Illuna copying Parallel Lives while one Parallel Lives is already on the battlefield. Its expected final state contains three Parallel Lives permanents, two token copies, and Mythos in its owner's graveyard.

## Production scenario adapters

Forge exposes a Guava event bus through `Game.subscribeToEvents`. The bundled companion in `src-tauri/resources/forge-adapter` registers a subscriber before `Match.startGame`, maps public `GameEvent` types into versioned JSON records, and prefixes its stdout protocol so Forge startup noise cannot be mistaken for evidence. Rust validates the protocol version, contiguous sequence, declared event count, result cardinality, and JSON structure. `forge::run_event_probe` writes disposable decks and the companion to a per-job application-data directory, invokes the installed Forge release, parses the records, and removes the entire job through its existing RAII cleanup.

The Forge companion source is checked in beside its compiled jar. It was compiled against Forge 2.0.14 with ECJ 3.38.0; `scripts/build-forge-conformance-adapter.ps1` reproduces it using a temporary compiler/build directory and removes those inputs afterward. Its canonical runner uses Forge's production `GameState`, a synchronous state-application hook at the first turn, the card's real `SpellAbility`, the public stack APIs, and actual final zones. Forge does not publish an explicit token-replacement event through `GameEvent`, so the scenario normalizer infers replacement application only when the raw stream contains more than one newly created token entering the battlefield. It never infers success from the expected assertion values.

HexDek revision `1b04ae5ae6d27264e1a983e0fa0aee58931fd08d` contains a parity framework, but its independent XMage adapter remains scaffolding. Metagene therefore builds its own small conformance companion against HexDek's production internal packages during the managed install. The checked-in source at `src-tauri/resources/hexdek-adapter/main.go` is copied into the temporary upstream module, compiled with `-trimpath -ldflags=-s -w`, and retained only as `hexdek-conformance`; the upstream source and Go caches are then deleted. The companion registers HexDek's full production per-card handler package and uses the real AST corpus, runtime card metadata, cast path, announced target, stack resolution, replacement registry, structured event log, and final zones.

The live seed-42 `tokens.parallel-lives-copy` run passes all three assertions in Forge and HexDek. Both normalize to four projected events—spell cast, replacement applied, two token copies represented as one quantity-bearing event, and Mythos moving from stack to graveyard—and both hash to `sha256:6075f701b27a244953d776b916e27592da13608e0bb6901419c0141e1c954d77`. `smoke_cross_engine_conformance` fails unless both assertions and both normalized event streams agree. This is genuine cross-engine evidence for this one interaction only; it does not generalize to every use of either card.

The managed Forge 2.0.14 matchup adapter is deliberately a different evidence path. It runs deterministic headless AI matches against each constructed opponent or a configured Commander pod and persists aggregate wins, games, draws/incomplete games, confidence intervals, and matchup matrices. Forge's stock quiet output also omits starting-player identity; reversing deck arguments would not prove play/draw balance because Forge chooses the first game's starting player through seeded game logic and normally lets the previous loser choose in subsequent games. Stock match results can corroborate or challenge a HexDek ranking but cannot be imported as interaction-verified or cross-engine-verified conformance evidence. Only a passing canonical scenario observation may promote the separately recorded event stream.

Metagene now has a compact, versioned scenario result store keyed by `(engine, engine revision, scenario, scenario version, seed)`. It retains only normalized outcomes, canonical event hashes, and affected card names in `conformance.sqlite` under application data. Imports are upserts and the store is capped at 100,000 observations; engine checkouts, JVM build products, and raw event logs remain disposable.

Every new result also carries provenance schema 1. The manifest records the engine build/adapter revision, the actual engine card-data revision (the Forge card-script archive digest or HexDek's Scryfall corpus timestamp), the scenario schema when applicable, and a canonical SHA-256 over every outcome-affecting configuration value. Conformance storage includes these dimensions in its identity, so changed card data or configuration creates separate evidence instead of overwriting or cross-verifying an older run. Matchup cache keys use the same engine/card/config inputs under cache domain v2, and the persisted report retains the manifest on both cold execution and cache hits. Legacy conformance rows migrate with `legacy-unknown` provenance and legacy matchup JSON remains readable, but the GUI tells users to rerun it before treating it as reproducible evidence.

The GUI accepts schema-1 JSON bundles and immediately audits the selected candidate and gauntlet. One passing observation is labeled interaction-verified. Cross-engine verification requires at least two distinct engine IDs to report the same passing normalized outcome and event hash for the same versioned scenario and seed. A failed observation is shown as failed; disagreement between observations is shown as a conflict and takes precedence over positive evidence for affected cards. Metagene never infers cross-engine verification from parser success or ordinary matchup results.

The user-facing taxonomy maps cross-engine agreement to **verified**, single-engine passing evidence to **partially covered**, disagreement to **disputed**, a consistently failing scenario to **unsupported**, and absence of relevant scenarios to **unverified**. These labels appear in pool search, focused-pool configuration, candidate inspection, deck validation, and the mechanic-scenario list. New workspaces exclude disputed and unsupported cards from evolution by default; each policy is independently configurable, and unverified cards remain visible rather than being silently treated as supported.

The current ten-scenario suite has eight current-run agreements and two visible disputes. The three token/copy scenarios agree, targeted lethal damage/SBA agrees, and `triggers.creature-etb-gains-life@1` proves Ajani's Welcome's observer trigger, trigger-stack resolution, and the 20→21 life change in both engines at `sha256:2c165f28ecc0ee224ff085c564ad7a607d80d294b5dbf7ba869bc2178a2d80cf`. `combat.unblocked-creature-damages-player@1` uses both engines' production combat paths to declare and tap Grizzly Bears, assign two unblocked combat damage to Seat 2, and verify the 20→18 life change; both projected traces hash to `sha256:6d63adf00b9ba89581d73afbafcbfbd1f0e9c920fb29d4330823f021f17b5247`.

`multiplayer.combat-eliminates-one-opponent@1` runs with three seats. Seat 1 chooses Seat 3 as the defender while Seat 2 remains at 20 life, Grizzly Bears deals lethal combat damage, Seat 3 loses at zero life, and Seat 3's owned Forest leaves the game. Forge runs the scenario under its Commander game type; HexDek uses its three-seat attacker targeting, multiplayer blocker partition, player-loss SBA, and §800.4a elimination cleanup. Both projected traces hash to `sha256:49f15486a8396361283aa780066549a2247a104988b00542a055c67899d2ad41`.

`arena.rebalanced-falcon-abomination-has-flash@1` compares the Arena rebalanced card with its paper original during Seat 2's main phase. Both engines reject Seat 1 casting the original Falcon Abomination, accept `A-Falcon Abomination` because its rebalanced definition has Flash, and resolve it onto Seat 1's battlefield. The canonical timing, cast, and zone trace agrees at `sha256:3ab1d50217f6b174543caed2030569c4e1b55c4d021c7549fa8fec3b32ed0eb6`.

`arena.oracle-of-the-alpha-conjures-power-nine@1` is a deliberate Arena-only disagreement. Forge's production card script resolves the ETB and places exactly one of every Power Nine card into the controller's library (`sha256:9a0ff2666b36dcde148fbdf4d7dd32eb9003fe09b8897fb034bf1dab8933d691`). HexDek fires the ETB but preserves Conjure as a parsed residual, creates no cards, fails the final-state assertions, and emits `effect.unsupported` (`sha256:397f5cd381de89fc862e3da0d61c53700caf00a4c5111ab47ceb4439fbab1f27`). The adapter reports that engine gap directly instead of manufacturing the missing cards.

`layers.clone-copies-base-before-anthem@1` remains disputed. Forge copies Grizzly Bears as base 2/2 and applies Glorious Anthem for effective 3/3 (`sha256:9b4ffab3bc04d09989b657829fb1b403fdafd6d5ac16b0fa4b829c2b085b44da`). HexDek copies the base characteristics but evaluates the result as 2/2 (`sha256:d620ea005f2d5feafd73b5ce7690240e7b067a8cfcc34701e88a88bec9fcd49b`): its parsed anthem modifier has no layer, so the production static-effect registration path skips it. Both traces and the failed assertion are locked into regressions rather than hidden by adapter-side correction.

```json
{
  "schema_version": 1,
  "producer": "forge-conformance-adapter 0.1.0",
  "observations": [
    {
      "engine": "forge",
      "engine_revision": "git-or-release-revision",
      "scenario_id": "tokens.parallel-lives",
      "scenario_version": 1,
      "seed": 42,
      "passed": true,
      "outcome": {
        "parallel_lives_permanents": 3,
        "created_tokens": 2
      },
      "event_hash": "sha256:canonical-event-stream-hash",
      "cards": ["Parallel Lives", "Mythos of Illuna"]
    }
  ]
}
```

JSON object keys are canonicalized before comparison. Array order and the event hash remain significant because they can encode game-event ordering. The Stage 1 category matrix is covered, but conformance is intentionally interaction-specific and should keep expanding across replacement ordering, stack copies, commander damage, hidden information, and additional Arena digital mechanics.

The local storage benchmark imports 10,000 observations representing 5,000 two-engine scenario runs, checks card-level evidence, and deletes its temporary database afterward. On the current development host it imported in 373 ms, audited in 100 ms, and used 3,526,536 bytes (about 353 bytes per observation). Run it with `cargo run -p metagene-app --example benchmark_conformance_store`.
