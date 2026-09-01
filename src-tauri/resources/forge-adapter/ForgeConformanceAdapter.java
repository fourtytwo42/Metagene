import com.google.common.eventbus.Subscribe;
import forge.GuiDesktop;
import forge.deck.Deck;
import forge.deck.io.DeckSerializer;
import forge.game.Game;
import forge.game.GameEndReason;
import forge.game.GameRules;
import forge.game.GameState;
import forge.game.GameType;
import forge.game.Match;
import forge.game.card.Card;
import forge.game.combat.Combat;
import forge.game.event.GameEvent;
import forge.game.player.RegisteredPlayer;
import forge.game.spellability.SpellAbility;
import forge.game.zone.ZoneType;
import forge.gui.GuiBase;
import forge.model.FModel;
import forge.player.GamePlayerUtil;
import forge.util.MyRandom;

import java.io.File;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Random;

/**
 * Small companion for Forge. Metagene bundles the reproducible 5-6 KiB compiled jar, so an end
 * user does not need a JDK, Forge checkout, Gradle cache, or generated class files.
 *
 * Protocol: stdout records intended for Metagene are prefixed with METAGENE_EVENT or
 * METAGENE_RESULT. Forge may write unrelated startup and game-log lines around them.
 */
public final class ForgeConformanceAdapter {
    private ForgeConformanceAdapter() {}

    public static void main(String[] args) {
        try {
            if (args.length == 3 && args[0].equals("scenario")) {
                initializeForge();
                runScenario(args[1], parseSeed(args[2]));
                System.exit(0);
            }
            if (args.length != 3) {
                System.err.println("usage: ForgeConformanceAdapter <deck-1.dck> <deck-2.dck> <seed>");
                System.err.println("   or: ForgeConformanceAdapter scenario <scenario-id> <seed>");
                System.exit(2);
            }
            long seed = parseSeed(args[2]);
            initializeForge();
            runMatchProbe(args[0], args[1], seed);
            System.exit(0);
        } catch (Throwable error) {
            error.printStackTrace(System.err);
            System.exit(1);
        }
    }

    private static void initializeForge() {
        System.setProperty("java.util.Arrays.useLegacyMergeSort", "true");
        System.setProperty("sun.java2d.d3d", "false");
        GuiBase.setInterface(new GuiDesktop());
        FModel.initialize(null, null);
    }

    private static long parseSeed(String value) {
        try {
            return Long.parseLong(value);
        } catch (NumberFormatException error) {
            System.err.println("seed must be a signed 64-bit integer");
            System.exit(2);
            return 0;
        }
    }

    private static void runMatchProbe(String firstPath, String secondPath, long seed) {
        MyRandom.setRandom(new Random(seed));

        Deck first = requireDeck(firstPath);
        Deck second = requireDeck(secondPath);
        List<RegisteredPlayer> players = new ArrayList<>();
        players.add(registeredPlayer(first, 0));
        players.add(registeredPlayer(second, 1));

        GameRules rules = new GameRules(GameType.Constructed);
        rules.setAppliedVariants(java.util.EnumSet.of(GameType.Constructed));
        Match match = new Match(rules, players, "Metagene conformance observation");
        Game game = match.createGame();
        CanonicalRecorder recorder = new CanonicalRecorder();
        game.subscribeToEvents(recorder);

        try {
            match.startGame(game);
        } catch (RuntimeException | StackOverflowError error) {
            emitResult(seed, recorder.sequence, "error", error.getClass().getName());
            throw error;
        } finally {
            game.setGameOver(GameEndReason.Draw);
        }

        String outcome = game.getOutcome().isDraw()
                ? "draw"
                : "winner:" + game.getOutcome().getWinningLobbyPlayer().getName();
        emitResult(seed, recorder.sequence, outcome, null);
    }

