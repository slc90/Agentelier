use crate::{SessionId, TurnId};
use std::collections::{BTreeMap, HashMap, hash_map::Entry};
use std::str::FromStr;
use std::{error::Error, fmt};
use uuid::Uuid;

mod stop;
pub use stop::{AgentStop, AgentStopSource};

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

impl AgentThreadId {
    pub fn session_id(self) -> SessionId {
        self.0
    }
}

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

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String")]
pub struct AgentPath(String);

impl AgentPath {
    const ROOT: &str = "root";
    const MAX_SEGMENT_BYTES: usize = 64;
    const MAX_PATH_BYTES: usize = 512;

    fn root() -> Self {
        Self(Self::ROOT.into())
    }

    fn child(&self, segment: &str) -> Result<Self, AgentRegistryError> {
        if !Self::valid_segment(segment) || self.0.len() + 1 + segment.len() > Self::MAX_PATH_BYTES
        {
            return Err(AgentRegistryError::InvalidPath);
        }
        Ok(Self(format!("{self}/{segment}")))
    }

    fn valid_segment(segment: &str) -> bool {
        !segment.is_empty()
            && segment.len() <= Self::MAX_SEGMENT_BYTES
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    }

    pub fn contains(&self, candidate: &Self) -> bool {
        candidate
            .0
            .strip_prefix(&self.0)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
    }

    fn validate(raw: &str) -> Result<(), AgentRegistryError> {
        let mut segments = raw.split('/');
        if raw.len() > Self::MAX_PATH_BYTES
            || segments.next() != Some(Self::ROOT)
            || !segments.all(Self::valid_segment)
        {
            return Err(AgentRegistryError::InvalidPath);
        }
        Ok(())
    }
}

impl FromStr for AgentPath {
    type Err = AgentRegistryError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::validate(raw)?;
        Ok(Self(raw.to_owned()))
    }
}

impl TryFrom<String> for AgentPath {
    type Error = AgentRegistryError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::validate(&raw)?;
        Ok(Self(raw))
    }
}

impl fmt::Display for AgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentOrigin {
    Root,
    Spawn { turn: TurnId, call_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentIdentity {
    pub id: AgentId,
    pub session: SessionId,
    pub thread: AgentThreadId,
    pub parent: Option<AgentId>,
    pub path: AgentPath,
    pub depth: u16,
    pub nickname: String,
    pub role: String,
    pub origin: AgentOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentState {
    Starting,
    Running,
    Waiting,
    Stopping,
    Completed,
    Interrupted,
    Failed(String),
    Closed,
}

impl AgentState {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed(_) => "failed",
            Self::Closed => "closed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentResidency {
    Unloaded,
    Resident(AgentRuntimeId),
}

#[derive(Clone, Debug)]
pub struct AgentNode {
    pub identity: AgentIdentity,
    pub state: AgentState,
    pub residency: AgentResidency,
}

impl AgentNode {
    fn new(identity: AgentIdentity, state: AgentState) -> Self {
        Self {
            identity,
            state,
            residency: AgentResidency::Unloaded,
        }
    }

    fn ensure_open(&self) -> Result<(), AgentRegistryError> {
        if self.state == AgentState::Closed {
            Err(AgentRegistryError::ClosedAgent(self.identity.id))
        } else {
            Ok(())
        }
    }
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
    UnknownPath(AgentPath),
    InvalidPath,
    InvalidTarget,
    NicknameTarget,
    ClosedAgent(AgentId),
    SelfParent(AgentId),
    AlreadyResident(AgentId),
    StaleInstance {
        id: AgentId,
        instance: AgentRuntimeId,
    },
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
            Self::UnknownPath(path) => write!(f, "当前会话没有 Agent 路径 {path}"),
            Self::InvalidPath => f.write_str("Agent 路径不符合格式或长度限制"),
            Self::InvalidTarget => f.write_str("Agent 目标为空或超出长度限制"),
            Self::NicknameTarget => f.write_str("Agent 目标必须使用完整路径或 ID，不能使用昵称"),
            Self::ClosedAgent(id) => write!(f, "Agent {id} 已关闭"),
            Self::SelfParent(id) => write!(f, "Agent {id} 不能将自己登记为父节点"),
            Self::AlreadyResident(id) => write!(f, "Agent {id} 已有驻留实例"),
            Self::StaleInstance { id, instance } => {
                write!(f, "Agent {id} 的运行实例 {instance} 已失效")
            }
        }
    }
}

impl Error for AgentRegistryError {}

#[derive(Clone)]
enum Registration {
    Reserved { identity: AgentIdentity, key: Uuid },
    Committed(AgentNode),
    Retired(SessionId),
}

impl Registration {
    fn session(&self) -> SessionId {
        match self {
            Self::Reserved { identity, .. } => identity.session,
            Self::Committed(node) => node.identity.session,
            Self::Retired(session) => *session,
        }
    }

