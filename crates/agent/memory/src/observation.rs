use crate::{attention, redaction::Redactor};
use agentelier_conversation::{SessionId, TurnId};
use agentelier_session_store::MemorySourceSnapshot;
use agentelier_tools::ToolStatus;
use regex::Regex;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::LazyLock;

const CANDIDATES: usize = 12;
const STORED_PER_SOURCE: usize = 48;
const BODY_BYTES: usize = 640;

static PATTERNS: LazyLock<Result<Patterns, regex::Error>> = LazyLock::new(|| {
    Ok(Patterns {
        global: Regex::new(
            r"(?i)\b(?:globally|global preference|standing preference|cross.project|across (?:all )?projects|(?:all|every) (?:project|repo|workspace)s?)\b|全局|跨项目|所有项目|每个项目|一直偏好",
        )?,
        temporary: Regex::new(
            r"(?i)\b(?:today|tonight|just this once|only this time|for now|this round)\b|今天|今晚|暂时|只限这次|仅限这次|仅这次|这一次",
        )?,
        fictional: Regex::new(concat!(
            r"(?im)(?:^|[.!?]\s+)[ \t]*(?:fictional story\b|make-believe\b|hypothetical scenario\b|",
            r"(?:please\s+)?(?:pretend\b|role.play\b)|(?:here is|this is)\b[^.!?\r\n]{0,60}\b(?:fictional|made.up|role.play|hypothetical)\b)",
            r"|(?:^|[。！？]\s*)\s*(?:(?:这(?:是|里有)(?:一(?:个|段|则))?\s*)?虚构(?:故事|情节|小故事)|",
            r"(?:请|来)?(?:假设情景|假想案例|角色扮演|编个故事))"
        ))?,
        correction: Regex::new(
            r"(?i)^(?:please\s+)?(?:correction|correct\b|i changed my mind\b)|^(?:请)?(?:更正|纠正|修正|我改主意了|我改口了)",
        )?,
    })
});

pub(crate) struct Patterns {
    global: Regex,
    temporary: Regex,
    pub(crate) fictional: Regex,
    correction: Regex,
}

pub(crate) fn patterns() -> io::Result<&'static Patterns> {
    PATTERNS.as_ref().map_err(io::Error::other)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    UserPreference,
    SharedExperience,
    OriginSelfObservation,
    WorkFact,
    ExperienceClue,
    AttentionClue,
    UserRevision,
}

