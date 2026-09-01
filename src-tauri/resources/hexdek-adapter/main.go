// Command metagene-conformance executes small, exact-state conformance
// scenarios against HexDek's production rules engine. The installer copies
// this file into the checked-out HexDek module so it may legally import the
// module's internal packages, then retains only the optimized executable.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/hexdek/hexdek/internal/astload"
	"github.com/hexdek/hexdek/internal/deckparser"
	"github.com/hexdek/hexdek/internal/gameengine"
	_ "github.com/hexdek/hexdek/internal/gameengine/per_card"
)

const resultPrefix = "METAGENE_HEXDEK_SCENARIO_RESULT "

type scenarioResult struct {
	ProtocolVersion uint32                 `json:"protocol_version"`
	ScenarioID      string                 `json:"scenario_id"`
	ScenarioVersion uint32                 `json:"scenario_version"`
	Seed            int64                  `json:"seed"`
	Events          []gameengine.Event     `json:"events"`
	FinalState      map[string]interface{} `json:"final_state"`
	Error           string                 `json:"error,omitempty"`
}

type scenarioConfig struct {
	initialLives int
	mythosCasts  int
	turn         int
}

func main() {
	scenario := flag.String("scenario", "", "canonical Metagene scenario id")
	seed := flag.Int64("seed", 0, "deterministic scenario seed")
	astPath := flag.String("ast", "", "HexDek AST dataset path")
	flag.Parse()

	result := scenarioResult{
		ProtocolVersion: 1,
		ScenarioID:      *scenario,
		ScenarioVersion: 1,
		Seed:            *seed,
		Events:          []gameengine.Event{},
		FinalState:      emptyFinalState(),
	}
	if *astPath == "" {
		result.Error = "--ast is required"
		emit(result)
		os.Exit(2)
	}
	var err error
	if config, ok := scenarioConfiguration(*scenario); ok {
		err = runParallelLivesCopy(&result, *astPath, config)
	} else {
		switch *scenario {
		case "layers.clone-copies-base-before-anthem":
			err = runCloneLayers(&result, *astPath)
		case "targeting.lethal-damage-state-based-action":
			err = runTargetedLethalDamage(&result, *astPath)
		case "triggers.creature-etb-gains-life":
			err = runCreatureETBLifeTrigger(&result, *astPath)
		case "combat.unblocked-creature-damages-player":
			err = runUnblockedCombatDamage(&result, *astPath)
		case "multiplayer.combat-eliminates-one-opponent":
			err = runMultiplayerCombatElimination(&result, *astPath)
		case "arena.oracle-of-the-alpha-conjures-power-nine":
			err = runOracleOfTheAlphaConjure(&result, *astPath)
		case "arena.rebalanced-falcon-abomination-has-flash":
			err = runRebalancedFalconAbominationFlash(&result, *astPath)
		default:
			err = fmt.Errorf("unsupported scenario %q", *scenario)
		}
	}
	if err != nil {
		result.Error = err.Error()
		emit(result)
		os.Exit(1)
	}
	emit(result)
}

func loadRuntime(astPath string, seats int, seed int64) (*gameengine.GameState, *astload.Corpus, *deckparser.MetaDB, error) {
	corpus, err := astload.Load(astPath)
	if err != nil {
		return nil, nil, nil, err
	}
	meta, err := deckparser.LoadMetaFromJSONL(astPath)
	if err != nil {
		return nil, nil, nil, err
	}
	return gameengine.NewGameStateSeeded(seats, seed, corpus), corpus, meta, nil
}

func battlefieldPermanent(gs *gameengine.GameState, card *gameengine.Card, controller int) *gameengine.Permanent {
	permanent := &gameengine.Permanent{
		Card:       card,
		Controller: controller,
		Owner:      card.Owner,
		Timestamp:  gs.NextTimestamp(),
		Counters:   map[string]int{},
		Flags:      map[string]int{},
	}
	gs.Seats[controller].Battlefield = append(gs.Seats[controller].Battlefield, permanent)
	gameengine.RegisterReplacementsForPermanent(gs, permanent)
	gameengine.FirePermanentETBTriggers(gs, permanent)
	return permanent
}

