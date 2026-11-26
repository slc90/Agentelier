use crate::{ConversationFailure, SessionId, TurnId};
use agentelier_llm_model::FunctionCall;
use agentelier_tools::{ToolMode, ToolStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

const LEGACY_MAX_UNDELIVERED_TASKS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskExecution {
    pub task: TaskId,
    pub instance: Uuid,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TaskCancellation {
    pub call_id: String,
    pub execution: TaskExecution,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCancelState {
    Requested,
    AlreadyRequested,
    Finished,
}

#[derive(Serialize)]
pub struct TaskCancelReceipt {
    pub task_id: TaskId,
    pub state: TaskCancelState,
    pub status: ToolStatus,
}

pub struct TaskCancellationPlan {
    pub targets: Vec<TaskCancellation>,
    pub receipts: Vec<TaskCancelReceipt>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TaskRegistration {
    pub execution: TaskExecution,
    pub turn: TurnId,
    pub request: u32,
    pub call_id: String,
    pub item_id: String,
    pub tool: String,
    pub memory_epoch: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_bytes: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "tasks", rename_all = "snake_case")]
pub enum ResponseTasks {
    Registered(Vec<TaskRegistration>),
    OverBudget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseDelivery {
    pub settled: Vec<TaskId>,
    pub rejected: Vec<String>,
}

impl ResponseTasks {
    pub(crate) fn validate(
        &self,
        calls: &[FunctionCall],
        turn: TurnId,
        request: u32,
        rejected: &[String],
    ) -> Result<(), ConversationFailure> {
        let Self::Registered(registrations) = self else {
            return Ok(());
        };
        let asynchronous: Vec<_> = calls
            .iter()
            .filter(|call| {
                call.mode() == ToolMode::Asynchronous && !rejected.contains(&call.call_id)
            })
            .collect();
        if asynchronous.len() != registrations.len() {
            return Err(ConversationFailure::new("异步调用缺少唯一任务登记"));
        }
        for (registration, call) in registrations.iter().zip(asynchronous) {
            if registration.turn != turn
                || registration.request != request
                || registration.call_id != call.call_id
                || registration.item_id != call.id
                || registration.tool != call.name
                || registration
                    .argument_bytes
                    .is_some_and(|bytes| bytes != call.arguments.len())
            {
                return Err(ConversationFailure::new("工具任务登记与原始调用不一致"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery {
    AwaitingResult,
    Saved(u64),
    Appended,
    Delivered(u64),
}

struct Task {
    registration: TaskRegistration,
    status: ToolStatus,
    registered_at: u64,
    cancellation_requested: bool,
    live: bool,
    delivery: Delivery,
}

impl Task {
    fn report(&self, tools: &agentelier_tools::ToolCatalog) -> TaskReport {
        let registration = &self.registration;
        let source = self.source(Some(tools));
        TaskReport {
            task_id: registration.execution.task,
            turn: registration.turn,
            request: registration.request,
            tool: registration.tool.clone(),
            source,
            argument_bytes: registration.argument_bytes,
            status: self.status,
            delivery: self.delivery(),
            blocked_by: (self.status == ToolStatus::Pending).then_some("admission"),
            cancelling: self.cancellation_requested && self.in_progress(),
            process: None,
        }
    }

    fn source(&self, tools: Option<&agentelier_tools::ToolCatalog>) -> &'static str {
        match tools
            .and_then(|tools| tools.definition(&self.registration.tool))
            .map(|tool| &tool.source)
        {
            Some(agentelier_tools::ToolSource::Builtin) => "built_in",
            Some(agentelier_tools::ToolSource::Mcp { .. }) => "mcp",
            Some(agentelier_tools::ToolSource::Extension { .. }) => "extension",
            None => "unavailable",
        }
    }

    fn delivery(&self) -> TaskDelivery {
        match self.delivery {
            Delivery::AwaitingResult => TaskDelivery::AwaitingResult,
            Delivery::Saved(_) => TaskDelivery::Saved,
            Delivery::Appended => TaskDelivery::Appended,
            Delivery::Delivered(_) => TaskDelivery::Delivered,
        }
    }

    fn in_progress(&self) -> bool {
        matches!(self.status, ToolStatus::Pending | ToolStatus::Running)
    }

    fn recover_interrupted(&mut self) -> bool {
        self.live = false;
        self.status = match self.status {
            ToolStatus::Pending => ToolStatus::NotExecuted,
            ToolStatus::Running => ToolStatus::Unconfirmed,
            _ => return false,
        };
        true
    }
}

#[derive(Clone)]
pub struct SessionTaskView {
    pub cancelling: bool,
    pub task: TaskId,
    pub instance: Option<Uuid>,
    pub turn: TurnId,
    pub request: u32,
    pub call_id: String,
    pub tool: String,
    pub status: ToolStatus,
    pub source: &'static str,
    pub argument_bytes: Option<usize>,
    pub delivery: TaskDelivery,
    pub access: agentelier_tools::ToolAccess,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDelivery {
    AwaitingResult,
    Saved,
    Appended,
    Delivered,
}

#[derive(Serialize)]
pub struct TaskReport {
    pub task_id: TaskId,
    pub turn: TurnId,
    pub request: u32,
    pub tool: String,
    pub source: &'static str,
    pub argument_bytes: Option<usize>,
    pub status: ToolStatus,
    pub delivery: TaskDelivery,
    pub blocked_by: Option<&'static str>,
    pub cancelling: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<agentelier_tools::ProcessView>,
}

impl TaskReport {
    pub fn result_saved(&self) -> bool {
        !matches!(self.delivery, TaskDelivery::AwaitingResult)
    }
}

#[derive(Serialize)]
pub struct TaskPage {
    pub tasks: Vec<TaskReport>,
    pub next_history_offset: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<agentelier_tools::ProcessView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by_process: Option<String>,
}

pub struct SessionTasks {
    pub session: SessionId,
    pub revision: u64,
    pub tasks: Vec<SessionTaskView>,
    pub background_paused: bool,
    pub queue_paused: bool,
    pub external_unconfirmed: Vec<String>,
}

#[derive(Default)]
pub(crate) struct TaskLedger {
    tasks: Vec<Task>,
    revision: u64,
    delivery_revision: u64,
}

impl TaskLedger {
    pub(crate) fn matches_execution(&self, task: TaskId, instance: Uuid) -> bool {
        self.tasks
            .iter()
            .any(|entry| entry.registration.execution == TaskExecution { task, instance })
    }
    pub(crate) fn prepare(
        &self,
        calls: &[FunctionCall],
        turn: TurnId,
        request: u32,
        memory_epoch: Option<i64>,
        result_calls: &[String],
    ) -> Result<(ResponseTasks, ResponseDelivery), ConversationFailure> {
        let delivery = ResponseDelivery {
            settled: self.settled_delivery(result_calls),
            rejected: Vec::new(),
        };
        let registrations: Vec<_> = calls
            .iter()
            .filter(|call| call.mode() == ToolMode::Asynchronous)
            .map(|call| TaskRegistration {
                execution: TaskExecution {
                    task: TaskId(Uuid::new_v4()),
                    instance: Uuid::new_v4(),
                },
                turn,
                request,
                call_id: call.call_id.clone(),
                item_id: call.id.clone(),
                tool: call.name.clone(),
                memory_epoch,
                argument_bytes: Some(call.arguments.len()),
            })
            .collect();
        self.validate_identities(&registrations)?;
        Ok((ResponseTasks::Registered(registrations), delivery))
    }

    pub(crate) fn settled_delivery(&self, result_calls: &[String]) -> Vec<TaskId> {
        let results: HashSet<_> = result_calls.iter().map(String::as_str).collect();
        self.tasks
            .iter()
            .filter(|task| {
                task.delivery == Delivery::Appended
                    && results.contains(task.registration.call_id.as_str())
            })
            .map(|task| task.registration.execution.task)
            .collect()
    }

    pub(crate) fn legacy_rejected_delivery(
        &self,
        calls: &[FunctionCall],
        settled: &[TaskId],
    ) -> Vec<String> {
        let occupied = self
            .tasks
            .iter()
            .filter(|task| !matches!(task.delivery, Delivery::Delivered(_)))
            .count();
        let available = LEGACY_MAX_UNDELIVERED_TASKS.saturating_sub(occupied - settled.len());
        calls
            .iter()
            .filter(|call| call.mode() == ToolMode::Asynchronous)
            .skip(available)
            .map(|call| call.call_id.clone())
            .collect()
    }

    pub(crate) fn install(
        &mut self,
        mapping: ResponseTasks,
        settled: &[TaskId],
        sequence: u64,
        live: bool,
    ) -> Result<bool, ConversationFailure> {
        let registrations = match mapping {
            ResponseTasks::Registered(registrations) => registrations,
            ResponseTasks::OverBudget => Vec::new(),
        };
        if registrations.is_empty() && settled.is_empty() {
            return Ok(false);
        }
        if sequence <= self.revision {
            return Err(ConversationFailure::new("异步任务登记序号无效"));
        }
        self.validate_identities(&registrations)?;
        let mut remaining = settled.iter().collect::<HashSet<_>>();
        if remaining.len() != settled.len() {
            return Err(ConversationFailure::new("异步任务交付结算身份重复"));
        }
        for task in &self.tasks {
            if task.delivery == Delivery::Appended {
                remaining.remove(&task.registration.execution.task);
            }
        }
        if !remaining.is_empty() {
            return Err(ConversationFailure::new("异步任务交付结算缺少已追加结果"));
        }
        for task in &mut self.tasks {
            if settled.contains(&task.registration.execution.task) {
                task.delivery = Delivery::Delivered(sequence);
            }
        }
        self.tasks
            .extend(registrations.into_iter().map(|registration| Task {
                registration,
                status: ToolStatus::Pending,
                registered_at: sequence,
                cancellation_requested: false,
                live,
                delivery: Delivery::AwaitingResult,
            }));
        self.revision = sequence;
        Ok(true)
    }

    fn validate_identities(
        &self,
        registrations: &[TaskRegistration],
    ) -> Result<(), ConversationFailure> {
        if registrations.is_empty() {
            return Ok(());
        }
        let mut ids: HashSet<_> = self
            .tasks
            .iter()
            .map(|task| task.registration.execution.task)
            .collect();
        let mut calls: HashSet<_> = self
            .tasks
            .iter()
            .map(|task| task.registration.call_id.as_str())
            .collect();
        if registrations.iter().any(|registration| {
            registration.execution.task.0.is_nil()
                || registration.execution.instance.is_nil()
                || !ids.insert(registration.execution.task)
                || !calls.insert(registration.call_id.as_str())
        }) {
            return Err(ConversationFailure::new("异步任务登记身份重复或无效"));
        }
        Ok(())
    }

    pub(crate) fn execution(&self, call_id: &str) -> Option<TaskExecution> {
        self.tasks
            .iter()
            .find(|task| task.registration.call_id == call_id && task.live)
            .map(|task| task.registration.execution)
    }

    pub(crate) fn transition(
        &mut self,
        turn: TurnId,
        request: u32,
        call_id: &str,
        execution: Option<TaskExecution>,
        status: ToolStatus,
        sequence: u64,
        replay: bool,
    ) -> Result<bool, ConversationFailure> {
        let Some(index) =
            self.matching_transition(turn, request, call_id, execution, status, replay)?
        else {
            return Ok(false);
        };
        if sequence <= self.revision {
            return Err(ConversationFailure::new("异步任务状态序号过期"));
        }
        let task = self
            .tasks
            .get_mut(index)
            .ok_or_else(|| ConversationFailure::new("异步任务索引不存在"))?;
        task.status = status;
        task.live = status == ToolStatus::Running && !replay;
        if status != ToolStatus::Running {
            task.delivery = Delivery::Saved(sequence);
        }
        self.revision = sequence;
        Ok(true)
    }

    pub(crate) fn append_result(
        &mut self,
        call_id: &str,
        saved_sequence: u64,
        sequence: u64,
    ) -> Result<bool, ConversationFailure> {
        let Some(task) = self
            .tasks
            .iter_mut()
            .find(|task| task.registration.call_id == call_id)
        else {
            return Ok(false);
        };
        if task.delivery != Delivery::Saved(saved_sequence) {
            return Err(ConversationFailure::new(
                "异步任务结果追加缺少匹配的保存事实",
            ));
        }
        task.delivery = Delivery::Appended;
        self.delivery_revision = self.delivery_revision.max(sequence);
        Ok(true)
    }

    pub(crate) fn restore_appended<'a>(&mut self, call_ids: impl Iterator<Item = &'a str>) {
        let seen: HashSet<_> = call_ids.collect();
        for task in &mut self.tasks {
            if matches!(task.delivery, Delivery::AwaitingResult | Delivery::Saved(_))
                && seen.contains(task.registration.call_id.as_str())
            {
                task.delivery = Delivery::Appended;
            }
        }
    }

    pub(crate) fn matching_transition(
        &self,
        turn: TurnId,
        request: u32,
        call_id: &str,
        execution: Option<TaskExecution>,
        status: ToolStatus,
        replay: bool,
    ) -> Result<Option<usize>, ConversationFailure> {
        let Some((index, task)) = self
            .tasks
            .iter()
            .enumerate()
            .find(|(_, task)| task.registration.call_id == call_id)
        else {
            return if execution.is_none() {
                Ok(None)
            } else {
                Err(ConversationFailure::new("工具状态缺少对应异步任务"))
            };
        };
        if Some(task.registration.execution) != execution
            || task.registration.turn != turn
            || task.registration.request != request
            || (!replay && !task.live)
        {
            return Err(ConversationFailure::new("异步任务状态不属于当前执行实例"));
        }
        let allowed = match status {
            ToolStatus::Running => task.status == ToolStatus::Pending,
            ToolStatus::NotExecuted => {
                matches!(task.status, ToolStatus::Pending | ToolStatus::Running)
            }
            ToolStatus::Succeeded
            | ToolStatus::Failed
            | ToolStatus::TimedOut
            | ToolStatus::Unconfirmed
            | ToolStatus::ExternalUnconfirmed
            | ToolStatus::Cancelled => task.status == ToolStatus::Running,
            _ => false,
        };
        if !allowed {
            return Err(ConversationFailure::new("异步任务状态转换无效"));
        }
        Ok(Some(index))
    }

    pub(crate) fn recover_interrupted(&mut self) {
        for task in &mut self.tasks {
            task.recover_interrupted();
        }
    }

    pub(crate) fn recover_external_unconfirmed(&mut self, call_id: &str) {
        if let Some(task) = self
            .tasks
            .iter_mut()
            .find(|task| task.registration.call_id == call_id)
        {
            task.status = ToolStatus::ExternalUnconfirmed;
        }
    }

    pub(crate) fn confirm_recovery(&mut self, sequence: u64) {
        if !self.tasks.is_empty() {
            self.revision = self.revision.max(sequence);
        }
    }

    pub(crate) fn close_execution(&mut self, turn: TurnId) {
        for task in self
            .tasks
            .iter_mut()
            .filter(|task| task.registration.turn == turn && !task.in_progress())
        {
            task.live = false;
        }
    }

    pub(crate) fn settle_turn(&mut self, turn: TurnId, sequence: u64) -> bool {
        let mut changed = false;
        for task in self
            .tasks
            .iter_mut()
            .filter(|task| task.registration.turn == turn && task.live && !task.in_progress())
        {
            task.live = false;
            changed = true;
        }
        if changed {
            self.revision = sequence;
        }
        changed
    }

    pub(crate) fn has_ready_result(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.delivery == Delivery::Appended)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn selectable(&self, ids: &[Uuid], cutoff: u64) -> bool {
        ids.iter().all(|id| {
            self.tasks.iter().any(|task| {
                task.registration.execution.task.0 == *id && task.registered_at <= cutoff
            })
        })
    }

    pub(crate) fn cancellation(
        &self,
        scope: &agentelier_tools::TaskTargets,
        cutoff: u64,
    ) -> Option<TaskCancellationPlan> {
        let selected = match scope {
            agentelier_tools::TaskTargets::Ids(ids) => ids
                .iter()
                .map(|id| {
                    self.tasks.iter().find(|task| {
                        task.registration.execution.task.0 == *id && task.registered_at <= cutoff
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            agentelier_tools::TaskTargets::All => self
                .tasks
                .iter()
                .filter(|task| task.registered_at <= cutoff && task.live && task.in_progress())
                .collect(),
        };
        let mut plan = TaskCancellationPlan {
            targets: Vec::new(),
            receipts: Vec::new(),
        };
        for task in selected {
            let execution = task.registration.execution;
            let state = if !task.live || !task.in_progress() {
                TaskCancelState::Finished
            } else if task.cancellation_requested {
                TaskCancelState::AlreadyRequested
            } else {
                plan.targets.push(TaskCancellation {
                    call_id: task.registration.call_id.clone(),
                    execution,
                });
                TaskCancelState::Requested
            };
            plan.receipts.push(TaskCancelReceipt {
                task_id: execution.task,
                state,
                status: task.status,
            });
        }
        Some(plan)
    }

    pub(crate) fn cancellation_requested(&self, call_id: &str) -> bool {
        self.tasks
            .iter()
            .any(|task| task.registration.call_id == call_id && task.cancellation_requested)
    }

    pub(crate) fn request_cancel(
        &mut self,
        targets: &[TaskCancellation],
        sequence: u64,
    ) -> Result<(), ConversationFailure> {
        if targets.is_empty() || sequence <= self.revision {
            return Err(ConversationFailure::new("任务取消记录为空或过期"));
        }
        let mut indexes = HashSet::new();
        for target in targets {
            let index = self
                .tasks
                .iter()
                .position(|task| {
                    task.registration.call_id == target.call_id
                        && task.registration.execution == target.execution
                        && task.in_progress()
                        && !task.cancellation_requested
                })
                .ok_or_else(|| ConversationFailure::new("取消记录不属于可取消的执行实例"))?;
            if !indexes.insert(index) {
                return Err(ConversationFailure::new("取消记录包含重复目标"));
            }
        }
        for index in indexes {
            let task = self
                .tasks
                .get_mut(index)
                .ok_or_else(|| ConversationFailure::new("取消目标在确认时丢失"))?;
            task.cancellation_requested = true;
        }
        self.revision = sequence;
        Ok(())
    }

    pub(crate) fn reports(
        &self,
        ids: &[Uuid],
        tools: &agentelier_tools::ToolCatalog,
    ) -> Option<Vec<TaskReport>> {
        ids.iter()
            .map(|id| {
                self.tasks
                    .iter()
                    .find(|task| task.registration.execution.task.0 == *id)
                    .map(|task| task.report(tools))
            })
            .collect()
    }

    pub(crate) fn page(
        &self,
        offset: Option<usize>,
        tools: &agentelier_tools::ToolCatalog,
    ) -> TaskPage {
        let mut tasks = Vec::new();
        let mut history = Vec::new();
        for task in &self.tasks {
            if let Delivery::Delivered(sequence) = task.delivery {
                history.push((sequence, task));
            } else if offset.is_none() {
                tasks.push(task.report(tools));
            }
        }
        history.sort_by_key(|(sequence, _)| *sequence);
        let offset = offset.unwrap_or_default();
        let page_space = agentelier_tools::MAX_TASK_PAGE.saturating_sub(tasks.len());
        tasks.extend(
            history
                .iter()
                .skip(offset)
                .take(page_space)
                .map(|(_, task)| task.report(tools)),
        );
        let next = offset.saturating_add(page_space);
        TaskPage {
            tasks,
            next_history_offset: (next < history.len()).then_some(next),
            processes: Vec::new(),
            blocked_by_process: None,
        }
    }

    pub(crate) fn view(
        &self,
        session: SessionId,
        task_policy: &crate::TaskPolicy,
        queue_paused: bool,
        tools: Option<&agentelier_tools::ToolCatalog>,
    ) -> SessionTasks {
        let tasks = self
            .tasks
            .iter()
            .map(|task| {
                let registration = &task.registration;
                SessionTaskView {
                    cancelling: task.cancellation_requested && task.in_progress(),
                    task: registration.execution.task,
                    instance: task.live.then_some(registration.execution.instance),
                    turn: registration.turn,
                    request: registration.request,
                    call_id: registration.call_id.clone(),
                    tool: registration.tool.clone(),
                    status: task.status,
                    source: task.source(tools),
                    argument_bytes: registration.argument_bytes,
                    delivery: task.delivery(),
                    access: tools.map_or_default(|tools| tools.access(&registration.tool)),
                }
            })
            .collect();
        SessionTasks {
            session,
            revision: self.revision.max(self.delivery_revision),
            tasks,
            background_paused: task_policy.background_paused,
            queue_paused,
            external_unconfirmed: task_policy.external_unconfirmed.iter().cloned().collect(),
        }
    }
}
