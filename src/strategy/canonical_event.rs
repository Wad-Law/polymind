//! canonical_event.rs
//!
//! Semantic layer on top of EventFeatures:
//!   - infer domain (politics / macro / finance / crypto / sports / entertainment / other)
//!   - infer event kind
//!   - map entities into roles
//!   - classify numeric tokens
//!   - reuse coarse TimeWindow

use crate::strategy::event_features::{Entity, EntityKind, EventFeatures, NumberToken, TimeWindow};
use crate::strategy::tokenization::TokenizedNews;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDomain {
    Politics,
    MacroEconomy,
    Finance,
    Crypto,
    Sports,
    Entertainment,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    // Politics
    ElectionResult,
    PollUpdate,
    PolicyAnnouncement,
    ConflictOrWar,
    TreatyOrDeal,

    // Macro / economy / finance / crypto
    RateDecision,
    InflationPrint,
    JobsReport,
    EarningsReport,
    RegulatoryAction,
    AssetPriceMove,
    DefaultOrBankruptcy,
    UpgradeDowngrade,

    // Sports
    MatchResult,
    Injury,
    TransferOrTrade,
    TournamentProgress,

    // Entertainment / celebrities
    ReleaseOrAnnouncement,
    AwardResult,
    LegalIssueOrScandal,

    // Fallback
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityRole {
    PrimaryActor,
    Counterparty,
    Jurisdiction,
    Instrument,
    Organization,
    Other,
}

#[derive(Debug, Clone)]
pub struct CanonicalEntity {
    pub role: EntityRole,
    pub kind: EntityKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberKind {
    Level,
    Change,
    Score,
    Votes,
    Probability,
    Volume,
    Other,
}

#[derive(Debug, Clone)]
pub struct NumberFeature {
    pub kind: NumberKind,
    pub raw: String,
    pub value: f64,
    pub unit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CanonicalEvent {
    pub domain: EventDomain,
    pub kind: EventKind,
    pub primary_entities: Vec<CanonicalEntity>,
    pub secondary_entities: Vec<CanonicalEntity>,
    pub numbers: Vec<NumberFeature>,
    pub time_window: Option<TimeWindow>,
    pub location: Option<String>, // country / league / region
}

/// Simple keyword-based dictionaries for semantic classification.
/// Extend or load from config as needed.
#[derive(Debug, Clone)]
pub struct CanonicalDictionaries {
    pub macro_keywords: Vec<&'static str>,
    pub crypto_keywords: Vec<&'static str>,
    pub political_keywords: Vec<&'static str>,
    pub sports_keywords: Vec<&'static str>,
    pub entertainment_keywords: Vec<&'static str>,
}

impl Default for CanonicalDictionaries {
    fn default() -> Self {
        Self {
            macro_keywords: vec![
                "inflation", "cpi", "gdp", "unemployment",
                "payrolls", "pmi", "rates", "interest rate",
            ],
            crypto_keywords: vec![
                "bitcoin", "btc", "ether", "eth", "crypto",
            ],
            political_keywords: vec![
                "election", "vote", "poll", "runoff", "ballot", "campaign",
            ],
            sports_keywords: vec![
                "defeats", "beats", "wins", "loses", "draw",
                "final", "semifinal", "quarterfinal",
                "match", "game", "score", "tournament",
            ],
            entertainment_keywords: vec![
                "album", "single", "movie", "film", "series", "season",
                "tour", "concert", "oscars", "emmys", "grammys", "award",
                "premiere", "release", "drops",
            ],
        }
    }
}

/// Turns low-level EventFeatures into a semantic CanonicalEvent.
pub struct CanonicalEventBuilder {
    dict: CanonicalDictionaries,
}

impl CanonicalEventBuilder {
    pub fn new() -> Self {
        Self {
            dict: CanonicalDictionaries::default(),
        }
    }

    pub fn with_dict(dict: CanonicalDictionaries) -> Self {
        Self { dict }
    }

    pub fn build(&self, tok: &TokenizedNews, feat: &EventFeatures) -> CanonicalEvent {
        let text = tok.normalized.as_str();
        let lower = text.to_lowercase();

        let domain = self.infer_domain(&lower, feat);
        let kind = self.infer_event_kind(domain, &lower, feat);

        let (primary_entities, secondary_entities, location) =
            self.map_entities(domain, &feat.entities);

        let numbers = self.map_numbers(domain, &feat.numbers, &lower);

        CanonicalEvent {
            domain,
            kind,
            primary_entities,
            secondary_entities,
            numbers,
            time_window: feat.time_window.clone(),
            location,
        }
    }

    fn infer_domain(&self, lower: &str, feat: &EventFeatures) -> EventDomain {
        let has_cbank = feat
            .entities
            .iter()
            .any(|e| matches!(e.kind, EntityKind::CentralBank));

        let has_macro_kw = self.dict.macro_keywords.iter().any(|kw| lower.contains(kw));
        let has_crypto_kw = self.dict.crypto_keywords.iter().any(|kw| lower.contains(kw));

        if has_crypto_kw {
            return EventDomain::Crypto;
        }
        if has_cbank || has_macro_kw {
            return EventDomain::MacroEconomy;
        }

        let has_politics_kw = self.dict.political_keywords.iter().any(|kw| lower.contains(kw));
        let has_person = feat
            .entities
            .iter()
            .any(|e| matches!(e.kind, EntityKind::Person));
        if has_politics_kw || has_person {
            return EventDomain::Politics;
        }

        let has_sports_kw = self.dict.sports_keywords.iter().any(|kw| lower.contains(kw));
        let has_team = feat
            .entities
            .iter()
            .any(|e| matches!(e.kind, EntityKind::SportsTeam));
        if has_sports_kw || has_team {
            return EventDomain::Sports;
        }

        let has_ent_kw = self.dict.entertainment_keywords.iter().any(|kw| lower.contains(kw));
        let has_celebrity = feat
            .entities
            .iter()
            .any(|e| matches!(e.kind, EntityKind::Celebrity));
        if has_ent_kw || has_celebrity {
            return EventDomain::Entertainment;
        }

        let has_ticker_or_company = feat.entities.iter().any(|e| {
            matches!(e.kind, EntityKind::Ticker | EntityKind::Company)
        });
        if has_ticker_or_company {
            return EventDomain::Finance;
        }

        EventDomain::Other
    }

    fn infer_event_kind(
        &self,
        domain: EventDomain,
        lower: &str,
        _feat: &EventFeatures,
    ) -> EventKind {
        match domain {
            EventDomain::MacroEconomy => {
                if lower.contains("rate") || lower.contains("hike") || lower.contains("cut") {
                    return EventKind::RateDecision;
                }
                if lower.contains("inflation") || lower.contains("cpi") {
                    return EventKind::InflationPrint;
                }
                if lower.contains("jobs")
                    || lower.contains("payrolls")
                    || lower.contains("unemployment")
                {
                    return EventKind::JobsReport;
                }
                if lower.contains("downgrade") || lower.contains("upgrade") {
                    return EventKind::UpgradeDowngrade;
                }
                if lower.contains("default") || lower.contains("bankruptcy") {
                    return EventKind::DefaultOrBankruptcy;
                }
                EventKind::Other
            }

            EventDomain::Finance | EventDomain::Crypto => {
                if lower.contains("sec") || lower.contains("regulator") || lower.contains("ban") {
                    return EventKind::RegulatoryAction;
                }
                if lower.contains("earnings") || lower.contains("results") {
                    return EventKind::EarningsReport;
                }
                if lower.contains("surges")
                    || lower.contains("plunges")
                    || lower.contains("rallies")
                    || lower.contains("slumps")
                    || lower.contains("hits")
                    || lower.contains("record")
                {
                    return EventKind::AssetPriceMove;
                }
                EventKind::Other
            }

            EventDomain::Politics => {
                if lower.contains("election")
                    || lower.contains("wins")
                    || lower.contains("concedes")
                {
                    return EventKind::ElectionResult;
                }
                if lower.contains("poll") || lower.contains("survey") {
                    return EventKind::PollUpdate;
                }
                if lower.contains("sanction")
                    || lower.contains("tariff")
                    || lower.contains("policy")
                    || lower.contains("law")
                {
                    return EventKind::PolicyAnnouncement;
                }
                if lower.contains("attack")
                    || lower.contains("invasion")
                    || lower.contains("war")
                {
                    return EventKind::ConflictOrWar;
                }
                EventKind::Other
            }

            EventDomain::Sports => {
                if lower.contains("defeats")
                    || lower.contains("beats")
                    || lower.contains("wins")
                    || lower.contains("loses")
                {
                    return EventKind::MatchResult;
                }
                if lower.contains("injury") || lower.contains("sidelined") {
                    return EventKind::Injury;
                }
                if lower.contains("transfer")
                    || lower.contains("signs for")
                    || lower.contains("trade")
                    || lower.contains("acquires")
                {
                    return EventKind::TransferOrTrade;
                }
                if lower.contains("final")
                    || lower.contains("semifinal")
                    || lower.contains("quarterfinal")
                    || lower.contains("title")
                {
                    return EventKind::TournamentProgress;
                }
                EventKind::Other
            }

            EventDomain::Entertainment => {
                if lower.contains("releases")
                    || lower.contains("drops")
                    || lower.contains("premieres")
                    || lower.contains("launches")
                {
                    return EventKind::ReleaseOrAnnouncement;
                }
                if lower.contains("wins") && lower.contains("award") {
                    return EventKind::AwardResult;
                }
                if lower.contains("lawsuit")
                    || lower.contains("charged")
                    || lower.contains("arrested")
                    || lower.contains("scandal")
                {
                    return EventKind::LegalIssueOrScandal;
                }
                EventKind::Other
            }

            EventDomain::Other => EventKind::Other,
        }
    }

    fn map_entities(
        &self,
        domain: EventDomain,
        entities: &[Entity],
    ) -> (Vec<CanonicalEntity>, Vec<CanonicalEntity>, Option<String>) {
        let mut primary = Vec::new();
        let mut secondary = Vec::new();
        let mut location: Option<String> = None;

        for e in entities {
            let role = match (domain, e.kind) {
                (EventDomain::MacroEconomy | EventDomain::Finance | EventDomain::Crypto, EntityKind::CentralBank) =>
                    EntityRole::PrimaryActor,
                (EventDomain::MacroEconomy | EventDomain::Finance | EventDomain::Crypto, EntityKind::Ticker) =>
                    EntityRole::Instrument,
                (EventDomain::Politics, EntityKind::Person) =>
                    EntityRole::PrimaryActor,
                (EventDomain::Politics, EntityKind::Organization) =>
                    EntityRole::Organization,
                (EventDomain::Sports, EntityKind::SportsTeam) =>
                    EntityRole::PrimaryActor,
                (EventDomain::Sports, EntityKind::League) =>
                    EntityRole::Organization,
                (EventDomain::Entertainment, EntityKind::Celebrity) =>
                    EntityRole::PrimaryActor,
                (EventDomain::Entertainment, EntityKind::Organization) =>
                    EntityRole::Organization,
                (_, EntityKind::Country) => {
                    location = Some(e.value.clone());
                    EntityRole::Jurisdiction
                }
                _ => EntityRole::Other,
            };

            let ce = CanonicalEntity {
                role,
                kind: e.kind,
                value: e.value.clone(),
            };

            match role {
                EntityRole::PrimaryActor | EntityRole::Instrument | EntityRole::Jurisdiction =>
                    primary.push(ce),
                _ => secondary.push(ce),
            }
        }

        (primary, secondary, location)
    }

    fn map_numbers(
        &self,
        domain: EventDomain,
        nums: &[NumberToken],
        lower: &str,
    ) -> Vec<NumberFeature> {
        nums.iter()
            .map(|n| {
                let kind = self.classify_number(domain, n, lower);
                NumberFeature {
                    kind,
                    raw: n.raw.clone(),
                    value: n.value,
                    unit: n.unit.clone(),
                }
            })
            .collect()
    }

    fn classify_number(
        &self,
        domain: EventDomain,
        n: &NumberToken,
        lower: &str,
    ) -> NumberKind {
        match domain {
            EventDomain::MacroEconomy | EventDomain::Finance | EventDomain::Crypto => {
                if let Some(unit) = &n.unit {
                    if unit == "%" {
                        if lower.contains("inflation") || lower.contains("cpi") {
                            return NumberKind::Level;
                        }
                        if lower.contains("rate") || lower.contains("yield") {
                            return NumberKind::Level;
                        }
                        if lower.contains("poll") || lower.contains("approval") {
                            return NumberKind::Probability;
                        }
                    }
                    if unit == "bps" {
                        return NumberKind::Change;
                    }
                    if unit == "plain" && lower.contains("volume") {
                        return NumberKind::Volume;
                    }
                }
                NumberKind::Other
            }

            EventDomain::Politics => {
                if let Some(unit) = &n.unit {
                    if unit == "%" && (lower.contains("poll") || lower.contains("approval")) {
                        return NumberKind::Probability;
                    }
                    if unit == "plain" && (lower.contains("votes") || lower.contains("electoral")) {
                        return NumberKind::Votes;
                    }
                }
                NumberKind::Other
            }

            EventDomain::Sports => {
                if n.raw.contains('-') || n.raw.contains('–') {
                    return NumberKind::Score;
                }
                NumberKind::Other
            }

            EventDomain::Entertainment | EventDomain::Other => NumberKind::Other,
        }
    }
}