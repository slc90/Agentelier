use crate::{SessionId, TurnId};
use std::collections::{HashMap, hash_map::Entry};
use std::{error::Error, fmt};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AgentId(Uuid);

impl AgentId {
    pub fn for_root(session: SessionId) -> Self {
        Self(session)
    }

    pub fn allocate() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AgentThreadId(Uuid);

impl fmt::Display for AgentThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentRuntimeId(Uuid);

impl AgentRuntimeId {
    pub(crate) fn allocate() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for AgentRuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AgentPath(String);

impl AgentPath {
    const ROOT: &str = "root";
    const MAX_SEGMENT_BYTES: usize = 64;
    const MAX_PATH_BYTES: usize = 512;

    fn root() -> Self {
        Self(Self::ROOT.into())
    }

    fn child(&self, segment: &str) -> Result<Self, AgentRegistryError> {
        if segment.is_empty()
            || segment.len() > Self::MAX_SEGMENT_BYTES
            || self.0.len() + 1 + segment.len() > Self::MAX_PATH_BYTES
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(AgentRegistryError::InvalidPath);
        }
        Ok(Self(format!("{self}/{segment}")))
    }
}

impl fmt::Display for AgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum AgentOrigin {
    Root,
    Spawn { turn: TurnId, call_id: String },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentIdentity {
    pub id: AgentId,
    pub session: SessionId,
    pub thread: AgentThreadId,
    pub parent: Option<AgentId>,
    pub path: AgentPath,
    pub nickname: String,
    pub role: String,
    pub origin: AgentOrigin,
}

pub struct ChildRegistration {
    pub id: AgentId,
    pub segment: String,
    pub nickname: String,
    pub role: String,
    pub turn: TurnId,
    pub call_id: String,
}

pub struct AgentReservation {
    identity: AgentIdentity,
    key: Uuid,
}

impl AgentReservation {
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }
}

#[derive(Debug)]
pub enum AgentRegistryError {
    DuplicateRoot {
        session: SessionId,
        existing: AgentId,
        requested: AgentId,
    },
    DuplicateId {
        id: AgentId,
        session: SessionId,
    },
    ForeignId {
        id: AgentId,
        owner: SessionId,
        requested: SessionId,
    },
    UnknownAgent(AgentId),
    UnknownReservation(AgentId),
    DuplicatePath(AgentPath),
    InvalidPath,
}

impl fmt::Display for AgentRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRoot {
                session,
                existing,
                requested,
            } => {
                write!(
                    f,
                    "会话 {session} 已登记 Root {existing}，无法登记 {requested}"
                )
            }
            Self::DuplicateId { id, session } => write!(f, "Agent {id} 已被会话 {session} 使用"),
            Self::ForeignId {
                id,
                owner,
                requested,
            } => {
                write!(f, "Agent {id} 属于会话 {owner}，请求属于会话 {requested}")
            }
            Self::UnknownAgent(id) => write!(f, "未登记 Agent {id}"),
            Self::UnknownReservation(id) => write!(f, "Agent {id} 没有待提交的预留"),
            Self::DuplicatePath(path) => write!(f, "Agent 路径已占用：{path}"),
            Self::InvalidPath => f.write_str("Agent 路径不符合格式或长度限制"),
        }
    }
}

impl Error for AgentRegistryError {}

enum Registration {
    Reserved { identity: AgentIdentity, key: Uuid },
    Committed(AgentIdentity),
    Retired(SessionId),
}

impl Registration {
    fn session(&self) -> SessionId {
        match self {
            Self::Reserved { identity, .. } | Self::Committed(identity) => identity.session,
            Self::Retired(session) => *session,
        }
    }

    fn identity(&self) -> Option<&AgentIdentity> {
        match self {
            Self::Reserved { identity, .. } | Self::Committed(identity) => Some(identity),
            Self::Retired(_) => None,
        }
    }
}

#[derive(Default)]
pub struct AgentRegistry {
    roots: HashMap<SessionId, AgentId>,
    entries: HashMap<AgentId, Registration>,
}

impl AgentRegistry {
    pub fn root(&self, session: SessionId) -> Option<AgentId> {
        self.roots.get(&session).copied()
    }

    pub fn get(
        &self,
        session: SessionId,
        id: AgentId,
    ) -> Result<&AgentIdentity, AgentRegistryError> {
        let entry = self
            .entries
            .get(&id)
            .ok_or(AgentRegistryError::UnknownAgent(id))?;
        ensure_session(id, entry.session(), session)?;
        match entry {
            Registration::Committed(identity) => Ok(identity),
            _ => Err(AgentRegistryError::UnknownAgent(id)),
        }
    }