    fn identity(&self) -> Option<&AgentIdentity> {
        match self {
            Self::Reserved { identity, .. } => Some(identity),
            Self::Committed(node) => Some(&node.identity),
            Self::Retired(_) => None,
        }
    }
}

#[derive(Clone, Default)]
pub struct AgentRegistry {
    roots: HashMap<SessionId, AgentId>,
    entries: HashMap<AgentId, Registration>,
    paths: HashMap<SessionId, BTreeMap<AgentPath, AgentId>>,
    stops: stop::Stops,
}

impl AgentRegistry {
    pub fn root(&self, session: SessionId) -> Option<AgentId> {
        self.roots.get(&session).copied()
    }

    pub fn get(&self, session: SessionId, id: AgentId) -> Result<&AgentNode, AgentRegistryError> {
        let entry = self
            .entries
            .get(&id)
            .ok_or(AgentRegistryError::UnknownAgent(id))?;
        ensure_session(id, entry.session(), session)?;
        match entry {
            Registration::Committed(node) => Ok(node),
            Registration::Retired(_) => Err(AgentRegistryError::ClosedAgent(id)),
            Registration::Reserved { .. } => Err(AgentRegistryError::UnknownAgent(id)),
        }
    }

    pub fn resolve(
        &self,
        session: SessionId,
        caller: AgentId,
        target: &str,
    ) -> Result<&AgentNode, AgentRegistryError> {
        self.get(session, caller)?.ensure_open()?;
        let target = target.trim();
        if target.is_empty() || target.len() > AgentPath::MAX_PATH_BYTES {
            return Err(AgentRegistryError::InvalidTarget);
        }
        let id = if let Ok(id) = Uuid::parse_str(target) {
            AgentId(id)
        } else if target == AgentPath::ROOT || target.contains('/') {
            let path: AgentPath = target.parse()?;
            self.paths
                .get(&session)
                .and_then(|paths| paths.get(&path))
                .copied()
                .filter(|id| matches!(self.entries.get(id), Some(Registration::Committed(_))))
                .ok_or(AgentRegistryError::UnknownPath(path))?
        } else {
            return Err(AgentRegistryError::NicknameTarget);
        };
        let node = self.get(session, id)?;
        node.ensure_open()?;
        Ok(node)
    }

    pub fn subtree(
        &self,
        session: SessionId,
        parent: AgentId,
    ) -> Result<impl Iterator<Item = &AgentNode>, AgentRegistryError> {
        let path = &self.get(session, parent)?.identity.path;
        Ok(self
            .paths
            .get(&session)
            .into_iter()
            .flat_map(move |paths| paths.range(path.clone()..))
            .filter_map(move |(candidate, id)| {
                if !path.contains(candidate) {
                    return None;
                }
                match self.entries.get(id) {
                    Some(Registration::Committed(node)) => Some(node),
                    _ => None,
                }
            }))
    }

    pub fn spawned_by(
        &self,
        session: SessionId,
        parent: AgentId,
        turn: TurnId,
        call_id: &str,
    ) -> Option<&AgentNode> {
        self.subtree(session, parent).ok()?.find(|node| {
            node.identity.parent == Some(parent) && matches!(&node.identity.origin,
                AgentOrigin::Spawn { turn: source, call_id: call } if *source == turn && call == call_id)
        })
    }

    pub(crate) fn restore_child(
        &mut self,
        identity: AgentIdentity,
    ) -> Result<(), AgentRegistryError> {
        let AgentOrigin::Spawn { turn, call_id } = &identity.origin else {
            return Err(AgentRegistryError::InvalidTarget);
        };
        let parent = identity.parent.ok_or(AgentRegistryError::InvalidPath)?;
        if let Some(entry) = self.entries.get(&identity.id) {
            if let Registration::Reserved {
                identity: reserved,
                key,
            } = entry
                && reserved == &identity
            {
                return self.commit(AgentReservation {
                    identity,
                    key: *key,
                });
            }
            let agent = self.get(identity.session, identity.id)?;
            return if agent.identity == identity {
                Ok(())
            } else {
                Err(AgentRegistryError::DuplicateId {
                    id: identity.id,
                    session: identity.session,
                })
            };
        }
        let segment = identity
            .path
            .0
            .rsplit('/')
            .next()
            .ok_or(AgentRegistryError::InvalidPath)?;
        let reservation = self.reserve_child(
            identity.session,
            parent,
            ChildRegistration {
                id: identity.id,
                segment: segment.into(),
                nickname: identity.nickname.clone(),
                role: identity.role.clone(),
                turn: *turn,
                call_id: call_id.clone(),
            },
        )?;
        if reservation.identity != identity {
            self.release(reservation)?;
            return Err(AgentRegistryError::InvalidPath);
        }
        self.commit(reservation)
    }