func resetCapturedEvents(gs *gameengine.GameState) {
	gs.EventLog = gs.EventLog[:0]
	gs.EventsLogged = 0
}

func runCloneLayers(result *scenarioResult, astPath string) error {
	gs, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn, gs.Phase, gs.Step, gs.Active = 5, "main", "precombat_main", 0
	gs.Seats[0].Life, gs.Seats[1].Life, gs.Seats[0].ManaPool = 20, 20, 4
	anthem, err := runtimeCard("Glorious Anthem", 0, corpus, meta)
	if err != nil {
		return err
	}
	battlefieldPermanent(gs, anthem, 0)
	bears, err := runtimeCard("Grizzly Bears", 0, corpus, meta)
	if err != nil {
		return err
	}
	bears.BasePower, bears.BaseToughness = 2, 2
	battlefieldPermanent(gs, bears, 0)
	cloneCard, err := runtimeCard("Clone", 0, corpus, meta)
	if err != nil {
		return err
	}
	gs.Seats[0].Hand = append(gs.Seats[0].Hand, cloneCard)
	resetCapturedEvents(gs)
	if err := gameengine.CastSpell(gs, 0, cloneCard, nil); err != nil {
		return fmt.Errorf("cast Clone: %w", err)
	}
	var clone *gameengine.Permanent
	for _, permanent := range gs.Seats[0].Battlefield {
		if permanent != nil && permanent.OriginalCard != nil && permanent.OriginalCard.DisplayName() == "Clone" {
			clone = permanent
			break
		}
	}
	if clone == nil {
		return fmt.Errorf("resolved Clone was not found on the battlefield")
	}
	base := gameengine.BaseCharacteristics(clone)
	effective := gameengine.GetEffectiveCharacteristics(gs, clone)
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = map[string]interface{}{
		"objects": map[string]interface{}{
			"clone": map[string]interface{}{
				"card_name":           clone.Card.DisplayName(),
				"base_power":          base.Power,
				"base_toughness":      base.Toughness,
				"effective_power":     effective.Power,
				"effective_toughness": effective.Toughness,
			},
		},
	}
	return nil
}

func runTargetedLethalDamage(result *scenarioResult, astPath string) error {
	gs, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn, gs.Phase, gs.Step, gs.Active = 3, "main", "precombat_main", 0
	gs.Seats[0].Life, gs.Seats[1].Life, gs.Seats[0].ManaPool = 20, 20, 1
	bears, err := runtimeCard("Grizzly Bears", 1, corpus, meta)
	if err != nil {
		return err
	}
	bears.BasePower, bears.BaseToughness = 2, 2
	bearsPermanent := battlefieldPermanent(gs, bears, 1)
	strangle, err := runtimeCard("Strangle", 0, corpus, meta)
	if err != nil {
		return err
	}
	gs.Seats[0].Hand = append(gs.Seats[0].Hand, strangle)
	resetCapturedEvents(gs)
	target := gameengine.Target{Kind: gameengine.TargetKindPermanent, Permanent: bearsPermanent}
	if err := gameengine.CastSpell(gs, 0, strangle, []gameengine.Target{target}); err != nil {
		return fmt.Errorf("cast Strangle: %w", err)
	}
	battlefieldBears, graveyardBears, graveyardStrangle := 0, 0, 0
	for _, permanent := range gs.Seats[1].Battlefield {
		if permanent != nil && permanent.Card != nil && permanent.Card.DisplayName() == "Grizzly Bears" {
			battlefieldBears++
		}
	}
	for _, card := range gs.Seats[1].Graveyard {
		if card != nil && card.DisplayName() == "Grizzly Bears" {
			graveyardBears++
		}
	}
	for _, card := range gs.Seats[0].Graveyard {
		if card != nil && card.DisplayName() == "Strangle" {
			graveyardStrangle++
		}
	}
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = map[string]interface{}{
		"battlefield": map[string]interface{}{
			"seat-2": map[string]interface{}{"cards": map[string]interface{}{"Grizzly Bears": map[string]int{"count": battlefieldBears}}},
		},
		"graveyard": map[string]interface{}{
			"seat-1": map[string]interface{}{"cards": map[string]interface{}{"Strangle": map[string]int{"count": graveyardStrangle}}},
			"seat-2": map[string]interface{}{"cards": map[string]interface{}{"Grizzly Bears": map[string]int{"count": graveyardBears}}},
		},
		"players": map[string]interface{}{"seat-2": map[string]int{"life": gs.Seats[1].Life}},
	}
	return nil
}

