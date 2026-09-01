# Real-World Feedback

Metagene keeps real-match feedback in the local application-data directory in `feedback.sqlite`. It does not automate Arena, upload logs, or retain the pasted raw log. At most 20,000 normalized match rows are retained.

## Exact revision identity

Copying/exporting a deck registers an immutable revision from its platform, format, and canonical export text. Arena exports also receive a fingerprint of the sorted Arena card IDs in the main deck. An imported Arena result becomes evidence only when a deck event has exactly the same card-ID multiset as a registered export. Missing or ambiguous identity is marked `quarantined`; quarantined rows remain visible but never enter summaries or reweighting.

The feedback summary and weight suggestions are additionally scoped to the currently selected platform and format. This prevents, for example, Historic results from affecting Standard experiments.

## Supported imports

- Arena `Player.log` JSON lines containing `mainDeck`, `MainDeck`, `deckCards`, or `DeckCards`, followed by a `MatchGameRoomStateChangedEvent` or `DuelScene.GameStop` result. Embedded JSON telemetry with `teamId`, `startingTeamId`, and `winningTeamId` is supported.
- Portable JSON schema 1: `{"schema_version":1,"matches":[...]}`. Each match may contain `external_match_id`, `revision_id`, `occurred_at`, `result`, `opponent_archetype`, `play_draw`, `sideboarded`, `notes`, and `predicted_win_rate`.

Arena can change its diagnostic format. Unrecognized lines are counted and ignored. Turn on Arena's detailed logs before collecting a log. Wizards documents the Windows `Player.log` location in its [log-file support article](https://mtgarena-support.wizards.com/hc/en-us/articles/360000726823-Creating-Log-Files-on-PC-Mac-Steam).

## Evidence and approval

The editor can correct archetype, play/draw, sideboarding, prediction, and notes. Observed win rate and prediction error use exact revisions only. Suggested gauntlet weights require at least three exact matches against a named archetype; suggestions emphasize observed weaknesses, normalize to 100%, and do not alter the field until **Approve suggested weights** is pressed. Applying a suggestion changes only the current editable field—it does not rewrite old simulator runs.

## Privacy and recovery

**Export + copy** produces portable schema-1 JSON, including quarantined rows, and that export can be imported again. Individual rows or all normalized history can be deleted in the UI. Clearing match history deliberately keeps exported-deck revision hashes so later imports can still be associated; no raw log, account token, player name, or opponent name is stored by this subsystem.
