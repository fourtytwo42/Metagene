# Evolution Quality and Robustness

Metagene separates deck generation, model selection, and final measurement so a run cannot choose its winner by looking at its own holdout result.

## Evidence domains

Policy `evaluation-splits-2026-08` is stored in `data/evaluation/splits.schema1.json`.

- A cycle root deterministically creates distinct train, validation, and holdout seed domains.
- Structural evolution uses only the train seed. Successive halving selects finalists with the validation seed. The selected winner and final runner-up are measured afterward with the holdout seed.
- Constructed fields of at least three decks are partitioned 60/20/20 after sorting by canonical deck hash. Smaller fields and Commander pods reuse opponents because splitting would make the game invalid, but still use disjoint seed domains and disclose the reuse.
- Holdout results are persisted with the policy, all three seeds, field reuse, selection interval, holdout interval, and generalization gap. They may promote a deck for future runs, but never change the winner of the run that generated them.

## Confidence calibration

`calibration.sqlite` retains at most 2,000 cycle observations. It measures validation-interval coverage, validation-to-holdout error, and whether the winner/runner-up ordering survives on identical held-out games.

Ten observations are required before changing racing policy. If empirical coverage is below the 90% target, the next hybrid run automatically increases risk aversion by the bounded recommendation and disables statistical early elimination. Ordinary halving continues, so the run remains useful while collecting better evidence. Racing is restored when coverage supports it. The GUI reports the state and recommendation.

This policy is intentionally conservative: binomial confidence intervals describe repeated games against the measured opponent; they do not by themselves cover a shift to an unseen archetype.

## Historical pressure and co-evolution

`hall-of-fame.sqlite` retains at most 64 exact champions per platform/format/mode and 512 globally. Exact genomes deduplicate. Active historical opponents are chosen by a blend of evidence strength and minimum weighted-multiset Jaccard distance, preventing a family of near-clones from consuming the archive.

Constructed fields reserve a user-configurable weight for historical champions. Commander cycles rotate a historical deck through otherwise current pods. Optional co-evolution adds the final runner-up as a bounded, weighted opponent for later cycles. All additions persist in recovery checkpoints.

## Guided operators

Mana mutation estimates curve-weighted colored demand and replaces off-color sources with deficit sources while respecting land-count bounds. Its rate is user-configurable. Package discovery mines recurring nonland pairs supported by at least two diverse successful decks; discovered packages extend, rather than replace, card-text and subtype packages.

## Reproducible gates

- `cargo run -p metagene-core --example benchmark_stage5_operators` compares disabled versus guided mana/package operators under the same seed, 1,280 unique evaluations, and five-second wall-clock cap.
- `cargo run -p metagene-app --example smoke_stage5_robustness` uses the installed Historic catalog and real HexDek games. It asserts disjoint fields/seeds, selects only on validation, compares the champion with a fixed baseline on holdout, records ten calibration observations, exercises the conservative feedback decision, promotes two finalists, and mines packages. Its temporary Hall and calibration databases are deleted on exit.

The live gate is a regression scenario, not a claim that one small synthetic field represents the Historic metagame. Simulator coverage and known conformance disputes remain attached to the result.