    pub(crate) fn clear_reservations(&mut self, session: SessionId) {
        if let Some(paths) = self.paths.get_mut(&session) {
            paths.retain(|_, id| {
                if matches!(self.entries.get(id), Some(Registration::Reserved { .. })) {
                    self.entries.remove(id);
                    false
                } else {
                    true
                }
            });
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
        let path = AgentPath::root();
        self.roots.insert(session, id);
        self.paths
            .entry(session)
            .or_default()
            .insert(path.clone(), id);
        self.entries.insert(
            id,
            Registration::Committed(AgentNode::new(
                AgentIdentity {
                    id,
                    session,
                    thread: AgentThreadId(id.0),
                    parent: None,
                    path,
                    depth: 0,
                    nickname: "root".into(),
                    role: "root".into(),
                    origin: AgentOrigin::Root,
                },
                AgentState::Waiting,
            )),
        );
        Ok(())
    }

    pub fn reserve_child(
        &mut self,
        session: SessionId,
        parent: AgentId,
        request: ChildRegistration,
    ) -> Result<AgentReservation, AgentRegistryError> {
        if request.id == parent {
            return Err(AgentRegistryError::SelfParent(parent));
        }
        let node = self.get(session, parent)?;
        node.ensure_open()?;
        let path = node.identity.path.child(&request.segment)?;
        let depth = node.identity.depth + 1;
        self.ensure_available(session, request.id)?;
        let paths = self.paths.entry(session).or_default();
        if paths.contains_key(&path) {
            return Err(AgentRegistryError::DuplicatePath(path));
        }
        let identities = || {
            paths
                .values()
                .filter_map(|id| self.entries.get(id))
                .filter_map(Registration::identity)
        };
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
        paths.insert(path.clone(), request.id);
        let identity = AgentIdentity {
            id: request.id,
            session,
            thread: AgentThreadId(request.id.0),
            parent: Some(parent),
            path,
            depth,
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
        *entry.into_mut() =
            Registration::Committed(AgentNode::new(reservation.identity, AgentState::Starting));
        Ok(())
    }

    pub fn release(&mut self, reservation: AgentReservation) -> Result<(), AgentRegistryError> {
        self.reservation(&reservation)?.remove();
        if let Some(paths) = self.paths.get_mut(&reservation.identity.session) {
            paths.remove(&reservation.identity.path);
        }
        Ok(())
    }

    pub(crate) fn retire_session(&mut self, session: SessionId) {
        self.stops.retire(session);
        self.roots.remove(&session);
        self.paths.remove(&session);
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

    pub fn set_state(
        &mut self,
        session: SessionId,
        id: AgentId,
        state: AgentState,
    ) -> Result<(), AgentRegistryError> {
        let node = self.node_mut(session, id)?;
        if node.state != state {
            node.ensure_open()?;
            node.state = state;
        }
        Ok(())
    }

    pub fn attach(
        &mut self,
        session: SessionId,
        id: AgentId,
    ) -> Result<AgentRuntimeId, AgentRegistryError> {
        let node = self.node_mut(session, id)?;
        node.ensure_open()?;
        if node.residency != AgentResidency::Unloaded {
            return Err(AgentRegistryError::AlreadyResident(id));
        }
        let instance = AgentRuntimeId::allocate();
        node.residency = AgentResidency::Resident(instance);
        Ok(instance)
    }

    pub fn detach(
        &mut self,
        session: SessionId,
        id: AgentId,
        instance: AgentRuntimeId,
    ) -> Result<(), AgentRegistryError> {
        let node = self.node_mut(session, id)?;
        if node.residency != AgentResidency::Resident(instance) {
            return Err(AgentRegistryError::StaleInstance { id, instance });
        }
        node.residency = AgentResidency::Unloaded;
        Ok(())
    }

    fn node_mut(
        &mut self,
        session: SessionId,
        id: AgentId,
    ) -> Result<&mut AgentNode, AgentRegistryError> {
        let entry = self
            .entries
            .get_mut(&id)
            .ok_or(AgentRegistryError::UnknownAgent(id))?;
        ensure_session(id, entry.session(), session)?;
        match entry {
            Registration::Committed(node) => Ok(node),
            Registration::Retired(_) => Err(AgentRegistryError::ClosedAgent(id)),
            Registration::Reserved { .. } => Err(AgentRegistryError::UnknownAgent(id)),
        }
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
