use crate::{
    classified::{Entry, Provenance, Status},
    observation::{Anchor, Evidence, EvidenceKind, Location, Observation, Scope, topic_identity},
    redaction::Redactor,
};
use agentelier_conversation::{SessionId, TurnId};
use agentelier_tools::ToolStatus;
use regex::Regex;
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io;
use std::sync::LazyLock;

const MAX_TEXT_BYTES: usize = 520;
const MAX_TOPIC_BYTES: usize = 120;
const MAX_REFS: usize = 6;

static AFFIRMED: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:i confirm|i prefer|explicitly confirm|keep using|worked well|worked for|was useful|helped|verified|validated|met (?:the|our) criteria)\b|我(?:确认|认可|偏好)|以后继续用|确实有效|实际验证|这个做法有帮助|符合预期",
    )
});
static CONFIRMED: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:i (?:explicitly )?confirm|i prefer|keep using|our standard for|adopt this method|use this method for)\b|我(?:明确)?(?:确认|认可|偏好)|以后(?:都)?继续用|正式采用",
    )
});
static OPPOSING: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:counterexample|did not work|didn['’]t work|failed|wrong|no benefit|no useful|made it worse|stop using|retire|replace|adds? (?:unnecessary )?delay)\b|反例|无效|失败了|效果更差|不适用|没有帮助|停止采用|更换为|替代",
    )
});
static STOPPED: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:stop using|reject this method|retire this method|withdraw|replace.+with|use.+instead of)\b|停止采用|停用|撤下|不再采用|正式替代|改用",
    )
});

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Feedback {
    Proposal,
    Positive,
    Confirmed,
    Outcome,
    Counterexample,
    Stop,
    Replacement,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExperienceState {
    #[default]
    Candidate,
    Supported,
    Disputed,
    Inactive,
    Superseded,
}

#[derive(Clone, Serialize, Deserialize)]
struct Reference {
    observation: String,
    session: SessionId,
    anchor: Anchor,
    evidence: EvidenceKind,
    feedback: Feedback,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Experience {
    pub(crate) id: String,
    pub(crate) topic: String,
    pub(crate) method: String,
    pub(crate) conditions: String,
    pub(crate) reason: String,
    input: TurnId,
    feedback: Feedback,
    pub(crate) state: ExperienceState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    support: Vec<Reference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    counterexamples: Vec<Reference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replaces: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draft {
    topic: String,
    method: String,
    conditions: String,
    reason: String,
    feedback: Feedback,
    replaces_topic: Option<String>,
}

impl Draft {
    pub(crate) fn validate(
        &self,
        evidence: &Evidence,
        kind: EvidenceKind,
        scope: &Scope,
        redactor: &Redactor,
    ) -> Option<Experience> {
        let anchor = evidence.anchor.as_ref()?;
        let user = kind == EvidenceKind::UserStatement;
        let contains = |pattern: &LazyLock<Result<Regex, regex::Error>>| {
            pattern
                .as_ref()
                .is_ok_and(|pattern| pattern.is_match(&evidence.text))
        };
        let feedback = match self.feedback {
            Feedback::Confirmed if user && contains(&CONFIRMED) => Feedback::Confirmed,
            Feedback::Positive if user && contains(&AFFIRMED) => Feedback::Positive,
            Feedback::Outcome
                if kind == EvidenceKind::ToolObservation
                    && matches!(
                        &anchor.location,
                        Location::Tool {
                            status: ToolStatus::Succeeded,
                            ..
                        }
                    ) =>
            {
                Feedback::Outcome
            }
            Feedback::Counterexample if user && contains(&OPPOSING) => Feedback::Counterexample,
            Feedback::Stop if user && contains(&STOPPED) => Feedback::Stop,
            Feedback::Replacement if user && contains(&STOPPED) => Feedback::Replacement,
            Feedback::Stop | Feedback::Counterexample | Feedback::Replacement => return None,
            _ => Feedback::Proposal,
        };
        let topic = clean(&self.topic, MAX_TOPIC_BYTES, redactor)?;
        let id = topic_identity(scope, &topic)?;
        let replaces = if feedback == Feedback::Replacement {
            let older = clean(self.replaces_topic.as_deref()?, MAX_TOPIC_BYTES, redactor)?;
            let older = topic_identity(scope, &older)?;
            if older == id {
                return None;
            }
            Some(older)
        } else {
            None
        };
        Some(Experience {
            id,
            topic,
            feedback,
            replaces,
            method: clean(&self.method, MAX_TEXT_BYTES, redactor)?,
            conditions: clean(&self.conditions, MAX_TEXT_BYTES, redactor)?,
            reason: clean(&self.reason, MAX_TEXT_BYTES, redactor)?,
            input: anchor.input.unwrap_or(anchor.turn),
            state: ExperienceState::Candidate,
            support: Vec::new(),
            counterexamples: Vec::new(),
        })
    }
}

fn clean(text: &str, limit: usize, redactor: &Redactor) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > limit || text.contains('\0') {
        return None;
    }
    Some(redactor.clean(text, limit))
}

impl Experience {
    pub(crate) fn related(&self, input: &str) -> bool {
        crate::observation::topic_related(&self.topic, &self.method, input)
    }

    pub(crate) fn navigation(&self) -> String {
        format!(
            "{} · 已有支持 · {} · 适用条件：{}",
            self.topic, self.method, self.conditions
        )
        .replace(['\r', '\n'], " ")
    }
}

pub(crate) fn apply(entries: &mut [Entry]) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.state.usable()
            && let Some(record) = &entry.experience
        {
            groups.entry(record.id.clone()).or_default().push(index);
        }
    }
    let mut selected = BTreeMap::new();
    for (id, indexes) in &mut groups {
        indexes.sort_by_key(|&index| {
            let entry = &entries[index];
            (entry.at_ms, entry.sequence().unwrap_or(0), entry.id.clone())
        });
        let Some((&current, older)) = indexes.split_last() else {
            continue;
        };
        let mut supporting_inputs = HashSet::new();
        let mut opposing_inputs = HashSet::new();
        let mut support = Vec::new();
        let mut counterexamples = Vec::new();
        let mut confirmed = false;
        let mut user_support = false;
        let mut stopped = false;
        for &index in indexes.iter() {
            let entry = &entries[index];
            let (Some(experience), Provenance::Observation { anchor, evidence }) =
                (&entry.experience, &entry.source)
            else {
                continue;
            };
            let reference = Reference {
                observation: entry.id.clone(),
                session: entry.session,
                anchor: anchor.clone(),
                evidence: *evidence,
                feedback: experience.feedback,
            };
            match experience.feedback {
                Feedback::Confirmed | Feedback::Replacement => {
                    confirmed = true;
                    if supporting_inputs.insert((entry.session, experience.input)) {
                        support.push(reference);
                    }
                }
                Feedback::Positive | Feedback::Outcome => {
                    user_support |= experience.feedback == Feedback::Positive;
                    if supporting_inputs.insert((entry.session, experience.input)) {
                        support.push(reference);
                    }
                }
                Feedback::Counterexample | Feedback::Stop => {
                    stopped |= experience.feedback == Feedback::Stop;
                    if opposing_inputs.insert((entry.session, experience.input)) {
                        counterexamples.push(reference);
                    }
                }
                Feedback::Proposal => {}
            }
        }
        let independent_sources = support
            .iter()
            .map(|evidence| evidence.session)
            .collect::<HashSet<_>>();
        let (state, status) = if stopped {
            (ExperienceState::Inactive, Status::Inactive)
        } else if !counterexamples.is_empty() {
            (ExperienceState::Disputed, Status::Disputed)
        } else if confirmed || user_support && independent_sources.len() >= 2 {
            (ExperienceState::Supported, Status::Supported)
        } else {
            (ExperienceState::Candidate, Status::Tentative)
        };
        for &index in older {
            entries[index].state = Status::Historical;
        }
        let replacement = indexes.iter().rev().find_map(|&index| {
            entries[index]
                .experience
                .as_ref()
                .and_then(|record| record.replaces.clone())
        });
        let entry = &mut entries[current];
        entry.state = status;
        if let Some(experience) = entry.experience.as_mut() {
            experience.state = state;
            experience.support = support.into_iter().rev().take(MAX_REFS).collect();
            experience.counterexamples = counterexamples.into_iter().rev().take(MAX_REFS).collect();
            experience.replaces = replacement;
        }
        selected.insert(id.clone(), current);
    }
    for &index in selected.values() {
        let older = entries[index]
            .experience
            .as_ref()
            .and_then(|item| item.replaces.as_ref())
            .and_then(|id| selected.get(id))
            .copied()
            .filter(|&older| {
                entries[older].at_ms < entries[index].at_ms
                    || (entries[older].at_ms == entries[index].at_ms
                        && entries[older].session == entries[index].session
                        && entries[older].experience.as_ref().is_some_and(|item| {
                            item.replaces.is_none() && item.feedback == Feedback::Stop
                        }))
            });
        if let Some(older) = older {
            entries[index].supersedes = Some(entries[older].id.clone());
            entries[older].state = Status::Superseded;
            if let Some(item) = entries[older].experience.as_mut() {
                item.state = ExperienceState::Superseded;
            }
        }
    }
}

