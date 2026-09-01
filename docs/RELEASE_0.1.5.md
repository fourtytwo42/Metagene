# Metagene 0.1.5

Metagene 0.1.5 hardens structural evaluation against restricted-mana and unsupported-synergy exploits with catalog model v4 and revision `restricted-mana-synergy-v4`.

- Physical mana sources now carry fixed or dynamic colors, timing/state, and exact spending predicates.
- Plaza of Heroes, Mech Hangar, Secluded Courtyard, Jasmine Dragon Tea Shop, Villainous Hideout, Maelstrom of the Spirit Dragon, Command Tower, and Mirrodin's Core receive explicit behavior.
- Command Tower produces nothing without a Commander. Unknown restricted colored mana is ignored; separate unconditional colorless abilities remain usable.
- Search actions require payable activation mana and a remaining valid target. Spinewoods Armadillo receives no fixing/ramp credit without a basic land or Desert.
- Per-card evidence reports on-curve/horizon castability and unrestricted, conditional, and unusable sources. Three-/four-copy core spells below the castability floor are blocked.
- Requirement/supply fulfillment, trial-observed legendary dead draws, and their fitness deductions are visible and configurable in Advanced settings.
- Blank starts use coherent one-to-five-color priors. Mandatory repair cannot use a source whose predicate excludes the deficient spell.
- The visible frontier retains the strongest feasible deck and favors candidates about five mainboard copy edits apart.
- All validation entrypoints perform a v4 rescore before spending a game budget. Legal rejected decks remain exportable.
- Catalog v4 rebuilds automatically while saved data and simulator caches remain intact. Older structural results remain inspectable and are marked for re-evaluation.
- The fixed 250-card restricted-source benchmark reaches 23,433 population slots/s and 1.43M structural trials/s on the release host, within the 2× 0.1.4 regression ceiling.

Windows installer: `artifacts/Metagene_0.1.5_x64-setup.exe`  
Size: 47,304,642 bytes  
SHA-256: `1BB9A4E38951781260C89B0F1ED50BFD3C6FF6B223054A5AD3F8598841AF905B`
