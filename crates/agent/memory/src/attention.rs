use crate::{
    classified::{Entry, Status},
    consolidation::sha256,
    observation::{Anchor, Evidence, EvidenceKind, Observation, Scope},
    redaction::Redactor,
};
use agentelier_conversation::SessionId;
use agentelier_session_store::MemorySourceSnapshot;
use regex::Regex;
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::LazyLock;

const MAX_ACTIVE_PER_SCOPE: usize = 3;
const QUESTION_BYTES: usize = 480;
const TOPIC_BYTES: usize = 120;

static EXPRESSION: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:i(?:['’]m| am) curious|i(?:['’]d| would) like to (?:know|understand|see)|i wonder|i want to (?:explore|know|understand)|i(?:['’]m| am) interested)\b|我(?:还|也|很|更|最)?好奇|我(?:还|更)?想(?:知道|弄清|了解|看看|继续探索)|我很想|我对.+感兴趣",
    )
});
static END: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?im)(?:^|[.!?]\s+)[ \t]*(?:please\s+)?(?:close|end|retire|drop)\b|",
        r"\b(?:we (?:have )?(?:resolved|settled|closed)|is now (?:closed|resolved|settled))\b|",
        r"(?:请|明确|永久)?结束|解决了|撤下|不再关注|以后别再"
    ))
});
static REOPEN: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:please[ \t]+)?(?:reopen|resume this topic)\b|^(?:请)?重新(?:打开|关注|讨论)",
    )
});

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttentionState {
    Continue,
    Wait,
    End,
}

