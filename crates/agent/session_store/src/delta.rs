use crate::{Record, RecordKind};
use agentelier_conversation::{AgentId, RecordEvent, SessionId, SessionRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io;

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stream {
    Session,
    Agent(AgentId),
}

impl Stream {
    fn of(event: &RecordEvent) -> Option<Self> {
        match event {
            RecordEvent::AgentTurn { agent, event, .. } if event.state().is_some() => {
                Some(Self::Agent(*agent))
            }
            _ if event.state().is_some() => Some(Self::Session),
            _ => None,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct EventDelta {
    session: SessionId,
    at_ms: u64,
    stream: Stream,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    change: Option<Change>,
}

pub(super) struct Snapshot {
    stream: Stream,
    sequence: u64,
    value: Value,
}

#[derive(Default)]
pub(super) struct Snapshots(HashMap<Stream, Snapshot>);

impl Snapshots {
    pub(super) fn carries_state(event: &RecordEvent) -> bool {
        Stream::of(event).is_some()
    }

    pub(super) fn encode(&self, record: &Record) -> io::Result<(Vec<u8>, Option<Snapshot>)> {
        if let RecordKind::Event(event) = &record.kind
            && let Some((delta, snapshot)) = self.prepare(record.sequence, event)?
        {
            let bytes = serde_json::to_vec(&Record {
                format: record.format.clone(),
                version: record.version,
                sequence: record.sequence,
                kind: RecordKind::EventDelta(Box::new(delta)),
            })?;
            return Ok((bytes, Some(snapshot)));
        }
        Ok((serde_json::to_vec(record)?, None))
    }

    fn prepare(
        &self,
        sequence: u64,
        record: &SessionRecord,
    ) -> io::Result<Option<(EventDelta, Snapshot)>> {
        let Some(stream) = Stream::of(&record.event) else {
            return Ok(None);
        };
        let value = serde_json::to_value(&record.event)?;
        let base = self.0.get(&stream);
        let change = base.map_or_else(
            || Some(Change::Replace(value.clone())),
            |base| Change::between(&base.value, &value),
        );
        Ok(Some((
            EventDelta {
                session: record.session,
                at_ms: record.at_ms,
                stream,
                base: base.map(|base| base.sequence),
                change,
            },
            Snapshot {
                stream,
                sequence,
                value,
            },
        )))
    }

    pub(super) fn commit(&mut self, snapshot: Snapshot) {
        self.0.insert(snapshot.stream, snapshot);
    }

    pub(super) fn decode(&mut self, sequence: u64, delta: EventDelta) -> io::Result<SessionRecord> {
        let EventDelta {
            session,
            at_ms,
            stream,
            base,
            change,
        } = delta;
        let value = if let Some(base) = base {
            let mut snapshot = self
                .0
                .remove(&stream)
                .filter(|snapshot| snapshot.sequence == base && base < sequence)
                .ok_or_else(|| io::Error::other("会话变更缺少对应的基准记录"))?;
            if let Some(change) = change {
                change.apply(&mut snapshot.value)?;
            }
            snapshot.value
        } else if let Some(Change::Replace(value)) = change {
            value
        } else {
            return Err(io::Error::other("会话变更缺少完整的起始快照"));
        };
        let event = RecordEvent::deserialize(&value)?;
        if Stream::of(&event) != Some(stream) {
            return Err(io::Error::other("会话变更的执行来源不匹配"));
        }
        self.commit(Snapshot {
            stream,
            sequence,
            value,
        });
        Ok(SessionRecord {
            sequence,
            session,
            at_ms,
            event,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Change {
    Replace(Value),
    Object {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        fields: BTreeMap<String, Change>,
    },
    Array {
        keep: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        entries: Vec<(usize, Change)>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        append: Vec<Value>,
    },
}

impl Change {
    fn between(before: &Value, after: &Value) -> Option<Self> {
        match (before, after) {
            (Value::Object(before), Value::Object(after)) => {
                let remove: Vec<_> = before
                    .keys()
                    .filter(|name| !after.contains_key(*name))
                    .cloned()
                    .collect();
                let fields: BTreeMap<_, _> = after
                    .iter()
                    .filter_map(|(name, value)| {
                        let change = match before.get(name) {
                            Some(old) => Self::between(old, value),
                            None => Some(Self::Replace(value.clone())),
                        };
                        change.map(|change| (name.clone(), change))
                    })
                    .collect();
                (!remove.is_empty() || !fields.is_empty())
                    .then_some(Self::Object { remove, fields })
            }
            (Value::Array(before), Value::Array(after)) => {
                let keep = before.len().min(after.len());
                let entries: Vec<_> = before
                    .iter()
                    .zip(after)
                    .enumerate()
                    .filter_map(|(index, (old, new))| {
                        Self::between(old, new).map(|change| (index, change))
                    })
                    .collect();
                (before.len() != after.len() || !entries.is_empty()).then(|| Self::Array {
                    keep,
                    entries,
                    append: after[keep..].to_vec(),
                })
            }
            (Value::Number(before), Value::Number(after)) if before.is_f64() && after.is_f64() => {
                (before.as_f64().map(f64::to_bits) != after.as_f64().map(f64::to_bits))
                    .then(|| Self::Replace(Value::Number(after.clone())))
            }
            _ => (before != after).then(|| Self::Replace(after.clone())),
        }
    }

    fn apply(self, value: &mut Value) -> io::Result<()> {
        match (self, value) {
            (Self::Replace(new), value) => *value = new,
            (Self::Object { remove, fields }, Value::Object(value)) => {
                for name in remove {
                    value
                        .remove(&name)
                        .ok_or_else(|| io::Error::other("会话变更试图删除不存在的字段"))?;
                }
                for (name, change) in fields {
                    if let Some(existing) = value.get_mut(&name) {
                        change.apply(existing)?;
                    } else if let Self::Replace(new) = change {
                        value.insert(name, new);
                    } else {
                        return Err(io::Error::other("会话变更试图修改不存在的字段"));
                    }
                }
            }
            (
                Self::Array {
                    keep,
                    entries,
                    append,
                },
                Value::Array(value),
            ) => {
                if keep > value.len() {
                    return Err(io::Error::other("会话变更的数组基准长度不一致"));
                }
                value.truncate(keep);
                for (index, change) in entries {
                    let existing = value
                        .get_mut(index)
                        .ok_or_else(|| io::Error::other("会话变更的数组索引超出范围"))?;
                    change.apply(existing)?;
                }
                value.extend(append);
            }
            _ => return Err(io::Error::other("会话变更与基准记录的字段类型不一致")),
        }
        Ok(())
    }
}
