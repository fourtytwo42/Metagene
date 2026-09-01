mod commander;
mod evolution;
mod export;
mod import;
mod legality;
mod model;
mod rng;
mod sideboard;

pub use commander::{
    CommanderCompatibilityEvidence, CommanderCompatibilityPolicy, CommanderCompatibilityRule,
    CommanderConformanceReport, CommanderConformanceResult, commander_compatibility,
    commander_compatibility_policy, commander_conformance_report, commanders_compatible,
};
pub use evolution::{
    CancellationToken, CardCastabilityEvidence, CopyEvidence, Evaluator, EvolutionEngine,
    EvolutionError, Fitness, GenerationProgress, HeuristicEvaluator, MapEliteCell,
    MapEliteDescriptor, PackageSummary, PromotionEvidence, RankedDeck, RequirementEvidence,
    RunResult, STRUCTURAL_MODEL_REVISION, apply_combo_package_evidence, apply_fitness_weights,
    evaluate_structural_deck, evaluate_structural_deck_with_context,
    evaluate_tune_deck_with_context, strategy_signature_for_deck,
};
pub use export::export_mtga;
pub use import::{DeckImportError, import_decklist};
pub use legality::{DeckLegalityError, is_commander_eligible, validate_deck_legality};
pub use model::{
    AbilityCostProfile, AbilityKind, AbilityProfile, AbilityTiming, AbilityZone,
    ActionTargetProfile, AmountExpression, Card, CardFeatures, CardId, CardMode, CardPool,
    ClauseClassification, ClauseCriticality, Color, ComboPackageProfile, CounterProfile, Deck,
    DeckConstraints, DeckEntry, EvolutionConfig, EvolutionMode, FitnessWeights, LockedCard,
    ManaColorRule, ManaCostProfile, ManaSourceProfile, ManaSpendRule, ManaStateRule,
    MechanicDescriptor, MechanicProfile, OpponentFieldProfile, OracleClause, OracleCoverageSummary,
    RuleAbility, RuleCondition, RuleCost, RuleEffect, RuleTarget, RuleTiming, SearchActionProfile,
    SearchTargetProfile, StructuralEvaluationContext, TokenProfile, TunePolicy,
};
pub use sideboard::{
    MatchupSideboardPlan, PlayDraw, SideboardExchange, SideboardPlanError,
    apply_sideboard_exchange, validate_sideboard_plans,
};
