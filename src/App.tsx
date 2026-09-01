import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Card,
  ArtCacheStatus,
  CatalogProgress,
  CatalogStatus,
  CalibrationStatus,
  CardConformance,
  ConformanceAuditReport,
  DeckConformanceReport,
  ConformanceImportResult,
  ConformanceSummary,
  CompetitiveFieldAcquisition,
  ComboCorpusStatus,
  ComboCorpusSyncReport,
  ComboProofReport,
  ContinuedRun,
  CommanderConformanceReport,
  Deck,
  EngineRoutePlan,
  GenerationProgress,
  GauntletMetadata,
  GauntletComparison,
  GauntletProvenance,
  GauntletSummary,
  HallOfFameEntry,
  HallOfFameStatus,
  MatchImportReport,
  MatchRecord,
  FeedbackSummary,
  ReweightSuggestion,
  HalvingProgress,
  HexdekCoverageReport,
  HybridComplete,
  HybridEvolutionRequest,
  HybridProgress,
  JobStatus,
  InstallerProgress,
  ImportedMetagameBundle,
  MetagameAcquisitionReport,
  MetagameFieldRevision,
  RankedDeck,
  RunRequest,
  ResumedRun,
  RunSummary,
  SavedHalving,
  SavedGauntlet,
  SavedValidation,
  SavedRun,
  BestOfThreeSimulationReport,
  SideboardEvolutionReport,
  SideboardExchange,
  SimulationReport,
  SimulationCacheStatus,
  SuccessiveHalvingReport,
  SurrogateStatus,
  SimulatorStatus,
  ResourceBudgetStatus,
  StorageHealth,
  SystemProfile
} from "./types";

type ProgressEnvelope = { job_id: string; progress: GenerationProgress };
type CompleteEnvelope = { job_id: string; status: JobStatus };
type WorkspaceView = "build" | "results" | "library" | "settings";
type DeckColor = "White" | "Blue" | "Black" | "Red" | "Green" | "Colorless";
type SearchPreset = "quick" | "balanced" | "deep" | "custom";

const VIEW_LABELS: Record<WorkspaceView, string> = {
  build: "Build",
  results: "Results",
  library: "Card Library",
  settings: "Settings"
};

const DECK_COLORS: { value: DeckColor; symbol: string; label: string }[] = [
  { value: "White", symbol: "W", label: "White" },
  { value: "Blue", symbol: "U", label: "Blue" },
  { value: "Black", symbol: "B", label: "Black" },
  { value: "Red", symbol: "R", label: "Red" },
  { value: "Green", symbol: "G", label: "Green" },
  { value: "Colorless", symbol: "C", label: "Colorless" }
];

const SEARCH_PRESETS: Record<Exclude<SearchPreset, "custom">, { population_size: number; generations: number; trials_per_candidate: number }> = {
  quick: { population_size: 64, generations: 20, trials_per_candidate: 96 },
  balanced: { population_size: 128, generations: 50, trials_per_candidate: 256 },
  deep: { population_size: 512, generations: 200, trials_per_candidate: 512 }
};

const percent = (value: number) => `${Math.round(value * 100)}%`;
const number = (value: number) => new Intl.NumberFormat().format(Math.round(value));
const bytes = (value: number) => value < 1024
  ? `${value} B`
  : value < 1024 * 1024
    ? `${(value / 1024).toFixed(1)} KiB`
    : `${(value / (1024 * 1024)).toFixed(1)} MiB`;

const deckGenomeKey = (deck: Deck) => JSON.stringify([
  [...deck.command_zone].sort((left, right) => left.card_id.localeCompare(right.card_id)),
  [...deck.mainboard].sort((left, right) => left.card_id.localeCompare(right.card_id)),
  [...deck.sideboard].sort((left, right) => left.card_id.localeCompare(right.card_id))
]);

const wilsonBounds = (wins: number, games: number): [number, number] => {
  if (!games) return [0, 1];
  const z = 1.959963984540054;
  const p = Math.min(wins, games) / games;
  const z2 = z * z;
  const denominator = 1 + z2 / games;
  const center = (p + z2 / (2 * games)) / denominator;
  const margin = z * Math.sqrt(p * (1 - p) / games + z2 / (4 * games * games)) / denominator;
  return [Math.max(0, center - margin), Math.min(1, center + margin)];
};

const seatPositionSummary = (report: SimulationReport, deck: string | undefined) => {
  const totals = new Map<number, { wins: number; games: number }>();
  for (const row of report.seat_order || []) {
    if (row.games <= 0 || (deck && row.deck !== deck)) continue;
    const total = totals.get(row.position) || { wins: 0, games: 0 };
    total.wins += row.wins;
    total.games += row.games;
    totals.set(row.position, total);
  }
  const positions = [...totals.entries()]
    .filter(([, total]) => total.games > 0)
    .map(([position, total]) => ({
      position,
      ...total,
      winRate: total.wins / total.games,
      lowerBound: wilsonBounds(total.wins, total.games)[0]
    }));
  if (!positions.length) return undefined;
  return {
    positions,
    worstRate: Math.min(...positions.map((position) => position.winRate)),
    worstLower: Math.min(...positions.map((position) => position.lowerBound)),
    spread: Math.max(...positions.map((position) => position.winRate)) - Math.min(...positions.map((position) => position.winRate))
  };
};

const evidenceScore = (report: SimulationReport, deck: string | undefined, riskAversion: number, worstWeight: number, seatWeight: number) => {
  const row = deck ? report.rows.find((item) => item.deck === deck) : report.rows[0];
  if (!row) return 0;
  const lower = row.win_rate_lower_bound ?? wilsonBounds(row.wins, row.games)[0];
  let worstRate = row.win_rate;
  let worstLower = lower;
  const matchupWins = deck ? report.matchup_wins?.[deck] : undefined;
  const matchupGames = deck ? report.matchup_games?.[deck] : undefined;
  if (matchupWins && matchupGames) {
    for (const [opponent, wins] of Object.entries(matchupWins)) {
      const games = matchupGames[opponent] || 0;
      if (!games) continue;
      worstRate = Math.min(worstRate, wins / games);
      worstLower = Math.min(worstLower, wilsonBounds(wins, games)[0]);
    }
  }
  const risk = Math.max(0, Math.min(1, riskAversion));
  const worst = Math.max(0, Math.min(1, worstWeight));
  const overallEvidence = row.win_rate * (1 - risk) + lower * risk;
  const worstEvidence = worstRate * (1 - risk) + worstLower * risk;
  const matchupEvidence = overallEvidence * (1 - worst) + worstEvidence * worst;
  const position = seatPositionSummary(report, row.deck);
  const positionWeight = Math.max(0, Math.min(1, seatWeight));
  const positionEvidence = position
    ? position.worstRate * (1 - risk) + position.worstLower * risk
    : matchupEvidence;
  const crashRate = report.crashes / Math.max(1, row.games + report.crashes);
  return Math.max(0, Math.min(1, matchupEvidence * (1 - (position ? positionWeight : 0)) + positionEvidence * (position ? positionWeight : 0) - crashRate));
};

const FORMATS: Record<string, { value: string; label: string }[]> = {
  arena: [
    { value: "standard", label: "Standard" },
    { value: "alchemy", label: "Alchemy" },
    { value: "explorer", label: "Explorer" },
    { value: "historic", label: "Historic" },
    { value: "timeless", label: "Timeless" },
    { value: "brawl", label: "Brawl" }
  ],
  paper: [
    { value: "standard", label: "Standard" },
    { value: "pioneer", label: "Pioneer" },
    { value: "modern", label: "Modern" },
    { value: "legacy", label: "Legacy" },
    { value: "vintage", label: "Vintage" },
    { value: "pauper", label: "Pauper" },
    { value: "commander", label: "Commander" }
  ],
  mtgo: [
    { value: "standard", label: "Standard" },
    { value: "pioneer", label: "Pioneer" },
    { value: "modern", label: "Modern" },
    { value: "legacy", label: "Legacy" },
    { value: "vintage", label: "Vintage" },
    { value: "pauper", label: "Pauper" },
    { value: "commander", label: "Commander" }
  ]
};

const DEFAULT_FITNESS_WEIGHTS = {
  consistency: 30,
  tempo: 24,
  synergy: 20,
  interaction: 12,
  resilience: 14,
  commander: 8,
  sideboard: 8,
  mana_failure_penalty: 35,
  legendary_dead_draw_penalty: 12,
  unsupported_synergy_penalty: 20,
  dead_action_penalty: 18,
  narrow_card_penalty: 12,
  closing_power: 18,
  role_balance: 10
};
const CURRENT_STRUCTURAL_REVISION = "closed-loop-quality-v11";
const MAX_HISTORY_POINTS = 1024;

function appendHistoryPoint(values: number[], score: number) {
  if (values.length < MAX_HISTORY_POINTS) return [...values, score];
  return [...values.slice(-(MAX_HISTORY_POINTS - 1)), score];
}

