# Metagene 0.2.1 — Combo Intelligence

Metagene 0.2.1 adds a bounded generic combo solver and a versioned `combos.sqlite` knowledge store. Commander Spellbook supplies logically resolved, format-filtered hypotheses; corpus membership alone is never presented as proof.

Typed state transitions track zones, targets, timing, mana and nonmana costs, counters, life, permanents, tokens, tapping, sacrifice, discard, exile, recursion, copying, cost reduction, untapping, tutors, and once-per-turn restrictions. Bounded beam search plus strongly connected component analysis identifies finite wins and resource-positive repeatable loops, rejecting any loop that cannot legally pay for another iteration.

Proof levels are Corpus only, Symbolically proven, HexDek executed, Cross-engine verified, Arena verified, Disputed, and Unsupported. Exact deterministic conformance observations are stored with engine and card-data provenance. Only valid proven lines create a HexDek combo-plan overlay; ordinary Yggdrasil legality remains authoritative and safely abandons stale hints.

Tune detects causal engine, enabler, payoff, tutor, protection, and recovery roles. Combo packages can mutate coherently and are scored for assembly probability, expected turn, redundancy, dead-card burden, resilience, recovery, and observed execution telemetry.
