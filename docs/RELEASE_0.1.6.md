# Metagene 0.1.6

Metagene 0.1.6 hardens competitive evolution around revision `closed-loop-field-v5` while retaining catalog model v4.

- Structural exploration is explicitly labeled proxy-only; competitive claims require closed-loop rules-engine evidence.
- Blank constructed and Commander closed loops start from the highest-weight compatible legal field deck.
- Competitive mutation pools stay within the seed's overlapping field archetype, preventing unrelated module and tag-only card leakage.
- The exact incumbent remains a finalist and cannot be replaced by sampling noise, a worse confidence floor, or a worse field floor.
- Installed Forge performs a bounded independent challenger/incumbent cross-check for constructed cycles and can veto an engine-specific false improvement.
- Forge batches now have a process wall-clock deadline in addition to their per-game timeout.
- Automatic copy growth respects legendary roles across random, package, and initialization paths; copy legality is aggregated by canonical card name across printings.
- Blank Commander proxy seeds favor identity-producing basics and commander-relevant packages rather than random utility lands and unrelated themes.
- The repeatable quality harness covers parser-complete Historic Wizards, Affinity, and Jund seeds/opponents and records both HexDek and Forge results.

Windows installer: `artifacts/Metagene_0.1.6_x64-setup.exe`  
Size: 47,328,249 bytes  
SHA-256: `F835EED0D661457C55AB0BAA521DA4516CF528C22820DFA56E89131B5ACCEA8C`