pub(crate) fn remember_objections(
    tx: &Transaction<'_>,
    observations: &[Observation],
) -> rusqlite::Result<()> {
    for item in observations {
        if let Some(experience) = &item.experience
            && matches!(
                experience.feedback,
                Feedback::Counterexample | Feedback::Stop
            )
        {
            let data = serde_json::to_string(item)
                .map_err(|failure| rusqlite::Error::ToSqlConversionFailure(Box::new(failure)))?;
            let at = i64::try_from(item.source.at_ms)
                .map_err(|failure| rusqlite::Error::ToSqlConversionFailure(Box::new(failure)))?;
            tx.execute(
                "INSERT INTO experience_objections (topic_id, session_id, observation, observed_ms, stopped)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (topic_id, session_id) DO UPDATE SET observation = excluded.observation,
                    observed_ms = excluded.observed_ms, stopped = excluded.stopped
                 WHERE (excluded.stopped, excluded.observed_ms) >= (experience_objections.stopped, experience_objections.observed_ms)",
                params![experience.id, item.session.to_string(), data, at, experience.feedback == Feedback::Stop]
            )?;
        }
    }
    Ok(())
}

pub(crate) fn retain_objections(
    connection: &Connection,
    sessions: &BTreeSet<String>,
    observations: &mut Vec<Observation>,
) -> io::Result<()> {
    let relevant = observations
        .iter()
        .filter_map(|item| item.experience.as_ref().map(|value| value.id.clone()))
        .collect::<BTreeSet<_>>();
    if relevant.is_empty() {
        return Ok(());
    }
    let mut selected = BTreeSet::new();
    let mut statement = connection
        .prepare(
            "SELECT topic_id, session_id, observation FROM experience_objections
         ORDER BY stopped DESC, observed_ms DESC, topic_id, session_id",
        )
        .map_err(io::Error::other)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(io::Error::other)?;
    for row in rows {
        let (topic, session, data) = row.map_err(io::Error::other)?;
        if relevant.contains(&topic)
            && sessions.contains(&session)
            && selected.insert(topic.clone())
        {
            let item: Observation = serde_json::from_str(&data).map_err(io::Error::other)?;
            if item.session.to_string() != session
                || item
                    .experience
                    .as_ref()
                    .is_none_or(|value| value.id != topic)
            {
                return Err(io::Error::other("经验反例与来源不一致"));
            }
            if !observations.iter().any(|existing| existing.id == item.id) {
                observations.push(item);
            }
        }
    }
    Ok(())
}