func runCreatureETBLifeTrigger(result *scenarioResult, astPath string) error {
	gs, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn, gs.Phase, gs.Step, gs.Active = 3, "main", "precombat_main", 0
	gs.Seats[0].Life, gs.Seats[1].Life, gs.Seats[0].ManaPool = 20, 20, 2
	welcome, err := runtimeCard("Ajani's Welcome", 0, corpus, meta)
	if err != nil {
		return err
	}
	battlefieldPermanent(gs, welcome, 0)
	bears, err := runtimeCard("Grizzly Bears", 0, corpus, meta)
	if err != nil {
		return err
	}
	bears.BasePower, bears.BaseToughness = 2, 2
	gs.Seats[0].Hand = append(gs.Seats[0].Hand, bears)
	resetCapturedEvents(gs)
	if err := gameengine.CastSpell(gs, 0, bears, nil); err != nil {
		return fmt.Errorf("cast Grizzly Bears: %w", err)
	}
	battlefieldWelcome, battlefieldBears := 0, 0
	for _, permanent := range gs.Seats[0].Battlefield {
		if permanent == nil || permanent.Card == nil {
			continue
		}
		switch permanent.Card.DisplayName() {
		case "Ajani's Welcome":
			battlefieldWelcome++
		case "Grizzly Bears":
			battlefieldBears++
		}
	}
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = map[string]interface{}{
		"players": map[string]interface{}{"seat-1": map[string]int{"life": gs.Seats[0].Life}},
		"battlefield": map[string]interface{}{
			"seat-1": map[string]interface{}{"cards": map[string]interface{}{
				"Ajani's Welcome": map[string]int{"count": battlefieldWelcome},
				"Grizzly Bears":   map[string]int{"count": battlefieldBears},
			}},
		},
	}
	return nil
}

func runUnblockedCombatDamage(result *scenarioResult, astPath string) error {
	gs, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn, gs.Phase, gs.Step, gs.Active = 4, "combat", "declare_attackers", 0
	gs.Seats[0].Life, gs.Seats[1].Life = 20, 20
	bears, err := runtimeCard("Grizzly Bears", 0, corpus, meta)
	if err != nil {
		return err
	}
	bears.BasePower, bears.BaseToughness = 2, 2
	perm := battlefieldPermanent(gs, bears, 0)
	perm.SummoningSick = false
	resetCapturedEvents(gs)
	attackers := gameengine.DeclareAttackers(gs, 0)
	blockers := gameengine.DeclareBlockers(gs, attackers, 1)
	gameengine.DealCombatDamageStep(gs, attackers, blockers, false)
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = map[string]interface{}{
		"players":     map[string]interface{}{"seat-2": map[string]int{"life": gs.Seats[1].Life}},
		"battlefield": map[string]interface{}{"seat-1": map[string]interface{}{"cards": map[string]interface{}{"Grizzly Bears": map[string]int{"count": len(gs.Seats[0].Battlefield)}}}},
		"objects":     map[string]interface{}{"bears": map[string]bool{"tapped": perm.Tapped}},
	}
	return nil
}

