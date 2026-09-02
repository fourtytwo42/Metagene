# Metagene 0.3.3 — Competitive Blank-Build Launch Fix

Metagene 0.3.3 fixes a launch failure in **Build competitive deck**. When tournament-field acquisition succeeded but rules-engine validation could not launch, 0.3.2 retried the original empty request and the backend rejected it as an unsafe all-catalog search.

The fallback now asks the backend to validate the acquired field, select its highest-weight compatible archetype as the seed, focus the mutation pool around that field, and start structural evolution with a clear **Provisional** label. Fully covered closed-loop evolution remains preferred, and genuinely ungrounded blank searches remain blocked.

This release changes no catalog, run, cache, art, or evidence schema. Existing application data is retained during an in-place upgrade.
