# Metagame Source Policy

Metagene only automates sources that explicitly document public programmatic access. Automatic competitive acquisition tries Spicerack first, TopDeck second when the user supplies a request-only API key, and GetPaird only as a public-list fallback. GetPaird evidence is permanently Provisional and cannot promote a candidate to Improved. Adapters are versioned, rate-limited by their upstream service, bounded by response-size and timeout limits, and must preserve enough provenance to reproduce the transformation. HTML scraping is not an acquisition adapter.

Before representative selection, every printing resolves to one logical card identity, exact fingerprints merge, and near-duplicates cluster by nonland copy-vector similarity, colors, commander identity, and strategy signature. Constructed selection begins with eight distinct archetypes and expands to at most sixteen until at least 90% of observed weight is represented. Commander uses five distinct legal opponents in a six-seat pod. The Field Audit retains source, date, occurrence weight, clustering decisions, represented/unrepresented weight, and quarantine reasons.

## Spicerack public decklist database

- Adapter: `spicerack-public-v1`
- Documentation: <https://docs.spicerack.gg/api-reference/public-decklist-database>
- Endpoint: `GET https://api.spicerack.gg/api/export-decklists/`
- Access: the provider describes the database endpoint as public and intended for community applications. The adapter supports an optional API key because the provider's example includes `X-API-Key`; keys are sent for the request only and are never included in reports, manifests, SQLite, logs, or portable bundles.
- Formats: Standard, Modern, Pioneer, Legacy, Vintage, Pauper, Historic, Explorer, Timeless, Commander, and Standard Brawl where the selected Metagene platform catalog supports the cards.

The adapter requests plaintext decklists, records the exact query URL and retrieval time, rejects events outside the selected observation window, resolves each card against the selected platform/format catalog, validates the complete deck, merges exact canonical deck genomes, and weights the survivors by observed occurrence count.

Spicerack does not provide canonical archetype labels in this endpoint. Metagene therefore creates a neutral editable label from prominent nonland cards; it never presents that generated label as upstream data. Missing lists, wrong formats, stale/future events, unknown cards, malformed text, and illegal decks remain inspectable in quarantine rather than silently disappearing.

## Adapter fixture rule

Every adapter must include immutable, synthetic or redistribution-safe response fixtures that cover accepted rows, duplicates, missing lists, malformed cards, format mismatch, stale data, and schema drift. A parser change that alters the accepted field or quarantine disposition must be explicit in tests and the adapter version.

## TopDeck.gg tournament API

- Adapter: `topdeck-tournaments-v2`
- Documentation: <https://topdeck.gg/docs/tournaments-v2>
- Endpoint: `POST https://topdeck.gg/api/v2/tournaments`
- Access: a free API key is required. TopDeck requires visible attribution, which the acquisition UI displays with a link to TopDeck.gg.

The key is sent only in the `Authorization` request header and is never persisted. The adapter requests completed Magic tournament standings for the chosen format and date window. Plaintext decklists use the same resolution, legality, merging, weighting, provenance, and quarantine pipeline as Spicerack. A returned external deck URL is not scraped; it is quarantined as missing plaintext content.

## GetPaird public deck library

- Adapter: `getpaird-public-decks-v1`
- Documentation: <https://getpaird.io/developers>
- Endpoints: `GET https://getpaird.io/api/v1/decks` and its documented public detail links
- Access: unauthenticated read-only feed, documented at 60 requests per minute per IP.

Metagene requests at most 25 recent public Magic decks, follows only the API detail URLs under the fixed GetPaird endpoint, and converts the structured command-zone/mainboard/sideboard quantities to the shared resolution pipeline. Published archetype labels are whitespace-normalized and retained; event timestamps use the source's `updated_at` value. The GUI visibly attributes GetPaird when selected.