func runMultiplayerCombatElimination(result *scenarioResult, astPath string) error {
	gs, corpus, meta, err := loadRuntime(astPath, 3, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn, gs.Phase, gs.Step, gs.Active = 4, "combat", "declare_attackers", 0
	gs.Seats[0].Life, gs.Seats[1].Life, gs.Seats[2].Life = 20, 20, 2
	bears, err := runtimeCard("Grizzly Bears", 0, corpus, meta)
	if err != nil {
		return err
	}
	bears.BasePower, bears.BaseToughness = 2, 2
	bearsPermanent := battlefieldPermanent(gs, bears, 0)
	bearsPermanent.SummoningSick = false
	forest, err := runtimeCard("Forest", 2, corpus, meta)
	if err != nil {
		return err
	}
	battlefieldPermanent(gs, forest, 2)
	resetCapturedEvents(gs)
	attackers := gameengine.DeclareAttackers(gs, 0)
	blockers := gameengine.DeclareBlockersMulti(gs, attackers)
	gameengine.DealCombatDamageStep(gs, attackers, blockers, false)
	gameengine.StateBasedActions(gs)
	gs.CheckEnd()
	defender, declared := gameengine.AttackerDefender(bearsPermanent)
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = map[string]interface{}{
		"players": map[string]interface{}{
			"seat-2": map[string]interface{}{"life": gs.Seats[1].Life, "lost": gs.Seats[1].Lost},
			"seat-3": map[string]interface{}{"life": gs.Seats[2].Life, "lost": gs.Seats[2].Lost, "left_game": gs.Seats[2].LeftGame},
		},
		"battlefield": map[string]interface{}{
			"seat-3": map[string]interface{}{"cards": map[string]interface{}{"Forest": map[string]int{"count": len(gs.Seats[2].Battlefield)}}},
		},
		"objects": map[string]interface{}{
			"bears": map[string]interface{}{"tapped": bearsPermanent.Tapped, "declared_attacker": declared, "defender": defender + 1},
		},
	}
	return nil
}

func runOracleOfTheAlphaConjure(result *scenarioResult, astPath string) error {
	gs, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn, gs.Phase, gs.Step, gs.Active = 3, "main", "precombat_main", 0
	gs.Seats[0].Life, gs.Seats[1].Life, gs.Seats[0].ManaPool = 20, 20, 3
	oracle, err := runtimeCard("Oracle of the Alpha", 0, corpus, meta)
	if err != nil {
		return err
	}
	oracle.BasePower, oracle.BaseToughness = 2, 3
	gs.Seats[0].Hand = append(gs.Seats[0].Hand, oracle)
	resetCapturedEvents(gs)
	if err := gameengine.CastSpell(gs, 0, oracle, nil); err != nil {
		return fmt.Errorf("cast Oracle of the Alpha: %w", err)
	}
	powerNine := []string{"Ancestral Recall", "Black Lotus", "Mox Emerald", "Mox Jet", "Mox Pearl", "Mox Ruby", "Mox Sapphire", "Time Walk", "Timetwister"}
	counts := map[string]interface{}{}
	for _, name := range powerNine {
		counts[name] = map[string]int{"count": 0}
	}
	for _, card := range gs.Seats[0].Library {
		if card == nil {
			continue
		}
		if _, tracked := counts[card.DisplayName()]; tracked {
			counts[card.DisplayName()] = map[string]int{"count": 1}
		}
	}
	oracleCount := 0
	for _, permanent := range gs.Seats[0].Battlefield {
		if permanent != nil && permanent.Card != nil && permanent.Card.DisplayName() == "Oracle of the Alpha" {
			oracleCount++
		}
	}
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = map[string]interface{}{
		"battlefield": map[string]interface{}{"seat-1": map[string]interface{}{"cards": map[string]interface{}{"Oracle of the Alpha": map[string]int{"count": oracleCount}}}},
		"library":     map[string]interface{}{"seat-1": map[string]interface{}{"total_count": len(gs.Seats[0].Library), "cards": counts}},
	}
	return nil
}

func runRebalancedFalconAbominationFlash(result *scenarioResult, astPath string) error {
	originalGame, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	originalGame.Turn, originalGame.Phase, originalGame.Step, originalGame.Active = 3, "main", "precombat_main", 1
	originalGame.Seats[0].Life, originalGame.Seats[1].Life, originalGame.Seats[0].ManaPool = 20, 20, 3
	original, err := runtimeCard("Falcon Abomination", 0, corpus, meta)
	if err != nil {
		return err
	}
	originalGame.Seats[0].Hand = append(originalGame.Seats[0].Hand, original)
	originalErr := gameengine.CastSpell(originalGame, 0, original, nil)

	rebalancedGame, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	rebalancedGame.Turn, rebalancedGame.Phase, rebalancedGame.Step, rebalancedGame.Active = 3, "main", "precombat_main", 1
	rebalancedGame.Seats[0].Life, rebalancedGame.Seats[1].Life, rebalancedGame.Seats[0].ManaPool = 20, 20, 3
	rebalanced, err := runtimeCard("A-Falcon Abomination", 0, corpus, meta)
	if err != nil {
		return err
	}
	rebalanced.BasePower, rebalanced.BaseToughness = 2, 3
	rebalancedGame.Seats[0].Hand = append(rebalancedGame.Seats[0].Hand, rebalanced)
	resetCapturedEvents(rebalancedGame)
	rebalancedErr := gameengine.CastSpell(rebalancedGame, 0, rebalanced, nil)

	count := 0
	for _, permanent := range rebalancedGame.Seats[0].Battlefield {
		if permanent != nil && permanent.Card != nil && permanent.Card.DisplayName() == "A-Falcon Abomination" {
			count++
		}
	}
	result.Events = append(result.Events, rebalancedGame.EventLog...)
	result.FinalState = map[string]interface{}{
		"timing": map[string]interface{}{
			"original_legal":   originalErr == nil,
			"rebalanced_legal": rebalancedErr == nil,
		},
		"battlefield": map[string]interface{}{
			"seat-1": map[string]interface{}{
				"cards": map[string]interface{}{"A-Falcon Abomination": map[string]int{"count": count}},
			},
		},
	}
	return nil
}

func scenarioConfiguration(id string) (scenarioConfig, bool) {
	switch id {
	case "tokens.parallel-lives-copy":
		return scenarioConfig{initialLives: 1, mythosCasts: 1, turn: 4}, true
	case "tokens.stacked-parallel-lives-replacements":
		return scenarioConfig{initialLives: 2, mythosCasts: 1, turn: 5}, true
	case "tokens.copied-doubler-retains-ability":
		return scenarioConfig{initialLives: 1, mythosCasts: 2, turn: 6}, true
	default:
		return scenarioConfig{}, false
	}
}

func runParallelLivesCopy(result *scenarioResult, astPath string, config scenarioConfig) error {
	gs, corpus, meta, err := loadRuntime(astPath, 2, result.Seed)
	if err != nil {
		return err
	}
	gs.Turn = config.turn
	gs.Phase = "main"
	gs.Step = "precombat_main"
	gs.Active = 0
	gs.Seats[0].Life = 20
	gs.Seats[1].Life = 20
	gs.Seats[0].ManaPool = 4 * config.mythosCasts
	var parallelPermanent *gameengine.Permanent
	for index := 0; index < config.initialLives; index++ {
		parallel, err := runtimeCard("Parallel Lives", 0, corpus, meta)
		if err != nil {
			return err
		}
		permanent := &gameengine.Permanent{
			Card:       parallel,
			Controller: 0,
			Owner:      0,
			Timestamp:  gs.NextTimestamp(),
			Counters:   map[string]int{},
			Flags:      map[string]int{},
		}
		gs.Seats[0].Battlefield = append(gs.Seats[0].Battlefield, permanent)
		gameengine.RegisterTokenDoublerControllerOnly(gs, permanent)
		if parallelPermanent == nil {
			parallelPermanent = permanent
		}
	}
	// This is the same production replacement registered by HexDek's
	// Parallel Lives ETB hook. Scenario setup itself is intentionally absent
	// from the captured event stream.
	for index := 0; index < config.mythosCasts; index++ {
		mythos, err := runtimeCard("Mythos of Illuna", 0, corpus, meta)
		if err != nil {
			return err
		}
		gs.Seats[0].Hand = append(gs.Seats[0].Hand, mythos)
	}
	resetCapturedEvents(gs)

	for cast := 0; cast < config.mythosCasts; cast++ {
		mythos := gs.Seats[0].Hand[0]
		target := gameengine.Target{Kind: gameengine.TargetKindPermanent, Permanent: parallelPermanent}
		if err := gameengine.CastSpell(gs, 0, mythos, []gameengine.Target{target}); err != nil {
			return fmt.Errorf("cast Mythos of Illuna %d: %w", cast+1, err)
		}
	}
	result.Events = append(result.Events, gs.EventLog...)
	result.FinalState = finalState(gs)
	return nil
}

func runtimeCard(name string, owner int, corpus *astload.Corpus, meta *deckparser.MetaDB) (*gameengine.Card, error) {
	ast, ok := corpus.Get(name)
	if !ok || ast == nil {
		return nil, fmt.Errorf("AST corpus has no card %q", name)
	}
	md := meta.Get(name)
	if md == nil {
		return nil, fmt.Errorf("metadata corpus has no card %q", name)
	}
	return &gameengine.Card{
		AST:              ast,
		Name:             md.Name,
		Owner:            owner,
		Types:            append([]string(nil), md.Types...),
		Colors:           append([]string(nil), md.Colors...),
		CMC:              md.CMC,
		ManaCostString:   md.ManaCost,
		TypeLine:         strings.ToLower(md.TypeLine),
		BasePower:        md.Power,
		BaseToughness:    md.Toughness,
		BackFaceName:     md.BackFaceName,
		BackFaceCMC:      md.BackFaceCMC,
		BackFaceTypeLine: strings.ToLower(md.BackFaceTypeLine),
		BackFaceTypes:    append([]string(nil), md.BackFaceTypes...),
	}, nil
}

func finalState(gs *gameengine.GameState) map[string]interface{} {
	total, tokens, mythosGraveyard := 0, 0, 0
	for _, permanent := range gs.Seats[0].Battlefield {
		if permanent == nil || permanent.Card == nil || permanent.Card.DisplayName() != "Parallel Lives" {
			continue
		}
		total++
		if permanent.IsToken() {
			tokens++
		}
	}
	for _, card := range gs.Seats[0].Graveyard {
		if card != nil && card.DisplayName() == "Mythos of Illuna" {
			mythosGraveyard++
		}
	}
	return map[string]interface{}{
		"battlefield": map[string]interface{}{
			"seat-1": map[string]interface{}{
				"cards":  map[string]interface{}{"Parallel Lives": map[string]int{"count": total}},
				"tokens": map[string]interface{}{"Parallel Lives": map[string]int{"count": tokens}},
			},
		},
		"graveyard": map[string]interface{}{
			"seat-1": map[string]interface{}{
				"cards": map[string]interface{}{"Mythos of Illuna": map[string]int{"count": mythosGraveyard}},
			},
		},
	}
}

func emptyFinalState() map[string]interface{} {
	return map[string]interface{}{
		"battlefield": map[string]interface{}{"seat-1": map[string]interface{}{
			"cards":  map[string]interface{}{"Parallel Lives": map[string]int{"count": 0}},
			"tokens": map[string]interface{}{"Parallel Lives": map[string]int{"count": 0}},
		}},
		"graveyard": map[string]interface{}{"seat-1": map[string]interface{}{
			"cards": map[string]interface{}{"Mythos of Illuna": map[string]int{"count": 0}},
		}},
	}
}

func emit(result scenarioResult) {
	data, err := json.Marshal(result)
	if err != nil {
		fmt.Printf("%s{\"protocol_version\":1,\"error\":%q}\n", resultPrefix, err.Error())
		return
	}
	fmt.Printf("%s%s\n", resultPrefix, data)
}