impl Category {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UserPreference => "user_preference",
            Self::SharedExperience => "shared_experience",
            Self::OriginSelfObservation => "origin_self_observation",
            Self::WorkFact => "work_fact",
            Self::ExperienceClue => "experience_clue",
            Self::AttentionClue => "attention_clue",
            Self::UserRevision => "user_revision",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Subject {
    User,
    Shared,
    Origin,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Scope {
    Context,
    Project { directory: String },
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceKind {
    UserStatement,
    RootExpression,
    ToolObservation,
    Inference,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Explicit,
    Tentative,
    Observed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Location {
    Input,
    Followup { id: u64 },
    Reply { message: String, content: usize },
    Tool { call: String, status: ToolStatus },
}

impl Location {
    pub(crate) fn kind(&self) -> EvidenceKind {
        match self {
            Self::Input | Self::Followup { .. } => EvidenceKind::UserStatement,
            Self::Reply { .. } => EvidenceKind::RootExpression,
            Self::Tool { .. } => EvidenceKind::ToolObservation,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Anchor {
    pub(crate) turn: TurnId,
    pub(crate) sequence: u64,
    pub(crate) at_ms: u64,
    pub(crate) location: Location,
}

#[derive(Serialize)]
pub(crate) struct Evidence {
    pub(crate) role: &'static str,
    pub(crate) text: String,
    pub(crate) anchor: Option<Anchor>,
    pub(crate) fictional_context: bool,
    pub(crate) project: String,
    #[serde(skip)]
    pub(crate) work_directory: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct Observation {
    pub(crate) id: String,
    pub(crate) category: Category,
    subject: Subject,
    pub(crate) scope: Scope,
    pub(crate) body: String,
    pub(crate) session: SessionId,
    pub(crate) source: Anchor,
    pub(crate) evidence: EvidenceKind,
    pub(crate) state: State,
    pub(crate) supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attention: Option<attention::Attention>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScopeCandidate {
    Context,
    Project,
    Global,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    category: Category,
    scope: ScopeCandidate,
    body: String,
    evidence: usize,
    inference: bool,
    replaces: Option<usize>,
    #[serde(default)]
    attention: Option<attention::Draft>,
}

pub(crate) fn validated(
    source: &MemorySourceSnapshot,
    evidence: &[Evidence],
    candidates: Vec<Candidate>,
    redactor: &Redactor,
    patterns: &Patterns,
) -> Vec<Observation> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for candidate in candidates.into_iter().take(CANDIDATES) {
        let Some(item) = evidence.get(candidate.evidence) else {
            continue;
        };
        let Some(observation) = observation(source, item, evidence, &candidate, redactor, patterns)
        else {
            continue;
        };
        if seen.insert(observation.id.clone()) {
            let replaces = candidate.replaces.filter(|index| {
                *index < candidate.evidence
                    && observation.evidence == EvidenceKind::UserStatement
                    && patterns.correction.is_match(item.text.trim_start())
            });
            entries.push((candidate.evidence, replaces, observation));
        }
    }
    let mut prior = BTreeMap::<(usize, Category), usize>::new();
    entries.sort_by_key(|(index, _, _)| *index);
    for i in 0..entries.len() {
        let (index, replaces, observation) = &entries[i];
        let category = observation.category;
        let replacement = replaces
            .and_then(|index| prior.get(&(index, category)))
            .map(|&i| &entries[i].2)
            .filter(|older| {
                older.scope == observation.scope
                    && older.subject == observation.subject
                    && older.attention.as_ref().map(|attention| &attention.id)
                        == observation
                            .attention
                            .as_ref()
                            .map(|attention| &attention.id)
            })
            .map(|older| older.id.clone());
        prior.insert((*index, category), i);
        entries[i].2.supersedes = replacement;
    }
    entries
        .into_iter()
        .map(|(_, _, observation)| observation)
        .collect()
}

fn observation(
    source: &MemorySourceSnapshot,
    item: &Evidence,
    evidence: &[Evidence],
    candidate: &Candidate,
    redactor: &Redactor,
    patterns: &Patterns,
) -> Option<Observation> {
    let anchor = item.anchor.as_ref()?;
    let kind = anchor.location.kind();
    let category = candidate.category;
    if category == Category::UserRevision
        || item.fictional_context
        || candidate.inference
            && !matches!(category, Category::ExperienceClue | Category::AttentionClue)
        || category == Category::UserPreference && kind != EvidenceKind::UserStatement
        || category == Category::OriginSelfObservation
            && (!source.origin || kind != EvidenceKind::RootExpression)
        || kind == EvidenceKind::ToolObservation && category != Category::WorkFact
    {
        return None;
    }
    let body = candidate.body.trim();
    if body.is_empty() || body.len() > BODY_BYTES || body.contains('\0') {
        return None;
    }
    let scope = match candidate.scope {
        _ if category == Category::UserPreference && patterns.temporary.is_match(&item.text) => {
            Scope::Context
        }
        ScopeCandidate::Context => Scope::Context,
        ScopeCandidate::Global
            if matches!(category, Category::UserPreference | Category::AttentionClue)
                && kind == EvidenceKind::UserStatement
                && patterns.global.is_match(&item.text) =>
        {
            Scope::Global
        }
        ScopeCandidate::Project | ScopeCandidate::Global => Scope::Project {
            directory: item.work_directory.to_str()?.to_owned(),
        },
    };
    let subject = match category {
        Category::UserPreference => Subject::User,
        Category::UserRevision => Subject::User,
        Category::OriginSelfObservation => Subject::Origin,
        Category::WorkFact => Subject::Project,
        Category::SharedExperience | Category::ExperienceClue | Category::AttentionClue => {
            Subject::Shared
        }
    };
    let state = match &anchor.location {
        Location::Tool {
            status: ToolStatus::Succeeded,
            ..
        } => State::Observed,
        Location::Input | Location::Followup { .. } if !candidate.inference => State::Explicit,
        _ => State::Tentative,
    };
    let kind = if candidate.inference {
        EvidenceKind::Inference
    } else {
        kind
    };
    let attention = if category == Category::AttentionClue {
        Some(
            candidate
                .attention
                .as_ref()?
                .validate(evidence, item, kind, &scope, source, redactor)?,
        )
    } else {
        None
    };
    let id = match &attention {
        Some(attention) => serde_json::to_vec(&(
            source.session,
            anchor.turn,
            &anchor.location,
            category,
            &attention.id,
        )),
        None => serde_json::to_vec(&(source.session, anchor.turn, &anchor.location, category)),
    }
    .ok()?;
    Some(Observation {
        id: format!("{:x}", Sha256::digest(id)),
        category,
        subject,
        scope,
        body: redactor.clean(body, BODY_BYTES),
        session: source.session,
        source: anchor.clone(),
        evidence: kind,
        state,
        supersedes: None,
        attention,
    })
}

pub(crate) fn merge(
    tx: &Transaction<'_>,
    session: &str,
    observations: Vec<Observation>,
) -> rusqlite::Result<()> {
    let previous: Option<String> = tx
        .query_row(
            "SELECT observations FROM source_observations WHERE session_id = ?1",
            [session],
            |row| row.get(0),
        )
        .optional()?;
    let previous: Vec<Observation> = previous
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|failure| rusqlite::Error::ToSqlConversionFailure(Box::new(failure)))?
        .unwrap_or_default();
    let mut merged = previous
        .into_iter()
        .chain(observations)
        .map(|item| (item.id.clone(), item))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .source
            .sequence
            .cmp(&left.source.sequence)
            .then_with(|| left.id.cmp(&right.id))
    });
    merged.truncate(STORED_PER_SOURCE);
    let json = serde_json::to_string(&merged)
        .map_err(|failure| rusqlite::Error::ToSqlConversionFailure(Box::new(failure)))?;
    tx.execute(
        "INSERT INTO source_observations (session_id, observations) VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET observations = excluded.observations",
        params![session, json],
    )?;
    Ok(())
}
