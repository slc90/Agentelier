use crate::{
    attention::{self, Attention},
    consolidation::sha256,
    experience::{self, Experience},
    observation::{Anchor, Category, EvidenceKind, Observation, Scope, State},
    revision::{Action, Revision, RevisionScope},
};
use agentelier_conversation::SessionId;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

const MAX_OBSERVATIONS: usize = 64;
const MAX_PROJECT_SOURCES: usize = 12;
const MAX_PUBLISHED: usize = 72;
const PERSONAL_NAVIGATION_BYTES: usize = 2 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Explicit,
    Tentative,
    Supported,
    Disputed,
    Inactive,
    Corrected,
    Forgotten,
    Superseded,
    Historical,
}

impl Status {
    pub(crate) fn usable(self) -> bool {
        matches!(self, Self::Explicit | Self::Tentative | Self::Supported)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Explicit => "明确陈述",
            Self::Tentative => "暂定",
            Self::Supported => "已有支持",
            Self::Disputed => "有争议",
            Self::Inactive => "已停用",
            Self::Corrected => "已纠正",
            Self::Forgotten => "已撤下",
            Self::Superseded => "已替代",
            Self::Historical => "历史情境",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Provenance {
    Observation {
        anchor: Anchor,
        evidence: EvidenceKind,
    },
    Revision {
        note: String,
        turn: u64,
        input: u64,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) category: Category,
    pub(crate) scope: Scope,
    pub(crate) session: SessionId,
    pub(crate) at_ms: u64,
    pub(crate) state: Status,
    pub(crate) source: Provenance,
    pub(crate) supersedes: Option<String>,
    pub(crate) body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attention: Option<Attention>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) experience: Option<Experience>,
}

impl Entry {
    pub(crate) fn visible_from(&self, directory: Option<&Path>) -> bool {
        match directory {
            None => true,
            Some(directory) if self.state.usable() => match &self.scope {
                Scope::Global => true,
                Scope::Project { directory: owner } => directory == Path::new(owner),
                Scope::Context => false,
            },
            Some(_) => false,
        }
    }

    pub(crate) fn artifact(&self) -> io::Result<(String, String, String)> {
        let metadata = serde_json::to_string(&self).map_err(io::Error::other)?;
        let content = format!(
            "# {}\n\n- 状态：{}\n- 来源会话：{}\n\n{}\n\n```json\n{metadata}\n```\n",
            self.category.label(),
            self.state.label(),
            self.session,
            self.body
        );
        let hash = sha256(&content);
        let name = format!("{}-{hash}.md", self.session);
        Ok((name, hash, content))
    }