function Metric({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div className={`metric ${tone || ""}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function Sparkline({ values }: { values: number[] }) {
  if (values.length < 2) return <div className="sparkline empty" />;
  const width = 640;
  const height = 120;
  let minimum = values[0];
  let maximum = values[0];
  for (const value of values) {
    minimum = Math.min(minimum, value);
    maximum = Math.max(maximum, value);
  }
  const spread = Math.max(maximum - minimum, 0.001);
  const points = values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * width;
      const y = height - ((value - minimum) / spread) * (height - 18) - 9;
      return `${x},${y}`;
    })
    .join(" ");
  return (
    <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none">
      <defs>
        <linearGradient id="scoreArea" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#8290ff" stopOpacity=".32" />
          <stop offset="100%" stopColor="#8290ff" stopOpacity="0" />
        </linearGradient>
      </defs>
      <polygon points={`0,${height} ${points} ${width},${height}`} fill="url(#scoreArea)" />
      <polyline points={points} fill="none" stroke="#8290ff" strokeWidth="3" />
    </svg>
  );
}

const classificationLabel = (classification: CardConformance["classification"]) =>
  classification.replaceAll("_", " ");

function ConformanceBadge({ evidence }: { evidence?: CardConformance }) {
  const classification = evidence?.classification || "unverified";
  return <span className={`conformance-badge ${classification}`} title={evidence?.scenarios.join(" · ") || "No canonical scenario evidence"}>{classificationLabel(classification)}</span>;
}

function CardTile({ card, quantity, conformance, rationale }: { card: Card; quantity: number; conformance?: CardConformance; rationale?: string }) {
  return (
    <article className="card-tile">
      <div className="card-art-wrap">
        <CachedCardArt card={card} alt={card.name} />
        <span className="quantity">{quantity}</span>
      </div>
      <div className="card-caption">
        <strong>{card.name}</strong>
        <span>{card.set_code.toUpperCase()} · {card.features.mana_value || "—"}</span>
        <ConformanceBadge evidence={conformance} />
        {rationale && <small title="Automatic copy-distribution role">{rationale}</small>}
      </div>
    </article>
  );
}

function CachedCardArt({ card, alt = "" }: { card: Card; alt?: string }) {
  const [source, setSource] = useState<string>();
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    let objectUrl: string | undefined;
    setSource(undefined);
    setFailed(false);
    if (!card.image_uri) {
      setFailed(true);
      return () => undefined;
    }
    void invoke<ArrayBuffer | Uint8Array | number[]>("card_art", { imageUri: card.image_uri })
      .then((payload) => {
        const imageBytes = payload instanceof ArrayBuffer
          ? new Uint8Array(payload)
          : payload instanceof Uint8Array
            ? payload
            : new Uint8Array(payload);
        const ownedBuffer = Uint8Array.from(imageBytes).buffer;
        objectUrl = URL.createObjectURL(new Blob([ownedBuffer], { type: "image/jpeg" }));
        if (disposed) URL.revokeObjectURL(objectUrl);
        else {
          setSource(objectUrl);
          window.dispatchEvent(new Event("metagene-art-cache-changed"));
        }
      })
      .catch(() => {
        if (!disposed) setFailed(true);
      });
    return () => {
      disposed = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [card.id, card.image_uri]);

  return source
    ? <img src={source} alt={alt} loading="lazy" />
    : <span className={`art-placeholder ${failed ? "failed" : ""}`} aria-label={alt || undefined} />;
}

function CandidateInspector({ candidate, compareTo, cards, conformance, platform, format, lockedCardIds = [], provisionalReason, onContinue, additionalGenerations }: { candidate?: RankedDeck; compareTo?: RankedDeck; cards: Map<string, Card>; conformance: Map<string, CardConformance>; platform: string; format: string; lockedCardIds?: string[]; provisionalReason?: string; onContinue?: () => void; additionalGenerations?: number }) {
  const [exportText, setExportText] = useState("");
  if (!candidate) {
    return <div className="empty-state">Run an evolution to inspect its strongest candidates.</div>;
  }

  const exportDeck = async () => {
    const text = await invoke<string>("export_deck", { deck: candidate.deck, platform, format });
    setExportText(text);
    await navigator.clipboard.writeText(text).catch(() => undefined);
  };
  const curve = Array.from({ length: 8 }, (_, bucket) => candidate.deck.mainboard.reduce((total, entry) => {
    const card = cards.get(entry.card_id);
    return total + (card && !card.features.is_land && Math.min(7, Math.floor(card.features.mana_value)) === bucket ? entry.quantity : 0);
  }, 0));
  const maximumCurve = Math.max(1, ...curve);
  const comparison = compareTo && compareTo.rank !== candidate.rank ? (() => {
    const counts = (deck: Deck) => new Map(deck.mainboard.map((entry) => [entry.card_id, entry.quantity]));
    const current = counts(candidate.deck); const baseline = counts(compareTo.deck);
    return [...new Set([...current.keys(), ...baseline.keys()])].map((id) => ({ id, delta: (current.get(id) || 0) - (baseline.get(id) || 0) })).filter((row) => row.delta !== 0).sort((left, right) => Math.abs(right.delta) - Math.abs(left.delta));
  })() : [];
  const packageExplanation = (tag: string) => tag.startsWith("type:") ? `Creature-type density supporting ${tag.slice(5)} payoffs` : ({ draw: "Card-flow engine and payoff density", ramp: "Mana acceleration and expensive-payoff support", interaction: "Removal, counters, and disruption working together", recursion: "Graveyard recovery and reusable value", tokens: "Token production and token-scaling payoffs" } as Record<string, string>)[tag] || "Cards sharing this catalog mechanic tag";
  const copyEvidence = new Map((candidate.fitness.copy_evidence || []).map((item) => [item.card_id, item]));
  const lockedCards = new Set(lockedCardIds);
  const copyRationale = (card: Card) => {
    if (lockedCards.has(card.id)) return "user-locked";
    const evidence = copyEvidence.get(card.id);
    if (!evidence) return card.features.is_land ? "land" : "legacy—unclassified";
    return `${evidence.rationale} · target ${evidence.functional_target}${evidence.excess_copies ? ` · ${evidence.excess_copies} excess` : ""}`;
  };
  const sourceLabels = ["W", "U", "B", "R", "G", "C"];
  const dedicatedLands = candidate.deck.mainboard.reduce((total, entry) => total + ((cards.get(entry.card_id)?.features.initial_modes || []).every((mode) => mode.is_land) ? entry.quantity : 0), 0);
  const modalLands = candidate.deck.mainboard.reduce((total, entry) => { const modes = cards.get(entry.card_id)?.features.initial_modes || []; return total + (modes.some((mode) => mode.is_land) && modes.some((mode) => !mode.is_land) ? entry.quantity : 0); }, 0);
  const statusLabel = candidate.candidate_status === "provisional" && provisionalReason
    ? `Provisional—${provisionalReason}`
    : (candidate.candidate_status || "provisional").replaceAll("_", " ");

  return (
    <section className="inspector">
      <header className="inspector-head">
        <div>
          <span className="eyebrow">{candidate.candidate_status === "baseline" ? "Original baseline" : `Candidate #${candidate.rank} · Pareto P${candidate.pareto_rank + 1}`} · {statusLabel}</span>
          <h2>{candidate.deck.name}</h2>
        </div>
        <div className="candidate-actions">
          {onContinue && <button className="primary" onClick={onContinue}>Continue +{number(additionalGenerations || 0)} generations</button>}
          <button className="secondary" onClick={exportDeck}>Copy deck list</button>
        </div>
      </header>
      <div className="fitness-strip">
        <Metric label="Screen score" value={candidate.fitness.score.toFixed(3)} tone="accent" />
        <Metric label="Consistency" value={percent(candidate.fitness.consistency)} />
        <Metric label="Tempo" value={percent(candidate.fitness.tempo)} />
        <Metric label="Synergy" value={percent(candidate.fitness.synergy)} />
        <Metric label="Interaction" value={percent(candidate.fitness.interaction)} />
        <Metric label="Resilience" value={percent(candidate.fitness.resilience)} />
        {candidate.fitness.commander_quality > 0 && <Metric label="Commander" value={percent(candidate.fitness.commander_quality)} />}
        {candidate.deck.sideboard.length > 0 && <Metric label="Sideboard" value={percent(candidate.fitness.sideboard_quality || 0)} />}
        <Metric label="Mana fail" value={percent(candidate.fitness.mana_failure_rate)} tone="warn" />
        <Metric label="On curve" value={percent(candidate.fitness.on_curve_castability || 0)} tone={candidate.fitness.severe_mana_failure ? "warn" : undefined} />
        <Metric label="By horizon" value={percent(candidate.fitness.turn_horizon_castability || 0)} tone={candidate.fitness.severe_mana_failure ? "warn" : undefined} />
        <Metric label="Dead legends" value={percent(candidate.fitness.legendary_dead_draw_rate || 0)} tone={(candidate.fitness.legendary_dead_draw_rate || 0) > 0.1 ? "warn" : undefined} />
        <Metric label="Unmet needs" value={percent(candidate.fitness.unmet_requirement_rate || 0)} tone={(candidate.fitness.unmet_requirement_rate || 0) > 0.1 ? "warn" : undefined} />
        <Metric label="Strategy retained" value={percent(candidate.fitness.strategy_similarity ?? 1)} tone={(candidate.fitness.strategy_similarity ?? 1) < 0.7 ? "warn" : undefined} />
        <Metric label="Oracle accounted" value={percent(candidate.fitness.accounted_clause_rate ?? 1)} tone={(candidate.fitness.accounted_clause_rate ?? 1) < 1 ? "warn" : "accent"} />
        <Metric label="Structurally parsed" value={percent(candidate.fitness.parsed_clause_rate ?? 1)} tone={(candidate.fitness.unresolved_clause_rate || 0) > 0 ? "warn" : undefined} />
        <Metric label="Functional actions" value={percent(candidate.fitness.functional_action_rate ?? 1)} tone={(candidate.fitness.dead_action_rate || 0) > 0.25 ? "warn" : undefined} />
        <Metric label="Closing power" value={percent(candidate.fitness.closing_power || 0)} tone={(candidate.fitness.closing_power || 0) < 0.25 ? "warn" : undefined} />
        {candidate.fitness.combo_proof_level && <Metric label="Combo assembly" value={percent(candidate.fitness.combo_assembly_probability || 0)} tone={(candidate.fitness.combo_assembly_probability || 0) < 0.35 ? "warn" : "accent"} />}
        {candidate.fitness.combo_proof_level && <Metric label="Combo turn" value={candidate.fitness.combo_expected_turn == null ? "—" : `~${candidate.fitness.combo_expected_turn.toFixed(1)}`} />}
        {candidate.fitness.combo_proof_level && <Metric label="Plan reach (screen)" value={percent(candidate.fitness.combo_execution_rate || 0)} />}
        {candidate.fitness.combo_proof_level && <Metric label="Combo resilience" value={percent(candidate.fitness.combo_resilience || 0)} />}
        <Metric label="Role balance" value={percent(candidate.fitness.role_balance || 0)} />
        <Metric label="Archetype" value={candidate.fitness.archetype || "legacy/unclassified"} />
      </div>
      {candidate.fitness.severe_mana_failure && <div className="error-box"><strong>Blocked: severe mana failure</strong>{candidate.fitness.mana_blocking_reasons.map((reason) => <span key={reason}>{reason}</span>)}</div>}
      {candidate.fitness.severe_structural_failure && <div className="error-box"><strong>Blocked: structural or Tune failure</strong>{candidate.fitness.structural_blocking_reasons?.map((reason) => <span key={reason}>{reason}</span>)}</div>}
      {candidate.fitness.synergy_warnings?.length > 0 && <div className="warning-box">{candidate.fitness.synergy_warnings.map((warning) => <span key={warning}>{warning}</span>)}</div>}
      <div className="deck-analytics"><div><span className="eyebrow">Mana curve · nonlands</span><div className="curve-chart">{curve.map((value, bucket) => <i key={bucket} title={`${bucket === 7 ? "7+" : bucket} mana · ${value} cards`}><b style={{ height: `${Math.max(3, value / maximumCurve * 100)}%` }} /><small>{bucket === 7 ? "7+" : bucket}</small><em>{value}</em></i>)}</div></div><div><span className="eyebrow">Eligible mana sources and demand</span><strong>{dedicatedLands} dedicated lands · {modalLands} modal land options</strong>{sourceLabels.map((color, index) => <span key={color}>{color} unrestricted {candidate.fitness.unrestricted_early_sources?.[index] || 0} · conditional {candidate.fitness.conditional_early_sources?.[index] || 0} · unusable {candidate.fitness.unusable_sources?.[index] || 0} · pips {candidate.fitness.colored_pip_demand?.[index] || 0}</span>)}</div></div>
      <div className="package-strip"><span>Screen deductions</span><b>Dead functional modes<small>{percent(candidate.fitness.dead_action_rate || 0)} dead · weight {DEFAULT_FITNESS_WEIGHTS.dead_action_penalty}</small></b><b>Narrow mainboard cards<small>{percent(candidate.fitness.narrow_mainboard_rate || 0)} narrow · weight {DEFAULT_FITNESS_WEIGHTS.narrow_card_penalty}</small></b>{candidate.fitness.combo_proof_level && <b>Combo dead pieces<small>{percent(candidate.fitness.combo_dead_card_rate || 0)} of nonlands · {candidate.fitness.combo_proof_level.replaceAll("_", " ")}</small></b>}<b>Copies above role target<small>{percent(candidate.fitness.copy_excess_rate || 0)} excess copies</small></b><b>Legendary dead draws<small>−{((candidate.fitness.legendary_penalty || 0) * 100).toFixed(1)} · observed {percent(candidate.fitness.legendary_dead_draw_rate || 0)}</small></b><b>Unsupported requirements<small>−{((candidate.fitness.unsupported_synergy_penalty || 0) * 100).toFixed(1)} · unmet {percent(candidate.fitness.unmet_requirement_rate || 0)}</small></b><b>Color complexity<small>−{((candidate.fitness.color_complexity_penalty || 0) * 100).toFixed(1)} · {candidate.fitness.required_color_count || 0} required colors</small></b></div>
      {candidate.fitness.requirement_evidence?.length > 0 && <details className="candidate-diff"><summary>Mechanic demand and supply</summary>{candidate.fitness.requirement_evidence.map((evidence, index) => <span key={`${evidence.mechanic}-${index}`}><b>{evidence.mechanic}</b>demand {evidence.demand.toFixed(1)} · supply {evidence.supply.toFixed(1)} · fulfilled {percent(evidence.fulfillment)}</span>)}</details>}
      {candidate.fitness.strategy_packages?.length > 0 && <details className="candidate-diff" open><summary>Strategy-package retention</summary>{candidate.fitness.strategy_packages.map((evidence) => <span key={evidence.package}><b>{evidence.primary ? "Primary" : "Support"} · {evidence.package}</b>root {evidence.root_support.toFixed(1)} · candidate {evidence.candidate_support.toFixed(1)} · retained {percent(evidence.retention)}</span>)}</details>}
      {candidate.fitness.action_evidence?.length > 0 && <details className="candidate-diff"><summary>Functional abilities and dead modes</summary>{candidate.fitness.action_evidence.map((evidence, index) => <span key={`${evidence.card_id}-${evidence.kind}-${index}`}><b>{evidence.copies}× {evidence.card_name} · {evidence.kind}</b>{evidence.action} · {evidence.zone || "legacy zone"} · usable {percent(evidence.availability)} · mana {percent(evidence.mana_availability)} · targets {percent(evidence.target_availability)}{evidence.narrow ? " · narrow" : ""}{evidence.conditions?.length ? ` · conditions: ${evidence.conditions.join("; ")}` : ""}{evidence.reasons.length ? ` · ${evidence.reasons.join("; ")}` : ""}</span>)}</details>}
      {candidate.fitness.unresolved_oracle_text?.length > 0 && <details className="candidate-diff"><summary>Unresolved Oracle clauses · Provisional</summary>{candidate.fitness.unresolved_oracle_text.map((text, index) => <span key={`${text}-${index}`}><b>Not structurally understood</b>{text}</span>)}</details>}
      {candidate.fitness.card_castability?.length > 0 && <details className="candidate-diff"><summary>Per-card castability and eligible sources</summary>{candidate.fitness.card_castability.map((evidence) => <span key={evidence.card_id}><b>{evidence.copies}× {evidence.name}</b>on curve {percent(evidence.on_curve_castability)} · horizon {percent(evidence.turn_horizon_castability)} · unrestricted {evidence.unrestricted_early_sources} · conditional {evidence.conditional_early_sources} · unusable {evidence.unusable_sources}{evidence.source_restrictions?.length > 0 ? ` · ${evidence.source_restrictions.join("; ")}` : ""}{evidence.reasons.length > 0 ? ` · ${evidence.reasons.join("; ")}` : ""}</span>)}</details>}
      {comparison.length > 0 && <details className="candidate-diff" open><summary>Diff versus candidate #{compareTo?.rank} · {comparison.length} changed cards</summary>{comparison.map((row) => <span key={row.id} className={row.delta > 0 ? "added" : "removed"}><b>{row.delta > 0 ? `+${row.delta}` : row.delta}</b>{cards.get(row.id)?.name || row.id}</span>)}</details>}
      {candidate.packages?.length > 0 && <div className="package-strip"><span>Detected packages</span>{candidate.packages.map((item) => <b key={item.tag}>{item.tag.replace("type:", "")}<small>{item.unique_cards} cards · {item.copies} copies · {packageExplanation(item.tag)}</small></b>)}</div>}
      {candidate.commander_compatibility && <div className="package-strip"><span>Command-zone legality</span><b>{candidate.commander_compatibility.rule_id.replaceAll("-", " ")}<small>{candidate.commander_compatibility.policy_id} · schema {candidate.commander_compatibility.schema_version}</small></b></div>}
      <div className="card-grid">
        {candidate.deck.command_zone.map((entry) => {
          const card = cards.get(entry.card_id);
          return card ? <div className="commander-card" key={`commander-${entry.card_id}`}><span>COMMANDER</span><CardTile card={card} quantity={entry.quantity} conformance={conformance.get(card.name.toLowerCase())} /></div> : null;
        })}
        {candidate.deck.mainboard.map((entry) => {
          const card = cards.get(entry.card_id);
          return card ? <CardTile key={entry.card_id} card={card} quantity={entry.quantity} conformance={conformance.get(card.name.toLowerCase())} rationale={copyRationale(card)} /> : null;
        })}
        {candidate.deck.sideboard.length > 0 && <div className="zone-divider">SIDEBOARD · {candidate.deck.sideboard.reduce((total, entry) => total + entry.quantity, 0)} CARDS</div>}
        {candidate.deck.sideboard.map((entry) => {
          const card = cards.get(entry.card_id);
          return card ? <CardTile key={`sideboard-${entry.card_id}`} card={card} quantity={entry.quantity} conformance={conformance.get(card.name.toLowerCase())} /> : null;
        })}
      </div>
      {exportText && (
        <details className="export-preview" open>
          <summary>Deck list copied</summary>
          <textarea readOnly value={exportText} />
        </details>
      )}
    </section>
  );
}

export default function App() {
  const bootstrapStarted = useRef(false);
  const [activeView, setActiveView] = useState<WorkspaceView>("build");
  const [searchPreset, setSearchPreset] = useState<SearchPreset>("balanced");
  const [workspace, setWorkspace] = useState<RunRequest>();
  const [profile, setProfile] = useState<SystemProfile>();
  const [resourceStatus, setResourceStatus] = useState<ResourceBudgetStatus>();
  const [storageHealth, setStorageHealth] = useState<StorageHealth>();
  const [catalog, setCatalog] = useState<CatalogStatus>();
  const [catalogProgress, setCatalogProgress] = useState<CatalogProgress>();
  const [syncingCatalog, setSyncingCatalog] = useState(false);
  const [platform, setPlatform] = useState("arena");
  const [format, setFormat] = useState("historic");
  const [commanderQuery, setCommanderQuery] = useState("");
  const [commanderResults, setCommanderResults] = useState<Card[]>([]);
  const [poolQuery, setPoolQuery] = useState("");
  const [poolResults, setPoolResults] = useState<Card[]>([]);
  const [searchingPool, setSearchingPool] = useState(false);
  const [importText, setImportText] = useState("");
  const [validatedImportText, setValidatedImportText] = useState("");
  const [importError, setImportError] = useState<string>();
  const [importing, setImporting] = useState(false);
  const [savedRuns, setSavedRuns] = useState<RunSummary[]>([]);
  const [runSearch, setRunSearch] = useState("");
  const [comparedRunIds, setComparedRunIds] = useState<string[]>([]);
  const [loadedSavedRun, setLoadedSavedRun] = useState<{ id: string; saved: SavedRun }>();
  const [simulators, setSimulators] = useState<SimulatorStatus[]>([]);
  const [simulationCache, setSimulationCache] = useState<SimulationCacheStatus>();
  const [artCache, setArtCache] = useState<ArtCacheStatus>();
  const [installerProgress, setInstallerProgress] = useState<InstallerProgress>();
  const [installingSimulator, setInstallingSimulator] = useState(false);
  const [firstRunBusy, setFirstRunBusy] = useState(false);
  const [firstRunError, setFirstRunError] = useState<string>();
  const [jobId, setJobId] = useState<string>();
  const [activeRunId, setActiveRunId] = useState<string>();
  const [status, setStatus] = useState<JobStatus>();
  const [startError, setStartError] = useState<string>();
  const [provisionalResultReason, setProvisionalResultReason] = useState<string>();
  const [history, setHistory] = useState<number[]>([]);
  const historyGenerationRef = useRef(0);
  const [selectedRank, setSelectedRank] = useState(1);
  const [notice, setNotice] = useState("Loading Arena workspace…");
  const [gauntletInput, setGauntletInput] = useState("");
  const [gauntletDecks, setGauntletDecks] = useState<Deck[]>([]);
  const [gauntletWeights, setGauntletWeights] = useState<number[]>([]);
  const [gauntletName, setGauntletName] = useState("");
  const [gauntletMetadata, setGauntletMetadata] = useState<GauntletMetadata>({ source: "", observed_at: "", notes: "" });
  const [gauntletProvenance, setGauntletProvenance] = useState<GauntletProvenance>();
  const [metagameBundle, setMetagameBundle] = useState("");
  const [savedGauntlets, setSavedGauntlets] = useState<GauntletSummary[]>([]);
  const [gauntletComparison, setGauntletComparison] = useState<GauntletComparison>();
  const [gauntletLibraryBusy, setGauntletLibraryBusy] = useState(false);
  const [hallOfFame, setHallOfFame] = useState<HallOfFameStatus>();
  const [selectedHallId, setSelectedHallId] = useState<string>();
  const [hallBusy, setHallBusy] = useState(false);
  const [calibration, setCalibration] = useState<CalibrationStatus>();
  const [surrogate, setSurrogate] = useState<SurrogateStatus>();
  const [comboCorpus, setComboCorpus] = useState<ComboCorpusStatus>();
  const [comboReport, setComboReport] = useState<ComboProofReport>();
  const [comboBusy, setComboBusy] = useState(false);
  const [matchRecords, setMatchRecords] = useState<MatchRecord[]>([]);
  const [feedbackSummary, setFeedbackSummary] = useState<FeedbackSummary>();
  const [matchImportSource, setMatchImportSource] = useState<"arena_log" | "json">("arena_log");
  const [matchImportText, setMatchImportText] = useState("");
  const [reweightSuggestions, setReweightSuggestions] = useState<ReweightSuggestion[]>([]);
  const [acquisitionDays, setAcquisitionDays] = useState(30);
  const [acquisitionSource, setAcquisitionSource] = useState<"getpaird" | "spicerack" | "topdeck">("getpaird");
  const [acquisitionKey, setAcquisitionKey] = useState("");
  const [acquisitionBusy, setAcquisitionBusy] = useState(false);
  const [acquisitionReport, setAcquisitionReport] = useState<MetagameAcquisitionReport>();
  const [competitiveField, setCompetitiveField] = useState<MetagameFieldRevision>();
  const [engineRoutePlan, setEngineRoutePlan] = useState<EngineRoutePlan>();
  const [validationGames, setValidationGames] = useState(1000);
  const [validationSeats, setValidationSeats] = useState(2);
  const [hatBudget, setHatBudget] = useState(20);
  const [validating, setValidating] = useState(false);
  const [forgeGames, setForgeGames] = useState(16);
  const [forgeTimeout, setForgeTimeout] = useState(30);
  const [forgeRunning, setForgeRunning] = useState(false);
  const [simulationReport, setSimulationReport] = useState<SimulationReport>();
  const [sideboardPopulation, setSideboardPopulation] = useState(12);
  const [sideboardGenerations, setSideboardGenerations] = useState(2);
  const [sideboardGames, setSideboardGames] = useState(8);
  const [sideboardMaximumSwaps, setSideboardMaximumSwaps] = useState(8);
  const [sideboardParallelJobs, setSideboardParallelJobs] = useState(2);
  const [sideboardEvolving, setSideboardEvolving] = useState(false);
  const [sideboardEvolutionReport, setSideboardEvolutionReport] = useState<SideboardEvolutionReport>();
  const [bestOfThreeMatches, setBestOfThreeMatches] = useState(64);
  const [bestOfThreeRunning, setBestOfThreeRunning] = useState(false);
  const [bestOfThreeReport, setBestOfThreeReport] = useState<BestOfThreeSimulationReport>();
  const [savedValidations, setSavedValidations] = useState<SavedValidation[]>([]);
  const [finalistCount, setFinalistCount] = useState(8);
  const [halvingRounds, setHalvingRounds] = useState(3);
  const [halvingBaseGames, setHalvingBaseGames] = useState(16);
  const [halvingTimeout, setHalvingTimeout] = useState(5);
  const [riskAversion, setRiskAversion] = useState(0.5);
  const [worstMatchupWeight, setWorstMatchupWeight] = useState(0.25);
  const [seatPositionWeight, setSeatPositionWeight] = useState(0.25);
  const [adaptiveElimination, setAdaptiveElimination] = useState(true);
  const [halvingRunning, setHalvingRunning] = useState(false);
  const [halvingProgress, setHalvingProgress] = useState<HalvingProgress>();
  const [halvingReport, setHalvingReport] = useState<SuccessiveHalvingReport>();
  const [savedHalvings, setSavedHalvings] = useState<SavedHalving[]>([]);
  const [hybridCycles, setHybridCycles] = useState(3);
  const [historicalChampionCount, setHistoricalChampionCount] = useState(4);
  const [historicalChampionWeight, setHistoricalChampionWeight] = useState(0.2);
  const [coevolveAdversaries, setCoevolveAdversaries] = useState(false);
  const [learnPackagesFromHall, setLearnPackagesFromHall] = useState(true);
  const [holdoutGames, setHoldoutGames] = useState(64);
  const [hybridRunning, setHybridRunning] = useState(false);
  const [hybridProgress, setHybridProgress] = useState<HybridProgress>();
  const [coverageReport, setCoverageReport] = useState<HexdekCoverageReport>();
  const [coverageLoading, setCoverageLoading] = useState(false);
  const [coverageError, setCoverageError] = useState<string>();
  const [conformanceSummary, setConformanceSummary] = useState<ConformanceSummary>();
  const [commanderConformance, setCommanderConformance] = useState<CommanderConformanceReport>();
  const [conformanceReport, setConformanceReport] = useState<ConformanceAuditReport>();
  const [poolConformance, setPoolConformance] = useState<DeckConformanceReport>();
  const [conformanceBundle, setConformanceBundle] = useState("");
  const [conformanceLoading, setConformanceLoading] = useState(false);
  const [conformanceError, setConformanceError] = useState<string>();
  const [importingConformance, setImportingConformance] = useState(false);

  useEffect(() => {
    void invoke<ResourceBudgetStatus>("resource_budget_status")
      .then(setResourceStatus)
      .catch((error) => setNotice(`Resource budgets unavailable · ${String(error)}`));
  }, []);

  useEffect(() => {
    void invoke<ComboCorpusStatus>("combo_corpus_status")
      .then(setComboCorpus)
      .catch(() => undefined);
  }, []);

  useEffect(() => setComboReport(undefined), [selectedRank, activeRunId]);

  useEffect(() => {
    if (activeView !== "settings") return;
    void invoke<StorageHealth>("storage_health").then(setStorageHealth).catch(() => undefined);
  }, [activeView]);

  useEffect(() => {
    void Promise.all([
      invoke<MatchRecord[]>("match_history", { limit: 200 }),
      invoke<FeedbackSummary>("feedback_summary", { platform, format })
    ]).then(([records, summary]) => { setMatchRecords(records); setFeedbackSummary(summary); }).catch(() => undefined);
  }, [platform, format]);

  useEffect(() => {
    if (bootstrapStarted.current) return;
    bootstrapStarted.current = true;
    const bootstrap = async () => {
      try {
        const [system, initialCatalog, runs, initialEngines, cacheStatus, artCacheStatus, evidenceSummary] = await Promise.all([invoke<SystemProfile>("system_profile"), invoke<CatalogStatus>("catalog_status"), invoke<RunSummary[]>("run_history", { limit: 12 }), invoke<SimulatorStatus[]>("simulator_status"), invoke<SimulationCacheStatus>("simulation_cache_status").catch(() => undefined), invoke<ArtCacheStatus>("art_cache_status").catch(() => undefined), invoke<ConformanceSummary>("conformance_summary").catch((error) => { setConformanceError(String(error)); return undefined; })]);
        setProfile(system);
        setCatalog(initialCatalog);
        setSavedRuns(runs);
        setSimulators(initialEngines);
        if (cacheStatus) setSimulationCache(cacheStatus);
        if (artCacheStatus) setArtCache(artCacheStatus);
        setConformanceSummary(evidenceSummary);

        let catalogStatus = initialCatalog;
        let engineStatus = initialEngines;
        let optionalSetupWarning: string | undefined;
        const hexdekMissing = !engineStatus.some((engine) => engine.id === "hexdek" && engine.installed && engine.data_ready);
        const forgeMissing = !engineStatus.some((engine) => engine.id === "forge" && engine.installed && engine.data_ready);
        if (!catalogStatus.ready || hexdekMissing || forgeMissing) {
          setFirstRunBusy(true);
          setNotice("Finishing automatic first-run setup…");
        }
        if (!catalogStatus.ready) {
          setSyncingCatalog(true);
          setNotice("Downloading and indexing the universal card catalog…");
          catalogStatus = await invoke<CatalogStatus>("sync_catalog");
          setCatalog(catalogStatus);
          setSyncingCatalog(false);
        }
        let request = await invoke<RunRequest>("new_workspace", { platform: "arena", format: "historic", deckSize: null });
        request.config.worker_threads = system.recommended_workers;
        setWorkspace(request);
        setNotice(`Blank genome ready · ${number(request.pool.cards.length)} legal cards`);

        if (hexdekMissing) {
          setInstallingSimulator(true);
          setNotice("Installing the bundled HexDek runtime — no compiler required…");
          await invoke<SimulatorStatus>("install_hexdek_engine");
          engineStatus = await invoke<SimulatorStatus[]>("simulator_status");
          setSimulators(engineStatus);
          request = await invoke<RunRequest>("new_workspace", { platform: "arena", format: "historic", deckSize: null });
          request.config.worker_threads = system.recommended_workers;
          setWorkspace(request);
        }
        if (forgeMissing) {
          setInstallingSimulator(true);
          setNotice("Downloading the prebuilt Forge cross-check engine…");
          try {
            await invoke<SimulatorStatus>("install_forge_engine");
            engineStatus = await invoke<SimulatorStatus[]>("simulator_status");
            setSimulators(engineStatus);
          } catch (error) {
            optionalSetupWarning = `Primary workflow ready; Forge remains optional: ${String(error)}`;
            setFirstRunError(optionalSetupWarning);
          }
        }
        setNotice(optionalSetupWarning || `Ready · ${number(request.pool.cards.length)} legal cards · no compiler setup required`);
      } catch (error) {
        setFirstRunError(String(error));
        setNotice(`First-run setup needs attention · ${String(error)}`);
      } finally {
        setSyncingCatalog(false);
        setInstallingSimulator(false);
        setFirstRunBusy(false);
      }
    };
    void bootstrap();
  }, []);

  useEffect(() => {
    const unlisten = listen<CatalogProgress>("catalog-progress", ({ payload }) => {
      setCatalogProgress(payload);
      setNotice(payload.stage === "importing"
        ? `Indexing card catalog · ${number(payload.retained)} retained`
        : `Card catalog · ${payload.stage}`);
    });
    return () => { void unlisten.then((dispose) => dispose()); };
  }, []);

  useEffect(() => {
    let refreshTimer: number | undefined;
    const scheduleRefresh = () => {
      if (refreshTimer != null) window.clearTimeout(refreshTimer);
      refreshTimer = window.setTimeout(() => {
        void invoke<ArtCacheStatus>("art_cache_status").then(setArtCache).catch(() => undefined);
      }, 400);
    };
    window.addEventListener("metagene-art-cache-changed", scheduleRefresh);
    return () => {
      window.removeEventListener("metagene-art-cache-changed", scheduleRefresh);
      if (refreshTimer != null) window.clearTimeout(refreshTimer);
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<InstallerProgress>("simulator-install-progress", ({ payload }) => {
      setInstallerProgress(payload);
      setNotice(payload.detail);
    });
    return () => { void unlisten.then((dispose) => dispose()); };
  }, []);

  useEffect(() => {
    const unlisten = listen<HalvingProgress>("halving-progress", ({ payload }) => {
      setHalvingProgress(payload);
      setNotice(`Finalist round ${payload.round}/${payload.total_rounds} · candidate ${payload.candidate_index}/${payload.candidates_in_round} · ${number(payload.completed_attempts)} attempts complete`);
    });
    return () => { void unlisten.then((dispose) => dispose()); };
  }, []);

  useEffect(() => {
    const unlistenProgress = listen<HybridProgress>("hybrid-progress", ({ payload }) => {
      if (payload.job_id !== jobId) return;
      setHybridProgress(payload);
      if (payload.phase === "structural") {
        setNotice(`Hybrid cycle ${payload.cycle}/${payload.total_cycles} · structural generation ${payload.generation?.generation || 0}/${payload.generation?.total_generations || workspace?.config.generations || "—"}`);
      } else if (payload.phase === "validation") {
        setNotice(`Hybrid cycle ${payload.cycle}/${payload.total_cycles} · game-ranking finalists · ${number(payload.completed_attempts)} attempts`);
      } else {
        setNotice(`Hybrid cycle ${payload.cycle}/${payload.total_cycles} complete · champion #${payload.winner_rank || "—"}`);
      }
    });
    const unlistenComplete = listen<HybridComplete>("hybrid-complete", ({ payload }) => {
      if (payload.job_id !== jobId) return;
      setHybridRunning(false);
      setStatus(payload.status);
      if (payload.winner_rank != null) {
        setSelectedRank(payload.winner_rank);
        const champion = payload.status.result?.frontier.find((candidate) => candidate.rank === payload.winner_rank);
        if (champion) setWorkspace((current) => current ? { ...current, seed_deck: champion.deck } : current);
      }
      setNotice(payload.status.state === "completed"
        ? `Hybrid evolution complete · ${payload.cycles_completed} cycles · ${number(payload.total_attempts)} game attempts`
        : payload.status.error || payload.status.state);
      if (payload.status.state === "completed") setActiveView("results");
      void Promise.all([
        invoke<SavedValidation[]>("validation_history", { runId: payload.job_id, limit: 50 }),
        invoke<SavedHalving[]>("halving_history", { runId: payload.job_id, limit: 20 }),
        invoke<RunSummary[]>("run_history", { limit: 12 }),
        invoke<SimulationCacheStatus>("simulation_cache_status")
      ]).then(([validations, halvings, runs, cacheStatus]) => {
        setSavedValidations(validations);
        setSimulationReport(validations[0]?.report);
        setSavedHalvings(halvings);
        setHalvingReport(halvings[0]?.report);
        setSavedRuns(runs);
        setSimulationCache(cacheStatus);
      });
      void invoke<HallOfFameStatus>("hall_of_fame_status", {
        platform,
        format,
        commander: (workspace?.constraints.minimum_commanders || 0) > 0
      }).then(setHallOfFame).catch(() => undefined);
      void invoke<CalibrationStatus>("calibration_status", {
        platform,
        format,
        commander: (workspace?.constraints.minimum_commanders || 0) > 0
      }).then(setCalibration).catch(() => undefined);
    });
    return () => {
      void unlistenProgress.then((dispose) => dispose());
      void unlistenComplete.then((dispose) => dispose());
    };
  }, [jobId, workspace?.config.generations, workspace?.constraints.minimum_commanders, platform, format]);

  useEffect(() => {
    historyGenerationRef.current = 0;
  }, [jobId]);

  useEffect(() => {
    const unlistenProgress = listen<ProgressEnvelope>("evolution-progress", ({ payload }) => {
      if (payload.job_id !== jobId) return;
      setStatus((previous) => ({
        id: payload.job_id,
        state: "running",
        progress: payload.progress,
        result: previous?.result
      }));
      if (payload.progress.generation > historyGenerationRef.current) {
        historyGenerationRef.current = payload.progress.generation;
        setHistory((values) => appendHistoryPoint(values, payload.progress.best.fitness.score));
      }
    });
    const unlistenComplete = listen<CompleteEnvelope>("evolution-complete", ({ payload }) => {
      if (payload.job_id !== jobId) return;
      setStatus(payload.status);
      setNotice(payload.status.state === "completed" ? "Evolution complete" : payload.status.state);
      if (payload.status.state === "completed") setActiveView("results");
      void invoke<RunSummary[]>("run_history", { limit: 12 }).then(setSavedRuns);
    });
    return () => {
      void unlistenProgress.then((dispose) => dispose());
      void unlistenComplete.then((dispose) => dispose());
    };
  }, [jobId]);

  useEffect(() => {
    if (!jobId) return;
    let disposed = false;
    const poll = async () => {
      try {
        const snapshot = await invoke<JobStatus>("evolution_status", { jobId });
        if (disposed) return;
        setStatus(snapshot);
        if (snapshot.progress && snapshot.progress.generation > historyGenerationRef.current) {
          historyGenerationRef.current = snapshot.progress.generation;
          setHistory((values) => appendHistoryPoint(values, snapshot.progress!.best.fitness.score));
        }
        if (snapshot.state !== "running") {
          setHybridRunning(false);
          setNotice(snapshot.state === "completed" ? "Evolution complete" : snapshot.error || snapshot.state);
        }
      } catch (error) {
        if (!disposed) setNotice(String(error));
      }
    };
    void poll();
    const interval = window.setInterval(poll, 250);
    return () => {
      disposed = true;
      window.clearInterval(interval);
    };
  }, [jobId]);

  const cards = useMemo(
    () => new Map(workspace?.pool.cards.map((card) => [card.id, card]) || []),
    [workspace]
  );
  const detectedSeedStrategies = useMemo(() => {
    if (!workspace?.seed_deck.mainboard.length) return [];
    const supported = new Map<string, { names: Set<string>; roles: Set<string> }>();
    const add = (key: string, role: string, name: string) => {
      const packageEvidence = supported.get(key) || { names: new Set<string>(), roles: new Set<string>() };
      packageEvidence.names.add(name);
      packageEvidence.roles.add(role);
      supported.set(key, packageEvidence);
    };
    for (const entry of workspace.seed_deck.mainboard) {
      const card = cards.get(entry.card_id);
      if (!card || card.features.is_land) continue;
      const abilities = (card.features.initial_modes || []).flatMap((mode) => mode.abilities || []);
      const usefulTrigger = abilities.some((ability) => ability.timing === "Triggered" && ["Draw", "Destroy", "Exile", "Bounce", "CreateToken", "AddCounters", "Ramp", "Recursion"].includes(ability.kind));
      if (usefulTrigger) {
        add("copy", "target-payoff", card.name);
        add("blink-etb", "target-payoff", card.name);
      }
      for (const ability of abilities) {
        if (ability.kind === "Copy") add("copy", "enabler", card.name);
        if (ability.kind === "CreateToken") {
          add("tokens", "producer", card.name);
          add("sacrifice", "fodder", card.name);
        }
        if (ability.kind === "TokenMultiplier") add("tokens", "payoff", card.name);
        if (ability.kind === "Blink") add("blink-etb", "enabler", card.name);
        if (ability.kind === "SacrificeOutlet") add("sacrifice", "outlet", card.name);
        if (ability.kind === "Recursion") add("graveyard", "payoff", card.name);
        if (ability.kind === "Mill") add("graveyard", "setup", card.name);
        if (ability.kind === "AddCounters") add("counters", "producer", card.name);
        if ((ability.conditions || []).some((condition) => condition.includes("cast") && condition.includes("spell"))) add("spellslinger", "payoff", card.name);
      }
      if (card.features.mechanics.graveyard_setup_supply > 0) add("graveyard", "setup", card.name);
      if (card.features.mechanics.requires_death) add("sacrifice", "payoff", card.name);
      if (card.type_line.includes("Instant") || card.type_line.includes("Sorcery")) add("spellslinger", "spell", card.name);
    }
    const primary = [...supported.entries()]
      .filter(([, evidence]) => evidence.names.size >= 2 && evidence.roles.size >= 2)
      .sort((left, right) => right[1].names.size - left[1].names.size)
      .map(([key]) => key);
    if (primary.length) return primary;
    const nonlands = workspace.seed_deck.mainboard.flatMap((entry) => Array.from({ length: entry.quantity }, () => cards.get(entry.card_id))).filter((card): card is Card => Boolean(card && !card.features.is_land));
    const colors = [...new Set(nonlands.flatMap((card) => card.colors))];
    const early = nonlands.filter((card) => card.features.mana_value <= 2).length;
    const threats = nonlands.filter((card) => card.features.pressure > 0).length;
    return [colors.length ? `colors:${colors.join("")}` : "colorless", early >= nonlands.length / 2 ? "curve:early" : "curve:midrange", threats >= nonlands.length / 3 ? "role:threats" : "role:value"];
  }, [workspace?.seed_deck, cards]);
  const candidates = status?.result?.frontier || status?.progress?.top || [];
  const mapElites = status?.result?.map_elites || [];
  const baselineCandidate = status?.result?.baseline || status?.progress?.baseline;
  const strategySignature = status?.result?.strategy_signature || status?.progress?.strategy_signature;
  const validationScores = useMemo(() => {
    const scores = new Map<number, number>();
    for (const validation of savedValidations) {
      if (validation.candidate_rank == null) continue;
      const validatedDeck = (validation.request as { candidate?: Deck }).candidate;
      const candidate = validatedDeck
        ? candidates.find((item) => deckGenomeKey(item.deck) === deckGenomeKey(validatedDeck))
        : candidates.find((item) => item.rank === validation.candidate_rank);
      if (!candidate || scores.has(candidate.rank)) continue;
      const commander = candidate?.deck.command_zone[0]
        ? cards.get(candidate.deck.command_zone[0].card_id)?.name
        : undefined;
      const row = validation.report.engine === "forge"
        ? validation.report.rows.find((item) => item.deck.startsWith("Candidate · "))
        : commander
        ? validation.report.rows.find((item) => item.deck === commander)
        : validation.report.rows[0];
      if (candidate && row) {
        const gameEvidence = evidenceScore(validation.report, row.deck, riskAversion, worstMatchupWeight, seatPositionWeight);
        scores.set(candidate.rank, workspace?.config.evolution_mode === "tune"
          ? gameEvidence
          : candidate.fitness.score * 0.35 + gameEvidence * 0.65);
      }
    }
    return scores;
  }, [savedValidations, candidates, cards, riskAversion, worstMatchupWeight, seatPositionWeight, workspace?.config.evolution_mode]);
  const orderedCandidates = [...candidates].sort((left, right) =>
    (validationScores.get(right.rank) ?? right.fitness.score) - (validationScores.get(left.rank) ?? left.fitness.score)
  );
  const selected = selectedRank === 0 && baselineCandidate
    ? baselineCandidate
    : orderedCandidates.find((candidate) => candidate.rank === selectedRank) || orderedCandidates[0] || baselineCandidate;
  const running = status?.state === "running";
  const loadedStructuralRevision = loadedSavedRun?.saved.status.result?.structural_model_revision
    || loadedSavedRun?.saved.status.progress?.structural_model_revision;
  const legacyStructuralRun = Boolean(
    loadedSavedRun && loadedStructuralRevision !== CURRENT_STRUCTURAL_REVISION
  );

  const updateConfig = <K extends keyof RunRequest["config"]>(key: K, value: RunRequest["config"][K]) => {
    setWorkspace((current) => current ? {
      ...current,
      config: { ...current.config, [key]: value }
    } : current);
  };

  const setDeckColors = (colors?: DeckColor[]) => {
    setWorkspace((current) => current ? {
      ...current,
      constraints: { ...current.constraints, allowed_colors: colors }
    } : current);
  };

  const toggleDeckColor = (color: DeckColor) => {
    const current = workspace?.constraints.allowed_colors || [];
    const next = current.includes(color)
      ? current.filter((item) => item !== color)
      : DECK_COLORS.map((item) => item.value).filter((item) => item === color || current.includes(item));
    setDeckColors(next.length ? next : undefined);
  };

  const applySearchPreset = (preset: SearchPreset) => {
    setSearchPreset(preset);
    if (preset === "custom") return;
    setWorkspace((current) => current ? {
      ...current,
      config: { ...current.config, ...SEARCH_PRESETS[preset] }
    } : current);
  };

  const updateFitnessWeight = (key: keyof RunRequest["config"]["fitness_weights"], value: number) => {
    setWorkspace((current) => current ? {
      ...current,
      config: {
        ...current.config,
        fitness_weights: { ...current.config.fitness_weights, [key]: Math.max(0, Math.min(100, value)) }
      }
    } : current);
  };

  const findPoolCards = async () => {
    if (!poolQuery.trim()) return;
    setSearchingPool(true);
    try {
      const results = await invoke<Card[]>("search_cards", { query: poolQuery, platform, format, limit: 50 });
      setPoolResults(results);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setSearchingPool(false);
    }
  };

  const addPoolCard = (card: Card) => {
    setWorkspace((current) => {
      if (!current) return current;
      const canonical = current.pool.cards.find((item) => item.name === card.name);
      if (!canonical || current.pool_filter.includes(canonical.name)) return current;
      return { ...current, pool_filter: [...current.pool_filter, canonical.name] };
    });
  };

  const removePoolCard = (card: Card) => {
    setWorkspace((current) => current ? {
      ...current,
      pool_filter: current.pool_filter.filter((name) => name !== card.name),
      constraints: {
        ...current.constraints,
        locked_cards: current.constraints.locked_cards.filter((locked) => locked.card_id !== card.id)
      }
    } : current);
  };

  const updateLockedCount = (card: Card, field: "minimum" | "maximum", rawValue: number) => {
    setWorkspace((current) => {
      if (!current) return current;
      const naturalMaximum = card.features.unlimited_copies
        ? current.constraints.target_mainboard_size + current.constraints.target_sideboard_size
        : current.constraints.default_max_copies;
      const existing = current.constraints.locked_cards.find((locked) => locked.card_id === card.id);
      let minimum = existing?.minimum || 0;
      let maximum = existing?.maximum ?? naturalMaximum;
      if (field === "minimum") minimum = Math.max(0, Math.min(naturalMaximum, rawValue));
      else maximum = Math.max(0, Math.min(naturalMaximum, rawValue));
      minimum = Math.min(minimum, maximum);
      const rest = current.constraints.locked_cards.filter((locked) => locked.card_id !== card.id);
      const locked_cards = minimum === 0 && maximum === naturalMaximum
        ? rest
        : [...rest, { card_id: card.id, minimum, maximum }];
      return { ...current, constraints: { ...current.constraints, locked_cards } };
    });
  };

  const focusOnSeedDeck = () => {
    setWorkspace((current) => {
      if (!current) return current;
      const byId = new Map(current.pool.cards.map((card) => [card.id, card.name]));
      const names = [...current.seed_deck.mainboard, ...current.seed_deck.sideboard]
        .map((entry) => byId.get(entry.card_id))
        .filter((name): name is string => Boolean(name));
      return { ...current, pool_filter: [...new Set(names)] };
    });
  };

  const start = async () => {
    if (!workspace) return;
    const competitiveBlankStart = workspace.config.evolution_mode === "rebuild"
      && workspace.seed_deck.mainboard.length === 0
      && workspace.seed_deck.command_zone.length === 0
      && workspace.pool_filter.length === 0;
    let provisionalReason = workspace.config.evolution_mode === "rebuild" ? "structural screening only" : "";
    setStartError(undefined);
    setProvisionalResultReason(undefined);
    setHistory([]);
    setSelectedRank(1);
    setStatus(undefined);
    setSavedValidations([]);
    setSavedHalvings([]);
    setSimulationReport(undefined);
    setHalvingReport(undefined);
    setHalvingProgress(undefined);
    setHybridRunning(false);
    setHybridProgress(undefined);
    setLoadedSavedRun(undefined);
    setNotice("Starting workers…");
    try {
      if (workspace.config.evolution_mode === "tune" || competitiveBlankStart) {
        let fieldDecks = [...gauntletDecks];
        let fieldWeights = [...gauntletWeights];
        let fieldProvenance = gauntletProvenance;
        let competitiveClaimsAllowed = gauntletMetadata.competitive_claims_allowed ?? true;
        let activeFieldRevisionId = gauntletMetadata.field_revision_id;
        if (fieldDecks.length === 0) {
          const sevenDaysAgo = Date.now() - 7 * 24 * 60 * 60 * 1000;
          const recent = savedGauntlets.find((field) => {
            const created = Date.parse(field.created_at);
            return Number.isFinite(created) && created >= sevenDaysAgo;
          });
          if (recent) {
            try {
              const saved = await invoke<SavedGauntlet>("load_gauntlet", {
                id: recent.id,
                platform,
                format,
                commander: requiresCommander
              });
              fieldDecks = saved.decks;
              fieldWeights = saved.weights.length === saved.decks.length ? saved.weights : saved.decks.map(() => 1);
              fieldProvenance = { id: saved.id, name: saved.name, revision: saved.revision };
              competitiveClaimsAllowed = saved.metadata.competitive_claims_allowed ?? true;
              activeFieldRevisionId = saved.metadata.field_revision_id;
              setGauntletDecks(fieldDecks);
              setGauntletWeights(fieldWeights);
              setGauntletMetadata(saved.metadata);
              setGauntletProvenance(fieldProvenance);
            } catch {
              fieldDecks = [];
              fieldWeights = [];
            }
          }
        }
        if (fieldDecks.length === 0) {
          try {
            setNotice(`No current field loaded · acquiring a 30-day ${format} tournament field…`);
            const acquisition = await invoke<CompetitiveFieldAcquisition>("acquire_competitive_field", {
              request: {
                platform,
                format,
                commander: requiresCommander,
                lookback_days: 30,
                topdeck_api_key: acquisitionKey.trim() || undefined,
                policy: {
                  minimum_constructed_archetypes: 8,
                  maximum_constructed_archetypes: 16,
                  target_observed_weight: 0.9,
                  similarity_threshold: 0.72,
                  commander_opponents: 5
                }
              }
            });
            const field = acquisition.field;
            fieldDecks = field.decks;
            fieldWeights = field.weights;
            setCompetitiveField(field);
            setAcquisitionReport(undefined);
            setGauntletDecks(fieldDecks);
            setGauntletWeights(fieldWeights);
            setGauntletName(acquisition.saved.name);
            setGauntletMetadata(acquisition.saved.metadata);
            competitiveClaimsAllowed = field.audit.competitive_claims_allowed;
            activeFieldRevisionId = field.revision_id;
            fieldProvenance = {
              id: acquisition.saved.id,
              name: acquisition.saved.name,
              revision: acquisition.saved.revision
            };
            setGauntletProvenance(fieldProvenance);
            void refreshGauntletLibrary();
            if (!field.audit.competitive_claims_allowed) {
              provisionalReason = "public-list fallback—tournament evidence unavailable";
            }
          } catch (error) {
            provisionalReason = `no opponent field (${String(error)})`;
          }
        }
        const requiredFieldDecks = requiresCommander
          ? Math.max(1, Math.min(5, validationSeats - 1))
          : 1;
        if (fieldDecks.length >= requiredFieldDecks && (hexdekReady || forgeReady)) {
          const launched = await startHybridEvolution(
            fieldDecks,
            fieldWeights,
            fieldProvenance,
            true,
            competitiveClaimsAllowed,
            activeFieldRevisionId
          );
          if (launched) return;
          provisionalReason = "simulator coverage could not prove the finalists";
        } else if (!provisionalReason && !hexdekReady && !forgeReady) {
          provisionalReason = "no rules engine is available";
        } else if (!provisionalReason) {
          provisionalReason = "the acquired field was incomplete";
        }
      }
      const id = await invoke<string>("start_evolution", { request: workspace });
      setProvisionalResultReason(provisionalReason || "game validation not completed");
      setJobId(id);
      setActiveRunId(id);
      setStatus({ id, state: "running" });
      setNotice(provisionalReason
        ? `Evolution running · Provisional—${provisionalReason}`
        : "Evolution running");
    } catch (error) {
      const message = String(error);
      setStartError(message);
      setNotice("Evolution could not start");
    }
  };

  const continueFromCandidate = async () => {
    if (!workspace || !selected || !activeRunId || running) return;
    setNotice(`Continuing candidate #${selected.rank} for ${number(workspace.config.generations)} more generations…`);
    try {
      if (workspace.config.evolution_mode === "tune" && (hexdekReady || forgeReady)) {
        let fieldDecks = [...gauntletDecks];
        let fieldWeights = [...gauntletWeights];
        let fieldProvenance = gauntletProvenance;
        let claimsAllowed = gauntletMetadata.competitive_claims_allowed ?? true;
        let fieldRevisionId = gauntletMetadata.field_revision_id;
        if (fieldDecks.length === 0) {
          const sevenDaysAgo = Date.now() - 7 * 24 * 60 * 60 * 1000;
          const recent = savedGauntlets.find((field) => {
            const created = Date.parse(field.created_at);
            return Number.isFinite(created) && created >= sevenDaysAgo;
          });
          if (recent) {
            try {
              const saved = await invoke<SavedGauntlet>("load_gauntlet", {
                id: recent.id,
                platform,
                format,
                commander: requiresCommander
              });
              fieldDecks = saved.decks;
              fieldWeights = saved.weights.length === saved.decks.length
                ? saved.weights
                : saved.decks.map(() => 1);
              fieldProvenance = { id: saved.id, name: saved.name, revision: saved.revision };
              claimsAllowed = saved.metadata.competitive_claims_allowed ?? true;
              fieldRevisionId = saved.metadata.field_revision_id;
              setGauntletDecks(fieldDecks);
              setGauntletWeights(fieldWeights);
              setGauntletMetadata(saved.metadata);
              setGauntletProvenance(fieldProvenance);
            } catch {
              fieldDecks = [];
              fieldWeights = [];
            }
          }
        }
        if (fieldDecks.length === 0) {
          try {
            setNotice(`Acquiring a 30-day ${format} tournament field before continuing…`);
            const acquisition = await invoke<CompetitiveFieldAcquisition>("acquire_competitive_field", {
              request: {
                platform,
                format,
                commander: requiresCommander,
                lookback_days: 30,
                topdeck_api_key: acquisitionKey.trim() || undefined,
                policy: {
                  minimum_constructed_archetypes: 8,
                  maximum_constructed_archetypes: 16,
                  target_observed_weight: 0.9,
                  similarity_threshold: 0.72,
                  commander_opponents: 5
                }
              }
            });
            fieldDecks = acquisition.field.decks;
            fieldWeights = acquisition.field.weights;
            fieldProvenance = {
              id: acquisition.saved.id,
              name: acquisition.saved.name,
              revision: acquisition.saved.revision
            };
            claimsAllowed = acquisition.field.audit.competitive_claims_allowed;
            fieldRevisionId = acquisition.field.revision_id;
            setCompetitiveField(acquisition.field);
            setGauntletDecks(fieldDecks);
            setGauntletWeights(fieldWeights);
            setGauntletName(acquisition.saved.name);
            setGauntletMetadata(acquisition.saved.metadata);
            setGauntletProvenance(fieldProvenance);
            void refreshGauntletLibrary();
          } catch (error) {
            setProvisionalResultReason(`no opponent field (${String(error)})`);
          }
        }
        const required = requiresCommander
          ? Math.max(1, Math.min(5, validationSeats - 1))
          : 1;
        if (fieldDecks.length >= required) {
          const launched = await startHybridEvolution(
            fieldDecks,
            fieldWeights,
            fieldProvenance,
            true,
            claimsAllowed,
            fieldRevisionId,
            activeRunId,
            selected
          );
          if (launched) return;
        }
      }
      const continued = await invoke<ContinuedRun>("continue_evolution", {
        request: workspace,
        candidate: selected,
        sourceRunId: activeRunId
      });
      setWorkspace(continued.request);
      setJobId(continued.id);
      setActiveRunId(continued.id);
      setStatus(continued.status);
      setSelectedRank(1);
      setHybridRunning(false);
      setHybridProgress(undefined);
      setLoadedSavedRun(undefined);
      setNotice(`Continuing candidate #${selected.rank} from cumulative generation ${number(continued.generations_completed)}`);
    } catch (error) {
      setNotice(String(error));
    }
  };

  const synchronizeComboCorpus = async (): Promise<ComboCorpusStatus | undefined> => {
    if (!workspace) return undefined;
    setComboBusy(true);
    setNotice("Synchronizing current Commander Spellbook combo hypotheses…");
    try {
      const report = await invoke<ComboCorpusSyncReport>("sync_combo_corpus", {
        request: { source_url: "", max_entries: 100000 },
        pool: workspace.pool
      });
      setComboCorpus(report.status);
      setNotice(`Combo corpus ready · ${number(report.resolved)}/${number(report.downloaded)} hypotheses resolved to logical cards`);
      return report.status;
    } catch (error) {
      setNotice(`Combo corpus unavailable · ${String(error)}`);
      return undefined;
    } finally {
      setComboBusy(false);
    }
  };

  const inspectSelectedCombos = async () => {
    if (!workspace || !selected) return;
    setComboBusy(true);
    setNotice(`Proving combo lines for candidate #${selected.rank}…`);
    try {
      let corpus = comboCorpus;
      if (!corpus?.hypotheses) {
        setComboBusy(false);
        corpus = await synchronizeComboCorpus();
        setComboBusy(true);
      }
      const report = await invoke<ComboProofReport>("prove_deck_combos", {
        request: {
          deck: selected.deck,
          pool: workspace.pool,
          limits: { max_pieces: 5, max_actions: 24, max_states: 100000, time_limit_ms: 2000 }
        }
      });
      setComboReport(report);
      setNotice(report.lines.length
        ? `Combo scan complete · ${report.lines.length} line${report.lines.length === 1 ? "" : "s"} found`
        : "Combo scan complete · no proven line found");
    } catch (error) {
      setNotice(`Combo scan failed · ${String(error)}`);
    } finally {
      setComboBusy(false);
    }
  };

  const cancel = async () => {
    if (!jobId) return;
    await invoke("cancel_evolution", { jobId });
    setNotice("Stopping after the current generation…");
  };

  const loadSavedRun = async (id: string) => {
    setNotice("Loading saved run…");
    try {
      const saved = await invoke<SavedRun>("load_saved_run", { id });
      setPlatform(saved.request.platform);
      setFormat(saved.request.format);
      setWorkspace(saved.request);
      setImportText("");
      setValidatedImportText("");
      setImportError(undefined);
      setGauntletDecks([]);
      setGauntletWeights([]);
      setGauntletName("");
      setGauntletMetadata({ source: "", observed_at: "", notes: "" });
      setGauntletProvenance(undefined);
      setMetagameBundle("");
      setStatus(saved.status);
      setLoadedSavedRun({ id, saved });
      setJobId(saved.status.state === "running" ? id : undefined);
      setActiveRunId(id);
      const [validations, halvings] = await Promise.all([
        invoke<SavedValidation[]>("validation_history", { runId: id, limit: 50 }),
        invoke<SavedHalving[]>("halving_history", { runId: id, limit: 10 })
      ]);
      setSavedValidations(validations);
      setProvisionalResultReason(
        saved.status.result?.promotion_evidence
          ? undefined
          : validations.length
            ? "validation incomplete or not promoted"
            : "saved structural result—no game proof"
      );
      setSimulationReport(validations[0]?.report);
      setSavedHalvings(halvings);
      setHalvingReport(halvings[0]?.report);
      setHybridRunning(saved.status.state === "running" && saved.run_kind === "hybrid");
      setHybridProgress(undefined);
      setHistory(saved.status.progress ? [saved.status.progress.best.fitness.score] : []);
      setSelectedRank(halvings[0]?.report.winner_rank || 1);
      setNotice(saved.resumed_from
        ? `Loaded resumed ${saved.request.platform} ${saved.request.format} run`
        : `Loaded ${saved.request.platform} ${saved.request.format} run`);
    } catch (error) {
      setNotice(String(error));
    }
  };

  const saveRunMetadata = async (run: RunSummary) => {
    try {
      const updated = await invoke<RunSummary>("update_run_metadata", {
        metadata: { id: run.id, title: run.title, notes: run.notes, tags: run.tags }
      });
      setSavedRuns((rows) => rows.map((row) => row.id === updated.id ? updated : row));
      setNotice("Experiment metadata saved");
    } catch (error) { setNotice(String(error)); }
  };

  const copyRunManifest = async (id: string) => {
    try {
      const manifest = await invoke<string>("export_run_manifest", { id });
      await navigator.clipboard.writeText(manifest);
      setNotice("Reproducibility manifest copied");
    } catch (error) { setNotice(String(error)); }
  };

  const bulkExportCandidates = async () => {
    const rows = orderedCandidates.slice(0, 8);
    if (!rows.length) return;
    try {
      const exports = await Promise.all(rows.map(async (candidate) => `# Candidate ${candidate.rank} · screen score ${candidate.fitness.score.toFixed(3)}\n${await invoke<string>("export_deck", { deck: candidate.deck, platform, format })}`));
      await navigator.clipboard.writeText(exports.join("\n\n"));
      setNotice(`${rows.length} candidate deck lists copied`);
    } catch (error) { setNotice(String(error)); }
  };

  const resumeSavedRun = async () => {
    if (!loadedSavedRun?.saved.resumable) return;
    setNotice("Resuming from the last durable checkpoint…");
    try {
      const resumed = await invoke<ResumedRun>("resume_evolution", { id: loadedSavedRun.id });
      setWorkspace(resumed.request);
      setJobId(resumed.id);
      setActiveRunId(resumed.id);
      setStatus({ id: resumed.id, state: "running" });
      setHistory([]);
      setSelectedRank(1);
      setSavedValidations([]);
      setSavedHalvings([]);
      setSimulationReport(undefined);
      setHalvingReport(undefined);
      setHalvingProgress(undefined);
      setHybridRunning(resumed.run_kind === "hybrid");
      setHybridProgress(undefined);
      setLoadedSavedRun(undefined);
      setNotice(resumed.run_kind === "hybrid"
        ? `Resumed hybrid cycle · ${resumed.remaining_cycles} cycle${resumed.remaining_cycles === 1 ? "" : "s"} remain`
        : `Resumed after generation ${resumed.resumed_generation} · ${resumed.request.config.generations} generations remain`);
    } catch (error) {
      setNotice(String(error));
    }
  };

  const syncArenaCatalog = async () => {
    setSyncingCatalog(true);
    setNotice("Downloading the compact Arena catalog…");
    try {
      const result = await invoke<CatalogStatus>("sync_catalog");
      setCatalog(result);
      const request = await invoke<RunRequest>("new_workspace", { platform, format, deckSize: null });
      if (profile) request.config.worker_threads = profile.recommended_workers;
      setWorkspace(request);
      setImportText("");
      setValidatedImportText("");
      setImportError(undefined);
      setNotice(`Universal catalog ready · ${number(result.card_count)} printings`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setSyncingCatalog(false);
    }
  };

  const installHexdek = async () => {
    setInstallingSimulator(true);
    setInstallerProgress({ stage: "starting", detail: "Preparing simulator installation", completed_units: 0 });
    setNotice("Preparing HexDek installation…");
    try {
      await invoke<SimulatorStatus>("install_hexdek_engine");
      const engineStatus = await invoke<SimulatorStatus[]>("simulator_status");
      setSimulators(engineStatus);
      setNotice("HexDek matchup simulator ready");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setInstallingSimulator(false);
    }
  };

  const installForge = async () => {
    setInstallingSimulator(true);
    setInstallerProgress({ stage: "starting", detail: "Preparing Forge installation", completed_units: 0 });
    setNotice("Preparing checksum-pinned Forge installation…");
    try {
      await invoke<SimulatorStatus>("install_forge_engine");
      const engineStatus = await invoke<SimulatorStatus[]>("simulator_status");
      setSimulators(engineStatus);
      setNotice("Forge independent cross-check engine ready");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setInstallingSimulator(false);
    }
  };

  const addGauntletDeck = async () => {
    if (!gauntletInput.trim()) return;
    setNotice("Validating gauntlet deck…");
    try {
      const deck = await invoke<Deck>("import_gauntlet_deck", { text: gauntletInput, platform, format });
      const maximumDecks = requiresCommander ? 5 : 32;
      setGauntletDecks((decks) => [...decks, deck].slice(0, maximumDecks));
      setGauntletWeights((weights) => [...weights, 1].slice(0, maximumDecks));
      setGauntletProvenance(undefined);
      setGauntletInput("");
      const commander = deck.command_zone[0] ? cards.get(deck.command_zone[0].card_id)?.name : deck.name;
      setNotice(`${commander || deck.name} added to the gauntlet`);
    } catch (error) {
      setNotice(String(error));
    }
  };

  const acquireCurrentMetagame = async () => {
    setAcquisitionBusy(true);
    setNotice(`Acquiring the last ${acquisitionDays} days of ${format} tournament lists…`);
    try {
      const report = await invoke<MetagameAcquisitionReport>("acquire_metagame", {
        request: {
          source: acquisitionSource,
          platform,
          format,
          commander: requiresCommander,
          lookback_days: acquisitionDays,
          api_key: acquisitionKey.trim() || undefined
        }
      });
      setAcquisitionReport(report);
      const accepted = report.accepted.slice(0, requiresCommander ? 5 : 32);
      setGauntletDecks(accepted.map((entry) => entry.deck));
      setGauntletWeights(accepted.map((entry) => entry.weight));
      setGauntletName(`${format[0]?.toUpperCase() || ""}${format.slice(1)} field ${new Date(report.provenance.retrieved_at_unix * 1000).toISOString().slice(0, 10)}`);
      const start = report.provenance.event_start_unix ? new Date(report.provenance.event_start_unix * 1000).toISOString().slice(0, 10) : "unknown";
      const end = report.provenance.event_end_unix ? new Date(report.provenance.event_end_unix * 1000).toISOString().slice(0, 10) : "unknown";
      setGauntletMetadata({
        source: report.provenance.source_url,
        observed_at: `${start} to ${end}`,
        notes: `${report.provenance.adapter}; ${report.provenance.event_count} events; ${report.provenance.accepted_deck_count}/${report.provenance.submitted_deck_count} legal published lists; exact duplicates merged by canonical genome.`,
        source_url: report.provenance.source_url,
        retrieved_at_unix: report.provenance.retrieved_at_unix,
        event_start_unix: report.provenance.event_start_unix,
        event_end_unix: report.provenance.event_end_unix,
        sample_size: report.provenance.accepted_deck_count,
        transformation_notes: report.provenance.transformation_notes
      });
      setGauntletProvenance(undefined);
      setNotice(`Loaded ${accepted.length} archetypes; ${report.quarantined.length} source rows remain quarantined for inspection`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setAcquisitionBusy(false);
    }
  };

  const refreshGauntletLibrary = async (nextPlatform = platform, nextFormat = format, commander = requiresCommander) => {
    try {
      const saved = await invoke<GauntletSummary[]>("gauntlet_history", {
        platform: nextPlatform,
        format: nextFormat,
        commander,
        limit: 50
      });
      setSavedGauntlets(saved);
    } catch {
      setSavedGauntlets([]);
    }
  };

  const saveCurrentGauntlet = async () => {
    if (!gauntletName.trim() || !gauntletDecks.length) return;
    setGauntletLibraryBusy(true);
    try {
      const saved = await invoke<SavedGauntlet>("save_gauntlet", {
        name: gauntletName,
        platform,
        format,
        commander: requiresCommander,
        decks: gauntletDecks,
        weights: gauntletWeights,
        metadata: gauntletMetadata
      });
      await refreshGauntletLibrary();
      setGauntletName(saved.name);
      setGauntletProvenance({ id: saved.id, name: saved.name, revision: saved.revision });
      setNotice(`${saved.name} v${saved.revision} saved · ${saved.deck_count} opponent${saved.deck_count === 1 ? "" : "s"}`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setGauntletLibraryBusy(false);
    }
  };

  const loadSavedGauntlet = async (id: string) => {
    setGauntletLibraryBusy(true);
    try {
      const saved = await invoke<SavedGauntlet>("load_gauntlet", {
        id,
        platform,
        format,
        commander: requiresCommander
      });
      setGauntletDecks(saved.decks);
      setGauntletWeights(saved.weights.length === saved.decks.length ? saved.weights : saved.decks.map(() => 1));
      setGauntletName(saved.name);
      setGauntletMetadata(saved.metadata || { source: "", observed_at: "", notes: "" });
      setGauntletProvenance({ id: saved.id, name: saved.name, revision: saved.revision });
      if (requiresCommander) setValidationSeats(Math.min(6, saved.decks.length + 1));
      setNotice(`${saved.name} v${saved.revision} loaded and revalidated`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setGauntletLibraryBusy(false);
    }
  };

  const compareGauntletRevisions = async (newer: GauntletSummary, older: GauntletSummary) => {
    setGauntletLibraryBusy(true);
    try {
      const comparison = await invoke<GauntletComparison>("compare_gauntlets", { fromId: older.id, toId: newer.id });
      setGauntletComparison(comparison);
      setNotice(`Compared ${newer.name} v${older.revision} → v${newer.revision}`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setGauntletLibraryBusy(false);
    }
  };

  const exportMetagameBundle = async () => {
    if (!gauntletName.trim() || !gauntletDecks.length) return;
    setGauntletLibraryBusy(true);
    try {
      const bundle = await invoke<string>("export_metagame_bundle", {
        name: gauntletName,
        platform,
        format,
        commander: requiresCommander,
        decks: gauntletDecks,
        weights: gauntletWeights,
        metadata: gauntletMetadata
      });
      setMetagameBundle(bundle);
      await navigator.clipboard.writeText(bundle).catch(() => undefined);
      setNotice(`Portable ${gauntletName} bundle copied`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setGauntletLibraryBusy(false);
    }
  };

  const importMetagameBundle = async () => {
    if (!metagameBundle.trim()) return;
    setGauntletLibraryBusy(true);
    try {
      const imported = await invoke<ImportedMetagameBundle>("import_metagame_bundle", {
        bundleJson: metagameBundle,
        platform,
        format,
        commander: requiresCommander
      });
      setGauntletDecks(imported.decks);
      setGauntletWeights(imported.weights);
      setGauntletName(imported.name);
      setGauntletMetadata(imported.metadata);
      setGauntletProvenance(undefined);
      if (requiresCommander) setValidationSeats(Math.min(6, imported.decks.length + 1));
      setNotice(`${imported.name} imported · ${imported.decks.length} legal opponents`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setGauntletLibraryBusy(false);
    }
  };

  const deleteSavedGauntlet = async (id: string) => {
    setGauntletLibraryBusy(true);
    try {
      await invoke("delete_gauntlet", { id });
      await refreshGauntletLibrary();
      setNotice("Saved gauntlet revision deleted");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setGauntletLibraryBusy(false);
    }
  };

  const importConformanceEvidence = async () => {
    if (!conformanceBundle.trim()) return;
    setImportingConformance(true);
    setConformanceError(undefined);
    setNotice("Importing versioned conformance evidence…");
    try {
      const result = await invoke<ConformanceImportResult>("import_conformance_bundle", {
        bundleJson: conformanceBundle
      });
      setConformanceSummary(result.summary);
      setConformanceBundle("");
      let refreshError: string | undefined;
      if (coverageDecks.length > 0) {
        try {
          const audit = await invoke<ConformanceAuditReport>("deck_conformance", { decks: coverageDecks });
          setConformanceReport(audit);
          setConformanceSummary(audit.summary);
        } catch (error) {
          refreshError = String(error);
          setConformanceError(refreshError);
        }
      }
      setNotice(`Imported ${number(result.imported_observations)} conformance observation${result.imported_observations === 1 ? "" : "s"}${refreshError ? " · deck audit refresh failed" : ""}`);
    } catch (error) {
      setConformanceError(String(error));
      setNotice(String(error));
    } finally {
      setImportingConformance(false);
    }
  };

  const validateCandidate = async () => {
    if (!selected || !workspace) return;
    if (selected.fitness.severe_mana_failure || selected.fitness.severe_structural_failure) {
      setNotice(`Validation blocked: ${[...(selected.fitness.mana_blocking_reasons || []), ...(selected.fitness.structural_blocking_reasons || [])].join(" · ")}`);
      return;
    }
    const seats = requiresCommander ? Math.max(2, Math.min(6, validationSeats)) : 2;
    const opponents = requiresCommander ? gauntletDecks.slice(0, seats - 1) : gauntletDecks;
    if (opponents.length < (requiresCommander ? seats - 1 : 1)) {
      setNotice(`Add ${requiresCommander ? seats - 1 : 1} opponent deck${requiresCommander && seats > 2 ? "s" : ""} for this matchup`);
      return;
    }
    const gameBudget = requiresCommander ? validationGames * seats : validationGames;
    setValidating(true);
    setNotice(`Simulating ${number(gameBudget * (requiresCommander ? 1 : opponents.length))} ${requiresCommander ? "Commander" : "constructed"} games…`);
    try {
      const report = await invoke<SimulationReport>("run_hexdek_validation", {
        request: {
          candidate: selected.deck,
          opponents,
          opponent_weights: requiresCommander ? opponents.map(() => 1) : gauntletWeights.slice(0, opponents.length),
          pool: workspace.pool,
          games: gameBudget,
          seed: workspace.config.seed,
          workers: profile?.recommended_workers || 1,
          seats,
          hat_budget: hatBudget,
          max_turns: 60,
          game_timeout_seconds: 30,
          commander: requiresCommander
        },
        runId: activeRunId || null,
        candidateRank: selected.rank
      });
      setSimulationReport(report);
      void invoke<SimulationCacheStatus>("simulation_cache_status").then(setSimulationCache).catch(() => undefined);
      const validations = await invoke<SavedValidation[]>("validation_history", { runId: activeRunId || null, limit: 20 });
      setSavedValidations(validations);
      setNotice(`Validation complete · ${number(report.games_per_second)} games/sec · ${report.crashes} crashes`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setValidating(false);
    }
  };

  const evolveCandidateSideboard = async () => {
    if (!selected || !workspace || requiresCommander || !gauntletDecks.length) return;
    if (!selected.deck.sideboard.length) {
      setNotice("This candidate has no sideboard genes to optimize");
      return;
    }
    setSideboardEvolving(true);
    setBestOfThreeReport(undefined);
    setNotice(`Evolving play/draw plans against ${gauntletDecks.length} weighted opponent${gauntletDecks.length === 1 ? "" : "s"}…`);
    try {
      const report = await invoke<SideboardEvolutionReport>("evolve_sideboard_plans", {
        request: {
          candidate: selected.deck,
          opponents: gauntletDecks,
          opponent_weights: gauntletWeights,
          pool: workspace.pool,
          constraints: workspace.constraints,
          population_size: sideboardPopulation,
          generations: sideboardGenerations,
          games_per_policy: sideboardGames,
          maximum_swaps: sideboardMaximumSwaps,
          seed: workspace.config.seed,
          workers: profile?.recommended_workers || 1,
          parallel_jobs: sideboardParallelJobs,
          hat_budget: hatBudget,
          max_turns: 60,
          game_timeout_seconds: 30
        },
        runId: activeRunId || null,
        candidateRank: selected.rank
      });
      setSideboardEvolutionReport(report);
      void invoke<SimulationCacheStatus>("simulation_cache_status").then(setSimulationCache).catch(() => undefined);
      setNotice(`Sideboard policy evolved · ${number(report.evaluated_policies)} policies · ${number(report.attempted_games)} actual games`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setSideboardEvolving(false);
    }
  };

  const cancelSideboardEvolution = async () => {
    await invoke("cancel_sideboard_evolution");
    setNotice("Stopping sideboard policy evolution…");
  };

  const runBestOfThree = async () => {
    if (!selected || !workspace || !sideboardEvolutionReport || requiresCommander) return;
    setBestOfThreeRunning(true);
    setNotice(`Playing ${number(bestOfThreeMatches)} explicit Best-of-Three matches…`);
    try {
      const report = await invoke<BestOfThreeSimulationReport>("run_hexdek_best_of_three", {
        request: {
          candidate: selected.deck,
          opponents: gauntletDecks,
          opponent_weights: gauntletWeights,
          pool: workspace.pool,
          constraints: workspace.constraints,
          sideboard_plans: sideboardEvolutionReport.plans,
          matches: Math.max(gauntletDecks.length, bestOfThreeMatches),
          seed: workspace.config.seed,
          workers: profile?.recommended_workers || 1,
          hat_budget: hatBudget,
          max_turns: 60,
          game_timeout_seconds: 30
        },
        runId: activeRunId || null,
        candidateRank: selected.rank
      });
      setBestOfThreeReport(report);
      void invoke<SimulationCacheStatus>("simulation_cache_status").then(setSimulationCache).catch(() => undefined);
      setNotice(`Best-of-Three complete · ${number(report.completed_matches)}/${number(report.requested_matches)} matches · ${percent(report.matches.win_rate)} match win rate`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBestOfThreeRunning(false);
    }
  };

  const exchangeLabel = (exchange: SideboardExchange) => {
    const zone = (entries: SideboardExchange["cards_in"]) => entries.length
      ? entries.map((entry) => `${entry.quantity} ${cards.get(entry.card_id)?.name || entry.card_id}`).join(", ")
      : "none";
    return `In: ${zone(exchange.cards_in)} · Out: ${zone(exchange.cards_out)}`;
  };

  const crossCheckCandidate = async () => {
    if (!selected || !workspace) return;
    const seats = requiresCommander ? Math.max(2, Math.min(6, validationSeats)) : 2;
    const opponents = requiresCommander ? gauntletDecks.slice(0, seats - 1) : gauntletDecks;
    if (opponents.length < (requiresCommander ? seats - 1 : 1)) {
      setNotice(`Add ${requiresCommander ? seats - 1 : 1} opponent deck${requiresCommander && seats > 2 ? "s" : ""} before the Forge cross-check`);
      return;
    }
    setForgeRunning(true);
    setNotice(`Cross-checking ${number(forgeGames * (requiresCommander ? 1 : opponents.length))} games in Forge…`);
    try {
      const report = await invoke<SimulationReport>("run_forge_validation", {
        request: {
          candidate: selected.deck,
          opponents,
          opponent_weights: requiresCommander ? opponents.map(() => 1) : gauntletWeights.slice(0, opponents.length),
          pool: workspace.pool,
          games: forgeGames,
          seed: workspace.config.seed,
          workers: 1,
          seats,
          hat_budget: 0,
          max_turns: 0,
          game_timeout_seconds: forgeTimeout,
          commander: requiresCommander
        },
        runId: activeRunId || null,
        candidateRank: selected.rank
      });
      setSimulationReport(report);
      void invoke<SimulationCacheStatus>("simulation_cache_status").then(setSimulationCache).catch(() => undefined);
      const validations = await invoke<SavedValidation[]>("validation_history", { runId: activeRunId || null, limit: 20 });
      setSavedValidations(validations);
      setNotice(`Forge cross-check complete · ${number(report.games)} games · ${report.crashes} incomplete`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setForgeRunning(false);
    }
  };

  const cancelForge = async () => {
    await invoke("cancel_forge_validation");
    setNotice("Stopping Forge and cleaning its temporary match files…");
  };

  const clearSimulationCache = async () => {
    try {
      const status = await invoke<SimulationCacheStatus>("clear_simulation_cache");
      setSimulationCache(status);
      setNotice("Deterministic matchup cache cleared");
    } catch (error) {
      setNotice(String(error));
    }
  };

  const refreshArtCache = async () => {
    try {
      setArtCache(await invoke<ArtCacheStatus>("art_cache_status"));
    } catch (error) {
      setNotice(String(error));
    }
  };

  const clearArtCache = async () => {
    try {
      setArtCache(await invoke<ArtCacheStatus>("clear_art_cache"));
      setNotice("Downloaded card-art cache cleared");
    } catch (error) {
      setNotice(String(error));
    }
  };

  const createBackup = async () => {
    try {
      await invoke("create_local_backup");
      setStorageHealth(await invoke<StorageHealth>("storage_health"));
      setNotice("Local recovery backup created");
    } catch (error) { setNotice(String(error)); }
  };

  const restoreBackup = async (id: string) => {
    if (!window.confirm("Restore this local backup? Metagene will create a safety backup of the current data first.")) return;
    try {
      setStorageHealth(await invoke<StorageHealth>("restore_local_backup", { id }));
      setSavedRuns(await invoke<RunSummary[]>("run_history", { limit: 200 }));
      setNotice("Backup restored; reload any currently open saved experiment before continuing");
    } catch (error) { setNotice(String(error)); }
  };

  const repairDatabase = async (name: string) => {
    if (!window.confirm(`Quarantine corrupt ${name} and let Metagene recreate it? The quarantined file is retained for recovery.`)) return;
    try { setStorageHealth(await invoke<StorageHealth>("repair_corrupt_database", { name })); setNotice(`${name} quarantined; its feature will recreate clean storage when next opened`); }
    catch (error) { setNotice(String(error)); }
  };

  const copyDiagnostics = async () => {
    try { await navigator.clipboard.writeText(await invoke<string>("export_safe_diagnostics")); setNotice("Privacy-safe diagnostics copied"); }
    catch (error) { setNotice(String(error)); }
  };

  const refreshFeedback = async () => {
    const [records, summary] = await Promise.all([
      invoke<MatchRecord[]>("match_history", { limit: 200 }),
      invoke<FeedbackSummary>("feedback_summary", { platform, format })
    ]);
    setMatchRecords(records);
    setFeedbackSummary(summary);
  };

  const importMatches = async () => {
    if (!matchImportText.trim()) return;
    try {
      const report = await invoke<MatchImportReport>("import_match_history", { source: matchImportSource, text: matchImportText });
      await refreshFeedback();
      setNotice(`Imported ${report.imported} matches · ${report.exact_revision_matches} exact revisions · ${report.quarantined_identity} quarantined`);
    } catch (error) { setNotice(String(error)); }
  };

  const saveMatchEdit = async (record: MatchRecord) => {
    try {
      await invoke<MatchRecord>("update_match_record", { edit: {
        id: record.id,
        opponent_archetype: record.opponent_archetype,
        play_draw: record.play_draw,
        sideboarded: record.sideboarded ?? null,
        notes: record.notes,
        predicted_win_rate: record.predicted_win_rate ?? null
      } });
      await refreshFeedback();
      setNotice("Match context saved locally");
    } catch (error) { setNotice(String(error)); }
  };

  const previewFeedbackWeights = async () => {
    if (!gauntletDecks.length) return;
    const current = gauntletDecks.map((_, index) => gauntletWeights[index] || 1);
    try {
      setReweightSuggestions(await invoke<ReweightSuggestion[]>("suggest_feedback_reweights", {
        platform, format, opponents: gauntletDecks.map((deck) => deck.name), currentWeights: current
      }));
    } catch (error) { setNotice(String(error)); }
  };

  const applyFeedbackWeights = () => {
    if (reweightSuggestions.length !== gauntletDecks.length) return;
    setGauntletWeights(reweightSuggestions.map((row) => row.suggested_weight));
    setReweightSuggestions([]);
    setNotice("User-approved match feedback applied to the current field weights");
  };

  const rankFinalists = async () => {
    if (!workspace || candidates.length < 2) return;
    const seats = requiresCommander ? Math.max(2, Math.min(6, validationSeats)) : 2;
    const opponents = requiresCommander ? gauntletDecks.slice(0, seats - 1) : gauntletDecks;
    if (opponents.length < (requiresCommander ? seats - 1 : 1)) {
      setNotice(`Add ${requiresCommander ? seats - 1 : 1} opponent deck${requiresCommander && seats > 2 ? "s" : ""} for this matchup`);
      return;
    }
    const finalistTotal = Math.min(64, Math.max(2, finalistCount), candidates.length);
    setHalvingRunning(true);
    setHalvingProgress(undefined);
    setNotice(`Starting successive halving for ${finalistTotal} finalists…`);
    try {
      const report = await invoke<SuccessiveHalvingReport>("run_hexdek_halving", {
        request: {
          candidates: candidates.slice(0, finalistTotal),
          opponents,
          opponent_weights: requiresCommander ? opponents.map(() => 1) : gauntletWeights.slice(0, opponents.length),
          pool: workspace.pool,
          base_games: requiresCommander ? halvingBaseGames * seats : halvingBaseGames,
          rounds: halvingRounds,
          seed: workspace.config.seed,
          workers: profile?.recommended_workers || 1,
          seats,
          hat_budget: hatBudget,
          max_turns: 60,
          game_timeout_seconds: halvingTimeout,
          structural_weight: 0.35,
          risk_aversion: riskAversion,
          worst_matchup_weight: worstMatchupWeight,
          seat_position_weight: seatPositionWeight,
          adaptive_elimination: adaptiveElimination,
          commander: requiresCommander
        },
        runId: activeRunId || null
      });
      setHalvingReport(report);
      void invoke<SimulationCacheStatus>("simulation_cache_status").then(setSimulationCache).catch(() => undefined);
      const [validations, halvings] = await Promise.all([
        invoke<SavedValidation[]>("validation_history", { runId: activeRunId || null, limit: 50 }),
        invoke<SavedHalving[]>("halving_history", { runId: activeRunId || null, limit: 10 })
      ]);
      setSavedValidations(validations);
      setSavedHalvings(halvings);
      if (report.winner_rank != null) {
        setSelectedRank(report.winner_rank);
        const winnerEvidence = [...report.rounds]
          .reverse()
          .flatMap((round) => round.results)
          .find((result) => result.candidate_rank === report.winner_rank && result.report);
        if (winnerEvidence?.report) setSimulationReport(winnerEvidence.report);
      }
      const throughput = report.elapsed_ms > 0 ? report.total_attempts / (report.elapsed_ms / 1000) : 0;
      setNotice(`Finalist ranking complete · winner #${report.winner_rank || "—"} · ${number(throughput)} attempts/sec · ${report.total_crashes} crashes`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setHalvingRunning(false);
    }
  };

  const startHybridEvolution = async (
    fieldDecks: Deck[] = gauntletDecks,
    fieldWeights: number[] = gauntletWeights,
    fieldProvenance: GauntletProvenance | undefined = gauntletProvenance,
    automated = false,
    competitiveClaimsAllowed = gauntletMetadata.competitive_claims_allowed !== false,
    fieldRevisionId = gauntletMetadata.field_revision_id,
    continueRunId?: string,
    continuationCandidate?: RankedDeck
  ): Promise<boolean> => {
    if (!workspace) return false;
    let claimsAllowed = competitiveClaimsAllowed;
    const seats = requiresCommander ? Math.max(2, Math.min(6, validationSeats)) : 2;
    const currentOpponents = requiresCommander ? fieldDecks.slice(0, seats - 1) : fieldDecks;
    if (currentOpponents.length < (requiresCommander ? seats - 1 : 1)) {
      setNotice(`Add ${requiresCommander ? seats - 1 : 1} opponent deck${requiresCommander && seats > 2 ? "s" : ""} before starting the hybrid loop`);
      return false;
    }
    const existingGenomes = new Set(currentOpponents.map(deckGenomeKey));
    const historicalOpponents = (automated ? [] : (hallOfFame?.entries || []))
      .filter((entry) => hallOfFame?.active_diverse_ids.includes(entry.id))
      .filter((entry) => !existingGenomes.has(deckGenomeKey(entry.candidate.deck)))
      .slice(0, historicalChampionCount)
      .map((entry) => ({ ...entry.candidate.deck, name: `Historical · ${entry.candidate.deck.name}` }));
    const opponents = [...currentOpponents, ...historicalOpponents];
    const currentWeightTotal = fieldWeights
      .slice(0, currentOpponents.length)
      .reduce((sum, weight) => sum + (Number.isFinite(weight) && weight > 0 ? weight : 0), 0) || currentOpponents.length;
    const historicalShare = historicalOpponents.length ? historicalChampionWeight : 0;
    const opponentWeights = requiresCommander
      ? opponents.map(() => 1)
      : [
        ...currentOpponents.map((_, index) => ((fieldWeights[index] || 1) / currentWeightTotal) * (1 - historicalShare)),
        ...historicalOpponents.map(() => historicalShare / historicalOpponents.length)
      ];
    const routeCandidate = continuationCandidate?.deck || workspace.seed_deck;
    if (routeCandidate.mainboard.length > 0) {
      try {
        const routePlan = await invoke<EngineRoutePlan>("build_engine_route_plan", {
          request: {
            platform,
            format,
            candidate: routeCandidate,
            opponents: currentOpponents,
            opponent_weights: fieldWeights.slice(0, currentOpponents.length),
            tournament_claims_allowed: claimsAllowed
          }
        });
        setEngineRoutePlan(routePlan);
        claimsAllowed = claimsAllowed && routePlan.claim.promotable;
        if (!routePlan.claim.promotable) {
          setProvisionalResultReason(routePlan.claim.reasons.join(" · "));
        }
      } catch (error) {
        claimsAllowed = false;
        setProvisionalResultReason(`engine route audit failed (${String(error)})`);
      }
    }
    const hybridRequest: HybridEvolutionRequest = {
      evolution: workspace,
      opponents,
      opponent_weights: opponentWeights,
      gauntlet_provenance: fieldProvenance,
      competitive_claims_allowed: claimsAllowed,
      field_revision_id: fieldRevisionId,
      cycles: automated ? 1 : hybridCycles,
      finalists: automated ? 8 : Math.min(16, Math.max(2, finalistCount)),
      base_games: automated ? (requiresCommander ? 16 * seats : 16) : (requiresCommander ? halvingBaseGames * seats : halvingBaseGames),
      rounds: automated ? 3 : halvingRounds,
      workers: profile?.recommended_workers || 1,
      seats,
      hat_budget: hatBudget,
      max_turns: 60,
      game_timeout_seconds: halvingTimeout,
      structural_weight: automated || workspace.config.evolution_mode === "tune" ? 0 : 0.35,
      risk_aversion: riskAversion,
      worst_matchup_weight: worstMatchupWeight,
      seat_position_weight: seatPositionWeight,
      adaptive_elimination: adaptiveElimination,
      coevolve_adversaries: coevolveAdversaries,
      learn_packages_from_hall: learnPackagesFromHall,
      holdout_games: automated ? 64 : holdoutGames,
      commander: requiresCommander,
      continue_run_id: continueRunId,
      continuation_candidate_rank: continuationCandidate?.rank
    };
    setHybridRunning(true);
    setHybridProgress(undefined);
    if (!continueRunId) {
      setHistory([]);
      setStatus(undefined);
      setSelectedRank(1);
      setSavedValidations([]);
      setSavedHalvings([]);
      setSimulationReport(undefined);
      setHalvingReport(undefined);
      setLoadedSavedRun(undefined);
    }
    const cycleCount = automated ? 1 : hybridCycles;
    setNotice(continueRunId
      ? `Continuing candidate #${continuationCandidate?.rank} for ${number(workspace.config.generations)} generations, then validating against the locked field…`
      : `Starting ${cycleCount} evolve-and-validate cycle${cycleCount === 1 ? "" : "s"} against the current field${historicalOpponents.length ? ` plus ${historicalOpponents.length} diverse historical champion${historicalOpponents.length === 1 ? "" : "s"}` : ""}…`);
    try {
      const id = await invoke<string>("start_hybrid_evolution", { request: hybridRequest });
      setJobId(id);
      setActiveRunId(id);
      setStatus({ id, state: "running" });
      return true;
    } catch (error) {
      setHybridRunning(false);
      setNotice(String(error));
      return false;
    }
  };

  const promoteValidatedCandidate = () => {
    if (!selected || !workspace || validatedRow == null) return;
    const promoted = {
      ...selected.deck,
      name: `${selectedCommander || selected.deck.name} · Validated Seed`
    };
    setWorkspace({ ...workspace, seed_deck: promoted });
    setStatus(undefined);
    setJobId(undefined);
    setActiveRunId(undefined);
    setHistory([]);
    setSelectedRank(1);
    setNotice(`Validated candidate promoted · next evolution will mutate from ${selectedCommander || selected.deck.name}`);
  };

  const promoteHallOfFame = async () => {
    if (!selected) return;
    setHallBusy(true);
    try {
      const entry = await invoke<HallOfFameEntry>("promote_hall_of_fame_candidate", {
        platform,
        format,
        commander: requiresCommander,
        candidate: selected,
        evidenceScore: validationScores.get(selected.rank) ?? selected.fitness.score,
        sourceRunId: activeRunId || null
      });
      const refreshed = await invoke<HallOfFameStatus>("hall_of_fame_status", { platform, format, commander: requiresCommander });
      setHallOfFame(refreshed);
      setSelectedHallId(entry.id);
      setNotice(`${selected.deck.name} added to the diverse Hall of Fame`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setHallBusy(false);
    }
  };

  const deleteHallOfFame = async (id: string) => {
    setHallBusy(true);
    try {
      await invoke("delete_hall_of_fame_entry", { id });
      const refreshed = await invoke<HallOfFameStatus>("hall_of_fame_status", { platform, format, commander: requiresCommander });
      setHallOfFame(refreshed);
      setSelectedHallId((current) => current === id ? refreshed.entries[0]?.id : current);
      setNotice("Hall of Fame entry deleted");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setHallBusy(false);
    }
  };

  const loadBlankWorkspace = async (nextPlatform: string, nextFormat: string) => {
    setPlatform(nextPlatform);
    setFormat(nextFormat);
    setCommanderResults([]);
    setPoolResults([]);
    setPoolQuery("");
    setImportText("");
    setValidatedImportText("");
    setImportError(undefined);
    setStartError(undefined);
    setValidationSeats(nextFormat === "commander" ? 4 : 2);
    if (!catalog?.ready) return;
    setNotice(`Loading ${nextPlatform} ${nextFormat} card pool…`);
    try {
      const request = await invoke<RunRequest>("new_workspace", { platform: nextPlatform, format: nextFormat, deckSize: null });
      if (profile) request.config.worker_threads = profile.recommended_workers;
      setWorkspace(request);
      setStatus(undefined);
      setHistory([]);
      setActiveRunId(undefined);
      setSimulationReport(undefined);
      setSavedValidations([]);
      setSavedHalvings([]);
      setHalvingReport(undefined);
      setHalvingProgress(undefined);
      setHybridRunning(false);
      setHybridProgress(undefined);
      setLoadedSavedRun(undefined);
      setGauntletDecks([]);
      setGauntletWeights([]);
      setGauntletName("");
      setGauntletMetadata({ source: "", observed_at: "", notes: "" });
      setGauntletProvenance(undefined);
      setMetagameBundle("");
      setSavedGauntlets([]);
      setNotice(`Blank genome ready · ${number(request.pool.cards.length)} legal cards`);
    } catch (error) {
      setNotice(String(error));
    }
  };

  const changePlatform = (nextPlatform: string) => {
    const nextFormat = FORMATS[nextPlatform][0].value;
    void loadBlankWorkspace(nextPlatform, nextFormat);
  };

  const findCommanders = async () => {
    if (!commanderQuery.trim()) return;
    const results = await invoke<Card[]>("search_cards", { query: commanderQuery, platform, format, limit: 50 });
    setCommanderResults(results.filter((card) =>
      card.type_line.includes("Legendary Creature") || card.oracle_text.toLowerCase().includes("can be your commander")
    ));
  };

  const importDeck = async () => {
    if (!importText.trim()) return;
    const submittedText = importText;
    setImporting(true);
    setImportError(undefined);
    setStartError(undefined);
    setNotice("Resolving and validating imported deck…");
    try {
      const request = await invoke<RunRequest>("import_workspace", { text: importText, platform, format });
      if (profile) request.config.worker_threads = profile.recommended_workers;
      setWorkspace((current) => current ? {
        ...request,
        simulator_coverage: current.simulator_coverage,
        exclude_disputed_cards: current.exclude_disputed_cards,
        exclude_unsupported_cards: current.exclude_unsupported_cards,
        constraints: {
          ...request.constraints
        },
        config: {
          ...current.config,
          evolve_commanders: request.config.evolve_commanders,
          evolve_sideboard: request.config.evolve_sideboard,
          evolution_mode: "tune",
          tune_policy: {
            root_deck: request.seed_deck,
            strategy_retention_floor: 0.70,
            primary_package_floor: 0.60,
            maximum_nonland_edit_fraction: 0.40
          }
        }
      } : request);
      setValidatedImportText(submittedText);
      setImportError(undefined);
      setStartError(undefined);
      setStatus(undefined);
      setHistory([]);
      setActiveRunId(undefined);
      setLoadedSavedRun(undefined);
      setCommanderQuery("");
      const count = request.seed_deck.mainboard.reduce((sum, entry) => sum + entry.quantity, 0);
      setNotice(`Imported ${count} cards · ready to evolve`);
    } catch (error) {
      const message = String(error);
      setValidatedImportText("");
      setImportError(message);
      setNotice(`Import failed · ${message}`);
    } finally {
      setImporting(false);
    }
  };

  const chooseCommander = (commander: Card) => {
    setWorkspace((current) => {
      if (!current) return current;
      const allowed = new Set(commander.color_identity);
      const legalCards = current.pool.cards.filter((card) =>
        card.id !== commander.id && card.color_identity.every((color) => allowed.has(color))
      );
      return {
        ...current,
        config: { ...current.config, evolve_commanders: false },
        pool: { cards: [commander, ...legalCards] },
        seed_deck: {
          ...current.seed_deck,
          name: `${commander.name} Evolution`,
          command_zone: [{ card_id: commander.id, quantity: 1 }]
        }
      };
    });
    setCommanderResults([]);
    setCommanderQuery(commander.name);
    setNotice(`${commander.name} selected · color identity enforced`);
  };

  const progress = status?.progress;
  const uniqueEvaluations = progress
    ? progress.unique_evaluations || progress.evaluated_candidates
    : 0;
  const uniqueEvaluationRate = progress
    ? progress.unique_evaluations_per_second || progress.candidates_per_second
    : 0;
  const hybridCycleFraction = hybridProgress?.phase === "cycle_complete"
    ? 1
    : hybridProgress?.phase === "validation"
      ? 0.5 + 0.5 * (((hybridProgress.halving?.round || 1) - 1 + (hybridProgress.halving?.candidate_index || 0) / Math.max(1, hybridProgress.halving?.candidates_in_round || 1)) / Math.max(1, hybridProgress.halving?.total_rounds || halvingRounds))
      : 0.5 * ((hybridProgress?.generation?.generation || 0) / Math.max(1, hybridProgress?.generation?.total_generations || workspace?.config.generations || 1));
  const completion = hybridRunning && hybridProgress
    ? ((hybridProgress.cycle - 1) + hybridCycleFraction) / hybridProgress.total_cycles
    : progress ? progress.generation / progress.total_generations : 0;
  const requiresCommander = (workspace?.constraints.minimum_commanders || 0) > 0;
  const hasCommander = (workspace?.seed_deck.command_zone.length || 0) > 0;
  const evolvingCommander = Boolean(workspace?.config.evolve_commanders);
  const hexdekReady = simulators.some((engine) => engine.id === "hexdek" && engine.installed && engine.data_ready);
  const forgeReady = simulators.some((engine) => engine.id === "forge" && engine.installed && engine.data_ready);
  const selectedHall = hallOfFame?.entries.find((entry) => entry.id === selectedHallId) || hallOfFame?.entries[0];
  const latestHoldout = savedValidations.find((validation) => validation.request.phase === "holdout");
  const activeHistoricalCount = Math.min(
    historicalChampionCount,
    hallOfFame?.active_diverse_ids.length || 0
  );

  useEffect(() => {
    if (!workspace) return;
    void invoke<HallOfFameStatus>("hall_of_fame_status", { platform, format, commander: requiresCommander })
      .then((archive) => {
        setHallOfFame(archive);
        setSelectedHallId((current) => archive.entries.some((entry) => entry.id === current) ? current : archive.entries[0]?.id);
      })
      .catch((error) => setNotice(`Hall of Fame unavailable · ${String(error)}`));
    void invoke<CalibrationStatus>("calibration_status", { platform, format, commander: requiresCommander })
      .then(setCalibration)
      .catch(() => setCalibration(undefined));
    void invoke<SurrogateStatus>("surrogate_status", { platform, format, commander: requiresCommander })
      .then(setSurrogate)
      .catch(() => setSurrogate(undefined));
  }, [workspace?.platform, workspace?.format, platform, format, requiresCommander]);

  useEffect(() => {
    if (!requiresCommander) {
      setCommanderConformance(undefined);
      return;
    }
    void invoke<CommanderConformanceReport>("commander_conformance")
      .then(setCommanderConformance)
      .catch((error) => setNotice(`Commander conformance unavailable · ${String(error)}`));
  }, [requiresCommander]);
  const selectedCommander = selected?.deck.command_zone[0]
    ? cards.get(selected.deck.command_zone[0].card_id)?.name
    : undefined;
  const validatedRow = simulationReport?.engine === "forge"
    ? simulationReport.rows.find((row) => row.deck.startsWith("Candidate · "))
    : selectedCommander
    ? simulationReport?.rows.find((row) => row.deck === selectedCommander)
    : simulationReport?.rows[0];
  const selectedResultName = validatedRow?.deck;
  const validatedEvidence = simulationReport
    ? (() => {
        const confidence = evidenceScore(simulationReport, selectedResultName, riskAversion, worstMatchupWeight, seatPositionWeight);
        return simulationReport.commander_evidence
          ? confidence * 0.5 + simulationReport.commander_evidence.objective_score * 0.5
          : confidence;
      })()
    : undefined;
  const validatedPosition = simulationReport
    ? seatPositionSummary(simulationReport, selectedResultName)
    : undefined;
  const focusedCards = useMemo(() => {
    const selectedIds = new Set(workspace?.pool_filter || []);
    return workspace?.pool.cards.filter((card) => selectedIds.has(card.name)) || [];
  }, [workspace]);
  const focusedCapacity = useMemo(() => focusedCards.reduce((total, card) => {
    const locked = workspace?.constraints.locked_cards.find((entry) => entry.card_id === card.id);
    const naturalMaximum = card.features.unlimited_copies
      ? (workspace?.constraints.target_mainboard_size || 0) + (workspace?.constraints.target_sideboard_size || 0)
      : workspace?.constraints.default_max_copies || 0;
    return total + (locked?.maximum ?? naturalMaximum);
  }, 0), [focusedCards, workspace]);
  const poolAuditNames = useMemo(
    () => [...new Set([...poolResults, ...focusedCards].map((card) => card.name))],
    [poolResults, focusedCards]
  );
  useEffect(() => {
    if (!poolAuditNames.length) {
      setPoolConformance(undefined);
      return;
    }
    let disposed = false;
    void invoke<DeckConformanceReport>("card_conformance", { cardNames: poolAuditNames })
      .then((report) => { if (!disposed) setPoolConformance(report); })
      .catch(() => { if (!disposed) setPoolConformance(undefined); });
    return () => { disposed = true; };
  }, [poolAuditNames]);
  const poolConformanceByName = useMemo(
    () => new Map((poolConformance?.cards || []).map((card) => [card.name.toLowerCase(), card])),
    [poolConformance]
  );
  const focusedPoolReady = !workspace?.pool_filter.length || (
    focusedCards.length === workspace.pool_filter.length
    && focusedCapacity >= workspace.constraints.target_mainboard_size + workspace.constraints.target_sideboard_size
  );
  const importedSeedCount = workspace?.seed_deck.mainboard.reduce((sum, entry) => sum + entry.quantity, 0) || 0;
  const importNeedsValidation = Boolean(importText.trim()) && importText !== validatedImportText;
  const importValidated = Boolean(importText.trim()) && importText === validatedImportText && importedSeedCount > 0;
  const canStart = Boolean(workspace && catalog?.ready && focusedPoolReady && !importNeedsValidation && !importError && (!requiresCommander || hasCommander || evolvingCommander));
  const activeValidationSeats = requiresCommander ? Math.max(2, Math.min(6, validationSeats)) : 2;
  const requiredOpponentCount = requiresCommander ? activeValidationSeats - 1 : 1;
  const maximumGauntletDecks = requiresCommander ? 5 : 32;
  const activeGauntlet = useMemo(
    () => requiresCommander ? gauntletDecks.slice(0, requiredOpponentCount) : gauntletDecks,
    [requiresCommander, gauntletDecks, requiredOpponentCount]
  );
  const gauntletWeightTotal = gauntletWeights
    .slice(0, gauntletDecks.length)
    .reduce((sum, weight) => sum + (Number.isFinite(weight) && weight > 0 ? weight : 0), 0) || gauntletDecks.length || 1;
  const matchupMultiplier = requiresCommander ? activeValidationSeats : Math.max(1, activeGauntlet.length);
  const hybridMatchupMultiplier = requiresCommander
    ? activeValidationSeats
    : Math.max(1, activeGauntlet.length + activeHistoricalCount);
  const validatedFitness = selected && validatedEvidence != null
    ? selected.fitness.score * 0.35 + validatedEvidence * 0.65
    : undefined;
  const estimatedHalvingGames = useMemo(() => {
    let entrants = Math.min(64, Math.max(2, finalistCount), candidates.length);
    let games = 0;
    for (let round = 0; round < Math.min(8, Math.max(1, halvingRounds)) && entrants > 0; round += 1) {
      games += entrants * Math.min(10_000_000, halvingBaseGames * 2 ** round);
      entrants = Math.ceil(entrants / 2);
      if (entrants <= 1) break;
    }
    return games * matchupMultiplier;
  }, [finalistCount, halvingRounds, halvingBaseGames, candidates.length, matchupMultiplier]);
  const estimatedHybridAttempts = useMemo(() => {
    let entrants = Math.min(16, Math.max(2, finalistCount));
    let games = 0;
    for (let round = 0; round < Math.min(8, Math.max(1, halvingRounds)) && entrants > 0; round += 1) {
      games += entrants * Math.min(10_000_000, halvingBaseGames * 2 ** round);
      entrants = Math.ceil(entrants / 2);
      if (entrants <= 1) break;
    }
    return games * Math.min(20, Math.max(1, hybridCycles)) * hybridMatchupMultiplier;
  }, [finalistCount, halvingRounds, halvingBaseGames, hybridCycles, hybridMatchupMultiplier]);
  const coverageDecks = useMemo<Deck[]>(() => {
    if (!selected) return [];
    return [selected.deck, ...activeGauntlet];
  }, [selected, activeGauntlet]);

  useEffect(() => {
    if (!workspace) {
      setSavedGauntlets([]);
      return;
    }
    void refreshGauntletLibrary(workspace.platform, workspace.format, requiresCommander);
  }, [workspace?.platform, workspace?.format, requiresCommander]);

  useEffect(() => {
    if (!hexdekReady || coverageDecks.length === 0) {
      setCoverageReport(undefined);
      setCoverageError(undefined);
      return;
    }
    let disposed = false;
    setCoverageLoading(true);
    setCoverageError(undefined);
    void invoke<HexdekCoverageReport>("hexdek_deck_coverage", { decks: coverageDecks })
      .then((coverage) => {
        if (!disposed) {
          setCoverageReport(coverage);
        }
      })
      .catch((error) => {
        if (!disposed) {
          setCoverageReport(undefined);
          setCoverageError(String(error));
        }
      })
      .finally(() => {
        if (!disposed) setCoverageLoading(false);
      });
    return () => { disposed = true; };
  }, [hexdekReady, coverageDecks]);

  useEffect(() => {
    if (coverageDecks.length === 0) {
      setConformanceReport(undefined);
      setConformanceError(undefined);
      return;
    }
    let disposed = false;
    setConformanceLoading(true);
    setConformanceError(undefined);
    void invoke<ConformanceAuditReport>("deck_conformance", { decks: coverageDecks })
      .then((report) => {
        if (!disposed) {
          setConformanceReport(report);
          setConformanceSummary(report.summary);
        }
      })
      .catch((error) => {
        if (!disposed) {
          setConformanceReport(undefined);
          setConformanceError(String(error));
        }
      })
      .finally(() => {
        if (!disposed) setConformanceLoading(false);
      });
    return () => { disposed = true; };
  }, [coverageDecks]);

  const coverageBlocked = Boolean(coverageReport?.decks.some((deck) => deck.missing_unique > 0));
  const gauntletCoverageBlocked = Boolean(coverageReport?.decks.slice(1).some((deck) => deck.missing_unique > 0));
  const coveragePartial = Boolean(coverageReport?.decks.some((deck) => deck.partial_unique > 0));
  const coverageSemanticRisk = Boolean(coverageReport?.decks.some((deck) => deck.semantic_risk_unique > 0));
  const candidateConformanceByName = useMemo(
    () => new Map((conformanceReport?.decks[0]?.cards || []).map((card) => [card.name.toLowerCase(), card])),
    [conformanceReport]
  );
  const halvingFailures = halvingReport?.rounds.flatMap((round) => round.results.filter((result) => result.error).map((result) => ({ round: round.round, ...result }))) || [];
  const halvingWinnerEvidence = halvingReport?.winner_rank == null ? undefined : [...(halvingReport?.rounds || [])]
    .reverse()
    .flatMap((round) => round.results)
    .find((result) => result.candidate_rank === halvingReport.winner_rank);
  const filteredRuns = savedRuns.filter((run) => `${run.title} ${run.notes} ${run.tags.join(" ")} ${run.platform} ${run.format} ${run.state}`.toLowerCase().includes(runSearch.trim().toLowerCase()));
  const comparedRuns = comparedRunIds.map((id) => savedRuns.find((run) => run.id === id)).filter((run): run is RunSummary => Boolean(run));

  return (
    <main className="shell">
      <aside className="sidebar">
        <div className="brand"><span className="brand-mark">M</span><span><b>Metagene</b><small>Deck intelligence</small></span></div>
        <nav aria-label="Workspace views">
          {([["build", "01", "Build"], ["results", "02", "Results"], ["library", "03", "Card Library"], ["settings", "04", "Settings"]] as const).map(([view, icon, label]) => <button key={view} aria-label={label} className={activeView === view ? "active" : ""} onClick={() => setActiveView(view)}><span aria-hidden="true">{icon}</span>{label}</button>)}
        </nav>
        <div className="sidebar-status"><i className={catalog?.ready && hexdekReady ? "ready" : "pending"} /><span><b>{catalog?.ready && hexdekReady ? "Ready" : firstRunBusy ? "Setting up" : "Needs attention"}</b><small>{notice}</small></span></div>
      </aside>

      <div className={`workspace view-${activeView}`}>
        <header className="topbar">
          <div>
            <span className="eyebrow">{VIEW_LABELS[activeView]}</span>
            <h1>{activeView === "build" || activeView === "results" ? workspace?.seed_deck.name || "Untitled Evolution" : VIEW_LABELS[activeView]}</h1>
          </div>
          <div className="format-controls">
            <select value={platform} disabled={!catalog?.ready} onChange={(event) => changePlatform(event.target.value)}>
              <option value="arena">Arena</option><option value="paper">Paper</option><option value="mtgo">Magic Online</option>
            </select>
            <select value={format} disabled={!catalog?.ready} onChange={(event) => void loadBlankWorkspace(platform, event.target.value)}>
              {FORMATS[platform].map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
            </select>
            <div className="format-pill"><span>{platform.toUpperCase()}</span> {format} · {(workspace?.constraints.target_mainboard_size || 60) + (workspace?.constraints.minimum_commanders || 0)} main{workspace?.constraints.target_sideboard_size ? ` + ${workspace.constraints.target_sideboard_size} side` : ""}</div>
          </div>
        </header>

        <section className="panel library-view">
          <div className="panel-title"><div><span className="step">LIB</span><h2>Format-legal card library</h2></div><span className="legal">{platform} · {format}</span></div>
          <p>Search the current legal catalog, inspect art and simulator evidence, then add cards to the focused mutation pool used in Build.</p>
          <div className="library-search"><input value={poolQuery} placeholder="Name, rules text, or card type" onChange={(event) => setPoolQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void findPoolCards(); }} /><button disabled={searchingPool || !poolQuery.trim()} onClick={() => void findPoolCards()}>{searchingPool ? "Searching…" : "Search library"}</button></div>
          <div className="library-grid">{poolResults.length ? poolResults.map((card) => <article key={card.id}><CachedCardArt card={card} /><div><b>{card.name}</b><span>{card.set_code.toUpperCase()} · {card.type_line}</span><small>{card.oracle_text || "No rules text"}</small><ConformanceBadge evidence={poolConformanceByName.get(card.name.toLowerCase())} /><button disabled={focusedCards.some((item) => item.name === card.name)} onClick={() => addPoolCard(card)}>{focusedCards.some((item) => item.name === card.name) ? "In focused pool" : "Add to focused pool"}</button></div></article>) : <div className="empty-state compact">Search the selected platform and format to browse cards.</div>}</div>
        </section>

        <section className="panel settings-view">
          <div className="panel-title"><div><span className="step">SYS</span><h2>Local engines and storage</h2></div><span className="legal">No cloud account required</span></div>
          <div className="settings-grid"><div><span className="eyebrow">Hardware</span><b>{profile?.cpu_name || "Detecting CPU"}</b><small>{profile ? `${profile.logical_cores} logical cores · ${profile.memory_gib.toFixed(0)} GiB RAM` : "—"}</small><small>{profile?.gpu_name || "No GPU reported"}</small></div><div><span className="eyebrow">Catalog</span><b>{catalog?.ready ? `${number(catalog.card_count)} printings ready` : "Catalog setup required"}</b><button disabled={syncingCatalog} onClick={syncArenaCatalog}>{syncingCatalog ? "Refreshing…" : catalog?.ready ? "Refresh catalog" : "Install catalog"}</button></div><div><span className="eyebrow">Local storage</span><b>{bytes(resourceStatus?.app_data_bytes || 0)} application data</b><small>{number(simulationCache?.entries || 0)} cached matchups · {number(artCache?.entries || 0)} art files</small><button disabled={!simulationCache?.entries} onClick={() => void clearSimulationCache()}>Clear matchup cache</button><button disabled={!artCache?.entries} onClick={() => void clearArtCache()}>Clear art cache</button></div></div>
          <div className="settings-grid">{simulators.map((engine) => <div key={engine.id}><span className="eyebrow">Simulation engine</span><b>{engine.name}</b><small>{engine.note}</small>{engine.installed && engine.data_ready ? <span className="legal">Ready</span> : <button disabled={installingSimulator} onClick={() => void (engine.id === "forge" ? installForge() : installHexdek())}>{installingSimulator ? "Installing…" : `Install / repair ${engine.name}`}</button>}</div>)}</div>
          {resourceStatus && <div className="controls-grid settings-budgets"><label>CPU threads<input type="number" min="1" max="256" value={resourceStatus.budgets.cpu_threads} onChange={(event) => setResourceStatus({ ...resourceStatus, budgets: { ...resourceStatus.budgets, cpu_threads: Number(event.target.value) } })} /></label><label>Engine processes<input type="number" min="1" max="256" value={resourceStatus.budgets.max_engine_processes} onChange={(event) => setResourceStatus({ ...resourceStatus, budgets: { ...resourceStatus.budgets, max_engine_processes: Number(event.target.value) } })} /></label><label>Memory MiB<input type="number" min="256" value={resourceStatus.budgets.memory_mib} onChange={(event) => setResourceStatus({ ...resourceStatus, budgets: { ...resourceStatus.budgets, memory_mib: Number(event.target.value) } })} /></label><label>Run limit (seconds)<input type="number" min="1" value={resourceStatus.budgets.run_time_seconds} onChange={(event) => setResourceStatus({ ...resourceStatus, budgets: { ...resourceStatus.budgets, run_time_seconds: Number(event.target.value) } })} /></label><label>Disk MiB<input type="number" min="256" value={resourceStatus.budgets.disk_mib} onChange={(event) => setResourceStatus({ ...resourceStatus, budgets: { ...resourceStatus.budgets, disk_mib: Number(event.target.value) } })} /></label><button className="primary" onClick={() => void invoke<ResourceBudgetStatus>("save_resource_budgets", { budgets: resourceStatus.budgets }).then(setResourceStatus)}>Save resource limits</button></div>}
          <div className={`setup-readiness ${catalog?.ready && simulators.some((engine) => engine.id === "hexdek" && engine.installed && engine.data_ready) ? "ready" : "needs-setup"}`}><b>{catalog?.ready && simulators.some((engine) => engine.id === "hexdek" && engine.installed && engine.data_ready) ? "Primary workflow ready" : "First-run setup incomplete"}</b><span>{catalog?.ready ? "Card catalog ready" : "Install the card catalog"} · {simulators.some((engine) => engine.id === "hexdek" && engine.installed && engine.data_ready) ? "HexDek ready" : "Install or repair HexDek for matchup evolution"}</span></div>
          <div className="storage-recovery"><div className="panel-title"><div><span className="step">REC</span><h2>Backup, recovery, and diagnostics</h2></div><button className="secondary" onClick={() => void copyDiagnostics()}>Copy safe diagnostics</button></div><p>Backups remain local and are capped at ten. Restore validates every database and creates a safety backup first. Diagnostics omit paths, decks, matches, logs, and account data.</p><div className="storage-actions"><button className="primary" onClick={() => void createBackup()}>Create backup</button>{storageHealth?.backups.slice(0, 5).map((backup) => <button className="secondary" key={backup.id} onClick={() => void restoreBackup(backup.id)}>Restore {backup.id.slice(-8)} · {bytes(backup.bytes)}</button>)}</div><div className="database-health">{storageHealth?.databases.map((database) => <div key={database.name} className={database.healthy ? "healthy" : "corrupt"}><b>{database.name}</b><span>{bytes(database.bytes)} · {database.healthy ? "integrity OK" : database.detail}</span>{!database.healthy && <button className="delete" onClick={() => void repairDatabase(database.name)}>Quarantine + recreate</button>}</div>)}</div></div>
        </section>

        <section className={`hero-grid ${running || hybridRunning ? "run-active" : "run-idle"}`}>
          <div className="panel setup-panel">
            <div className="panel-title"><div><span className="step">01</span><h2>Build a deck</h2></div><span className="legal">{platform} · {format} legal</span></div>
            {!workspace && <div className={`workspace-setup ${firstRunError ? "error" : ""}`}><b>{firstRunError ? "Automatic setup needs attention" : "Preparing your first workspace"}</b><span>{firstRunError || installerProgress?.detail || catalogProgress?.stage || (firstRunBusy ? "Installing the card catalog and bundled simulation tools…" : "Loading local components…")}</span><small>These controls unlock with their normal defaults as soon as the legal card catalog is ready. HexDek is bundled and does not compile on your computer; Forge is downloaded prebuilt.</small></div>}
            <div className="quick-start">
              <div className="quick-field color-field">
                <div><label>Deck colors</label><small>{workspace?.constraints.allowed_colors?.length ? workspace.constraints.allowed_colors.join(" + ") : "Automatic palette"}</small></div>
                <div className="color-choices" role="group" aria-label="Deck colors">
                  <button type="button" className={!workspace?.constraints.allowed_colors ? "selected auto" : "auto"} disabled={!workspace} onClick={() => setDeckColors(undefined)}>Auto</button>
                  {DECK_COLORS.map((color) => <button type="button" key={color.value} title={color.label} aria-label={color.label} aria-pressed={Boolean(workspace?.constraints.allowed_colors?.includes(color.value))} className={`mana-color color-${color.symbol.toLowerCase()} ${workspace?.constraints.allowed_colors?.includes(color.value) ? "selected" : ""}`} disabled={!workspace} onClick={() => toggleDeckColor(color.value)}>{color.symbol}</button>)}
                </div>
                <p>Select only <b>C</b> for a zero-color deck, or combine colorless with any W/U/B/R/G palette.</p>
              </div>
              <label className="quick-field effort-field">Search depth<select value={searchPreset} disabled={!workspace} onChange={(event) => applySearchPreset(event.target.value as SearchPreset)}><option value="quick">Quick · preview</option><option value="balanced">Balanced · recommended</option><option value="deep">Deep · competitive search</option><option value="custom">Custom · advanced settings</option></select><small>{number(workspace?.config.population_size || 0)} decks × {number(workspace?.config.generations || 0)} generations</small></label>
            </div>
            <details className="advanced-setup" open={searchPreset === "custom"}>
              <summary><span>Advanced optimizer controls</span><small>Population, mutation, mana guidance, card coverage, and scoring</small></summary>
              <div className="advanced-body">
            <div className="controls-grid">
              <label>Population<input type="number" min="8" max="8192" disabled={!workspace} value={workspace?.config.population_size ?? 128} onChange={(event) => updateConfig("population_size", Number(event.target.value))} /></label>
              <label>Generations<input type="number" min="1" max="10000" disabled={!workspace} value={workspace?.config.generations ?? 50} onChange={(event) => updateConfig("generations", Number(event.target.value))} /></label>
              <label>Trials / deck<input type="number" min="8" max="100000" disabled={!workspace} value={workspace?.config.trials_per_candidate ?? 256} onChange={(event) => updateConfig("trials_per_candidate", Number(event.target.value))} /></label>
              <label>CPU workers<input type="number" min="1" max={profile?.logical_cores || 64} disabled={!workspace} value={workspace?.config.worker_threads ?? profile?.recommended_workers ?? 1} onChange={(event) => updateConfig("worker_threads", Number(event.target.value))} /></label>
              <label>Sideboard cards<input type="number" min="0" max="30" disabled={!workspace} value={workspace?.constraints.target_sideboard_size ?? 0} onChange={(event) => setWorkspace((current) => current ? { ...current, constraints: { ...current.constraints, target_sideboard_size: Math.max(0, Math.min(30, Number(event.target.value))) }, config: { ...current.config, evolve_sideboard: Number(event.target.value) > 0 } } : current)} /></label>
              <label className="check-label"><input type="checkbox" checked={Boolean(workspace?.config.evolve_sideboard)} disabled={!workspace?.constraints.target_sideboard_size} onChange={(event) => updateConfig("evolve_sideboard", event.target.checked)} /> Evolve sideboard</label>
              <label>Evolution card pool<select disabled={!workspace} value={workspace?.simulator_coverage || "parser_complete"} onChange={(event) => setWorkspace((current) => current ? { ...current, simulator_coverage: event.target.value as RunRequest["simulator_coverage"] } : current)}><option value="parser_complete">HexDek parser-complete</option><option value="present">Present in HexDek</option><option value="all">All format-legal</option></select></label>
              <label className="check-label"><input type="checkbox" disabled={!workspace} checked={workspace?.exclude_disputed_cards ?? true} onChange={(event) => setWorkspace((current) => current ? { ...current, exclude_disputed_cards: event.target.checked } : current)} /> Exclude disputed cards</label>
              <label className="check-label"><input type="checkbox" disabled={!workspace} checked={workspace?.exclude_unsupported_cards ?? true} onChange={(event) => setWorkspace((current) => current ? { ...current, exclude_unsupported_cards: event.target.checked } : current)} /> Exclude unsupported cards</label>
            </div>
            <div className="range-row">
              <label><span>Mutation pressure</span><strong>{Math.round((workspace?.config.mutation_rate ?? 0.85) * 100)}%</strong></label>
              <input type="range" min="5" max="100" disabled={!workspace} value={(workspace?.config.mutation_rate ?? 0.85) * 100} onChange={(event) => updateConfig("mutation_rate", Number(event.target.value) / 100)} />
            </div>
            <div className="range-row">
              <label><span>Package-aware mutations</span><strong>{Math.round((workspace?.config.package_mutation_rate ?? 0.35) * 100)}%</strong></label>
              <input type="range" min="0" max="100" disabled={!workspace} value={(workspace?.config.package_mutation_rate ?? 0.35) * 100} onChange={(event) => updateConfig("package_mutation_rate", Number(event.target.value) / 100)} />
              <label>Package width<input type="number" min="2" max="8" disabled={!workspace} value={workspace?.config.package_size ?? 3} onChange={(event) => updateConfig("package_size", Number(event.target.value))} /></label>
              <small>Correlated swaps add cards sharing a mechanic tag, helping engines, payoffs, ramp, draw, recursion, and interaction evolve together. Set the percentage to zero for independent swaps only.</small>
            </div>
            <div className="range-row">
              <label><span>Mana-base guidance</span><strong>{Math.round((workspace?.config.mana_mutation_rate ?? 0.35) * 100)}%</strong></label>
              <input type="range" min="0" max="100" disabled={!workspace} value={(workspace?.config.mana_mutation_rate ?? 0.35) * 100} onChange={(event) => updateConfig("mana_mutation_rate", Number(event.target.value) / 100)} />
              <small>Retune land count and colored sources from the candidate's curve-weighted casting demand. Set to zero for an unguided benchmark.</small>
            </div>
            <details className="fitness-objectives">
              <summary>Fitness objectives <b>User weighted</b></summary>
              <p>Reward weights are normalized together. Sideboard quality rewards broad, efficient answers when a sideboard is present; matchup engines still rank game one unless they explicitly report sideboarding. Mana failure remains an independent penalty.</p>
              <div className="weight-grid">
                {([
                  ["consistency", "Consistency"],
                  ["tempo", "Tempo"],
                  ["synergy", "Synergy"],
                  ["interaction", "Interaction"],
                  ["resilience", "Resilience"],
                  ["commander", "Commander quality"],
                  ["sideboard", "Sideboard quality"],
                  ["mana_failure_penalty", "Mana-failure penalty"],
                  ["legendary_dead_draw_penalty", "Legendary dead-draw penalty"],
                  ["unsupported_synergy_penalty", "Unsupported-requirement penalty"],
                  ["dead_action_penalty", "Dead-action penalty"],
                  ["narrow_card_penalty", "Narrow-card penalty"],
                  ["closing_power", "Closing power"],
                  ["role_balance", "Role balance"]
                ] as const).map(([key, label]) => <label key={key}><span>{label}<b>{Math.round(workspace?.config.fitness_weights[key] ?? DEFAULT_FITNESS_WEIGHTS[key])}</b></span><input type="range" min="0" max="100" disabled={!workspace} value={workspace?.config.fitness_weights[key] ?? DEFAULT_FITNESS_WEIGHTS[key]} onChange={(event) => updateFitnessWeight(key, Number(event.target.value))} /></label>)}
              </div>
              <button onClick={() => setWorkspace((current) => current ? { ...current, config: { ...current.config, fitness_weights: { ...DEFAULT_FITNESS_WEIGHTS } } } : current)}>Reset balanced defaults</button>
            </details>
              </div>
            </details>
            {requiresCommander && <div className="commander-picker">
              <label>Commander strategy</label>
              <div className="commander-mode">
                <button className={evolvingCommander ? "selected" : ""} onClick={() => setWorkspace((current) => current ? { ...current, config: { ...current.config, evolve_commanders: true } } : current)}>Evolve commander + 99</button>
                <span>or choose a fixed commander below</span>
              </div>
              <div className="commander-search"><input value={commanderQuery} placeholder="Search a legal commander" onChange={(event) => setCommanderQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void findCommanders(); }} /><button onClick={findCommanders}>Find</button></div>
              {commanderResults.length > 0 && <div className="commander-results">{commanderResults.slice(0, 8).map((card) => <button key={card.id} onClick={() => chooseCommander(card)}>{card.name}<small>{card.color_identity.join("") || "Colorless"}</small></button>)}</div>}
              {commanderConformance && <details className={`commander-conformance ${commanderConformance.failed ? "conflict" : "verified"}`}>
                <summary><span>Command-zone rules</span><b>{commanderConformance.passed}/{commanderConformance.cases.length} passing</b></summary>
                <small>{commanderConformance.policy_id} · schema {commanderConformance.schema_version} · {commanderConformance.covered_rules.length} pairing variants covered</small>
                <div>{commanderConformance.covered_rules.map((rule) => <span key={rule}>{rule.replaceAll("-", " ")}</span>)}</div>
                <section>{commanderConformance.cases.map((item) => <p key={item.id} className={item.passed ? "verified" : "conflict"}><span>{item.id.replaceAll("-", " ")}</span><b>{item.actual_rule?.replaceAll("-", " ") || "rejected"}</b></p>)}</section>
              </details>}
            </div>}
            <details className="pool-builder">
              <summary>Focused evolution pool <b>{workspace?.pool_filter.length ? `${focusedCards.length} cards` : "All legal cards"}</b></summary>
              <p>Search this format's legal catalog to constrain mutation to an archetype, synergy package, or custom cube. Minimum and maximum counts become hard evolutionary constraints.</p>
              {Boolean(workspace?.seed_deck.mainboard.length) && <button className="focus-seed" onClick={focusOnSeedDeck}>Use imported deck as focused pool</button>}
              <div className="pool-search"><input value={poolQuery} placeholder="Search legal cards" onChange={(event) => setPoolQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void findPoolCards(); }} /><button disabled={searchingPool || !poolQuery.trim()} onClick={() => void findPoolCards()}>{searchingPool ? "Searching…" : "Find"}</button></div>
              {poolResults.length > 0 && <div className="pool-results">{poolResults.map((card) => <button key={card.id} disabled={focusedCards.some((item) => item.name === card.name)} onClick={() => addPoolCard(card)}><CachedCardArt card={card} /><span>{card.name}<small>{card.set_code.toUpperCase()} · {card.type_line}</small><ConformanceBadge evidence={poolConformanceByName.get(card.name.toLowerCase())} /></span><b>{focusedCards.some((item) => item.name === card.name) ? "Added" : "+ Add"}</b></button>)}</div>}
              {focusedCards.length > 0 && <div className="focused-pool"><div className="focused-head"><span>{focusedCards.length} selected · capacity {number(focusedCapacity)} / {(workspace?.constraints.target_mainboard_size || 0) + (workspace?.constraints.target_sideboard_size || 0)}</span><button onClick={() => setWorkspace((current) => current ? { ...current, pool_filter: [], constraints: { ...current.constraints, locked_cards: [] } } : current)}>Use full legal pool</button></div>{focusedCards.map((card) => {
                const naturalMaximum = card.features.unlimited_copies ? (workspace?.constraints.target_mainboard_size || 0) + (workspace?.constraints.target_sideboard_size || 0) : workspace?.constraints.default_max_copies || 0;
                const locked = workspace?.constraints.locked_cards.find((entry) => entry.card_id === card.id);
                return <div className="focused-card" key={card.id}><CachedCardArt card={card} /><span>{card.name}<small>{card.features.tags.slice(0, 3).join(" · ") || card.type_line}</small><ConformanceBadge evidence={poolConformanceByName.get(card.name.toLowerCase())} /></span><label>Min<input type="number" min="0" max={naturalMaximum} value={locked?.minimum || 0} onChange={(event) => updateLockedCount(card, "minimum", Number(event.target.value))} /></label><label>Max<input type="number" min="0" max={naturalMaximum} value={locked?.maximum ?? naturalMaximum} onChange={(event) => updateLockedCount(card, "maximum", Number(event.target.value))} /></label><button onClick={() => removePoolCard(card)}>Remove</button></div>;
              })}{focusedCapacity < (workspace?.constraints.target_mainboard_size || 0) + (workspace?.constraints.target_sideboard_size || 0) && <small className="pool-warning">Add more cards or raise maximum counts before starting; this pool cannot currently fill the mainboard and sideboard.</small>}</div>}
            </details>
            <details className={`deck-import ${importError ? "failed" : importValidated ? "validated" : importNeedsValidation ? "pending" : ""}`}>
              <summary><span>Import an existing deck</span><b>{importError ? "Import failed" : importValidated ? `${number(importedSeedCount)} cards active` : importNeedsValidation ? "Validation required" : "Optional"}</b></summary>
              <p>Paste Arena, paper, or Magic Online quantity/name text. Commander, Deck, and Sideboard sections are recognized.</p>
              <textarea value={importText} onChange={(event) => { setImportText(event.target.value); if (event.target.value !== validatedImportText) setImportError(undefined); }} placeholder={requiresCommander ? "Commander\n1 Atraxa, Praetors' Voice\n\nDeck\n1 Sol Ring\n…" : "Deck\n4 Card Name (SET) 123\n…"} />
              <button className="secondary" disabled={importing || !importText.trim()} onClick={importDeck}>{importing ? "Validating…" : "Import and validate"}</button>
              {importError && <small className="import-state error" role="alert">Import failed: {importError}. Evolution is blocked until this list validates or you clear it.</small>}
              {!importError && importNeedsValidation && <small className="import-state warning">Validate this pasted list before evolving. The blank workspace will not run in its place.</small>}
              {importValidated && <small className="import-state success">This {number(importedSeedCount)}-card deck is the active evolutionary seed.</small>}
            </details>
            <div className="evolution-intent">
              <span className="eyebrow">Evolution intent</span>
              <div className="commander-mode">
                <button className={workspace?.config.evolution_mode === "tune" ? "selected" : ""} disabled={!workspace?.seed_deck.mainboard.length} onClick={() => setWorkspace((current) => current ? { ...current, config: { ...current.config, evolution_mode: "tune", tune_policy: { ...(current.config.tune_policy || { strategy_retention_floor: 0.70, primary_package_floor: 0.60, maximum_nonland_edit_fraction: 0.40 }), root_deck: current.config.tune_policy?.root_deck || current.seed_deck } } } : current)}>Tune this strategy</button>
                <button className={workspace?.config.evolution_mode === "rebuild" ? "selected" : ""} onClick={() => updateConfig("evolution_mode", "rebuild")}>Rebuild</button>
              </div>
              <small>{workspace?.config.evolution_mode === "tune" ? "The imported root stays pinned. Primary packages, strategy retention, and the nonland edit budget are hard constraints." : "The seed is generation zero, but evolution may replace its strategy completely."}</small>
              {detectedSeedStrategies.length > 0 && <div className="strategy-chips">{detectedSeedStrategies.map((strategy) => <span key={strategy}>{strategy.replaceAll("-", " ")}</span>)}</div>}
              {(strategySignature?.packages?.length || 0) > 0 && <details><summary>Measured Tune signature</summary>{strategySignature?.packages.map((item) => <span key={item.package}>{item.primary ? "Primary" : "Support"} · {item.package} · {item.root_unique_cards} cards / {item.root_support.toFixed(0)} copies</span>)}</details>}
              {workspace?.config.evolution_mode === "tune" && <details><summary>Balanced Tune limits</summary><div className="controls-grid"><label>Overall retention<input type="number" min="0" max="100" value={Math.round((workspace.config.tune_policy?.strategy_retention_floor ?? 0.70) * 100)} onChange={(event) => setWorkspace((current) => current ? { ...current, config: { ...current.config, tune_policy: { ...current.config.tune_policy, strategy_retention_floor: Number(event.target.value) / 100 } } } : current)} /></label><label>Primary package retention<input type="number" min="0" max="100" value={Math.round((workspace.config.tune_policy?.primary_package_floor ?? 0.60) * 100)} onChange={(event) => setWorkspace((current) => current ? { ...current, config: { ...current.config, tune_policy: { ...current.config.tune_policy, primary_package_floor: Number(event.target.value) / 100 } } } : current)} /></label><label>Maximum nonland edits<input type="number" min="0" max="100" value={Math.round((workspace.config.tune_policy?.maximum_nonland_edit_fraction ?? 0.40) * 100)} onChange={(event) => setWorkspace((current) => current ? { ...current, config: { ...current.config, tune_policy: { ...current.config.tune_policy, maximum_nonland_edit_fraction: Number(event.target.value) / 100 } } } : current)} /></label></div></details>}
            </div>
            <div className="anchors">
              <span>{workspace?.seed_deck.mainboard.length ? "Imported seed" : "Starting mode"}</span>
              <b>{workspace?.seed_deck.mainboard.length ? `${workspace.seed_deck.mainboard.length} unique cards` : "Blank genome"}</b>
              <b>{number(workspace?.pool.cards.length || 0)} format-legal cards</b>
              <b>{workspace?.simulator_coverage === "parser_complete" ? "Parser-complete pool" : workspace?.simulator_coverage === "present" ? "HexDek-present pool" : "Unrestricted legal pool"}</b>
              <b>{workspace?.exclude_disputed_cards ? "Disputed excluded" : "Disputed allowed"} · {workspace?.exclude_unsupported_cards ? "Unsupported excluded" : "Unsupported allowed"}</b>
            </div>
            <button className="primary launch-button" disabled={!canStart || running || hybridRunning || forgeRunning} onClick={start}>{running ? "Evolution in progress" : requiresCommander && !hasCommander && !evolvingCommander ? "Choose a commander" : evolvingCommander ? "Evolve commander + deck" : workspace?.config.evolution_mode === "tune" ? "Tune deck" : workspace?.seed_deck.mainboard.length ? "Rebuild freely" : "Build competitive deck"}</button>
            {startError && <div className="launch-error" role="alert"><b>Evolution did not start</b><span>{startError}</span><small>Adjust the named setting and try again. Your imported seed is still active.</small></div>}
            <small className="resume-unavailable">Screen scores rank fast structural evidence, not win rate. Tune keeps the original baseline and uses staged validation when a compatible opponent field and complete engine coverage are available.</small>
            {loadedSavedRun?.saved.resumable && <button className="secondary resume-run" disabled={running || hybridRunning || forgeRunning} onClick={() => void resumeSavedRun()}>{loadedSavedRun.saved.run_kind === "hybrid" ? `Resume hybrid ${loadedSavedRun.saved.hybrid_phase?.replace("_", " ") || "checkpoint"} · ${number(loadedSavedRun.saved.remaining_cycles)} cycle${loadedSavedRun.saved.remaining_cycles === 1 ? "" : "s"} left` : `Resume checkpoint · ${number(loadedSavedRun.saved.remaining_generations)} generations left`}</button>}
            {legacyStructuralRun && <small className="resume-unavailable">Legacy structural model—re-evaluate before continuing or validating. Continuing a stored deck recomputes its immutable Tune signature and performs a catalog-v11 Rules IR and capability rescore under {CURRENT_STRUCTURAL_REVISION}.</small>}
            {loadedSavedRun?.saved.hybrid_gauntlet_provenance && <small className="resume-unavailable">Opponent field: {loadedSavedRun.saved.hybrid_gauntlet_provenance.name} v{loadedSavedRun.saved.hybrid_gauntlet_provenance.revision}</small>}
            {loadedSavedRun?.saved.status.state === "interrupted" && !loadedSavedRun.saved.resumable && <small className="resume-unavailable">{loadedSavedRun.saved.run_kind === "hybrid" ? "This legacy hybrid row predates phase-aware checkpoints; its saved champion remains inspectable and promotable." : "This interrupted run stopped before its first durable candidate checkpoint."}</small>}
            {running && <button className="cancel" onClick={cancel}>Stop safely</button>}
          </div>

          <div className="panel live-panel">
            <div className="panel-title"><div><span className="step">02</span><h2>Live evolution</h2></div><span className={running ? "pulse" : "idle"}>{running ? "Running" : "Idle"}</span></div>
            <div className="run-stats">
              {hybridProgress && <Metric label="Hybrid cycle" value={`${hybridProgress.cycle} / ${hybridProgress.total_cycles}`} />}
              <Metric label="Generation" value={progress ? `${progress.generation} / ${progress.total_generations}` : "—"} />
              <Metric label="Population slots" value={progress ? number(progress.evaluated_candidates) : "—"} />
              <Metric label="Unique evaluations" value={progress ? number(uniqueEvaluations) : "—"} />
              <Metric label="Reused duplicates" value={progress ? number(progress.evaluations_avoided || 0) : "—"} />
              <Metric label="Unique evals / sec" value={progress ? number(uniqueEvaluationRate) : "—"} tone="accent" />
              <Metric label="Current generation" value={progress ? `${number(progress.unique_in_generation || workspace?.config.population_size || 0)} unique · ${number(progress.duplicates_in_generation || 0)} reused` : "—"} />
              <Metric label="Strategy niches" value={progress ? number(progress.map_elite_cells || 0) : mapElites.length ? number(mapElites.length) : "—"} />
            </div>
            <div className="progress-track"><div style={{ width: `${completion * 100}%` }} /></div>
            <div className="chart-label"><span>Best screen score by generation</span><strong>{progress?.best.fitness.score.toFixed(3) || "Awaiting run"}</strong></div>
            <Sparkline values={history} />
          </div>
        </section>

        <section className="results-layout">
          <div className="candidate-list panel">
            <div className="panel-title"><div><span className="step">03</span><h2>Leading candidates</h2></div><button className="secondary" disabled={!orderedCandidates.length} onClick={() => void bulkExportCandidates()}>Export top 8</button></div>
            {status?.result?.promotion_evidence && <div className={`promotion-evidence ${status.result.promotion_evidence.status}`}><b>{status.result.promotion_evidence.status.replaceAll("_", " ")}</b><span>{status.result.promotion_evidence.reason}</span><small>{status.result.promotion_evidence.selection_gain != null ? `Selection ${status.result.promotion_evidence.selection_gain >= 0 ? "+" : ""}${percent(status.result.promotion_evidence.selection_gain)} · ` : ""}{status.result.promotion_evidence.holdout_gain != null ? `Holdout ${status.result.promotion_evidence.holdout_gain >= 0 ? "+" : ""}${percent(status.result.promotion_evidence.holdout_gain)} · ` : ""}{status.result.promotion_evidence.forge_gain != null ? `Forge ${status.result.promotion_evidence.forge_gain >= 0 ? "+" : ""}${percent(status.result.promotion_evidence.forge_gain)}` : status.result.promotion_evidence.single_engine_validated ? "Single-engine validated" : "Paired validation"}</small></div>}
            {baselineCandidate && <button className={selected?.rank === 0 ? "candidate baseline selected" : "candidate baseline"} onClick={() => setSelectedRank(0)}><span className="rank">ROOT</span><span><strong>{baselineCandidate.fitness.score.toFixed(3)}</strong><small>Original baseline · {percent(baselineCandidate.fitness.strategy_similarity ?? 1)} strategy retained</small></span></button>}
            {orderedCandidates.length === 0 ? <div className="empty-state compact">The frontier will appear after generation one.</div> : orderedCandidates.map((candidate) => (
              <button key={candidate.rank} className={selected?.rank === candidate.rank ? "candidate selected" : "candidate"} onClick={() => setSelectedRank(candidate.rank)}>
                <span className="rank">{String(candidate.rank).padStart(2, "0")}</span>
                <span><strong>{(validationScores.get(candidate.rank) ?? candidate.fitness.score).toFixed(3)}</strong><small>{candidate.candidate_status === "provisional" && provisionalResultReason ? `Provisional—${provisionalResultReason} · ` : candidate.candidate_status && candidate.candidate_status !== "provisional" ? `${candidate.candidate_status} · ` : ""}{validationScores.has(candidate.rank) ? "game evidence" : `Pareto P${candidate.pareto_rank + 1} · ${percent(candidate.fitness.consistency)} consistent`}</small></span>
                <i style={{ width: `${Math.max(8, (validationScores.get(candidate.rank) ?? candidate.fitness.score) * 100)}%` }} />
              </button>
            ))}
            {mapElites.length > 0 && <details className="map-elites"><summary>{mapElites.length} preserved strategy niches</summary><div>{mapElites.map((cell) => <article key={`${cell.descriptor.archetype}-${cell.descriptor.colors}-${cell.descriptor.curve}-${cell.descriptor.interaction}-${cell.descriptor.victory_path}`}><b>{cell.descriptor.archetype} · {cell.descriptor.colors}</b><span>{cell.descriptor.curve} curve · {cell.descriptor.interaction} interaction · {cell.descriptor.victory_path}</span><small>Screen {cell.candidate.fitness.score.toFixed(3)} · {cell.candidate.deck.mainboard.reduce((sum, entry) => sum + entry.quantity, 0)} cards</small></article>)}</div></details>}
          </div>
          <div className="panel detail-panel">{selected ? <CandidateInspector candidate={selected} compareTo={selected.rank === 0 ? orderedCandidates[0] : baselineCandidate || orderedCandidates.find((candidate) => candidate.rank !== selected?.rank)} cards={cards} conformance={candidateConformanceByName} platform={platform} format={format} lockedCardIds={(workspace?.constraints.locked_cards || []).map((entry) => entry.card_id)} provisionalReason={provisionalResultReason} onContinue={selected.rank !== 0 && !running && activeRunId && status?.state !== "failed" ? () => void continueFromCandidate() : undefined} additionalGenerations={workspace?.config.generations} /> : <div className="empty-state">Run evolution to inspect a candidate.</div>}</div>
        </section>

        <details className="panel combo-lab" open={Boolean(comboReport?.lines.length)}>
          <summary className="section-summary"><span><b>Combo intelligence</b><small>Corpus hypotheses are never treated as proof; the bounded action solver must account for another legal iteration.</small></span><i>{comboReport ? `${comboReport.lines.length} lines` : comboCorpus?.hypotheses ? `${number(comboCorpus.hypotheses)} hypotheses` : "Not synced"}</i></summary>
          <div className="section-content">
            <div className="panel-title"><div><span className="step">CMB</span><h2>Combo line inspector</h2></div><div><button className="secondary" disabled={comboBusy || !workspace} onClick={() => void synchronizeComboCorpus()}>{comboBusy ? "Working…" : "Refresh corpus"}</button><button className="primary" disabled={comboBusy || !selected} onClick={() => void inspectSelectedCombos()}>{comboBusy ? "Proving…" : `Scan candidate #${selected?.rank || "—"}`}</button></div></div>
            <p>{comboCorpus?.hypotheses ? `${number(comboCorpus.fully_resolved)} of ${number(comboCorpus.hypotheses)} current corpus hypotheses resolve to this logical-card catalog; ${number(comboCorpus.conformance_observations || 0)} immutable deterministic engine observations are retained.` : "The first scan downloads the current open Commander Spellbook corpus automatically."} Search is capped at five pieces, 24 actions, 100,000 states, and two seconds by default.</p>
            {!comboReport ? <div className="empty-state compact">Select a candidate and scan it to inspect resources, zones, proof level, setup, and disruption points.</div> : comboReport.lines.length === 0 ? <div className="empty-state compact">No corpus-matched or state-proven combo line was found in this deck.</div> : <div className="combo-lines">{comboReport.lines.map((line) => <article key={line.id} className={`combo-line ${line.proof}`}><header><div><b>{line.cards.join(" + ")}</b><small>{line.corpus_id ? `Commander Spellbook ${line.corpus_id} · ` : "Discovered · "}{line.finite_win ? "finite win" : line.repeatable ? "repeatable loop" : "hypothesis"}</small></div><span>{line.proof.replaceAll("_", " ")}</span></header><div className="combo-resources"><span>Turn <b>{line.expected_turn?.toFixed(1) || "?"}</b></span>{Object.entries(line.minimum_resources).filter(([, value]) => value > 0).map(([resource, value]) => <span key={`cost-${resource}`}>{resource} <b>{value}</b></span>)}{Object.entries(line.loop_output).filter(([, value]) => value > 0).map(([resource, value]) => <span className="output" key={`output-${resource}`}>+{value} {resource}</span>)}</div>{line.actions.length > 0 && <div className="combo-steps">{line.actions.map((action, index) => <div key={action.id}><i>{index + 1}</i><b>{action.card_name}</b><span>{action.label || action.kind.replaceAll("_", " ")}</span><small>{action.zone.toLowerCase()} · confidence {percent(action.parser_confidence)}{action.once_per_turn ? " · once/turn" : ""}{action.consumes_self ? " · consumes self" : ""}</small></div>)}</div>}<div className="combo-notes">{line.exact_state && <p><b>Exact state</b> {line.exact_state.applied_actions}/{line.actions.length} actions · sources {line.exact_state.source_state_restored ? "restored" : "not restored"} · next iteration {line.exact_state.another_iteration_legal ? "legal" : "not proven"}</p>}{line.setup.length > 0 && <p><b>Setup</b> {line.setup.join(" · ")}</p>}{line.disruption_points.length > 0 && <p><b>Interaction points</b> {line.disruption_points.join(" · ")}</p>}{line.unsupported_assumptions.length > 0 && <p className="warn"><b>Not proven</b> {line.unsupported_assumptions.join(" · ")}</p>}</div></article>)}</div>}
          </div>
        </details>

        <details className="panel experiment-history">
          <summary className="section-summary"><span><b>Run history</b><small>Search, compare, label, and reopen saved experiments.</small></span><i>{savedRuns.length} saved</i></summary>
          <div className="section-content">
          <div className="panel-title"><div><span className="step">LOG</span><h2>Experiment history and comparison</h2></div><span className="legal">Schema-1 manifests</span></div>
          <div className="experiment-tools"><input value={runSearch} onChange={(event) => setRunSearch(event.target.value)} placeholder="Search title, notes, tags, platform, format, or state" /><small>Select up to two runs to compare. Titles, notes, and comma-separated tags save locally when their field loses focus.</small></div>
          {comparedRuns.length > 0 && <div className="run-comparison">{comparedRuns.map((run) => <div key={run.id}><b>{run.title || `${run.platform} ${run.format}`}</b><span>{run.best_score == null ? "No score" : run.best_score.toFixed(3)} best · {number(run.evaluated_candidates)} slots · {(run.elapsed_ms / 1000).toFixed(1)}s</span></div>)}{comparedRuns.length === 2 && <strong>Δ score {comparedRuns.every((run) => run.best_score != null) ? Math.abs((comparedRuns[0].best_score || 0) - (comparedRuns[1].best_score || 0)).toFixed(3) : "—"} · Δ evaluations {number(Math.abs(comparedRuns[0].evaluated_candidates - comparedRuns[1].evaluated_candidates))}</strong>}</div>}
          <div className="experiment-list">{filteredRuns.length ? filteredRuns.map((run) => <article key={run.id}><label className="compare-run"><input type="checkbox" checked={comparedRunIds.includes(run.id)} onChange={(event) => setComparedRunIds((ids) => event.target.checked ? [...ids.filter((id) => id !== run.id), run.id].slice(-2) : ids.filter((id) => id !== run.id))} /> Compare</label><input value={run.title} onChange={(event) => setSavedRuns((rows) => rows.map((row) => row.id === run.id ? { ...row, title: event.target.value } : row))} onBlur={() => void saveRunMetadata(run)} placeholder={`${run.platform} ${run.format} experiment`} /><textarea value={run.notes} onChange={(event) => setSavedRuns((rows) => rows.map((row) => row.id === run.id ? { ...row, notes: event.target.value } : row))} onBlur={() => void saveRunMetadata(run)} placeholder="Experiment notes" /><input value={run.tags.join(", ")} onChange={(event) => setSavedRuns((rows) => rows.map((row) => row.id === run.id ? { ...row, tags: event.target.value.split(",") } : row))} onBlur={() => void saveRunMetadata(run)} placeholder="tags, comma, separated" /><span>{run.platform} · {run.format} · {run.state} · {run.best_score == null ? "—" : run.best_score.toFixed(3)} · {number(run.evaluated_candidates)} slots</span><div><button className="secondary" onClick={() => void loadSavedRun(run.id)}>Open</button><button className="secondary" onClick={() => void copyRunManifest(run.id)}>Copy manifest</button></div></article>) : <div className="empty-state compact">No saved experiments match this search.</div>}</div>
          </div>
        </details>

        <details className="panel hall-of-fame">
          <summary className="section-summary"><span><b>Hall of Fame</b><small>Diverse validated champions retained as future seeds and opponents.</small></span><i>{hallOfFame?.entries.length || 0} champions</i></summary>
          <div className="section-content">
          <div className="panel-title"><div><span className="step">HOF</span><h2>Diverse Hall of Fame</h2></div><button className="secondary" disabled={!selected || hallBusy} onClick={() => void promoteHallOfFame()}>{hallBusy ? "Updating…" : "Archive selected candidate"}</button></div>
          <p>Hybrid cycle winners are archived automatically. The active set greedily balances validated strength with deck distance, so near-identical champions cannot fill every historical slot.</p>
          <div className="hall-layout">
            <div className="hall-list">{hallOfFame?.entries.length ? hallOfFame.entries.map((entry) => <div className={selectedHall?.id === entry.id ? "selected" : ""} key={entry.id}><button onClick={() => setSelectedHallId(entry.id)}><b>{entry.candidate.deck.name}</b><span>{entry.evidence_score.toFixed(3)} evidence · {percent(entry.novelty)} nearest-deck distance</span><small>{hallOfFame.active_diverse_ids.includes(entry.id) ? "Active diverse champion" : "Archived champion"}{entry.source_cycle == null ? "" : ` · cycle ${entry.source_cycle}`} · seen {entry.promotion_count}×</small></button><button className="delete" disabled={hallBusy} onClick={() => void deleteHallOfFame(entry.id)}>Delete</button></div>) : <div className="empty-state compact">Run a hybrid cycle or archive a candidate to establish historical champions.</div>}</div>
            <div className="hall-preview">{selectedHall ? <><div className="hall-actions"><button className="secondary" onClick={() => { if (!workspace) return; setWorkspace({ ...workspace, seed_deck: selectedHall.candidate.deck }); setNotice(`${selectedHall.candidate.deck.name} loaded as the next evolutionary seed`); }}>Use as seed</button><span>{hallOfFame?.entries.length || 0}/{hallOfFame?.maximum_entries_for_scope || 64} retained · {hallOfFame?.active_diverse_ids.length || 0} active historical tests</span></div><CandidateInspector candidate={selectedHall.candidate} cards={cards} conformance={candidateConformanceByName} platform={platform} format={format} /></> : <div className="empty-state compact">Select an archived champion to inspect its full deck.</div>}</div>
          </div>
          </div>
        </details>

        <details className="panel validation-lab">
          <summary className="validation-summary"><span><b>Competitive validation</b><small>Add a representative field, rank finalists with game evidence, and run protected evolve → validate → reseed cycles.</small></span><i>{gauntletDecks.length ? `${gauntletDecks.length} opponent${gauntletDecks.length === 1 ? "" : "s"} loaded` : "Optional"}</i></summary>
          <div className="validation-content">
          <div className="panel-title"><div><span className="step">04</span><h2>{requiresCommander ? "Commander gauntlet" : "Constructed gauntlet"}</h2></div><span className={hexdekReady ? "legal" : "idle"}>{hexdekReady ? "HexDek ready" : "Engine required"}</span></div>
          {hexdekReady && <div className={`coverage-audit ${coverageBlocked ? "blocked" : coveragePartial ? "partial" : coverageSemanticRisk ? "review" : "ready"}`}>
            <div><span className="eyebrow">Rules corpus preflight</span><strong>{coverageLoading ? "Auditing cards…" : coverageError ? "Audit unavailable" : coverageBlocked ? "Missing card definitions" : coveragePartial ? "Partial parser coverage" : coverageSemanticRisk ? "Semantic review recommended" : coverageReport ? "Parsed · no flagged mechanics" : "Load a candidate"}</strong><small>{coverageReport ? `${number(coverageReport.corpus_entries)} corpus entries · parsing is not rules conformance` : coverageError || "HexDek execution support is checked before simulation"}</small></div>
            {coverageReport?.decks.map((deck, index) => <div className="coverage-deck" key={`${deck.deck_name}-${index}`}><span>{index === 0 ? `Candidate · ${deck.deck_name}` : deck.deck_name}</span><b>{deck.parser_complete_unique}/{deck.unique_cards} parsed</b>{deck.missing_unique > 0 && <em>{deck.missing_unique} missing</em>}{deck.partial_unique > 0 && <em>{deck.partial_unique} partial</em>}{deck.semantic_risk_unique > 0 && <em>{deck.semantic_risk_unique} need parity evidence</em>}{deck.issues.length > 0 && <details><summary>Inspect</summary>{deck.issues.slice(0, 20).map((issue) => <p key={`${issue.status}-${issue.name}`}><i>{issue.status.replace("_", " ")}</i>{issue.quantity}× {issue.name}<small>{issue.parse_errors[0] || `${issue.zone} card is absent from the corpus`}</small></p>)}</details>}</div>)}
          </div>}
          <div className={`conformance-audit ${conformanceSummary && (conformanceSummary.conflicting_scenarios || conformanceSummary.failed_scenarios) ? "conflict" : conformanceSummary?.cross_engine_verified_scenarios ? "verified" : "empty"}`}>
            <div className="conformance-head">
              <div><span className="eyebrow">Independent rules evidence</span><strong>{conformanceLoading ? "Checking scenarios…" : conformanceError ? "Evidence unavailable" : conformanceSummary?.conflicting_scenarios ? "Cross-engine conflict needs review" : conformanceSummary?.failed_scenarios ? "Failed scenarios need review" : conformanceSummary?.cross_engine_verified_scenarios ? "Cross-engine evidence available" : "No cross-engine claims yet"}</strong><small>{conformanceSummary ? `${number(conformanceSummary.observations)} observations · ${number(conformanceSummary.scenario_runs)} versioned scenario runs · ${bytes(conformanceSummary.database_bytes)}` : conformanceError || "Import canonical scenario output from engine adapters"}</small></div>
              <div className="conformance-metrics"><span><b>{number(conformanceSummary?.interaction_verified_scenarios || 0)}</b> engine-verified</span><span><b>{number(conformanceSummary?.cross_engine_verified_scenarios || 0)}</b> cross-engine</span><span className={conformanceSummary && (conformanceSummary.failed_scenarios || conformanceSummary.conflicting_scenarios) ? "warn" : ""}><b>{number((conformanceSummary?.failed_scenarios || 0) + (conformanceSummary?.conflicting_scenarios || 0))}</b> failed/conflicting</span></div>
            </div>
            {conformanceReport?.decks.map((deck, index) => <div className="conformance-deck" key={`${deck.deck_name}-${index}`}>
              <span>{index === 0 ? `Candidate · ${deck.deck_name}` : deck.deck_name}</span><b>{deck.verified_unique}/{deck.unique_cards} verified</b>
              <small>{deck.partially_covered_unique} partially covered · {deck.unverified_unique} unverified</small>{deck.disputed_unique + deck.unsupported_unique > 0 && <em>{deck.disputed_unique} disputed · {deck.unsupported_unique} unsupported</em>}
              {deck.cards.some((card) => card.classification !== "unverified") && <details><summary>Inspect evidence</summary>{deck.cards.filter((card) => card.classification !== "unverified").slice(0, 30).map((card) => <p key={card.name}><i className={card.classification}>{classificationLabel(card.classification)}</i>{card.name}<small>{card.scenarios.join(" · ")}</small></p>)}</details>}
            </div>)}
            {Boolean(conformanceReport?.scenarios.length) && <details className="conformance-scenarios"><summary>Inspect mechanic scenarios</summary>{conformanceReport?.scenarios.map((scenario) => <p key={`${scenario.scenario_id}-${scenario.scenario_version}-${scenario.seed}`}><ConformanceBadge evidence={{ name: scenario.scenario_id, status: "unverified", classification: scenario.classification, scenarios: [], claims: [] }} /><b>{scenario.scenario_id}@{scenario.scenario_version}</b><span>seed {scenario.seed} · {scenario.engines.join(" + ")}</span><small>{scenario.cards.join(" · ")}</small></p>)}</details>}
            <details className="conformance-import"><summary>Import conformance bundle</summary><p>Schema 2 bundles name the exact clause/action claims exercised by assertions and may separately mark AI-decision coverage. Schema 1 observations remain readable as legacy card-level evidence but cannot verify a clause. Cross-engine labels require matching configuration, normalized outcomes, and event hashes.</p><textarea value={conformanceBundle} onChange={(event) => setConformanceBundle(event.target.value)} placeholder={'{\n  "schema_version": 2,\n  "producer": "forge adapter",\n  "observations": […]\n}'} /><button className="secondary" disabled={!conformanceBundle.trim() || importingConformance} onClick={() => void importConformanceEvidence()}>{importingConformance ? "Importing…" : "Validate and import evidence"}</button></details>
          </div>
          <div className="validation-grid">
            <div className="gauntlet-builder">
              <span className="eyebrow">Opponent {requiresCommander ? "pod" : "matchups"}</span>
              <p>{requiresCommander
                ? "Paste each complete legal Commander list needed to fill the selected pod."
                : `Paste complete legal ${format} lists. Every loaded opponent is played as a separate head-to-head matchup.`}</p>
              <details className="metagame-acquisition" open={Boolean(acquisitionReport || competitiveField || engineRoutePlan)}>
                <summary>Acquire current tournament field</summary>
                <p>Documented public deck and tournament APIs are resolved against this exact {platform} {format} catalog. Nothing illegal or unresolved enters the field. Source attribution remains attached to every saved field.</p>
                <div className="validation-controls">
                  <label>Source<select value={acquisitionSource} onChange={(event) => { setAcquisitionSource(event.target.value as "getpaird" | "spicerack" | "topdeck"); setAcquisitionReport(undefined); }}><option value="getpaird">GetPaird public decks</option><option value="spicerack">Spicerack public DB</option><option value="topdeck">TopDeck.gg API v2</option></select></label>
                  <label>Observation days<input type="number" min="1" max="365" value={acquisitionDays} onChange={(event) => setAcquisitionDays(Math.max(1, Math.min(365, Number(event.target.value))))} /></label>
                  <label>{acquisitionSource === "topdeck" ? "TopDeck API key" : "Optional API key"}<input type="password" autoComplete="off" value={acquisitionKey} onChange={(event) => setAcquisitionKey(event.target.value)} placeholder="Used for this request only" /></label>
                </div>
                <small>{acquisitionSource === "getpaird" ? <>Data provided by <a href="https://getpaird.io" target="_blank" rel="noreferrer">GetPaird</a>.</> : acquisitionSource === "topdeck" ? <>Data provided by <a href="https://topdeck.gg" target="_blank" rel="noreferrer">TopDeck.gg</a>.</> : <>Data provided by <a href="https://spicerack.gg" target="_blank" rel="noreferrer">Spicerack</a>.</>}</small>
                <button className="secondary" disabled={acquisitionBusy} onClick={() => void acquireCurrentMetagame()}>{acquisitionBusy ? "Acquiring + validating…" : acquisitionReport ? "Refresh field" : "Acquire field"}</button>
                {acquisitionReport && <div className="acquisition-report">
                  <small>{acquisitionReport.provenance.event_count} events · {acquisitionReport.provenance.accepted_deck_count}/{acquisitionReport.provenance.submitted_deck_count} accepted lists · {acquisitionReport.accepted.length} exact-list archetypes · {acquisitionReport.quarantined.length} quarantined</small>
                  <details><summary>Provenance and transformations</summary><a href={acquisitionReport.provenance.source_url} target="_blank" rel="noreferrer">Exact source query</a>{acquisitionReport.provenance.transformation_notes.map((note) => <p key={note}>{note}</p>)}</details>
                  <details><summary>Acquired archetypes</summary>{acquisitionReport.accepted.map((entry) => <p key={entry.fingerprint}><b>{entry.archetype}</b><span>{entry.occurrences} list{entry.occurrences === 1 ? "" : "s"} · {percent(entry.weight)}</span><small>{entry.provenance.map((item) => item.event_name).join(" · ")}</small></p>)}</details>
                  {acquisitionReport.quarantined.length > 0 && <details><summary>Quarantine ({acquisitionReport.quarantined.length})</summary>{acquisitionReport.quarantined.map((item, index) => <p key={`${item.event_id}-${index}`}><b>{item.reason.replaceAll("_", " ")}</b><span>{item.event_name}</span><small>{item.detail}</small></p>)}</details>}
                </div>}
                {competitiveField && <details className="field-audit" open>
                  <summary>Competitive field audit · {competitiveField.audit.source_tier.replaceAll("_", " ")}</summary>
                  <div className="fitness-strip">
                    <Metric label="Observed weight" value={percent(competitiveField.audit.observed_weight_covered)} />
                    <Metric label="Representatives" value={`${competitiveField.audit.selected_archetypes}`} />
                    <Metric label="Exact duplicates" value={`${competitiveField.audit.exact_duplicates_merged}`} />
                    <Metric label="Near duplicates" value={`${competitiveField.audit.near_duplicates_clustered}`} />
                    <Metric label="Claim" value={competitiveField.audit.competitive_claims_allowed ? "Tournament eligible" : "Provisional only"} tone={competitiveField.audit.competitive_claims_allowed ? "accent" : "warn"} />
                  </div>
                  <div className="package-strip"><span>Selected strategy clusters</span>{competitiveField.clusters.map((cluster) => <b key={cluster.id}>{cluster.label}<small>{cluster.strategy} · {percent(cluster.observed_weight)} observed · {cluster.member_fingerprints.length} exact list{cluster.member_fingerprints.length === 1 ? "" : "s"}</small></b>)}</div>
                  {competitiveField.audit.warnings.length > 0 && <div className="warning-box">{competitiveField.audit.warnings.map((warning) => <span key={warning}>{warning}</span>)}</div>}
                  {competitiveField.audit.quarantined.length > 0 && <details><summary>Quarantined source lists ({competitiveField.audit.quarantined.length})</summary>{competitiveField.audit.quarantined.map((item, index) => <p key={`${item.event_id}-${index}`}><b>{item.reason.replaceAll("_", " ")}</b><span>{item.event_name}</span><small>{item.detail}</small></p>)}</details>}
                </details>}
                {engineRoutePlan && <details className="field-audit">
                  <summary>Locked engine route · {engineRoutePlan.claim.label}</summary>
                  <div className="fitness-strip">
                    <Metric label="Oracle accounted" value={percent(engineRoutePlan.candidate.oracle_accounted_rate ?? 0)} tone={(engineRoutePlan.candidate.oracle_accounted_rate ?? 0) < 1 ? "warn" : "accent"} />
                    <Metric label="Structurally parsed" value={percent(engineRoutePlan.candidate.structurally_parsed_rate ?? 0)} tone={(engineRoutePlan.candidate.structurally_parsed_rate ?? 0) < 1 ? "warn" : "accent"} />
                    <Metric label="Engine executable" value={percent(engineRoutePlan.candidate.engine_executable_rate ?? 0)} tone={(engineRoutePlan.candidate.engine_executable_rate ?? 0) < 1 ? "warn" : "accent"} />
                    <Metric label="Scenario verified" value={percent(engineRoutePlan.candidate.scenario_verified_rate ?? 0)} tone={(engineRoutePlan.candidate.scenario_verified_rate ?? 0) < 1 ? "warn" : "accent"} />
                    <Metric label="AI decision-covered" value={percent(engineRoutePlan.candidate.ai_decision_coverage ?? 0)} tone={(engineRoutePlan.candidate.ai_decision_coverage ?? 0) < 1 ? "warn" : "accent"} />
                    <Metric label="Field routed" value={percent(engineRoutePlan.field_weighted_coverage)} tone={engineRoutePlan.field_weighted_coverage < 0.95 ? "warn" : "accent"} />
                    <Metric label="Evidence" value={engineRoutePlan.claim.tier.replaceAll("_", " ")} />
                  </div>
                  {(engineRoutePlan.candidate.unresolved_clauses ?? []).length > 0 && <details className="candidate-diff"><summary>Unresolved Oracle text · blocks Improved</summary>{(engineRoutePlan.candidate.unresolved_clauses ?? []).map((text, index) => <span key={`${text}-${index}`}><b>Unresolved gameplay clause</b>{text}</span>)}</details>}
                  {engineRoutePlan.routes.map((route) => <p key={`${route.opponent_index}-${route.opponent_name}`}><b>{route.opponent_name}</b><span>{route.engine || "blocked"} · {percent(route.weight)}</span><small>{route.reasons.join(" · ") || "Candidate and opponent critical actions are covered by the same engine."}</small></p>)}
                  {engineRoutePlan.claim.reasons.length > 0 && <div className="warning-box">{engineRoutePlan.claim.reasons.map((reason) => <span key={reason}>{reason}</span>)}</div>}
                </details>}
              </details>
              <textarea value={gauntletInput} onChange={(event) => setGauntletInput(event.target.value)} placeholder={requiresCommander ? "Commander\n1 Opposing Commander\n\nDeck\n1 Sol Ring\n…" : "Deck\n4 Opposing Card\n…"} />
              <button className="secondary" disabled={!gauntletInput.trim() || gauntletDecks.length >= maximumGauntletDecks} onClick={() => void addGauntletDeck()}>Validate and add opponent</button>
              <div className="opponent-list">
                {gauntletDecks.length === 0 ? <small>No opponents loaded</small> : gauntletDecks.map((deck, index) => {
                  const commander = deck.command_zone[0] ? cards.get(deck.command_zone[0].card_id)?.name : deck.name;
                  const weight = gauntletWeights[index] || 1;
                  return <div key={`${commander}-${index}`}><span><b>{requiresCommander ? index + 2 : index + 1}</b><input aria-label={`Opponent ${index + 1} archetype`} value={deck.name || commander || `Opponent ${index + 1}`} maxLength={80} onChange={(event) => { const name = event.target.value; setGauntletDecks((decks) => decks.map((current, deckIndex) => deckIndex === index ? { ...current, name } : current)); setGauntletProvenance(undefined); }} /></span>{!requiresCommander && <label>Share {percent(weight / gauntletWeightTotal)}<input type="number" min="0.001" step="0.01" value={weight} onChange={(event) => { setGauntletWeights((weights) => weights.map((current, weightIndex) => weightIndex === index ? Math.max(0.001, Number(event.target.value)) : current)); setGauntletProvenance(undefined); }} /></label>}<button onClick={() => { setGauntletDecks((decks) => decks.filter((_, deckIndex) => deckIndex !== index)); setGauntletWeights((weights) => weights.filter((_, weightIndex) => weightIndex !== index)); setGauntletProvenance(undefined); }}>Remove</button></div>;
                })}
              </div>
              <div className="gauntlet-library">
                <div className="gauntlet-save"><input value={gauntletName} maxLength={80} onChange={(event) => { setGauntletName(event.target.value); setGauntletProvenance(undefined); }} placeholder={requiresCommander ? "Pod name" : "Metagame field name"} /><button className="secondary" disabled={gauntletLibraryBusy || !gauntletName.trim() || !gauntletDecks.length} onClick={() => void saveCurrentGauntlet()}>Save revision</button><button disabled={!gauntletDecks.length} onClick={() => { setGauntletDecks([]); setGauntletWeights([]); setGauntletName(""); setGauntletMetadata({ source: "", observed_at: "", notes: "" }); setGauntletProvenance(undefined); }}>Clear</button></div>
                <small>Saved locally for {platform} {format}. Loading always rechecks current legality and card IDs.</small>
                <details className="metagame-bundle"><summary>Portable metagame bundle</summary><div className="metagame-metadata"><input value={gauntletMetadata.source} maxLength={500} onChange={(event) => { setGauntletMetadata((metadata) => ({ ...metadata, source: event.target.value })); setGauntletProvenance(undefined); }} placeholder="Source URL or event" /><input value={gauntletMetadata.observed_at} maxLength={80} onChange={(event) => { setGauntletMetadata((metadata) => ({ ...metadata, observed_at: event.target.value })); setGauntletProvenance(undefined); }} placeholder="Observed date / window" /><input value={gauntletMetadata.notes} maxLength={2000} onChange={(event) => { setGauntletMetadata((metadata) => ({ ...metadata, notes: event.target.value })); setGauntletProvenance(undefined); }} placeholder="Notes" /></div><p>Schema 1 stores source metadata, normalized shares, and portable MTGA text. Imports resolve every card against the current catalog and recheck legality.</p><textarea value={metagameBundle} onChange={(event) => setMetagameBundle(event.target.value)} placeholder={'{\n  "schema_version": 1,\n  "name": "Historic field",\n  "opponents": […]\n}'} /><div><button className="secondary" disabled={gauntletLibraryBusy || !gauntletName.trim() || !gauntletDecks.length} onClick={() => void exportMetagameBundle()}>Export + copy</button><button className="secondary" disabled={gauntletLibraryBusy || !metagameBundle.trim()} onClick={() => void importMetagameBundle()}>Validate + import</button></div></details>
                <div className="gauntlet-history">{savedGauntlets.length === 0 ? <span>No saved fields for this format</span> : savedGauntlets.map((saved, index) => {
                  const previous = savedGauntlets.slice(index + 1).find((candidate) => candidate.name.toLowerCase() === saved.name.toLowerCase());
                  return <div key={saved.id}><button disabled={gauntletLibraryBusy} onClick={() => void loadSavedGauntlet(saved.id)}><b>{saved.name}</b><small>v{saved.revision} · {saved.deck_count} deck{saved.deck_count === 1 ? "" : "s"}</small></button>{previous && <button disabled={gauntletLibraryBusy} onClick={() => void compareGauntletRevisions(saved, previous)}>Diff v{previous.revision}</button>}<button className="delete" disabled={gauntletLibraryBusy} onClick={() => void deleteSavedGauntlet(saved.id)}>Delete</button></div>;
                })}</div>
                {gauntletComparison && <div className="acquisition-report"><small>{gauntletComparison.from.name} v{gauntletComparison.from.revision} → v{gauntletComparison.to.revision}: {gauntletComparison.added.length} added · {gauntletComparison.removed.length} removed · {gauntletComparison.unchanged} retained · {gauntletComparison.weight_changes.length} share changes</small>{Boolean(gauntletComparison.added.length) && <p><b>Added</b><span>{gauntletComparison.added.join(" · ")}</span></p>}{Boolean(gauntletComparison.removed.length) && <p><b>Removed</b><span>{gauntletComparison.removed.join(" · ")}</span></p>}{Boolean(gauntletComparison.weight_changes.length) && <details><summary>Weight changes</summary>{gauntletComparison.weight_changes.map((change) => <p key={change.name}><b>{change.name}</b><span>{percent(change.from_weight)} → {percent(change.to_weight)}</span></p>)}</details>}</div>}
              </div>
              <div className="validation-controls">
                <label>{requiresCommander ? "Complete seat rotations" : "Games / opponent"}<input type="number" min="16" max="10000000" value={validationGames} onChange={(event) => setValidationGames(Math.max(16, Number(event.target.value)))} /></label>
                {requiresCommander && <label>Pod seats<input type="number" min="2" max="6" value={validationSeats} onChange={(event) => setValidationSeats(Math.max(2, Math.min(6, Number(event.target.value))))} /></label>}
                <label>AI budget<input type="number" min="0" max="10000" value={hatBudget} onChange={(event) => setHatBudget(Math.max(0, Number(event.target.value)))} /></label>
              </div>
              {requiresCommander && <small className="gauntlet-summary">{number(validationGames * activeValidationSeats)} total pod games · every deck occupies every one of {activeValidationSeats} seats {number(validationGames)} times under a deterministic seed</small>}
              {!requiresCommander && gauntletDecks.length > 0 && <small className="gauntlet-summary">{number(validationGames * gauntletDecks.length)} total games across {gauntletDecks.length} opponent{gauntletDecks.length === 1 ? "" : "s"}</small>}
              <button className="primary" disabled={!selected || selected.fitness.severe_mana_failure || selected.fitness.severe_structural_failure || !hexdekReady || validating || forgeRunning || halvingRunning || hybridRunning || running || coverageLoading || coverageBlocked || gauntletDecks.length < requiredOpponentCount} onClick={() => void validateCandidate()}>{selected?.fitness.severe_mana_failure ? "Validation blocked · repair mana" : selected?.fitness.severe_structural_failure ? "Validation blocked · Tune boundary" : validating ? `Simulating ${requiresCommander ? "pod" : "gauntlet"}…` : `Validate candidate #${selected?.rank || "—"}`}</button>
              {!requiresCommander && <div className="sideboard-lab">
                <span className="eyebrow">Real Best-of-Three sideboarding</span>
                <p>Evolve a separate legal on-play and on-draw plan for every weighted opponent using actual HexDek games, then rank the candidate in explicit game-one/game-two/game-three matches.</p>
                <div className="validation-controls sideboard-controls">
                  <label>Policies / generation<input type="number" min="2" max="128" value={sideboardPopulation} onChange={(event) => setSideboardPopulation(Math.max(2, Math.min(128, Number(event.target.value))))} /></label>
                  <label>Generations<input type="number" min="1" max="12" value={sideboardGenerations} onChange={(event) => setSideboardGenerations(Math.max(1, Math.min(12, Number(event.target.value))))} /></label>
                  <label>Games / policy<input type="number" min="1" max="10000" value={sideboardGames} onChange={(event) => setSideboardGames(Math.max(1, Math.min(10000, Number(event.target.value))))} /></label>
                  <label>Maximum cards swapped<input type="number" min="0" max={workspace?.constraints.target_sideboard_size || 15} value={sideboardMaximumSwaps} onChange={(event) => setSideboardMaximumSwaps(Math.max(0, Math.min(workspace?.constraints.target_sideboard_size || 15, Number(event.target.value))))} /></label>
                  <label>Parallel engine jobs<input type="number" min="1" max="16" value={sideboardParallelJobs} onChange={(event) => setSideboardParallelJobs(Math.max(1, Math.min(16, Number(event.target.value))))} /></label>
                </div>
                <div className="sideboard-actions">
                  <button className="secondary" disabled={!selected?.deck.sideboard.length || !hexdekReady || sideboardEvolving || bestOfThreeRunning || validating || forgeRunning || halvingRunning || hybridRunning || running || coverageLoading || gauntletCoverageBlocked || !gauntletDecks.length} onClick={() => void evolveCandidateSideboard()}>{sideboardEvolving ? "Evolving sideboard policy…" : `Evolve candidate #${selected?.rank || "—"} sideboard`}</button>
                  {sideboardEvolving && <button className="cancel" onClick={() => void cancelSideboardEvolution()}>Stop policy search</button>}
                </div>
                {sideboardEvolutionReport && <div className="sideboard-policy-report">
                  <div className="fitness-strip validation-metrics">
                    <Metric label="Policies" value={number(sideboardEvolutionReport.evaluated_policies)} />
                    <Metric label="Actual games" value={number(sideboardEvolutionReport.attempted_games)} />
                    <Metric label="Cached" value={number(sideboardEvolutionReport.cached_games)} tone={sideboardEvolutionReport.cached_games ? "accent" : undefined} />
                    <Metric label="Games / sec" value={number(sideboardEvolutionReport.games_per_second)} tone="accent" />
                  </div>
                  <details className="conformance-import"><summary>Policy reproducibility manifest</summary><p>Engine <code>{sideboardEvolutionReport.provenance.engine_revision}</code> · cards <code>{sideboardEvolutionReport.provenance.card_data_revision}</code> · configuration <code>{sideboardEvolutionReport.provenance.configuration_hash}</code></p></details>
                  <div className="sideboard-plans">{sideboardEvolutionReport.matchups.map((matchup) => <div key={matchup.opponent_key}>
                    <b>{matchup.opponent_name}</b><small>{percent(matchup.weight)} field share · {number(matchup.games_per_policy)} games/policy</small>
                    <span><i>On play · {percent(matchup.on_play.best_evidence.win_rate)} (L {percent(matchup.on_play.best_evidence.win_rate_lower_bound)})</i>{exchangeLabel(matchup.on_play.best_exchange)}</span>
                    <span><i>On draw · {percent(matchup.on_draw.best_evidence.win_rate)} (L {percent(matchup.on_draw.best_evidence.win_rate_lower_bound)})</i>{exchangeLabel(matchup.on_draw.best_exchange)}</span>
                    <details><summary>Inspect alternatives</summary>{[...matchup.on_play.alternatives.slice(0, 4), ...matchup.on_draw.alternatives.slice(0, 4)].map((alternative, index) => <p key={`${alternative.configuration_hash}-${index}`}><b>{index < Math.min(4, matchup.on_play.alternatives.length) ? "Play" : "Draw"} · {percent(alternative.evidence.win_rate)}</b><span>{exchangeLabel(alternative.exchange)}</span></p>)}</details>
                  </div>)}</div>
                  <div className="best-of-three-action"><label>Match budget<input type="number" min={Math.max(1, gauntletDecks.length)} max="1000000" value={bestOfThreeMatches} onChange={(event) => setBestOfThreeMatches(Math.max(gauntletDecks.length, Math.min(1000000, Number(event.target.value))))} /></label><button className="primary" disabled={bestOfThreeRunning || sideboardEvolving || validating || forgeRunning || halvingRunning || hybridRunning || running} onClick={() => void runBestOfThree()}>{bestOfThreeRunning ? "Playing Best-of-Three…" : "Rank with post-board matches"}</button></div>
                </div>}
                {bestOfThreeReport && <div className="best-of-three-report">
                  <div className="fitness-strip validation-metrics">
                    <Metric label="Completed matches" value={`${number(bestOfThreeReport.completed_matches)}/${number(bestOfThreeReport.requested_matches)}`} />
                    <Metric label="Pre-board" value={percent(bestOfThreeReport.preboard.win_rate)} />
                    <Metric label="Post-board" value={percent(bestOfThreeReport.postboard.win_rate)} tone="accent" />
                    <Metric label="Match win rate" value={percent(bestOfThreeReport.matches.win_rate)} tone="accent" />
                    <Metric label="Cached games" value={number(bestOfThreeReport.cached_games)} />
                  </div>
                  <details className="conformance-import"><summary>Match reproducibility manifest</summary><p>Engine <code>{bestOfThreeReport.provenance.engine_revision}</code> · cards <code>{bestOfThreeReport.provenance.card_data_revision}</code> · configuration <code>{bestOfThreeReport.provenance.configuration_hash}</code></p></details>
                  <table><thead><tr><th>Opponent</th><th>Share</th><th>Pre-board</th><th>Post play</th><th>Post draw</th><th>Post-board</th><th>Matches</th><th>Complete</th></tr></thead><tbody>{bestOfThreeReport.matchups.map((matchup) => <tr key={matchup.opponent_key}><td>{matchup.opponent_name}</td><td>{percent(matchup.weight)}</td><td>{percent(matchup.preboard.win_rate)}</td><td>{percent(matchup.postboard_on_play.win_rate)}</td><td>{percent(matchup.postboard_on_draw.win_rate)}</td><td>{percent(matchup.postboard.win_rate)}</td><td>{percent(matchup.matches.win_rate)}</td><td>{number(matchup.matches.games - matchup.matches.draws)}/{number(matchup.matches.games)}</td></tr>)}</tbody></table>
                </div>}
              </div>}
              <div className="forge-crosscheck">
                <span><b>Independent Forge check</b><small>Low-volume second-engine evidence. Forge’s heuristic AI is slower and weaker with combo/control, so this is a finalist sanity check rather than the primary search engine.</small></span>
                <label>{requiresCommander ? "Games" : "Games / opponent"}<input type="number" min="1" max="256" value={forgeGames} onChange={(event) => setForgeGames(Math.max(1, Math.min(256, Number(event.target.value))))} /></label>
                <label>Clock / game<input type="number" min="1" max="3600" value={forgeTimeout} onChange={(event) => setForgeTimeout(Math.max(1, Math.min(3600, Number(event.target.value))))} /></label>
                <button className="secondary" disabled={!selected || !forgeReady || forgeRunning || validating || halvingRunning || hybridRunning || running || gauntletDecks.length < requiredOpponentCount} onClick={() => void crossCheckCandidate()}>{forgeRunning ? "Forge is playing…" : forgeReady ? "Cross-check candidate" : "Install Forge to cross-check"}</button>
                {forgeRunning && <button className="cancel" onClick={() => void cancelForge()}>Stop Forge</button>}
              </div>
              <div className="halving-config">
                <span className="eyebrow">Automated finalist ranking</span>
                <p>Start every finalist on the same seeds, {requiresCommander ? "play the configured pod" : "play every loaded opponent head-to-head"}, eliminate candidates whose 95% score ceiling is below the best floor, halve the remaining field, and double the budget for close survivors.</p>
                <div className="validation-controls halving-controls">
                  <label>Finalists<input type="number" min="2" max="64" value={finalistCount} onChange={(event) => setFinalistCount(Math.max(2, Math.min(64, Number(event.target.value))))} /></label>
                  <label>Rounds<input type="number" min="1" max="8" value={halvingRounds} onChange={(event) => setHalvingRounds(Math.max(1, Math.min(8, Number(event.target.value))))} /></label>
                  <label>{requiresCommander ? "Base seat rotations" : "Base games"}<input type="number" min="16" max="10000000" value={halvingBaseGames} onChange={(event) => setHalvingBaseGames(Math.max(16, Number(event.target.value)))} /></label>
                  <label>Timeout / game<input type="number" min="1" max="3600" value={halvingTimeout} onChange={(event) => setHalvingTimeout(Math.max(1, Math.min(3600, Number(event.target.value))))} /></label>
                </div>
                <div className="evidence-controls">
                  <label><span>Confidence caution <b>{percent(riskAversion)}</b></span><input type="range" min="0" max="100" value={riskAversion * 100} onChange={(event) => setRiskAversion(Number(event.target.value) / 100)} /></label>
                  <label><span>Worst-matchup weight <b>{percent(worstMatchupWeight)}</b></span><input type="range" min="0" max="100" value={worstMatchupWeight * 100} onChange={(event) => setWorstMatchupWeight(Number(event.target.value) / 100)} /></label>
                  <label><span>Position-floor weight <b>{percent(seatPositionWeight)}</b></span><input type="range" min="0" max="100" value={seatPositionWeight * 100} onChange={(event) => setSeatPositionWeight(Number(event.target.value) / 100)} /></label>
                </div>
                <label className="adaptive-toggle"><input type="checkbox" checked={adaptiveElimination} onChange={(event) => setAdaptiveElimination(event.target.checked)} /><span><b>Adaptive confidence racing</b><small>Stop statistically dominated decks early; give overlapping contenders larger rounds.</small></span></label>
                <small>{number(estimatedHalvingGames)} {adaptiveElimination ? "maximum" : "estimated"} attempts · 95% Wilson confidence · worst-matchup and position-floor aware · crash-penalized{workspace?.config.evolution_mode === "tune" ? " · game evidence alone promotes Tune finalists" : " · 35% structural prior for manual ranking"}</small>
                <button className="secondary auto-rank" disabled={candidates.length < 2 || !hexdekReady || halvingRunning || hybridRunning || validating || forgeRunning || running || coverageLoading || gauntletCoverageBlocked || gauntletDecks.length < requiredOpponentCount} onClick={() => void rankFinalists()}>{halvingRunning ? `Round ${halvingProgress?.round || 1}/${halvingProgress?.total_rounds || halvingRounds} · candidate ${halvingProgress?.candidate_index || 0}/${halvingProgress?.candidates_in_round || "—"}` : "Auto-rank frontier"}</button>
                <div className="hybrid-loop">
                  <label>Hybrid cycles<input type="number" min="1" max="20" value={hybridCycles} onChange={(event) => setHybridCycles(Math.max(1, Math.min(20, Number(event.target.value))))} /></label>
                  <label>Historical champions<input type="number" min="0" max="16" value={historicalChampionCount} onChange={(event) => setHistoricalChampionCount(Math.max(0, Math.min(16, Number(event.target.value))))} /></label>
                  {!requiresCommander && <label>Historical field share<input type="number" min="0" max="0.75" step="0.05" value={historicalChampionWeight} onChange={(event) => setHistoricalChampionWeight(Math.max(0, Math.min(0.75, Number(event.target.value))))} /></label>}
                  <label>{requiresCommander ? "Holdout pod games" : "Holdout games / opponent"}<input type="number" min="4" max="10000000" value={holdoutGames} onChange={(event) => setHoldoutGames(Math.max(4, Math.min(10000000, Number(event.target.value))))} /></label>
                  <span><b>Evolve → current + historical validation → reseed</b><small>{workspace?.seed_deck.mainboard.length ? "Starts from the imported deck" : requiresCommander && hasCommander ? "Starts from a generated shell around the selected commander" : "Starts from the highest-weight compatible gauntlet deck"} · competitive mutations stay within the seed's overlapping field archetype · {number(estimatedHybridAttempts)} estimated game attempts plus structural screening · {activeHistoricalCount} diverse historical champion{activeHistoricalCount === 1 ? "" : "s"} available{requiresCommander && activeHistoricalCount ? " · one rotates into each pod cycle" : ""}</small></span>
                  <label className="adaptive-toggle coevolve-toggle"><input type="checkbox" checked={coevolveAdversaries} onChange={(event) => setCoevolveAdversaries(event.target.checked)} /><span><b>Co-evolve adversaries</b><small>Archive each cycle's runner-up as a new opponent for later cycles.</small></span></label>
                  <label className="adaptive-toggle coevolve-toggle"><input type="checkbox" checked={learnPackagesFromHall} onChange={(event) => setLearnPackagesFromHall(event.target.checked)} /><span><b>Discover successful packages</b><small>Mine recurring nonland card pairs from diverse champions and add them to package mutation for this run.</small></span></label>
                  <button className="primary" disabled={!canStart || (!hexdekReady && !forgeReady) || hybridRunning || running || validating || forgeRunning || halvingRunning || coverageLoading || gauntletCoverageBlocked || gauntletDecks.length < requiredOpponentCount} onClick={() => void startHybridEvolution()}>{hybridRunning ? `Cycle ${hybridProgress?.cycle || 1}/${hybridProgress?.total_cycles || hybridCycles} · ${hybridProgress?.phase === "validation" ? "ranking" : "evolving"}` : "Run closed-loop evolution"}</button>
                </div>
              </div>
            </div>

            <div className="matchup-report">
              <div className="report-head"><span className="eyebrow">Persisted matchup evidence{simulationReport ? ` · ${simulationReport.engine}` : ""}</span>{savedValidations.length > 0 && <small>{savedValidations.length} saved validation{savedValidations.length === 1 ? "" : "s"}</small>}</div>
              {latestHoldout && <div className="winner-evidence"><b>Latest untouched holdout · cycle {String(latestHoldout.request.cycle || "—")}</b><span>selection {percent(Number(latestHoldout.request.selection_evidence_score || 0))} → holdout {percent(Number(latestHoldout.request.holdout_evidence_score || 0))}</span><i>Generalization gap {Number(latestHoldout.request.generalization_gap || 0) >= 0 ? "+" : ""}{percent(Number(latestHoldout.request.generalization_gap || 0))} · {String(latestHoldout.request.split_policy_id || "legacy split")}{latestHoldout.request.opponents_reused ? " · opponents reused with disjoint seeds" : " · disjoint gauntlet"}</i>{calibration && <small>Calibration {calibration.state} · {calibration.observations} held-out cycles · interval coverage {percent(calibration.empirical_interval_coverage)} / {percent(calibration.target_interval_coverage)} target · finalist-order agreement {calibration.selection_agreement == null ? "collecting" : percent(calibration.selection_agreement)} · suggested caution change {calibration.recommended_risk_aversion_delta >= 0 ? "+" : ""}{percent(calibration.recommended_risk_aversion_delta)}{calibration.state === "undercovered" ? " · statistical early elimination is automatically suspended" : ""}</small>}</div>}
              {surrogate && <div className="winner-evidence"><b>Learned matchup screen · {surrogate.ready ? "ready" : surrogate.stale ? "drift bypass" : "collecting"}</b><span>{number(surrogate.labels)} / {number(surrogate.minimum_labels)} completed, fully covered holdout labels{surrogate.manifest ? ` · ${surrogate.manifest.lineage_field_groups} isolated lineage/field groups` : ""}</span><i>{surrogate.gpu_decision}</i>{surrogate.manifest && <small>Held-out log loss {surrogate.manifest.validation_log_loss.toFixed(3)} vs screen baseline {surrogate.manifest.baseline_log_loss.toFixed(3)} · MAE {surrogate.manifest.validation_mae.toFixed(3)} · active-run holdouts never train their own screen.</small>}{surrogate.device && <small>{surrogate.device.cpu_backend} {number(surrogate.device.cpu_predictions_per_second)} predictions/s · {surrogate.device.gpu_available ? `${surrogate.device.gpu_backend} ${number(surrogate.device.gpu_predictions_per_second)} predictions/s (${surrogate.device.speedup.toFixed(2)}×, max parity delta ${surrogate.device.maximum_absolute_difference.toExponential(1)})` : "WGPU unavailable; automatic CPU fallback"}</small>}{surrogate.labels >= surrogate.minimum_labels && <button className="secondary" onClick={() => void invoke<SurrogateStatus>("train_surrogate_model", { platform, format, commander: requiresCommander }).then(setSurrogate).catch((error) => setNotice(`Model training failed · ${String(error)}`))}>Train + benchmark device</button>}</div>}
              {!simulationReport ? <div className="empty-state">Run a gauntlet to replace proxy-only judgment with deterministic {requiresCommander ? "multiplayer" : "matchup"} evidence.</div> : <>
                <div className="fitness-strip validation-metrics">
                  <Metric label="Games" value={number(simulationReport.games)} />
                  <Metric label="Games / sec" value={number(simulationReport.games_per_second)} tone="accent" />
                  <Metric label="Crashes" value={number(simulationReport.crashes)} tone={simulationReport.crashes ? "warn" : undefined} />
                  <Metric label="Cached" value={number(simulationReport.cached_games || 0)} tone={simulationReport.cached_games ? "accent" : undefined} />
                  {simulationReport.performance && <Metric label="Engine / total" value={`${number(simulationReport.performance.engine_ms)} / ${number(simulationReport.performance.total_ms)} ms`} tone="accent" />}
                  {simulationReport.performance && <Metric label="Process starts" value={number(simulationReport.performance.process_starts)} />}
                  <Metric label="Seat evidence" value={simulationReport.seat_order?.length ? `${simulationReport.seat_order.length} rows` : "Unavailable"} />
                  <Metric label="Position floor" value={validatedPosition ? `${percent(validatedPosition.worstRate)} · L ${percent(validatedPosition.worstLower)}` : "Unavailable"} tone={validatedPosition && validatedPosition.worstRate < 0.4 ? "warn" : undefined} />
                  <Metric label="Position spread" value={validatedPosition ? percent(validatedPosition.spread) : "Unavailable"} tone={validatedPosition && validatedPosition.spread > 0.25 ? "warn" : undefined} />
                  <Metric label="Game evidence" value={validatedEvidence == null ? "—" : percent(validatedEvidence)} />
                  <Metric label="Validated fitness" value={validatedFitness?.toFixed(3) || "—"} />
                  {simulationReport.combo_execution?.plan_available && <Metric label="Combo plan executable" value={percent(simulationReport.combo_execution.executable_rate)} tone={simulationReport.combo_execution.executable_rate > 0 ? "accent" : "warn"} />}
                  {simulationReport.combo_execution?.plan_available && <Metric label="Combo-plan wins" value={`${number(simulationReport.combo_execution.plan_wins)} / ${number(simulationReport.combo_execution.executable_games)}`} />}
                </div>
                {simulationReport.combo_execution?.plan_available && <details className="conformance-import" open><summary>Combo-plan engine telemetry · {simulationReport.combo_execution.planned_lines} proven line{simulationReport.combo_execution.planned_lines === 1 ? "" : "s"}</summary><p>Assembling in {number(simulationReport.combo_execution.assembling_games)} games ({percent(simulationReport.combo_execution.assembling_rate)}); executable state reached in {number(simulationReport.combo_execution.executable_games)} ({percent(simulationReport.combo_execution.executable_rate)}); {number(simulationReport.combo_execution.priority_actions)} legal plan-prioritized actions; {number(simulationReport.combo_execution.plan_wins)} wins after reaching an executable state and taking a plan-prioritized action.</p><p>Exact-state revision <code>{simulationReport.combo_execution.proof_revision}</code>. This telemetry measures plan reach and legal AI priorities; only a deterministic terminal-state conformance test can upgrade a line to engine-executed proof.</p>{simulationReport.combo_execution.limitations.map((note) => <p className="warn" key={note}>{note}</p>)}</details>}
                {!!Object.keys(simulationReport.engine_routes || {}).length && <details className="conformance-import" open><summary>Locked engine route · {Object.keys(simulationReport.engine_routes || {}).length} matchup{Object.keys(simulationReport.engine_routes || {}).length === 1 ? "" : "s"}</summary><p>{Object.entries(simulationReport.engine_routes || {}).map(([opponent, engine]) => `${opponent}: ${engine}`).join(" · ")}</p>{(simulationReport.route_failures || []).map((failure) => <p className="warn" key={failure}>{failure}</p>)}</details>}
                {simulationReport.commander_evidence && <div className="commander-evidence">
                  <div className="conformance-head"><div><strong>{simulationReport.commander_evidence.evidence_mode === "multiplayer_pod" ? `${simulationReport.commander_evidence.pod_size}-player pod evidence` : "Two-player Commander evidence"}</strong><small>{simulationReport.commander_evidence.policy_id} · schema {simulationReport.commander_evidence.schema_version} · seed {simulationReport.commander_evidence.seed}</small></div><div className="conformance-metrics"><span>Objective <b>{percent(simulationReport.commander_evidence.objective_score)}</b></span><span>Rotations <b>{number(simulationReport.commander_evidence.complete_rotations)}</b></span><span className={simulationReport.commander_evidence.partial_rotation_games ? "warn" : ""}>Partial <b>{simulationReport.commander_evidence.partial_rotation_games}</b></span><span className={simulationReport.commander_evidence.penalties.total_penalty ? "warn" : ""}>Penalty <b>{percent(simulationReport.commander_evidence.penalties.total_penalty)}</b></span></div></div>
                  <div className="commander-objectives">{([
                    ["Pod win share", simulationReport.commander_evidence.pod_win_share],
                    ["Seat floor", simulationReport.commander_evidence.seat_floor],
                    ["Threat", simulationReport.commander_evidence.threat_development],
                    ["Interaction", simulationReport.commander_evidence.interaction],
                    ["Recovery", simulationReport.commander_evidence.recovery],
                    ["Mana reliability", simulationReport.commander_evidence.mana_reliability],
                    ["Closing", simulationReport.commander_evidence.closing_ability]
                  ] as const).map(([label, metric]) => <div key={label}><span>{label}</span><b>{percent(metric.value)}</b><small>{metric.source.replaceAll("_", " ")}</small></div>)}</div>
                  <details><summary>Pod, penalties, and AI limitations</summary><p>Archetypes: {simulationReport.commander_evidence.pod_archetypes.join(" · ")}</p><p>Rotation: {simulationReport.commander_evidence.rotation_policy.replaceAll("_", " ")}. Draw/stall {percent(simulationReport.commander_evidence.penalties.draw_or_stall_rate)} · timeout/crash {percent(simulationReport.commander_evidence.penalties.timeout_or_crash_rate)} · concession {percent(simulationReport.commander_evidence.penalties.concession_rate)} · seat bias {percent(simulationReport.commander_evidence.penalties.seat_bias)} · kingmaking {simulationReport.commander_evidence.penalties.kingmaking_signal == null ? "not observable" : percent(simulationReport.commander_evidence.penalties.kingmaking_signal)}.</p>{Object.keys(simulationReport.commander_evidence.target_actions).length > 0 && <p>Directed actions: {Object.entries(simulationReport.commander_evidence.target_actions).map(([target, actions]) => `${target} ${actions}`).join(" · ")}</p>}{simulationReport.commander_evidence.limitations.map((note) => <p key={note}>{note}</p>)}</details>
                </div>}
                <details className="conformance-import"><summary>Reproducibility manifest</summary>{simulationReport.provenance ? <p>Schema {simulationReport.provenance.schema_version} · engine <code>{simulationReport.provenance.engine_revision}</code> · cards <code>{simulationReport.provenance.card_data_revision}</code> · configuration <code>{simulationReport.provenance.configuration_hash}</code>{simulationReport.provenance.scenario_schema_version == null ? "" : ` · scenario schema ${simulationReport.provenance.scenario_schema_version}`}</p> : <p>Legacy saved result: complete provenance was not recorded. Re-run this matchup before treating it as reproducible evidence.</p>}</details>
                <p className="fitness-note">{workspace?.config.evolution_mode === "tune" ? "Tune promotion uses game evidence only; screen score has zero promotion weight. " : "Manual validation ranking blends 65% confidence-adjusted game evidence with 35% screen score. "}The evidence score uses the 95% lower bound, worst opponent, native HexDek position floor when available, and crash rate according to your controls. Forge remains position-neutral because its stock summaries do not expose starting-player identity.</p>
                {validatedRow && <button className="secondary promote" onClick={promoteValidatedCandidate}>Promote validated deck as next seed</button>}
                <table><thead><tr><th>{requiresCommander ? "Commander" : "Deck marker"}</th>{!requiresCommander && <th>Field share</th>}<th>Wins</th><th>Games</th><th>Win rate</th><th>95% CI</th><th>Turn to win</th></tr></thead><tbody>{simulationReport.rows.map((row) => <tr key={row.deck} className={row.deck === selectedResultName ? "candidate-row" : ""}><td>{row.deck}</td>{!requiresCommander && <td>{simulationReport.matchup_weights?.[row.deck] == null ? "—" : percent(simulationReport.matchup_weights[row.deck])}</td>}<td>{number(row.wins)}</td><td>{number(row.games)}</td><td>{percent(row.win_rate)}</td><td>{percent(row.win_rate_lower_bound ?? wilsonBounds(row.wins, row.games)[0])}–{percent(row.win_rate_upper_bound ?? wilsonBounds(row.wins, row.games)[1])}</td><td>{row.average_turn_to_win?.toFixed(1) || "—"}</td></tr>)}</tbody></table>
                {!!simulationReport.seat_order?.length && <details className="seat-evidence" open><summary>Seat-position evidence · HexDek rotates deck assignment every game</summary><table><thead><tr><th>Matchup</th><th>Deck</th><th>Position</th><th>Wins</th><th>Games</th><th>Win rate</th><th>95% lower</th></tr></thead><tbody>{simulationReport.seat_order.map((row) => <tr key={`${row.matchup}-${row.deck}-${row.position}`} className={row.deck === selectedResultName ? "candidate-row" : ""}><td>{row.matchup}</td><td>{row.deck}</td><td>{row.position}</td><td>{number(row.wins)}</td><td>{number(row.games)}</td><td>{percent(row.win_rate)}</td><td>{percent(wilsonBounds(row.wins, row.games)[0])}</td></tr>)}</tbody></table></details>}
                {savedValidations.length > 1 && <div className="validation-history"><span>Previous</span>{savedValidations.slice(0, 8).map((validation) => <button key={validation.id} onClick={() => setSimulationReport(validation.report)}>{validation.report.engine} · #{validation.candidate_rank || "—"} · {number(validation.report.games)} games</button>)}</div>}
              </>}
              {halvingReport && <div className="halving-tree"><span className="eyebrow">Successive-halving tree{halvingReport.maximum_game_budget ? ` · ${number(halvingReport.scheduled_game_budget || 0)}/${number(halvingReport.maximum_game_budget)} games allocated${halvingReport.avoided_game_budget ? ` · ${number(halvingReport.avoided_game_budget)} avoided` : ""}` : ""}</span>{halvingWinnerEvidence && <div className="winner-evidence"><b>Winner #{halvingWinnerEvidence.candidate_rank}</b><span>{percent(halvingWinnerEvidence.game_win_rate)} observed · score CI {percent(halvingWinnerEvidence.evidence_lower_bound ?? 0)}–{percent(halvingWinnerEvidence.evidence_upper_bound ?? 1)}</span><i>Worst matchup {percent(halvingWinnerEvidence.worst_matchup_win_rate)}{halvingWinnerEvidence.worst_position_win_rate == null ? "" : ` · position ${percent(halvingWinnerEvidence.worst_position_win_rate)} · spread ${percent(halvingWinnerEvidence.position_win_rate_spread || 0)}`} · evidence {percent(halvingWinnerEvidence.evidence_score)}</i></div>}{halvingReport.rounds.map((round) => <div key={round.round}><b>R{round.round}</b><span>{number(round.games_per_candidate)} games each</span><i>{round.entrants} → {round.survivors.length}{round.statistically_eliminated?.length ? ` · ${round.statistically_eliminated.length} CI drop${round.statistically_eliminated.length === 1 ? "" : "s"}` : ""}</i><em>{round.survivors.map((rank) => `#${rank}`).join(" · ") || "No survivors"}</em></div>)}</div>}
              {halvingFailures.length > 0 && <details className="halving-failures"><summary>{halvingFailures.length} finalist check{halvingFailures.length === 1 ? "" : "s"} rejected</summary>{halvingFailures.slice(0, 20).map((failure) => <p key={`${failure.round}-${failure.candidate_rank}`}><b>R{failure.round} · #{failure.candidate_rank}</b><span>{failure.error}</span></p>)}</details>}
              {savedHalvings.length > 1 && <div className="validation-history"><span>Saved brackets</span>{savedHalvings.slice(0, 5).map((halving) => <button key={halving.id} onClick={() => setHalvingReport(halving.report)}>Winner #{halving.report.winner_rank || "—"}</button>)}</div>}
            </div>
          </div>
          </div>
        </details>
        <details className="panel feedback-lab">
          <summary className="section-summary"><span><b>Real-world feedback</b><small>Bring Arena match outcomes back into local evaluation.</small></span><i>{matchRecords.length} matches</i></summary>
          <div className="section-content">
          <div className="panel-title"><div><span className="step">04</span><h2>Real-world feedback</h2></div><span className="legal">Local only · explicit opt-in</span></div>
          <p>Copy a candidate once to register its exact exported revision, then paste Arena Player.log JSON lines or portable schema-1 match JSON. Logs without an exact Arena-ID deck match are quarantined and never influence weights.</p>
          <div className="metagame-import-row"><select value={matchImportSource} onChange={(event) => setMatchImportSource(event.target.value as "arena_log" | "json")}><option value="arena_log">Arena log</option><option value="json">Portable JSON</option></select><button className="secondary" disabled={!matchImportText.trim()} onClick={() => void importMatches()}>Import locally</button><button className="secondary" disabled={!matchRecords.length} onClick={() => void invoke<string>("export_match_history").then((text) => navigator.clipboard.writeText(text).then(() => setNotice("Match history copied")))}>Export + copy</button><button className="delete" disabled={!matchRecords.length} onClick={() => void invoke<number>("clear_match_history").then(() => refreshFeedback()).then(() => setNotice("Local match history deleted"))}>Delete all</button></div>
          <textarea value={matchImportText} onChange={(event) => setMatchImportText(event.target.value)} placeholder={matchImportSource === "arena_log" ? "Paste Arena Player.log JSON lines…" : '{"schema_version":1,"matches":[…]}'} />
          {feedbackSummary && <div className="fitness-strip validation-metrics"><Metric label="Exact matches" value={number(feedbackSummary.matches)} /><Metric label="Observed" value={percent(feedbackSummary.observed_win_rate)} tone="accent" /><Metric label="Predicted" value={feedbackSummary.predicted_win_rate == null ? "—" : percent(feedbackSummary.predicted_win_rate)} /><Metric label="Prediction error" value={feedbackSummary.prediction_error == null ? "—" : `${feedbackSummary.prediction_error >= 0 ? "+" : ""}${percent(feedbackSummary.prediction_error)}`} /></div>}
          <div className="feedback-actions"><button className="secondary" disabled={!gauntletDecks.length || !feedbackSummary?.matches} onClick={() => void previewFeedbackWeights()}>Preview field reweighting</button>{reweightSuggestions.length > 0 && <button className="primary" onClick={applyFeedbackWeights}>Approve suggested weights</button>}<small>Nothing is reweighted until you approve it.</small></div>
          {reweightSuggestions.length > 0 && <div className="acquisition-report">{reweightSuggestions.map((row) => <p key={row.opponent}><b>{row.opponent} · {row.matched_games} matches</b><span>{percent(row.current_weight)} → {percent(row.suggested_weight)} · {row.reason}</span></p>)}</div>}
          <div className="match-feedback-list">{matchRecords.length === 0 ? <div className="empty-state compact">No real-world matches imported.</div> : matchRecords.slice(0, 100).map((record) => <div key={record.id} className={record.identity_status === "exact" ? "" : "quarantined"}><b>{record.result.toUpperCase()} · {record.deck_name || "Unmatched deck"}</b><small>{record.identity_status === "exact" ? `Exact revision ${record.revision_id?.slice(0, 18)}…` : "Quarantined: no exact exported revision"} · {record.source}</small><input value={record.opponent_archetype} onChange={(event) => setMatchRecords((rows) => rows.map((row) => row.id === record.id ? { ...row, opponent_archetype: event.target.value } : row))} placeholder="Opponent archetype" /><select value={record.play_draw} onChange={(event) => setMatchRecords((rows) => rows.map((row) => row.id === record.id ? { ...row, play_draw: event.target.value as MatchRecord["play_draw"] } : row))}><option value="unknown">Play/draw unknown</option><option value="play">On play</option><option value="draw">On draw</option></select><select value={record.sideboarded == null ? "unknown" : record.sideboarded ? "yes" : "no"} onChange={(event) => setMatchRecords((rows) => rows.map((row) => row.id === record.id ? { ...row, sideboarded: event.target.value === "unknown" ? undefined : event.target.value === "yes" } : row))}><option value="unknown">Sideboard unknown</option><option value="yes">Post-board</option><option value="no">Pre-board</option></select><input type="number" min="0" max="1" step="0.01" value={record.predicted_win_rate ?? ""} onChange={(event) => setMatchRecords((rows) => rows.map((row) => row.id === record.id ? { ...row, predicted_win_rate: event.target.value === "" ? undefined : Number(event.target.value) } : row))} placeholder="Predicted 0–1" /><input value={record.notes} onChange={(event) => setMatchRecords((rows) => rows.map((row) => row.id === record.id ? { ...row, notes: event.target.value } : row))} placeholder="Notes" /><button className="secondary" onClick={() => void saveMatchEdit(record)}>Save</button><button className="delete" onClick={() => void invoke<number>("delete_match_records", { ids: [record.id] }).then(() => refreshFeedback())}>Delete</button></div>)}</div>
          </div>
        </details>
      </div>
    </main>
  );
}