impl AttentionState {
    fn label(self) -> &'static str {
        match self {
            Self::Continue => "仍可继续",
            Self::Wait => "等待材料或新证据",
            Self::End => "已结束",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Author {
    User,
    Origin,
    Both,
    Unknown,
}

impl Author {
    fn label(self) -> &'static str {
        match self {
            Self::User => "用户提出",
            Self::Origin => "Origin 表达的关注",
            Self::Both => "双方提出",
            Self::Unknown => "提出者未确认",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Attention {
    pub(crate) id: String,
    pub(crate) topic: String,
    pub(crate) question: String,
    pub(crate) reason: Option<String>,
    pub(crate) author: Author,
    pub(crate) state: AttentionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) supporting_source: Option<Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason_source: Option<ReasonSource>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reopened: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct ReasonSource {
    session: SessionId,
    anchor: Anchor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draft {
    topic: String,
    question: String,
    reason: Option<String>,
    state: AttentionState,
    shared_with: Option<usize>,
}

impl Draft {
    pub(crate) fn validate(
        &self,
        items: &[Evidence],
        primary: &Evidence,
        kind: EvidenceKind,
        scope: &Scope,
        source: &MemorySourceSnapshot,
        redactor: &Redactor,
    ) -> Option<Attention> {
        let primary_anchor = primary.anchor.as_ref()?;
        let companion = self
            .shared_with
            .and_then(|id| items.get(id))
            .filter(|item| {
                item.anchor.as_ref().is_some_and(|other| {
                    other.turn == primary_anchor.turn
                        && other.location.kind() != primary_anchor.location.kind()
                })
            });
        let declared_interest = |item: &Evidence| {
            item.anchor
                .as_ref()
                .is_some_and(|anchor| anchor.location.kind() == EvidenceKind::RootExpression)
                && EXPRESSION
                    .as_ref()
                    .is_ok_and(|pattern| pattern.is_match(&item.text))
        };
        let reason_anchor = source
            .origin
            .then(|| {
                if declared_interest(primary) {
                    primary.anchor.clone()
                } else {
                    companion
                        .filter(|item| declared_interest(item))
                        .and_then(|item| item.anchor.clone())
                }
            })
            .flatten();
        let own_interest = reason_anchor.is_some();
        let user_statement = kind == EvidenceKind::UserStatement
            || companion
                .and_then(|item| item.anchor.as_ref())
                .is_some_and(|anchor| anchor.location.kind() == EvidenceKind::UserStatement);
        let author = match (kind, own_interest, user_statement) {
            (EvidenceKind::Inference, _, _) => Author::Unknown,
            (_, true, true) => Author::Both,
            (_, true, false) => Author::Origin,
            (EvidenceKind::UserStatement, false, _) => Author::User,
            _ => Author::Unknown,
        };
        if self.state == AttentionState::End
            && (kind != EvidenceKind::UserStatement
                || !END
                    .as_ref()
                    .is_ok_and(|pattern| pattern.is_match(&primary.text)))
        {
            return None;
        }
        let topic = clean(&self.topic, TOPIC_BYTES, redactor)?;
        let question = clean(&self.question, QUESTION_BYTES, redactor)?;
        let key = topic
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if key.is_empty() {
            return None;
        }
        let scope_key = serde_json::to_string(scope).ok()?;
        let reason = own_interest
            .then_some(self.reason.as_deref())
            .flatten()
            .and_then(|reason| clean(reason, QUESTION_BYTES, redactor));
        let reason_source = reason
            .as_ref()
            .and(reason_anchor)
            .map(|anchor| ReasonSource {
                session: source.session,
                anchor,
            });
        Some(Attention {
            id: sha256(&format!("{scope_key}\n{key}")),
            topic,
            question,
            reason,
            author,
            state: self.state,
            supporting_source: companion.and_then(|item| item.anchor.clone()),
            reason_source,
            reopened: kind == EvidenceKind::UserStatement
                && REOPEN
                    .as_ref()
                    .is_ok_and(|pattern| pattern.is_match(primary.text.trim_start())),
        })
    }
}

fn clean(text: &str, bytes: usize, redactor: &Redactor) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > bytes || text.contains('\0') {
        return None;
    }
    Some(redactor.clean(text, bytes))
}

impl Attention {
    pub(crate) fn related(&self, input: &str) -> bool {
        let lower = input.to_lowercase();
        if [
            "what are you curious about",
            "what would you like to explore",
            "what interests you",
            "choose a question to explore",
            "你想探索什么",
            "你对什么好奇",
            "你最关心什么",
            "自己选一个问题",
        ]
        .iter()
        .any(|phrase| lower.contains(phrase))
        {
            return true;
        }
        let unique = |value: &str| {
            let mut terms = BTreeSet::new();
            for word in value.split(|character: char| !character.is_alphanumeric()) {
                if word.is_ascii() && word.len() >= 4 {
                    let lower = word.to_ascii_lowercase();
                    if !matches!(
                        lower.as_str(),
                        "could"
                            | "what"
                            | "want"
                            | "about"
                            | "from"
                            | "please"
                            | "your"
                            | "that"
                            | "this"
                            | "with"
                            | "which"
                            | "might"
                    ) {
                        terms.insert(lower);
                    }
                } else if !word.is_ascii() {
                    let characters = word.chars().collect::<Vec<_>>();
                    terms.extend(characters.windows(3).map(|window| window.iter().collect()));
                    for pair in characters.windows(2).filter(|pair| {
                        pair.iter()
                            .all(|character| ('一'..='鿿').contains(character))
                    }) {
                        let term = pair.iter().collect::<String>();
                        if !matches!(
                            term.as_str(),
                            "关于"
                                | "可以"
                                | "需要"
                                | "项目"
                                | "这个"
                                | "什么"
                                | "继续"
                                | "看看"
                                | "我们"
                        ) {
                            terms.insert(term);
                        }
                    }
                }
            }
            terms
        };
        let input_terms = unique(input);
        unique(&self.topic)
            .intersection(&input_terms)
            .next()
            .is_some()
            || unique(&self.question)
                .intersection(&input_terms)
                .take(2)
                .count()
                == 2
    }

    pub(crate) fn navigation(&self) -> String {
        let question = self.question.replace(['\r', '\n'], " ");
        let reason = self.reason.as_ref().map_or_else(String::new, |reason| {
            format!(" · Origin 的理由：{}", reason.replace(['\r', '\n'], " "))
        });
        format!(
            "{} · {} · {} · {question}{reason}",
            self.topic,
            self.state.label(),
            self.author.label()
        )
    }
}

pub(crate) fn remember_boundaries(
    tx: &Transaction<'_>,
    observations: &[Observation],
) -> rusqlite::Result<()> {
    for item in observations {
        if let Some(attention) = &item.attention
            && (attention.state == AttentionState::End || attention.reopened)
        {
            let integer = |value| {
                i64::try_from(value)
                    .map_err(|failure| rusqlite::Error::ToSqlConversionFailure(Box::new(failure)))
            };
            tx.execute(
                "INSERT INTO attention_boundaries (topic_id, session_id, observed_ms, sequence, ended)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (topic_id, session_id) DO UPDATE SET
                   observed_ms = excluded.observed_ms, sequence = excluded.sequence, ended = excluded.ended
                 WHERE (excluded.observed_ms, excluded.sequence) >= (attention_boundaries.observed_ms, attention_boundaries.sequence)",
                params![attention.id, item.session.to_string(), integer(item.source.at_ms)?, integer(item.source.sequence)?, attention.state == AttentionState::End]
            )?;
        }
    }
    Ok(())
}

pub(crate) fn ended_topics(
    connection: &Connection,
    sessions: &BTreeSet<String>,
) -> io::Result<BTreeSet<String>> {
    let mut statement = connection
        .prepare(
            "SELECT topic_id, session_id, ended FROM attention_boundaries
         ORDER BY observed_ms DESC, sequence DESC, session_id",
        )
        .map_err(io::Error::other)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(io::Error::other)?;
    let mut visited = BTreeSet::new();
    let mut ended = BTreeSet::new();
    for row in rows {
        let (topic, session, closed) = row.map_err(io::Error::other)?;
        if sessions.contains(&session) && visited.insert(topic.clone()) && closed {
            ended.insert(topic);
        }
    }
    Ok(ended)
}

pub(crate) fn apply(entries: &mut [Entry], ended: &BTreeSet<String>) {
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_by_key(|index| (entries[*index].at_ms, entries[*index].id.clone()));
    let mut latest = BTreeMap::<String, usize>::new();
    for index in order {
        let entry = &entries[index];
        if !entry.state.usable() {
            continue;
        }
        let Some(attention) = &entry.attention else {
            continue;
        };
        if let Some(&older) = latest.get(&attention.id)
            && entries[older]
                .attention
                .as_ref()
                .is_some_and(|older| older.state == AttentionState::End)
            && !attention.reopened
        {
            entries[index].state = Status::Historical;
            continue;
        }
        if let Some(older) = latest.insert(attention.id.clone(), index) {
            entries[index].supersedes = Some(entries[older].id.clone());
            let held = entries[older].attention.as_ref().map(|older| {
                (
                    older.author,
                    older.reason.clone(),
                    older.reason_source.clone(),
                )
            });
            if let (Some((author, reason, origin)), Some(current)) =
                (held, entries[index].attention.as_mut())
            {
                if author != Author::Unknown {
                    current.author = author;
                }
                if current.reason.is_none() {
                    current.reason = reason;
                    current.reason_source = origin;
                }
            }
            entries[older].state = Status::Historical;
        }
    }
    let mut active = latest.into_values().collect::<Vec<_>>();
    active.sort_by_key(|index| {
        let item = &entries[*index];
        (
            item.attention
                .as_ref()
                .is_none_or(|attention| attention.author == Author::Unknown),
            std::cmp::Reverse(item.at_ms),
            item.id.clone(),
        )
    });
    let mut by_scope = BTreeMap::new();
    for index in active {
        let entry = &mut entries[index];
        let Some(attention) = &entry.attention else {
            continue;
        };
        if attention.state == AttentionState::End || ended.contains(&attention.id) {
            entry.state = Status::Historical;
            continue;
        }
        let scope = match &entry.scope {
            Scope::Global => None,
            Scope::Project { directory } => Some(directory.clone()),
            Scope::Context => {
                entry.state = Status::Historical;
                continue;
            }
        };
        let count = by_scope.entry(scope).or_insert(0);
        if *count >= MAX_ACTIVE_PER_SCOPE {
            entry.state = Status::Historical;
        }
        *count += 1;
    }
}