    pub fn register_root(
        &mut self,
        session: SessionId,
        id: AgentId,
    ) -> Result<(), AgentRegistryError> {
        if let Some(existing) = self.root(session) {
            return if existing == id {
                Ok(())
            } else {
                Err(AgentRegistryError::DuplicateRoot {
                    session,
                    existing,
                    requested: id,
                })
            };
        }
        self.ensure_available(session, id)?;
        self.roots.insert(session, id);
        self.entries.insert(
            id,
            Registration::Committed(AgentIdentity {
                id,
                session,
                thread: AgentThreadId(id.0),
                parent: None,
                path: AgentPath::root(),
                nickname: "root".into(),
                role: "root".into(),
                origin: AgentOrigin::Root,
            }),
        );
        Ok(())
    }

    pub fn reserve_child(
        &mut self,
        session: SessionId,
        parent: AgentId,
        request: ChildRegistration,
    ) -> Result<AgentReservation, AgentRegistryError> {
        let path = self.get(session, parent)?.path.child(&request.segment)?;
        self.ensure_available(session, request.id)?;
        let identities = || {
            self.entries
                .values()
                .filter_map(Registration::identity)
                .filter(|identity| identity.session == session)
        };
        if identities().any(|identity| identity.path == path) {
            return Err(AgentRegistryError::DuplicatePath(path));
        }
        let base = request.nickname.trim();
        let base = if base.is_empty() {
            request.segment.as_str()
        } else {
            base
        };
        let mut nickname = base.to_owned();
        let mut suffix = 2;
        while identities().any(|identity| identity.nickname == nickname) {
            nickname = format!("{base}-{suffix}");
            suffix += 1;
        }
        let identity = AgentIdentity {
            id: request.id,
            session,
            thread: AgentThreadId(request.id.0),
            parent: Some(parent),
            path,
            nickname,
            role: request.role,
            origin: AgentOrigin::Spawn {
                turn: request.turn,
                call_id: request.call_id,
            },
        };
        let key = Uuid::new_v4();
        self.entries.insert(
            request.id,
            Registration::Reserved {
                identity: identity.clone(),
                key,
            },
        );
        Ok(AgentReservation { identity, key })
    }

    pub fn commit(&mut self, reservation: AgentReservation) -> Result<(), AgentRegistryError> {
        let entry = self.reservation(&reservation)?;
        *entry.into_mut() = Registration::Committed(reservation.identity);
        Ok(())
    }

    pub fn release(&mut self, reservation: AgentReservation) -> Result<(), AgentRegistryError> {
        self.reservation(&reservation)?.remove();
        Ok(())
    }

    pub(crate) fn retire_session(&mut self, session: SessionId) {
        self.roots.remove(&session);
        self.entries.retain(|_, entry| {
            if entry.session() != session {
                return true;
            }
            if matches!(entry, Registration::Reserved { .. }) {
                return false;
            }
            *entry = Registration::Retired(session);
            true
        });
    }

    pub(crate) fn ensure_available(
        &self,
        session: SessionId,
        id: AgentId,
    ) -> Result<(), AgentRegistryError> {
        if let Some(entry) = self.entries.get(&id) {
            ensure_session(id, entry.session(), session)?;
            return Err(AgentRegistryError::DuplicateId { id, session });
        }
        Ok(())
    }

    fn reservation(
        &mut self,
        reservation: &AgentReservation,
    ) -> Result<
        std::collections::hash_map::OccupiedEntry<'_, AgentId, Registration>,
        AgentRegistryError,
    > {
        let AgentIdentity { id, session, .. } = reservation.identity;
        let Entry::Occupied(entry) = self.entries.entry(id) else {
            return Err(AgentRegistryError::UnknownReservation(id));
        };
        ensure_session(id, entry.get().session(), session)?;
        if !matches!(entry.get(), Registration::Reserved { key, .. } if *key == reservation.key) {
            return Err(AgentRegistryError::UnknownReservation(id));
        }
        Ok(entry)
    }
}

fn ensure_session(
    id: AgentId,
    owner: SessionId,
    requested: SessionId,
) -> Result<(), AgentRegistryError> {
    if owner == requested {
        Ok(())
    } else {
        Err(AgentRegistryError::ForeignId {
            id,
            owner,
            requested,
        })
    }
}
