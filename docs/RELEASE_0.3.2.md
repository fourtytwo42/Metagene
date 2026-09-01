# Metagene 0.3.2 — Closed-Loop Deck Quality

Metagene 0.3.2 closes evaluator loopholes exposed by two poor Historic decks after 100,000 structural generations. Catalog model v11 and structural revision `closed-loop-quality-v11` distinguish a missing printed mana cost from `{0}`, ignore mana text inherited from reminder-text tokens, require a direct activated mana ability, and conservatively reject unmet conditional activations. Mox Amber now derives colors only from controlled legendary creatures and planeswalkers. Trigger conditions no longer masquerade as their effects, and one ordered Oracle clause is not multiplied into duplicate compatibility actions.

Search targets are consumable across the whole deck, graveyard hate receives a one-copy mainboard target by default, excess copies carry a stronger deduction, and closing power combines static threat supply with turn-by-turn goldfish deployment. The submitted 100,000-generation outputs are retained under `data/evaluation/closed-loop-quality-v11-candidates.json` as regression evidence.

Blank competitive builds no longer fall back silently to an unrestricted all-catalog structural search. The primary Build action acquires or reuses a tournament field, starts from the highest-weight compatible legal archetype, focuses mutation on related lists, and validates the baseline and challengers through a rules engine. If no compatible field/engine route exists, Metagene stops with an actionable message. Explicit focused-pool exploration remains available under Advanced.

Long-run progress is bounded to 1,024 rendered points and deduplicated by generation. Backend events are emitted at most every 100 ms and durable checkpoints at most every second plus the first/final boundary; exact live and final counters remain intact.

Existing runs, fields, caches, art, feedback, engines, combo evidence, and training labels are retained. Older structural results remain inspectable and are rescored before continuation or validation.
