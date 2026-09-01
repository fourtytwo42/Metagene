use metagene_core::{
    CardPool, Deck, DeckConstraints, DeckImportError, export_mtga, import_decklist,
    validate_deck_legality,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const ACQUISITION_SCHEMA_VERSION: u32 = 1;
pub const SPICERACK_ENDPOINT: &str = "https://api.spicerack.gg/api/export-decklists/";
pub const TOPDECK_ENDPOINT: &str = "https://topdeck.gg/api/v2/tournaments";
pub const GETPAIRD_ENDPOINT: &str = "https://getpaird.io/api/v1/decks";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetagameAcquisitionRequest {
    pub source: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub lookback_days: usize,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceProvenance {
    pub adapter: String,
    pub source_url: String,
    pub retrieved_at_unix: u64,
    pub event_start_unix: Option<i64>,
    pub event_end_unix: Option<i64>,
    pub event_count: usize,
    pub submitted_deck_count: usize,
    pub accepted_deck_count: usize,
    pub transformation_notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckOccurrence {
    pub event_id: String,
    pub event_name: String,
    pub event_url: String,
    pub event_start_unix: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcquiredArchetype {
    pub archetype: String,
    pub deck: Deck,
    pub decklist: String,
    pub occurrences: usize,
    pub weight: f64,
    pub fingerprint: String,
    pub provenance: Vec<DeckOccurrence>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineReason {
    MissingDecklist,
    StaleEvent,
    FormatMismatch,
    UnresolvableCard,
    AmbiguousDecklist,
    IllegalDeck,
    MalformedSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuarantinedDeck {
    pub event_id: String,
    pub event_name: String,
    pub submitted_name: String,
    pub reason: QuarantineReason,
    pub detail: String,
    pub decklist: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetagameAcquisitionReport {
    pub schema_version: u32,
    pub source: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub provenance: SourceProvenance,
    pub accepted: Vec<AcquiredArchetype>,
    pub quarantined: Vec<QuarantinedDeck>,
}

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error("unsupported metagame source: {0}")]
    UnsupportedSource(String),
    #[error("unsupported Spicerack format: {0}")]
    UnsupportedFormat(String),
    #[error("metagame source request failed: {0}")]
    Request(String),
    #[error("metagame source response was invalid: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceEvent {
    #[serde(rename = "TID", alias = "tid")]
    id: String,
    #[serde(rename = "tournamentName")]
    name: String,
    format: String,
    #[serde(rename = "bracketUrl", alias = "eventUrl", default)]
    bracket_url: String,
    #[serde(rename = "startDate")]
    start_date: i64,
    #[serde(default)]
    standings: Vec<SourceStanding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SourceStanding {
    #[serde(default)]
    name: String,
    #[serde(default, alias = "decklist")]
    decklist_text: Option<String>,
    #[serde(default)]
    archetype: Option<String>,
}

pub fn acquire(
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    match request.source.trim().to_ascii_lowercase().as_str() {
        "spicerack" => acquire_spicerack(request, pool),
        "topdeck" => acquire_topdeck(request, pool),
        "getpaird" => acquire_getpaird(request, pool),
        _ => Err(AcquisitionError::UnsupportedSource(request.source.clone())),
    }
}

#[derive(Debug, Deserialize)]
struct GetPairdList {
    data: Vec<GetPairdSummary>,
}

#[derive(Debug, Deserialize)]
struct GetPairdSummary {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct GetPairdDetailEnvelope {
    data: GetPairdDetail,
}

#[derive(Debug, Deserialize)]
struct GetPairdDetail {
    slug: String,
    name: String,
    format: String,
    #[serde(default)]
    archetype: Option<String>,
    updated_at: String,
    urls: GetPairdUrls,
    cards: GetPairdCards,
}

#[derive(Debug, Deserialize)]
struct GetPairdUrls {
    web: String,
}

#[derive(Debug, Deserialize)]
struct GetPairdCards {
    #[serde(default)]
    mainboard: Vec<GetPairdCard>,
    #[serde(default)]
    sideboard: Vec<GetPairdCard>,
    #[serde(default)]
    command_zone: Vec<GetPairdCard>,
}

#[derive(Debug, Deserialize)]
struct GetPairdCard {
    name: String,
    quantity: u8,
}

pub fn spicerack_format(format: &str, commander: bool) -> Result<&'static str, AcquisitionError> {
    let normalized = format.trim().to_ascii_lowercase();
    let mapped = match normalized.as_str() {
        "standard" => "STANDARD",
        "modern" => "MODERN",
        "pioneer" => "PIONEER",
        "legacy" => "LEGACY",
        "vintage" => "VINTAGE",
        "pauper" => "PAUPER",
        "historic" => "HISTORIC",
        "explorer" => "EXPLORER",
        "timeless" => "TIMELESS",
        "commander" if commander => "COMMANDER2",
        "brawl" if commander => "STANDARD_BRAWL",
        _ => return Err(AcquisitionError::UnsupportedFormat(format.into())),
    };
    Ok(mapped)
}

pub fn acquire_spicerack(
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    if !request.source.eq_ignore_ascii_case("spicerack") {
        return Err(AcquisitionError::UnsupportedSource(request.source.clone()));
    }
    let source_format = spicerack_format(&request.format, request.commander)?;
    let lookback_days = request.lookback_days.clamp(1, 365);
    let source_url = format!(
        "{SPICERACK_ENDPOINT}?num_days={lookback_days}&event_format={source_format}&decklist_as_text=true"
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Metagene/0.1 metagame acquisition")
        .build()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?;
    let mut call = client.get(&source_url).header("Accept", "application/json");
    if let Some(key) = request
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        call = call.header("X-API-Key", key.trim());
    }
    let response = call
        .send()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?;
    if !response.status().is_success() {
        return Err(AcquisitionError::Request(format!(
            "Spicerack returned HTTP {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(AcquisitionError::InvalidResponse(
            "response exceeds the 32 MiB safety limit".into(),
        ));
    }
    parse_spicerack(&bytes, request, pool, unix_now(), source_url, source_format)
}

pub fn topdeck_format(format: &str, commander: bool) -> Result<&'static str, AcquisitionError> {
    let normalized = format.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "standard" => Ok("Standard"),
        "modern" => Ok("Modern"),
        "pioneer" => Ok("Pioneer"),
        "legacy" => Ok("Legacy"),
        "vintage" => Ok("Vintage"),
        "pauper" => Ok("Pauper"),
        "historic" => Ok("Historic"),
        "explorer" => Ok("Explorer"),
        "timeless" => Ok("Timeless"),
        "commander" if commander => Ok("EDH"),
        _ => Err(AcquisitionError::UnsupportedFormat(format.into())),
    }
}

pub fn acquire_topdeck(
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    let api_key = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| AcquisitionError::Request("TopDeck requires a free API key".into()))?;
    let source_format = topdeck_format(&request.format, request.commander)?;
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Metagene/0.1 metagame acquisition")
        .build()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?
        .post(TOPDECK_ENDPOINT)
        .header("Accept", "application/json")
        .header("Authorization", api_key)
        .json(&serde_json::json!({
            "game": "Magic: The Gathering",
            "format": source_format,
            "last": request.lookback_days.clamp(1, 365),
            "columns": ["name", "decklist", "wins", "draws", "losses"]
        }))
        .send()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?;
    if !response.status().is_success() {
        return Err(AcquisitionError::Request(format!(
            "TopDeck returned HTTP {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(AcquisitionError::InvalidResponse(
            "response exceeds the 32 MiB safety limit".into(),
        ));
    }
    parse_source(
        &bytes,
        request,
        pool,
        unix_now(),
        TOPDECK_ENDPOINT.into(),
        source_format,
        "TopDeck.gg tournament API",
        "topdeck-tournaments-v2",
    )
}

pub fn getpaird_format(format: &str, commander: bool) -> Result<&'static str, AcquisitionError> {
    let normalized = format.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "standard" => Ok("Standard"),
        "modern" => Ok("Modern"),
        "pioneer" => Ok("Pioneer"),
        "legacy" => Ok("Legacy"),
        "vintage" => Ok("Vintage"),
        "pauper" => Ok("Pauper"),
        "historic" => Ok("Historic"),
        "explorer" => Ok("Explorer"),
        "timeless" => Ok("Timeless"),
        "commander" if commander => Ok("Commander Multiplayer"),
        _ => Err(AcquisitionError::UnsupportedFormat(format.into())),
    }
}

pub fn acquire_getpaird(
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    let source_format = getpaird_format(&request.format, request.commander)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Metagene/0.1 metagame acquisition")
        .build()
        .map_err(|error| AcquisitionError::Request(error.to_string()))?;
    let source_url = reqwest::Url::parse_with_params(
        GETPAIRD_ENDPOINT,
        &[
            ("game", "Magic: The Gathering"),
            ("format", source_format),
            ("sort", "recent"),
            ("per_page", "25"),
        ],
    )
    .map_err(|error| AcquisitionError::Request(error.to_string()))?
    .to_string();
    let listing = client
        .get(&source_url)
        .header("Accept", "application/json")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| AcquisitionError::Request(error.to_string()))?
        .json::<GetPairdList>()
        .map_err(|error| AcquisitionError::InvalidResponse(error.to_string()))?;
    let mut events = Vec::with_capacity(listing.data.len());
    for summary in listing.data {
        let detail_url = format!("{GETPAIRD_ENDPOINT}/{}", summary.slug);
        let detail = client
            .get(&detail_url)
            .header("Accept", "application/json")
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::json::<GetPairdDetailEnvelope>);
        match detail {
            Ok(envelope) => {
                events.push(getpaird_source_event(envelope.data)?);
            }
            Err(error) => events.push(SourceEvent {
                id: summary.slug.clone(),
                name: summary.slug,
                format: source_format.into(),
                bracket_url: detail_url,
                start_date: unix_now() as i64,
                standings: vec![SourceStanding {
                    name: "Unavailable public deck".into(),
                    decklist_text: None,
                    archetype: Some(format!("detail request failed: {error}")),
                }],
            }),
        }
    }
    let payload = serde_json::to_vec(&events)
        .map_err(|error| AcquisitionError::InvalidResponse(error.to_string()))?;
    parse_source(
        &payload,
        request,
        pool,
        unix_now(),
        source_url,
        source_format,
        "GetPaird public deck library",
        "getpaird-public-decks-v1",
    )
}

fn getpaird_source_event(detail: GetPairdDetail) -> Result<SourceEvent, AcquisitionError> {
    let start_date = time::OffsetDateTime::parse(
        &detail.updated_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|error| AcquisitionError::InvalidResponse(error.to_string()))?
    .unix_timestamp();
    Ok(SourceEvent {
        id: detail.slug,
        name: detail.name.clone(),
        format: detail.format,
        bracket_url: detail.urls.web,
        start_date,
        standings: vec![SourceStanding {
            name: detail.name,
            decklist_text: Some(getpaird_decklist(&detail.cards)),
            archetype: detail.archetype,
        }],
    })
}

pub fn parse_getpaird_detail(
    payload: &[u8],
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
    retrieved_at_unix: u64,
    source_format: &str,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    let envelope: GetPairdDetailEnvelope = serde_json::from_slice(payload)
        .map_err(|error| AcquisitionError::InvalidResponse(error.to_string()))?;
    let event = getpaird_source_event(envelope.data)?;
    let normalized = serde_json::to_vec(&[event])
        .map_err(|error| AcquisitionError::InvalidResponse(error.to_string()))?;
    parse_source(
        &normalized,
        request,
        pool,
        retrieved_at_unix,
        GETPAIRD_ENDPOINT.into(),
        source_format,
        "GetPaird public deck library",
        "getpaird-public-decks-v1",
    )
}

fn getpaird_decklist(cards: &GetPairdCards) -> String {
    let mut output = String::new();
    for (heading, entries) in [
        ("Commander", &cards.command_zone),
        ("Deck", &cards.mainboard),
        ("Sideboard", &cards.sideboard),
    ] {
        if entries.is_empty() {
            continue;
        }
        output.push_str(heading);
        output.push('\n');
        for card in entries.iter().filter(|card| card.quantity > 0) {
            output.push_str(&format!("{} {}\n", card.quantity, card.name));
        }
        output.push('\n');
    }
    output
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn parse_spicerack(
    payload: &[u8],
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
    retrieved_at_unix: u64,
    source_url: String,
    source_format: &str,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    parse_source(
        payload,
        request,
        pool,
        retrieved_at_unix,
        source_url,
        source_format,
        "Spicerack public decklist database",
        "spicerack-public-v1",
    )
}

pub fn parse_topdeck(
    payload: &[u8],
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
    retrieved_at_unix: u64,
    source_format: &str,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    parse_source(
        payload,
        request,
        pool,
        retrieved_at_unix,
        TOPDECK_ENDPOINT.into(),
        source_format,
        "TopDeck.gg tournament API",
        "topdeck-tournaments-v2",
    )
}

#[allow(clippy::too_many_arguments)]
fn parse_source(
    payload: &[u8],
    request: &MetagameAcquisitionRequest,
    pool: &CardPool,
    retrieved_at_unix: u64,
    source_url: String,
    source_format: &str,
    source_name: &str,
    adapter: &str,
) -> Result<MetagameAcquisitionReport, AcquisitionError> {
    let events: Vec<SourceEvent> = serde_json::from_slice(payload)
        .map_err(|error| AcquisitionError::InvalidResponse(error.to_string()))?;
    let cutoff = (retrieved_at_unix as i64)
        .saturating_sub((request.lookback_days.clamp(1, 365) as i64).saturating_mul(86_400));
    let mut quarantine = Vec::new();
    let mut accepted: HashMap<String, AcquiredArchetype> = HashMap::new();
    let mut submitted = 0usize;
    let mut event_start = None;
    let mut event_end = None;

    for event in &events {
        event_start =
            Some(event_start.map_or(event.start_date, |value: i64| value.min(event.start_date)));
        event_end =
            Some(event_end.map_or(event.start_date, |value: i64| value.max(event.start_date)));
        for standing in &event.standings {
            submitted += 1;
            let base = || QuarantinedDeck {
                event_id: event.id.clone(),
                event_name: event.name.clone(),
                submitted_name: standing.name.clone(),
                reason: QuarantineReason::MalformedSource,
                detail: String::new(),
                decklist: standing.decklist_text.clone(),
            };
            if !event.format.eq_ignore_ascii_case(source_format) {
                let mut item = base();
                item.reason = QuarantineReason::FormatMismatch;
                item.detail = format!(
                    "source reported {} instead of {source_format}",
                    event.format
                );
                quarantine.push(item);
                continue;
            }
            if event.start_date < cutoff || event.start_date > retrieved_at_unix as i64 + 86_400 {
                let mut item = base();
                item.reason = QuarantineReason::StaleEvent;
                item.detail = "event is outside the requested observation window".into();
                quarantine.push(item);
                continue;
            }
            let Some(text) = standing
                .decklist_text
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            else {
                let mut item = base();
                item.reason = QuarantineReason::MissingDecklist;
                item.detail = "source did not publish plaintext deck contents".into();
                quarantine.push(item);
                continue;
            };
            if text.trim().starts_with("http://") || text.trim().starts_with("https://") {
                let mut item = base();
                item.reason = QuarantineReason::MissingDecklist;
                item.detail =
                    "source supplied an external deck URL, not plaintext card contents".into();
                quarantine.push(item);
                continue;
            }
            let mut deck = match import_decklist(text, pool) {
                Ok(deck) => deck,
                Err(error) => {
                    let mut item = base();
                    item.reason = match error {
                        DeckImportError::UnknownCard { .. } => QuarantineReason::UnresolvableCard,
                        DeckImportError::EmptyDeck => QuarantineReason::MissingDecklist,
                        DeckImportError::InvalidLine { .. }
                        | DeckImportError::InvalidQuantity { .. } => {
                            QuarantineReason::AmbiguousDecklist
                        }
                    };
                    item.detail = error.to_string();
                    quarantine.push(item);
                    continue;
                }
            };
            if let Err(error) =
                validate_acquired_deck(&deck, pool, request.commander, &request.format)
            {
                let mut item = base();
                item.reason = QuarantineReason::IllegalDeck;
                item.detail = error;
                quarantine.push(item);
                continue;
            }
            let fingerprint = deck_fingerprint(&deck);
            let archetype = standing
                .archetype
                .as_deref()
                .and_then(normalize_archetype_name)
                .unwrap_or_else(|| archetype_name(&deck, pool));
            deck.name = archetype.clone();
            let occurrence = DeckOccurrence {
                event_id: event.id.clone(),
                event_name: event.name.clone(),
                event_url: event.bracket_url.clone(),
                event_start_unix: event.start_date,
            };
            accepted
                .entry(fingerprint.clone())
                .and_modify(|entry| {
                    entry.occurrences += 1;
                    entry.provenance.push(occurrence.clone());
                })
                .or_insert_with(|| AcquiredArchetype {
                    archetype,
                    decklist: export_mtga(&deck, pool),
                    deck,
                    occurrences: 1,
                    weight: 0.0,
                    fingerprint,
                    provenance: vec![occurrence],
                });
        }
    }

    let accepted_total = accepted
        .values()
        .map(|entry| entry.occurrences)
        .sum::<usize>();
    let mut accepted = accepted.into_values().collect::<Vec<_>>();
    for entry in &mut accepted {
        entry.weight = entry.occurrences as f64 / accepted_total.max(1) as f64;
    }
    accepted.sort_by(|left, right| {
        right
            .occurrences
            .cmp(&left.occurrences)
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
    });
    quarantine.sort_by(|left, right| left.event_id.cmp(&right.event_id));

    Ok(MetagameAcquisitionReport {
        schema_version: ACQUISITION_SCHEMA_VERSION,
        source: source_name.into(),
        platform: request.platform.to_ascii_lowercase(),
        format: request.format.to_ascii_lowercase(),
        commander: request.commander,
        provenance: SourceProvenance {
            adapter: adapter.into(),
            source_url,
            retrieved_at_unix,
            event_start_unix: event_start,
            event_end_unix: event_end,
            event_count: events.len(),
            submitted_deck_count: submitted,
            accepted_deck_count: accepted_total,
            transformation_notes: vec![
                "Resolved plaintext card names against the selected platform/format catalog.".into(),
                "Revalidated deck size, command zone, singleton, and shared copy limits.".into(),
                "Merged exact canonical deck genomes and weighted them by occurrence count.".into(),
                "Generated neutral archetype labels from the most-played nonland cards; labels remain editable.".into(),
                "Quarantined missing, stale, mismatched, unresolvable, and illegal submissions.".into(),
            ],
        },
        accepted,
        quarantined: quarantine,
    })
}

fn validate_acquired_deck(
    deck: &Deck,
    pool: &CardPool,
    commander: bool,
    format: &str,
) -> Result<(), String> {
    let normalized = format.to_ascii_lowercase();
    let expected = if normalized == "commander" {
        Some(99)
    } else if normalized.contains("brawl") {
        Some(59)
    } else {
        None
    };
    if expected.is_some_and(|size| deck.mainboard_count() != size)
        || expected.is_none() && deck.mainboard_count() < 60
    {
        return Err(format!("main deck has {} cards", deck.mainboard_count()));
    }
    let constraints = DeckConstraints {
        target_mainboard_size: deck.mainboard_count(),
        target_sideboard_size: deck.sideboard_count(),
        minimum_commanders: usize::from(commander),
        maximum_commanders: if normalized == "commander" {
            2
        } else {
            usize::from(commander)
        },
        default_max_copies: if commander { 1 } else { 4 },
        locked_cards: Vec::new(),
        minimum_lands: None,
        maximum_lands: None,
        allowed_colors: None,
    };
    validate_deck_legality(deck, pool, &constraints).map_err(|error| error.to_string())
}

fn deck_fingerprint(deck: &Deck) -> String {
    let mut zones = [&deck.command_zone, &deck.mainboard, &deck.sideboard]
        .into_iter()
        .map(|zone| {
            let mut entries = zone
                .iter()
                .map(|entry| (entry.card_id.0.as_str(), entry.quantity))
                .collect::<Vec<_>>();
            entries.sort_unstable();
            entries
        });
    let canonical =
        serde_json::to_vec(&(zones.next(), zones.next(), zones.next())).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(canonical))
}

fn archetype_name(deck: &Deck, pool: &CardPool) -> String {
    let by_id = pool.index_by_id();
    let mut anchors = deck
        .mainboard
        .iter()
        .filter_map(|entry| {
            let card = &pool.cards[*by_id.get(&entry.card_id)?];
            (!card.features.is_land).then_some((entry.quantity, card.name.as_str()))
        })
        .collect::<Vec<_>>();
    anchors.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let names = anchors
        .into_iter()
        .take(2)
        .map(|(_, name)| name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        "Unclassified deck".into()
    } else {
        names.join(" / ")
    }
}

fn normalize_archetype_name(value: &str) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let canonical = match collapsed.to_ascii_lowercase().as_str() {
        "mono red" | "mono-red" | "monored" => "Mono-Red",
        "mono white" | "mono-white" | "monowhite" => "Mono-White",
        "mono blue" | "mono-blue" | "monoblue" => "Mono-Blue",
        "mono black" | "mono-black" | "monoblack" => "Mono-Black",
        "mono green" | "mono-green" | "monogreen" => "Mono-Green",
        _ => return Some(collapsed),
    };
    Some(canonical.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Card, CardFeatures, CardId, Color};
    use std::collections::BTreeMap;

    fn pool() -> CardPool {
        let cards = [
            ("plains", "Plains", true),
            ("hawk", "Healer's Hawk", false),
            ("pridemate", "Ajani's Pridemate", false),
        ]
        .into_iter()
        .map(|(id, name, land)| Card {
            id: CardId(id.into()),
            arena_id: Some(1),
            platforms: vec!["arena".into()],
            name: name.into(),
            set_code: "TST".into(),
            collector_number: "1".into(),
            type_line: if land {
                "Basic Land — Plains"
            } else {
                "Creature"
            }
            .into(),
            oracle_text: String::new(),
            colors: vec![Color::White],
            color_identity: vec![Color::White],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: land,
                unlimited_copies: land,
                ..Default::default()
            },
        })
        .collect();
        CardPool { cards }
    }

    fn commander_pool() -> CardPool {
        let mut pool = pool();
        pool.cards.push(Card {
            id: CardId("commander".into()),
            arena_id: Some(2),
            platforms: vec!["arena".into()],
            name: "Fixture Commander".into(),
            set_code: "TST".into(),
            collector_number: "2".into(),
            type_line: "Legendary Creature — Human".into(),
            oracle_text: String::new(),
            colors: vec![Color::White],
            color_identity: vec![Color::White],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures::default(),
        });
        pool
    }

    #[test]
    fn fixture_merges_duplicates_and_quarantines_bad_rows() {
        let request = MetagameAcquisitionRequest {
            source: "spicerack".into(),
            platform: "arena".into(),
            format: "standard".into(),
            commander: false,
            lookback_days: 30,
            api_key: None,
        };
        let report = parse_spicerack(
            include_bytes!("../tests/fixtures/spicerack-standard.json"),
            &request,
            &pool(),
            1_800_000_000,
            "https://example.test/query".into(),
            "STANDARD",
        )
        .unwrap();
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].occurrences, 2);
        assert_eq!(report.accepted[0].weight, 1.0);
        assert_eq!(report.quarantined.len(), 2);
        assert!(
            report
                .quarantined
                .iter()
                .any(|item| item.reason == QuarantineReason::MissingDecklist)
        );
        assert!(
            report
                .quarantined
                .iter()
                .any(|item| item.reason == QuarantineReason::UnresolvableCard)
        );
        assert_eq!(report.provenance.accepted_deck_count, 2);
    }

    #[test]
    fn source_format_mapping_is_explicit() {
        assert_eq!(spicerack_format("Historic", false).unwrap(), "HISTORIC");
        assert_eq!(spicerack_format("Commander", true).unwrap(), "COMMANDER2");
        assert!(spicerack_format("Alchemy", false).is_err());
        assert_eq!(topdeck_format("Historic", false).unwrap(), "Historic");
        assert_eq!(topdeck_format("Commander", true).unwrap(), "EDH");
    }

    #[test]
    fn topdeck_fixture_uses_the_shared_contract_and_quarantines_external_urls() {
        let request = MetagameAcquisitionRequest {
            source: "topdeck".into(),
            platform: "arena".into(),
            format: "standard".into(),
            commander: false,
            lookback_days: 30,
            api_key: Some("not-used-by-parser".into()),
        };
        let report = parse_topdeck(
            include_bytes!("../tests/fixtures/topdeck-standard.json"),
            &request,
            &pool(),
            1_800_000_000,
            "Standard",
        )
        .unwrap();
        assert_eq!(report.provenance.adapter, "topdeck-tournaments-v2");
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].occurrences, 2);
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(
            report.quarantined[0].reason,
            QuarantineReason::MissingDecklist
        );
    }

    #[test]
    fn commander_fixture_resolves_command_zone_and_singleton_exceptions() {
        let request = MetagameAcquisitionRequest {
            source: "spicerack".into(),
            platform: "arena".into(),
            format: "commander".into(),
            commander: true,
            lookback_days: 30,
            api_key: None,
        };
        let report = parse_spicerack(
            include_bytes!("../tests/fixtures/spicerack-commander.json"),
            &request,
            &commander_pool(),
            1_800_000_000,
            "https://example.test/commander".into(),
            "COMMANDER2",
        )
        .unwrap();
        assert_eq!(report.accepted.len(), 1);
        assert!(report.quarantined.is_empty());
        assert_eq!(report.accepted[0].deck.command_zone.len(), 1);
        assert_eq!(report.accepted[0].deck.mainboard_count(), 99);
    }

    #[test]
    fn getpaird_detail_fixture_preserves_normalized_published_archetype() {
        let request = MetagameAcquisitionRequest {
            source: "getpaird".into(),
            platform: "arena".into(),
            format: "standard".into(),
            commander: false,
            lookback_days: 30,
            api_key: None,
        };
        let report = parse_getpaird_detail(
            include_bytes!("../tests/fixtures/getpaird-standard-detail.json"),
            &request,
            &pool(),
            1_800_100_000,
            "Standard",
        )
        .unwrap();
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].archetype, "Mono-White");
        assert_eq!(report.accepted[0].deck.sideboard_count(), 4);
        assert_eq!(report.provenance.adapter, "getpaird-public-decks-v1");
    }

    #[test]
    fn edge_fixture_quarantines_each_failure_without_rejecting_schema_additions() {
        let request = MetagameAcquisitionRequest {
            source: "spicerack".into(),
            platform: "arena".into(),
            format: "standard".into(),
            commander: false,
            lookback_days: 30,
            api_key: None,
        };
        let report = parse_spicerack(
            include_bytes!("../tests/fixtures/spicerack-edge-cases.json"),
            &request,
            &pool(),
            1_800_000_000,
            "https://example.test/query".into(),
            "STANDARD",
        )
        .unwrap();
        assert_eq!(report.accepted.len(), 1);
        for reason in [
            QuarantineReason::StaleEvent,
            QuarantineReason::FormatMismatch,
            QuarantineReason::AmbiguousDecklist,
            QuarantineReason::IllegalDeck,
        ] {
            assert!(report.quarantined.iter().any(|item| item.reason == reason));
        }
    }

    #[test]
    fn missing_required_source_fields_fail_the_whole_response() {
        let request = MetagameAcquisitionRequest {
            source: "spicerack".into(),
            platform: "arena".into(),
            format: "standard".into(),
            commander: false,
            lookback_days: 30,
            api_key: None,
        };
        assert!(
            parse_spicerack(
                br#"[{"TID":"broken","format":"STANDARD","standings":[]}]"#,
                &request,
                &pool(),
                1_800_000_000,
                "https://example.test/query".into(),
                "STANDARD",
            )
            .is_err()
        );
    }
}
