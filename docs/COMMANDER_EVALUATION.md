# Commander Evaluation

Metagene treats Commander as a separate evaluation domain. A two-player Commander result is labeled `commander_duel`; a game with three to six players is labeled `multiplayer_pod`. The two modes are never presented as equivalent evidence.

## Command-zone legality

Command-zone pairing is driven by `data/commander/compatibility.schema1.json`, currently policy `commander-pairing-2026-08`. The same policy is used by legality checks, blank-deck generation, commander mutation, persisted candidate evidence, and the GUI. It supports Partner, Partner With, Friends Forever, Choose a Background, and Doctor's companion. The valid and invalid examples in `data/commander/command-zone-conformance.schema1.json` protect those rules from silent drift.

Library size, singleton/basic-land exceptions, color identity, and the selected platform/format catalog are still enforced independently of pairing compatibility.

## Pods and seat rotations

Users choose two through six seats and provide named opponent decks. Those deck names are retained as pod-archetype labels in the result. HexDek's `cyclic-deck-order-seeded-start-v1` policy rotates the candidate and opponents through seats from a reproducible seed. Fixed-order Best-of-Three work uses its separately labeled fixed-order policy.

The GUI specifies complete rotations, not a raw game count. The Rust validation and successive-halving APIs also round Commander budgets to a multiple of the pod size, so direct callers cannot accidentally schedule an incomplete rotation. Every report exposes pod size, seed, policy, complete rotations, partial games, per-seat results, and global seat-rate spread.

## Objective contract

`data/commander/multiplayer-objectives.schema1.json` is the versioned scoring contract. Ranking combines ordinary confidence-aware game evidence with the Commander objective using the policy's `ranking_weight`.

The objective contains:

- pod win share and the candidate's worst-seat win share;
- threat development from damage, peak board size, and spell rate;
- interaction from counterspells, removal, and opposing creatures killed;
- recovery from survival, recovered life, and creature exchanges;
- mana reliability from mana use and land development;
- closing ability from win share and average winning turn.

Pod outcomes, seats, game length, elimination order, draws, crashes, and concessions come directly from the rules engine. The managed HexDek adapter emits compact event-derived telemetry for the other dimensions and directed target counts. A result produced by an older worker clearly labels those dimensions `structural_proxy`; unavailable evidence is never described as native simulation.

## Failure and exploit handling

Draws and max-turn stalls, timeouts/crashes, concessions, seat bias, and the conservative kingmaking signal subtract from the Commander objective. A failed validation candidate receives a losing halving score rather than a useful result. Missing simulator cards reject a run, while normal workspace defaults exclude parser-incomplete, cross-engine-disputed, and unsupported cards from the evolutionary pool. These are hard gates against known simulator exploits, not bonus signals the optimizer can learn to seek.

The kingmaking diagnostic is intentionally narrow: it flags a losing player's unusually concentrated directed actions toward another non-winner while applying substantially less pressure to the eventual winner. It is a behavioral correlation, not a claim about intent.

## Limitations

Current AI does not model negotiation, promises, hidden social preferences, threat-table persuasion, or human political memory. It can therefore misjudge cards whose strength depends on diplomacy. Seat spread, elimination slots, directed targets, penalty components, metric sources, pod labels, and the exact policy ID are exposed so users can spot systematic pod or seat bias instead of accepting a single opaque score.

Commander evidence should be used to rank simulated candidates, then validated with held-out pods and human play. It is not a substitute for multiplayer playtesting.