    pub(crate) fn sequence(&self) -> Option<u64> {
        match &self.source {
            Provenance::Observation { anchor, .. } => Some(anchor.sequence),
            Provenance::Revision { .. } => None,
        }
    }
}

pub(crate) fn inputs(
    connection: &Connection,
    sessions: &BTreeSet<String>,
) -> io::Result<(String, Vec<Observation>)> {
    let mut select = connection
        .prepare(
            "SELECT s.session_id, o.observations FROM source_observations o
         JOIN sources s ON s.session_id = o.session_id
         WHERE s.status IN ('summary', 'empty') ORDER BY s.activity_ms DESC, s.session_id",
        )
        .map_err(io::Error::other)?;
    let mut rows = select.query([]).map_err(io::Error::other)?;
    let mut observations = Vec::new();
    let mut admitted = 0;
    while admitted < MAX_PROJECT_SOURCES
        && observations.len() < MAX_OBSERVATIONS
        && let Some(row) = rows.next().map_err(io::Error::other)?
    {
        let id: String = row.get(0).map_err(io::Error::other)?;
        if sessions.contains(&id) {
            let records: String = row.get(1).map_err(io::Error::other)?;
            let records: Vec<Observation> =
                serde_json::from_str(&records).map_err(io::Error::other)?;
            if records
                .iter()
                .any(|record| record.session.to_string() != id)
            {
                return Err(io::Error::other("分类记忆的来源会话不一致"));
            }
            observations.extend(records);
            admitted += 1;
        }
    }
    observations.sort_by(|left, right| {
        right
            .source
            .at_ms
            .cmp(&left.source.at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    observations.truncate(MAX_OBSERVATIONS);
    experience::retain_objections(connection, sessions, &mut observations)?;
    let serialized = serde_json::to_string(&observations).map_err(io::Error::other)?;
    Ok((sha256(&serialized), observations))
}

pub(crate) fn affected(note: &Revision, scope: &Scope) -> bool {
    note.action != Action::Remember
        && match (&note.scope, scope) {
            (RevisionScope::Global, _) => true,
            (RevisionScope::Project { directory: scope }, Scope::Project { directory }) => {
                Path::new(scope) == Path::new(directory)
            }
            _ => false,
        }
}

pub(crate) fn revision_scope(note: &Revision) -> Scope {
    match &note.scope {
        RevisionScope::Global => Scope::Global,
        RevisionScope::Project { directory } => Scope::Project {
            directory: directory.clone(),
        },
    }
}

fn exact_match(note: &Revision, body: &str) -> bool {
    body.to_lowercase().contains(&note.target.to_lowercase())
}

pub(crate) fn saved(
    connection: &Connection,
    record: Option<&str>,
) -> io::Result<Vec<(String, String, Entry)>> {
    let mut statement = connection
        .prepare(
            "SELECT file_name, content_hash, metadata FROM published_classified
         WHERE ?1 IS NULL OR file_name = ?1 ORDER BY file_name",
        )
        .map_err(io::Error::other)?;
    let mut rows = statement.query([record]).map_err(io::Error::other)?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next().map_err(io::Error::other)? {
        let name: String = row.get(0).map_err(io::Error::other)?;
        let hash: String = row.get(1).map_err(io::Error::other)?;
        let raw: String = row.get(2).map_err(io::Error::other)?;
        let entry: Entry = serde_json::from_str(&raw).map_err(io::Error::other)?;
        let (expected_name, expected_hash, _) = entry.artifact()?;
        if expected_name != name || expected_hash != hash {
            return Err(io::Error::other("分类记忆索引校验失败"));
        }
        entries.push((name, hash, entry));
    }
    entries.sort_by(|(_, _, left), (_, _, right)| {
        right
            .at_ms
            .cmp(&left.at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(entries)
}

pub(crate) fn classify(
    observations: &[Observation],
    notes: &[Revision],
    excluded: &BTreeSet<String>,
    ended_attention: &BTreeSet<String>,
) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut replacements = BTreeSet::new();
    for item in observations {
        let index = notes
            .iter()
            .rposition(|note| {
                affected(note, &item.scope)
                    && (exact_match(note, &item.body)
                        || item.attention.as_ref().is_some_and(|attention| {
                            exact_match(note, &attention.topic)
                                || exact_match(note, &attention.question)
                                || attention
                                    .reason
                                    .as_deref()
                                    .is_some_and(|reason| exact_match(note, reason))
                        })
                        || item.experience.as_ref().is_some_and(|experience| {
                            exact_match(note, &experience.topic)
                                || exact_match(note, &experience.method)
                                || exact_match(note, &experience.conditions)
                                || exact_match(note, &experience.reason)
                        }))
            })
            .or_else(|| {
                excluded
                    .contains(&item.id)
                    .then(|| notes.iter().rposition(|note| affected(note, &item.scope)))
                    .flatten()
            });
        let state = match index.map(|index| notes[index].action) {
            Some(Action::Forget) => Status::Forgotten,
            Some(Action::Correct) => Status::Corrected,
            _ if item.category == Category::ExperienceClue && item.experience.is_none() => {
                Status::Historical
            }
            _ if item.category == Category::AttentionClue && item.attention.is_none() => {
                Status::Historical
            }
            _ if item.scope == Scope::Context
                && item.category != Category::OriginSelfObservation =>
            {
                Status::Historical
            }
            _ => match item.state {
                State::Explicit => Status::Explicit,
                State::Tentative => Status::Tentative,
                State::Observed => Status::Supported,
            },
        };
        let supersedes = item.supersedes.clone().filter(|_| index.is_none());
        replacements.extend(supersedes.iter().cloned());
        let body = if index.is_some() {
            String::new()
        } else {
            item.body.clone()
        };
        entries.push(Entry {
            id: item.id.clone(),
            category: item.category,
            scope: item.scope.clone(),
            session: item.session,
            at_ms: item.source.at_ms,
            state,
            source: Provenance::Observation {
                anchor: item.source.clone(),
                evidence: item.evidence,
            },
            supersedes,
            body,
            attention: item.attention.clone().filter(|_| index.is_none()),
            experience: item.experience.clone().filter(|_| index.is_none()),
        });
    }
    for (index, note) in notes.iter().enumerate() {
        let Some(body) = (match note.action {
            Action::Remember => Some(&note.target),
            Action::Correct => note.replacement.as_ref(),
            Action::Forget => None,
        }) else {
            continue;
        };
        let scope = revision_scope(note);
        // Apply later corrections to text saved by explicit earlier revisions as well.
        if excluded.contains(&note.id)
            || notes[index + 1..]
                .iter()
                .any(|later| affected(later, &scope) && exact_match(later, body))
        {
            continue;
        }
        let id = note.id.clone();
        entries.push(Entry {
            id,
            category: Category::UserRevision,
            scope,
            session: note.session_id,
            at_ms: note.at_ms,
            state: Status::Explicit,
            source: Provenance::Revision {
                note: note.id.clone(),
                turn: note.turn,
                input: note.input,
            },
            supersedes: None,
            body: body.clone(),
            attention: None,
            experience: None,
        });
    }
    let mut support: BTreeMap<(&str, &str), BTreeSet<SessionId>> = BTreeMap::new();
    for entry in &mut entries {
        if entry.state.usable() && replacements.contains(&entry.id) {
            entry.state = Status::Superseded;
        }
        if entry.category == Category::OriginSelfObservation
            && entry.state == Status::Tentative
            && let Scope::Project { directory } = &entry.scope
        {
            support
                .entry((&entry.body, directory))
                .or_default()
                .insert(entry.session);
        }
    }
    let supported = support
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|((body, directory), _)| (body.to_owned(), directory.to_owned()))
        .collect::<BTreeSet<_>>();
    for entry in &mut entries {
        if entry.category == Category::OriginSelfObservation
            && entry.state == Status::Tentative
            && let Scope::Project { directory } = &entry.scope
            && supported.contains(&(entry.body.clone(), directory.clone()))
        {
            entry.state = Status::Supported;
        }
    }
    attention::apply(&mut entries, ended_attention);
    experience::apply(&mut entries);
    entries.sort_by(|left, right| {
        right
            .at_ms
            .cmp(&left.at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    entries.truncate(MAX_PUBLISHED);
    entries
}

pub(crate) fn personal_navigation(entries: &[Entry]) -> io::Result<String> {
    let mut navigation = String::new();
    for entry in entries.iter().filter(|entry| {
        entry.state.usable()
            && entry.scope == Scope::Global
            && entry.attention.is_none()
            && entry.experience.is_none()
    }) {
        let (name, _, _) = entry.artifact()?;
        let body = entry.body.replace(['\r', '\n'], " ");
        let line = format!(
            "- {} · {} · {} [来源](rollout_summaries/{name})\n",
            entry.category.label(),
            entry.state.label(),
            body
        );
        if navigation.len() + line.len() > PERSONAL_NAVIGATION_BYTES {
            break;
        }
        navigation.push_str(&line);
    }
    if !navigation.is_empty() {
        navigation.insert_str(0, "## 全局个人记忆\n\n");
    }
    Ok(navigation)
}