    private static void runScenario(String scenarioId, long seed) {
        if (scenarioId.equals("layers.clone-copies-base-before-anthem")) {
            runCloneLayerScenario(scenarioId, seed);
            return;
        }
        if (scenarioId.equals("targeting.lethal-damage-state-based-action")) {
            runTargetedLethalDamageScenario(scenarioId, seed);
            return;
        }
        if (scenarioId.equals("triggers.creature-etb-gains-life")) {
            runCreatureETBLifeTriggerScenario(scenarioId, seed);
            return;
        }
        if (scenarioId.equals("combat.unblocked-creature-damages-player")) {
            runUnblockedCombatDamageScenario(scenarioId, seed);
            return;
        }
        if (scenarioId.equals("multiplayer.combat-eliminates-one-opponent")) {
            runMultiplayerCombatEliminationScenario(scenarioId, seed);
            return;
        }
        if (scenarioId.equals("arena.oracle-of-the-alpha-conjures-power-nine")) {
            runOracleOfTheAlphaConjureScenario(scenarioId, seed);
            return;
        }
        if (scenarioId.equals("arena.rebalanced-falcon-abomination-has-flash")) {
            runRebalancedFalconAbominationFlashScenario(scenarioId, seed);
            return;
        }
        int initialLives;
        int mythosCasts;
        int turn;
        String manaPool;
        switch (scenarioId) {
            case "tokens.parallel-lives-copy" -> {
                initialLives = 1;
                mythosCasts = 1;
                turn = 4;
                manaPool = "G U U U";
            }
            case "tokens.stacked-parallel-lives-replacements" -> {
                initialLives = 2;
                mythosCasts = 1;
                turn = 5;
                manaPool = "G U U U";
            }
            case "tokens.copied-doubler-retains-ability" -> {
                initialLives = 1;
                mythosCasts = 2;
                turn = 6;
                manaPool = "G G U U U U U U";
            }
            default -> throw new IllegalArgumentException("unsupported scenario: " + scenarioId);
        }
        MyRandom.setRandom(new Random(seed));
        List<RegisteredPlayer> players = new ArrayList<>();
        players.add(registeredPlayer(fillerDeck("Scenario Seat 1", "Island"), 0));
        players.add(registeredPlayer(fillerDeck("Scenario Seat 2", "Forest"), 1));
        GameRules rules = new GameRules(GameType.Constructed);
        rules.setAppliedVariants(java.util.EnumSet.of(GameType.Constructed));
        Match match = new Match(rules, players, "Metagene canonical scenario");
        Game game = match.createGame();
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        game.subscribeToEvents(recorder);
        String[] result = new String[1];

        try {
            match.startGame(game, () -> {
                ImmediateGameState state = new ImmediateGameState();
                state.parse(List.of(
                        "turn=" + turn,
                        "activeplayer=p0",
                        "activephase=MAIN1",
                        "p0life=20",
                        "p0manapool=" + manaPool,
                        "p0battlefield=" + repeatedCards("Parallel Lives", initialLives),
                        "p0hand=" + repeatedCards("Mythos of Illuna", mythosCasts),
                        "p0library=",
                        "p1life=20",
                        "p1library="
                ));
                state.applyNow(game);
                recorder.reset();

                Card parallelLives = requireCard(game.getPlayers().get(0), ZoneType.Battlefield, "Parallel Lives");
                for (int cast = 0; cast < mythosCasts; cast++) {
                    castMythosAt(game, parallelLives);
                }
                game.getAction().checkStateEffects(true);

                int battlefieldCopies = countCards(game, ZoneType.Battlefield, 0, "Parallel Lives", false);
                int battlefieldTokens = countCards(game, ZoneType.Battlefield, 0, "Parallel Lives", true);
                int graveyardMythos = countCards(game, ZoneType.Graveyard, 0, "Mythos of Illuna", false);
                result[0] = scenarioResult(scenarioId, seed, recorder.sequence,
                        battlefieldCopies, battlefieldTokens, graveyardMythos);
                recorder.stop();
                game.setGameOver(GameEndReason.Draw);
            });
        } catch (RuntimeException | StackOverflowError error) {
            emitScenarioError(scenarioId, seed, recorder.sequence, error);
            throw error;
        }
        if (result[0] == null) {
            throw new IllegalStateException("scenario start hook did not run");
        }
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static Game scenarioGame(long seed, CanonicalRecorder recorder) {
        return scenarioGame(seed, recorder, 2);
    }

    private static Game scenarioGame(long seed, CanonicalRecorder recorder, int seats) {
        return scenarioGame(seed, recorder, seats, GameType.Constructed);
    }

    private static Game scenarioGame(long seed, CanonicalRecorder recorder, int seats, GameType gameType) {
        MyRandom.setRandom(new Random(seed));
        List<RegisteredPlayer> players = new ArrayList<>();
        String[] fillerCards = { "Island", "Forest", "Plains", "Mountain", "Swamp", "Wastes" };
        for (int seat = 0; seat < seats; seat++) {
            players.add(registeredPlayer(
                    fillerDeck("Scenario Seat " + (seat + 1), fillerCards[seat]), seat));
        }
        GameRules rules = new GameRules(gameType);
        rules.setAppliedVariants(java.util.EnumSet.of(gameType));
        Match match = new Match(rules, players, "Metagene canonical scenario");
        Game game = match.createGame();
        game.subscribeToEvents(recorder);
        return game;
    }

    private static void runCloneLayerScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder);
        String[] result = new String[1];
        try {
            game.getMatch().startGame(game, () -> {
                ImmediateGameState state = new ImmediateGameState();
                state.parse(List.of(
                        "turn=5",
                        "activeplayer=p0",
                        "activephase=MAIN1",
                        "p0life=20",
                        "p0manapool=U U U U",
                        "p0battlefield=Glorious Anthem;Grizzly Bears",
                        "p0hand=Clone",
                        "p0library=",
                        "p1life=20",
                        "p1library="
                ));
                state.applyNow(game);
                recorder.reset();
                castSpellAt(game, "Clone", null);
                game.getAction().checkStateEffects(true);

                Card clone = null;
                for (Card card : game.getPlayers().get(0).getCardsIn(ZoneType.Battlefield)) {
                    if (card.getName().equals("Grizzly Bears")) {
                        clone = card;
                    }
                }
                if (clone == null || countCards(game, ZoneType.Battlefield, 0, "Grizzly Bears", false) != 2) {
                    throw new IllegalStateException("Clone did not enter as the only available creature copy");
                }
                int basePower = reflectedInt(clone, "getBasePower");
                int baseToughness = reflectedInt(clone, "getBaseToughness");
                int effectivePower = reflectedInt(clone, "getNetPower", "getCurrentPower");
                int effectiveToughness = reflectedInt(clone, "getNetToughness", "getCurrentToughness");
                result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                        + ",\"scenario_version\":1,\"seed\":" + seed
                        + ",\"events\":" + recorder.sequence
                        + ",\"final_state\":{\"objects\":{\"clone\":{"
                        + "\"card_name\":\"Grizzly Bears\",\"base_power\":" + basePower
                        + ",\"base_toughness\":" + baseToughness
                        + ",\"effective_power\":" + effectivePower
                        + ",\"effective_toughness\":" + effectiveToughness + "}}}}";
                recorder.stop();
                game.setGameOver(GameEndReason.Draw);
            });
        } catch (RuntimeException | StackOverflowError error) {
            emitScenarioError(scenarioId, seed, recorder.sequence, error);
            throw error;
        }
        if (result[0] == null) {
            throw new IllegalStateException("scenario start hook did not run");
        }
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void runTargetedLethalDamageScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder);
        String[] result = new String[1];
        try {
            game.getMatch().startGame(game, () -> {
                ImmediateGameState state = new ImmediateGameState();
                state.parse(List.of(
                        "turn=3",
                        "activeplayer=p0",
                        "activephase=MAIN1",
                        "p0life=20",
                        "p0manapool=R",
                        "p0hand=Strangle",
                        "p0library=",
                        "p1life=20",
                        "p1battlefield=Grizzly Bears",
                        "p1library="
                ));
                state.applyNow(game);
                recorder.reset();
                Card bears = requireCard(game.getPlayers().get(1), ZoneType.Battlefield, "Grizzly Bears");
                castSpellAt(game, "Strangle", bears);
                game.getAction().checkStateEffects(true);

                int battlefieldBears = countCards(game, ZoneType.Battlefield, 1, "Grizzly Bears", false);
                int graveyardBears = countCards(game, ZoneType.Graveyard, 1, "Grizzly Bears", false);
                int graveyardStrangle = countCards(game, ZoneType.Graveyard, 0, "Strangle", false);
                int opponentLife = game.getPlayers().get(1).getLife();
                result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                        + ",\"scenario_version\":1,\"seed\":" + seed
                        + ",\"events\":" + recorder.sequence
                        + ",\"final_state\":{\"battlefield\":{\"seat-2\":{\"cards\":{"
                        + "\"Grizzly Bears\":{\"count\":" + battlefieldBears + "}}}},"
                        + "\"graveyard\":{\"seat-1\":{\"cards\":{\"Strangle\":{\"count\":"
                        + graveyardStrangle + "}}},\"seat-2\":{\"cards\":{\"Grizzly Bears\":{\"count\":"
                        + graveyardBears + "}}}},\"players\":{\"seat-2\":{\"life\":" + opponentLife + "}}}}";
                recorder.stop();
                game.setGameOver(GameEndReason.Draw);
            });
        } catch (RuntimeException | StackOverflowError error) {
            emitScenarioError(scenarioId, seed, recorder.sequence, error);
            throw error;
        }
        if (result[0] == null) {
            throw new IllegalStateException("scenario start hook did not run");
        }
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void runCreatureETBLifeTriggerScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder);
        String[] result = new String[1];
        try {
            game.getMatch().startGame(game, () -> {
                ImmediateGameState state = new ImmediateGameState();
                state.parse(List.of(
                        "turn=3",
                        "activeplayer=p0",
                        "activephase=MAIN1",
                        "p0life=20",
                        "p0manapool=W G G",
                        "p0hand=Ajani's Welcome;Grizzly Bears",
                        "p0library=",
                        "p1life=20",
                        "p1library="
                ));
                state.applyNow(game);
                castSpellAt(game, "Ajani's Welcome", null);
                recorder.reset();
                castSpellAt(game, "Grizzly Bears", null);
                game.getStack().addAllTriggeredAbilitiesToStack();
                if (!game.getStack().isEmpty()) {
                    game.getStack().resolveStack();
                }
                game.getAction().checkStateEffects(true);

                int welcomeCount = countCards(game, ZoneType.Battlefield, 0, "Ajani's Welcome", false);
                int bearsCount = countCards(game, ZoneType.Battlefield, 0, "Grizzly Bears", false);
                int life = game.getPlayers().get(0).getLife();
                result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                        + ",\"scenario_version\":1,\"seed\":" + seed
                        + ",\"events\":" + recorder.sequence
                        + ",\"final_state\":{\"players\":{\"seat-1\":{\"life\":" + life + "}},"
                        + "\"battlefield\":{\"seat-1\":{\"cards\":{\"Ajani's Welcome\":{\"count\":"
                        + welcomeCount + "},\"Grizzly Bears\":{\"count\":" + bearsCount + "}}}}}}";
                recorder.stop();
                game.setGameOver(GameEndReason.Draw);
            });
        } catch (RuntimeException | StackOverflowError error) {
            emitScenarioError(scenarioId, seed, recorder.sequence, error);
            throw error;
        }
        if (result[0] == null) {
            throw new IllegalStateException("scenario start hook did not run");
        }
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void runUnblockedCombatDamageScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder);
        String[] result = new String[1];
        game.getMatch().startGame(game, () -> {
            ImmediateGameState state = new ImmediateGameState();
            state.parse(List.of("turn=4", "activeplayer=p0", "activephase=COMBAT_DECLARE_ATTACKERS",
                    "p0life=20", "p0battlefield=Grizzly Bears", "p0library=", "p1life=20", "p1library="));
            state.applyNow(game);
            Card bears = requireCard(game.getPlayers().get(0), ZoneType.Battlefield, "Grizzly Bears");
            recorder.reset();
            Combat combat = new Combat(game.getPlayers().get(0));
            game.getPhaseHandler().setCombat(combat);
            combat.addAttacker(bears, game.getPlayers().get(1));
            setCardBoolean(bears, "setTapped", true);
            combat.fireTriggersForUnblockedAttackers(game);
            combat.assignCombatDamage(false);
            combat.dealAssignedDamage();
            game.getAction().checkStateEffects(true);
            int life = game.getPlayers().get(1).getLife();
            boolean declared = combat.getAttackers().contains(bears);
            result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                    + ",\"scenario_version\":1,\"seed\":" + seed + ",\"events\":" + recorder.sequence
                    + ",\"final_state\":{\"players\":{\"seat-2\":{\"life\":" + life + "}},"
                    + "\"battlefield\":{\"seat-1\":{\"cards\":{\"Grizzly Bears\":{\"count\":1}}}},"
                    + "\"objects\":{\"bears\":{\"tapped\":" + bears.isTapped() + ",\"declared_attacker\":" + declared + "}}}}";
            recorder.stop();
            game.setGameOver(GameEndReason.Draw);
        });
        if (result[0] == null) throw new IllegalStateException("scenario start hook did not run");
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void runMultiplayerCombatEliminationScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder, 3, GameType.Commander);
        String[] result = new String[1];
        game.getMatch().startGame(game, () -> {
            ImmediateGameState state = new ImmediateGameState();
            state.parse(List.of("turn=4", "activeplayer=p0", "activephase=COMBAT_DECLARE_ATTACKERS",
                    "p0life=20", "p0battlefield=Grizzly Bears", "p0library=",
                    "p1life=20", "p1library=",
                    "p2life=2", "p2battlefield=Forest", "p2library="));
            state.applyNow(game);
            Card bears = requireCard(game.getPlayers().get(0), ZoneType.Battlefield, "Grizzly Bears");
            forge.game.player.Player seatTwo = game.getPlayers().get(1);
            forge.game.player.Player seatThree = game.getPlayers().get(2);
            recorder.reset();
            Combat combat = new Combat(game.getPlayers().get(0));
            game.getPhaseHandler().setCombat(combat);
            combat.addAttacker(bears, seatThree);
            setCardBoolean(bears, "setTapped", true);
            combat.fireTriggersForUnblockedAttackers(game);
            combat.assignCombatDamage(false);
            combat.dealAssignedDamage();
            game.getAction().checkStateEffects(true);
            int seatTwoLife = seatTwo.getLife();
            int seatThreeLife = seatThree.getLife();
            boolean seatTwoLost = seatTwo.hasLost();
            boolean seatThreeLost = seatThree.hasLost();
            int seatThreeForests = 0;
            for (Card card : seatThree.getCardsIn(ZoneType.Battlefield)) {
                if (card.getName().equals("Forest")) seatThreeForests++;
            }
            boolean declared = combat.getAttackers().contains(bears);
            result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                    + ",\"scenario_version\":1,\"seed\":" + seed + ",\"events\":" + recorder.sequence
                    + ",\"final_state\":{\"players\":{\"seat-2\":{\"life\":" + seatTwoLife
                    + ",\"lost\":" + seatTwoLost + "},\"seat-3\":{\"life\":" + seatThreeLife
                    + ",\"lost\":" + seatThreeLost + "}},\"battlefield\":{\"seat-3\":{\"cards\":{\"Forest\":{\"count\":"
                    + seatThreeForests + "}}}},\"objects\":{\"bears\":{\"tapped\":" + bears.isTapped()
                    + ",\"declared_attacker\":" + declared + ",\"defender\":3}}}}";
            recorder.stop();
            game.setGameOver(GameEndReason.Draw);
        });
        if (result[0] == null) throw new IllegalStateException("scenario start hook did not run");
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void runOracleOfTheAlphaConjureScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder);
        String[] result = new String[1];
        game.getMatch().startGame(game, () -> {
            ImmediateGameState state = new ImmediateGameState();
            state.parse(List.of("turn=3", "activeplayer=p0", "activephase=MAIN1",
                    "p0life=20", "p0manapool=U U U", "p0hand=Oracle of the Alpha", "p0library=",
                    "p1life=20", "p1library="));
            state.applyNow(game);
            recorder.reset();
            castSpellAt(game, "Oracle of the Alpha", null);
            game.getStack().addAllTriggeredAbilitiesToStack();
            if (!game.getStack().isEmpty()) game.getStack().resolveStack();
            game.getAction().checkStateEffects(true);
            String[] powerNine = { "Ancestral Recall", "Black Lotus", "Mox Emerald", "Mox Jet",
                    "Mox Pearl", "Mox Ruby", "Mox Sapphire", "Time Walk", "Timetwister" };
            StringBuilder cards = new StringBuilder();
            int total = 0;
            for (int index = 0; index < powerNine.length; index++) {
                if (index > 0) cards.append(',');
                int count = countCards(game, ZoneType.Library, 0, powerNine[index], false);
                total += count;
                cards.append(quote(powerNine[index])).append(":{\"count\":").append(count).append('}');
            }
            int oracleCount = countCards(game, ZoneType.Battlefield, 0, "Oracle of the Alpha", false);
            result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                    + ",\"scenario_version\":1,\"seed\":" + seed + ",\"events\":" + recorder.sequence
                    + ",\"final_state\":{\"battlefield\":{\"seat-1\":{\"cards\":{\"Oracle of the Alpha\":{\"count\":"
                    + oracleCount + "}}}},\"library\":{\"seat-1\":{\"total_count\":" + total
                    + ",\"cards\":{" + cards + "}}}}}";
            recorder.stop();
            game.setGameOver(GameEndReason.Draw);
        });
        if (result[0] == null) throw new IllegalStateException("scenario start hook did not run");
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void runRebalancedFalconAbominationFlashScenario(String scenarioId, long seed) {
        CanonicalRecorder recorder = new CanonicalRecorder(false);
        Game game = scenarioGame(seed, recorder);
        String[] result = new String[1];
        game.getMatch().startGame(game, () -> {
            ImmediateGameState state = new ImmediateGameState();
            state.parse(List.of("turn=3", "activeplayer=p1", "activephase=MAIN1",
                    "p0life=20", "p0manapool=U U U",
                    "p0hand=Falcon Abomination;A-Falcon Abomination", "p0library=",
                    "p1life=20", "p1library="));
            state.applyNow(game);
            Card original = requireCard(game.getPlayers().get(0), ZoneType.Hand, "Falcon Abomination");
            Card rebalanced = requireCard(game.getPlayers().get(0), ZoneType.Hand, "A-Falcon Abomination");
            SpellAbility originalSpell = original.getSpells().get(0);
            SpellAbility rebalancedSpell = rebalanced.getSpells().get(0);
            originalSpell.setActivatingPlayer(game.getPlayers().get(0));
            rebalancedSpell.setActivatingPlayer(game.getPlayers().get(0));
            boolean originalLegal = originalSpell.canPlay();
            boolean rebalancedLegal = rebalancedSpell.canPlay();
            recorder.reset();
            if (rebalancedLegal) {
                castSpellAt(game, "A-Falcon Abomination", null);
            }
            int count = countCards(game, ZoneType.Battlefield, 0, "A-Falcon Abomination", false);
            result[0] = "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                    + ",\"scenario_version\":1,\"seed\":" + seed + ",\"events\":" + recorder.sequence
                    + ",\"final_state\":{\"timing\":{\"original_legal\":" + originalLegal
                    + ",\"rebalanced_legal\":" + rebalancedLegal
                    + "},\"battlefield\":{\"seat-1\":{\"cards\":{\"A-Falcon Abomination\":{\"count\":"
                    + count + "}}}}}}";
            recorder.stop();
            game.setGameOver(GameEndReason.Draw);
        });
        if (result[0] == null) throw new IllegalStateException("scenario start hook did not run");
        System.out.println("METAGENE_SCENARIO_RESULT " + result[0]);
    }

    private static void setCardBoolean(Card card, String method, boolean value) {
        try {
            card.getClass().getMethod(method, boolean.class).invoke(card, value);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Forge card has no " + method + " boolean setter", error);
        }
    }

    private static int reflectedInt(Card card, String... methodNames) {
        for (String methodName : methodNames) {
            try {
                Object value = card.getClass().getMethod(methodName).invoke(card);
                if (value instanceof Number number) {
                    return number.intValue();
                }
            } catch (ReflectiveOperationException ignored) {
                // Try the next stable accessor name used by supported Forge revisions.
            }
        }
        throw new IllegalStateException("Forge card has no requested numeric accessor");
    }

    private static String repeatedCards(String cardName, int count) {
        return String.join(";", java.util.Collections.nCopies(count, cardName));
    }

    private static void castMythosAt(Game game, Card parallelLives) {
        Card mythos = requireCard(game.getPlayers().get(0), ZoneType.Hand, "Mythos of Illuna");
        SpellAbility spell = mythos.getSpells().get(0);
        spell.setActivatingPlayer(game.getPlayers().get(0));
        if (!spell.canTarget(parallelLives) || !spell.getTargets().add(parallelLives)) {
            throw new IllegalStateException("Mythos of Illuna could not target Parallel Lives");
        }
        game.getStack().freezeStack(spell);
        spell.setHostCard(game.getAction().moveToStack(mythos, spell));
        game.getStack().addAndUnfreeze(spell);
        game.getStack().resolveStack();
        game.getAction().checkStateEffects(true);
    }

    private static void castSpellAt(Game game, String cardName, Card target) {
        Card card = requireCard(game.getPlayers().get(0), ZoneType.Hand, cardName);
        SpellAbility spell = card.getSpells().get(0);
        spell.setActivatingPlayer(game.getPlayers().get(0));
        if (target != null && (!spell.canTarget(target) || !spell.getTargets().add(target))) {
            throw new IllegalStateException(cardName + " could not target " + target.getName());
        }
        game.getStack().freezeStack(spell);
        spell.setHostCard(game.getAction().moveToStack(card, spell));
        game.getStack().addAndUnfreeze(spell);
        game.getStack().resolveStack();
        game.getAction().checkStateEffects(true);
    }

    private static Deck fillerDeck(String name, String cardName) {
        Deck deck = new Deck(name);
        deck.getMain().add(cardName, 60);
        return deck;
    }

    private static Card requireCard(forge.game.player.Player player, ZoneType zone, String name) {
        for (Card card : player.getCardsIn(zone)) {
            if (card.getName().equals(name)) {
                return card;
            }
        }
        throw new IllegalStateException("missing " + name + " in " + zone);
    }

    private static int countCards(Game game, ZoneType zone, int seat, String name, boolean tokensOnly) {
        int count = 0;
        for (Card card : game.getPlayers().get(seat).getCardsIn(zone)) {
            if (card.getName().equals(name) && (!tokensOnly || card.isToken())) {
                count++;
            }
        }
        return count;
    }

    private static String scenarioResult(String scenarioId, long seed, long events,
            int battlefieldCopies, int battlefieldTokens, int graveyardMythos) {
        return "{\"protocol_version\":1,\"scenario_id\":" + quote(scenarioId)
                + ",\"scenario_version\":1,\"seed\":" + seed
                + ",\"events\":" + events
                + ",\"final_state\":{\"battlefield\":{\"seat-1\":{"
                + "\"cards\":{\"Parallel Lives\":{\"count\":" + battlefieldCopies + "}},"
                + "\"tokens\":{\"Parallel Lives\":{\"count\":" + battlefieldTokens + "}}}},"
                + "\"graveyard\":{\"seat-1\":{\"cards\":{\"Mythos of Illuna\":{\"count\":"
                + graveyardMythos + "}}}}}}";
    }

    private static void emitScenarioError(String scenarioId, long seed, long events, Throwable error) {
        System.out.println("METAGENE_SCENARIO_RESULT {\"protocol_version\":1,\"scenario_id\":"
                + quote(scenarioId) + ",\"scenario_version\":1,\"seed\":" + seed
                + ",\"events\":" + events + ",\"error\":"
                + quote(error.getClass().getName() + ": " + error.getMessage()) + "}");
    }

    private static Deck requireDeck(String path) {
        File file = new File(path);
        Deck deck = DeckSerializer.fromFile(file);
        if (deck == null) {
            throw new IllegalArgumentException("Forge could not load deck: " + file.getAbsolutePath());
        }
        return deck;
    }

    private static RegisteredPlayer registeredPlayer(Deck deck, int seat) {
        String name = "Metagene-Seat-" + (seat + 1);
        RegisteredPlayer player = new RegisteredPlayer(deck);
        player.setPlayer(GamePlayerUtil.createAiPlayer(name, seat));
        return player;
    }

    private static void emitResult(long seed, long events, String outcome, String error) {
        StringBuilder json = new StringBuilder();
        json.append("{\"protocol_version\":1,\"seed\":").append(seed)
                .append(",\"events\":").append(events)
                .append(",\"outcome\":").append(quote(outcome));
        if (error != null) {
            json.append(",\"error\":").append(quote(error));
        }
        json.append('}');
        System.out.println("METAGENE_RESULT " + json);
    }

    public static final class CanonicalRecorder {
        private long sequence;
        private boolean enabled;

        public CanonicalRecorder() {
            this(true);
        }

        public CanonicalRecorder(boolean enabled) {
            this.enabled = enabled;
        }

        public void reset() {
            sequence = 0;
            enabled = true;
        }

        public void stop() {
            enabled = false;
        }

        @Subscribe
        public void receiveGameEvent(GameEvent event) {
            if (!enabled) {
                return;
            }
            String kind = canonicalKind(event.getClass().getSimpleName());
            if (kind == null) {
                return;
            }
            sequence++;
            StringBuilder json = new StringBuilder();
            json.append("{\"event_schema_version\":1,\"sequence\":").append(sequence)
                    .append(",\"kind\":").append(quote(kind))
                    .append(",\"forge_type\":").append(quote(event.getClass().getSimpleName()))
                    .append(",\"text\":").append(quote(event.toString()));
            appendAccessor(json, event, "card");
            appendAccessor(json, event, "from");
            appendAccessor(json, event, "to");
            appendAccessor(json, event, "hasFizzled");
            appendAccessor(json, event, "stackDescription");
            appendAccessor(json, event, "targetDescription");
            json.append('}');
            System.out.println("METAGENE_EVENT " + json);
        }

        private static void appendAccessor(StringBuilder json, Object event, String name) {
            try {
                Method accessor = event.getClass().getMethod(name);
                Object value = accessor.invoke(event);
                json.append(',').append(quote(name)).append(':')
                        .append(value == null ? "null" : quote(value.toString()));
            } catch (ReflectiveOperationException ignored) {
                // Not every Forge event record has every projected component.
            }
        }

        private static String canonicalKind(String forgeType) {
            return switch (forgeType) {
                case "GameEventSpellAbilityCast" -> "spell.cast";
                case "GameEventSpellResolved" -> "spell.resolved";
                case "GameEventSpellRemovedFromStack" -> "spell.removed";
                case "GameEventCardChangeZone" -> "zone.changed";
                case "GameEventTokenCreated" -> "token.created";
                case "GameEventCardCounters", "GameEventPlayerCounters" -> "counter.changed";
                case "GameEventCardDamaged", "GameEventPlayerDamaged" -> "damage.dealt";
                case "GameEventCardDestroyed" -> "card.destroyed";
                case "GameEventCardSacrificed" -> "card.sacrificed";
                case "GameEventCardTapped" -> "card.tapped";
                case "GameEventPlayerLivesChanged" -> "life.changed";
                case "GameEventTurnBegan" -> "turn.began";
                case "GameEventTurnPhase" -> "phase.changed";
                case "GameEventGameOutcome", "GameEventGameFinished" -> "game.outcome";
                default -> null;
            };
        }
    }

    private static String quote(String value) {
        if (value == null) {
            return "null";
        }
        StringBuilder result = new StringBuilder(value.length() + 2).append('"');
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            switch (character) {
                case '"' -> result.append("\\\"");
                case '\\' -> result.append("\\\\");
                case '\b' -> result.append("\\b");
                case '\f' -> result.append("\\f");
                case '\n' -> result.append("\\n");
                case '\r' -> result.append("\\r");
                case '\t' -> result.append("\\t");
                default -> {
                    if (character < 0x20) {
                        result.append(String.format(Locale.ROOT, "\\u%04x", (int) character));
                    } else {
                        result.append(character);
                    }
                }
            }
        }
        return result.append('"').toString();
    }

    private static final class ImmediateGameState extends GameState {
        void applyNow(Game game) {
            applyGameOnThread(game);
        }
    }
}
