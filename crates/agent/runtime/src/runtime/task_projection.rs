use agentelier_conversation::{
    ConversationEvent, GlobalTask, GlobalTaskGroup, GlobalTaskProcess, GlobalTaskStatus,
    GlobalTasks, SessionId, SessionProcesses, SessionTaskView, SessionTasks, TaskDelivery, TaskId,
    TaskNotification, TaskNotificationKind, ToolStatus,
};
use agentelier_tools::{ProcessPhase, ToolAccess};
use std::collections::{BTreeMap, HashMap};

#[derive(Default)]
pub(super) struct Projection {
    sessions: BTreeMap<SessionId, Scope>,
    titles: HashMap<SessionId, String>,
    published: Vec<GlobalTaskGroup>,
    revision: u64,
}

#[derive(Default)]
struct Scope {
    tasks: Option<SessionTasks>,
    processes: HashMap<TaskId, Process>,
    process_revision: u64,
}

struct Process {
    phase: GlobalTaskProcess,
    succeeded: bool,
}

impl Projection {
    pub(super) fn seed_history(
        &mut self,
        history: Vec<agentelier_session_store::ArchivedTasks>,
    ) -> Option<ConversationEvent> {
        for item in history {
            self.sessions.entry(item.tasks.session).or_insert_with(|| {
                let mut scope = Scope::default();
                scope.update_processes(&item.processes);
                scope.tasks = Some(item.tasks);
                scope
            });
        }
        self.snapshot()
    }

    pub(super) fn observe(&mut self, event: &ConversationEvent) -> Vec<ConversationEvent> {
        let mut events = Vec::new();
        match event {
            ConversationEvent::Tasks(tasks) => {
                let scope = self.sessions.entry(tasks.session).or_default();
                if scope
                    .tasks
                    .as_ref()
                    .is_some_and(|old| old.revision > tasks.revision)
                {
                    return events;
                }
                let outcomes = scope.outcomes();
                let existing = scope.tasks.is_some();
                scope.tasks = Some(SessionTasks {
                    session: tasks.session,
                    revision: tasks.revision,
                    tasks: tasks.tasks.clone(),
                    background_paused: tasks.background_paused,
                    queue_paused: tasks.queue_paused,
                    external_unconfirmed: tasks.external_unconfirmed.clone(),
                });
                if existing {
                    collect_notifications(
                        &mut events,
                        scope,
                        &outcomes,
                        self.titles.get(&tasks.session),
                    );
                }
            }
            ConversationEvent::Processes(update) => {
                let scope = self.sessions.entry(update.session).or_default();
                if scope.process_revision > update.revision {
                    return events;
                }
                let outcomes = scope.outcomes();
                scope.update_processes(update);
                collect_notifications(
                    &mut events,
                    scope,
                    &outcomes,
                    self.titles.get(&update.session),
                );
            }
            ConversationEvent::Archives(entries) => {
                self.titles.extend(
                    entries
                        .iter()
                        .map(|entry| (entry.summary.session, entry.summary.title.clone())),
                );
            }
            ConversationEvent::MetadataSaved {
                session,
                result: Ok(entry),
            } => {
                self.titles.insert(*session, entry.summary.title.clone());
            }
            ConversationEvent::Deleted { session, .. } => {
                self.sessions.remove(session);
                self.titles.remove(session);
            }
            _ => return events,
        }
        if let Some(snapshot) = self.snapshot() {
            events.insert(0, snapshot);
        }
        events
    }

    fn snapshot(&mut self) -> Option<ConversationEvent> {
        let groups = self
            .sessions
            .iter()
            .filter_map(|(session, scope)| {
                let tasks = scope.visible();
                (!tasks.is_empty()).then(|| GlobalTaskGroup {
                    session: *session,
                    title: title(self.titles.get(session)).into(),
                    tasks,
                })
            })
            .collect::<Vec<_>>();
        if groups != self.published {
            self.revision = self.revision.saturating_add(1);
            self.published = groups.clone();
            Some(ConversationEvent::GlobalTasks(GlobalTasks {
                revision: self.revision,
                groups,
            }))
        } else {
            None
        }
    }
}

fn title(title: Option<&String>) -> &str {
    title.map_or("新会话", String::as_str)
}

fn collect_notifications(
    events: &mut Vec<ConversationEvent>,
    scope: &Scope,
    previous: &HashMap<TaskId, TaskNotificationKind>,
    session_title: Option<&String>,
) {
    if let Some(tasks) = &scope.tasks {
        for task in &tasks.tasks {
            if let Some(kind) = scope.outcome(task, tasks)
                && previous.get(&task.task).is_none()
            {
                tracing::debug!(session_id = %tasks.session, task_id = %task.task.0, "已发布后台任务状态通知");
                events.push(ConversationEvent::TaskNotification(TaskNotification {
                    task: task.task,
                    session: tasks.session,
                    session_title: title(session_title).into(),
                    name: task_name(&task.tool),
                    kind,
                }));
            }
        }
    }
}

impl Scope {
    fn update_processes(&mut self, update: &SessionProcesses) {
        self.process_revision = update.revision;
        for process in &update.processes {
            let Some(id) = process.origin.task_id else {
                continue;
            };
            let phase = match process.phase {
                ProcessPhase::Running => GlobalTaskProcess::Running,
                ProcessPhase::Exited | ProcessPhase::Consumed => GlobalTaskProcess::Exited,
                ProcessPhase::CleanupUnconfirmed | ProcessPhase::HistoricalUnconfirmed => {
                    GlobalTaskProcess::Unconfirmed
                }
            };
            self.processes.insert(
                TaskId(id),
                Process {
                    phase,
                    succeeded: process.exit_code == Some(0),
                },
            );
        }
    }

    fn outcomes(&self) -> HashMap<TaskId, TaskNotificationKind> {
        self.tasks.as_ref().map_or_else(HashMap::new, |tasks| {
            tasks
                .tasks
                .iter()
                .filter_map(|task| self.outcome(task, tasks).map(|kind| (task.task, kind)))
                .collect()
        })
    }

    fn outcome(
        &self,
        task: &SessionTaskView,
        tasks: &SessionTasks,
    ) -> Option<TaskNotificationKind> {
        if tasks.external_unconfirmed.contains(&task.call_id) {
            return Some(TaskNotificationKind::NeedsConfirmation);
        }
        if let Some(process) = self.processes.get(&task.task) {
            match process.phase {
                GlobalTaskProcess::Running => return None,
                GlobalTaskProcess::Unconfirmed => return Some(TaskNotificationKind::Failed),
                GlobalTaskProcess::Exited
                    if !process.succeeded && task.status != ToolStatus::Cancelled =>
                {
                    return Some(TaskNotificationKind::Failed);
                }
                _ => {}
            }
        }
        match task.status {
            ToolStatus::Succeeded => Some(TaskNotificationKind::Succeeded),
            ToolStatus::Failed
            | ToolStatus::TimedOut
            | ToolStatus::Unconfirmed
            | ToolStatus::ExternalUnconfirmed => Some(TaskNotificationKind::Failed),
            _ => None,
        }
    }

    fn visible(&self) -> Vec<GlobalTask> {
        let Some(tasks) = &self.tasks else {
            return Vec::new();
        };
        let process_running = self.processes.values().any(|process| {
            matches!(
                process.phase,
                GlobalTaskProcess::Running | GlobalTaskProcess::Unconfirmed
            )
        });
        let mut ordinary_running = false;
        let mut exclusive_running = process_running;
        for task in tasks
            .tasks
            .iter()
            .filter(|task| task.status == ToolStatus::Running)
        {
            ordinary_running |= task.access != ToolAccess::ProcessControl;
            exclusive_running |= task.access == ToolAccess::Exclusive;
        }
        tasks
            .tasks
            .iter()
            .filter_map(|task| {
                let process = self
                    .processes
                    .get(&task.task)
                    .map_or(GlobalTaskProcess::None, |process| process.phase);
                let waiting = task.access != ToolAccess::ProcessControl
                    && (exclusive_running
                        || task.access == ToolAccess::Exclusive && ordinary_running);
                let status = match (self.outcome(task, tasks), task.status) {
                    _ if task.cancelling => GlobalTaskStatus::Cancelling,
                    _ if process == GlobalTaskProcess::Running => GlobalTaskStatus::Running,
                    (Some(TaskNotificationKind::NeedsConfirmation), _) => {
                        GlobalTaskStatus::NeedsConfirmation
                    }
                    (Some(TaskNotificationKind::Failed), _) => GlobalTaskStatus::Failed,
                    (Some(TaskNotificationKind::Succeeded), _)
                        if task.delivery == TaskDelivery::Delivered =>
                    {
                        return None;
                    }
                    (Some(TaskNotificationKind::Succeeded), _) => GlobalTaskStatus::ResultReady,
                    (_, ToolStatus::Running) => GlobalTaskStatus::Running,
                    (_, ToolStatus::Pending) if waiting => GlobalTaskStatus::WaitingForResources,
                    (_, ToolStatus::Cancelled | ToolStatus::NotExecuted) => {
                        GlobalTaskStatus::Cancelled
                    }
                    _ => GlobalTaskStatus::Waiting,
                };
                Some(GlobalTask {
                    id: task.task,
                    name: task_name(&task.tool),
                    status,
                    delivery: task.delivery,
                    process,
                })
            })
            .collect()
    }
}

fn task_name(tool: &str) -> String {
    match tool {
        "exec_command" => "运行 PowerShell",
        "view_image" => "读取图片",
        "generate_image" => "生成图片",
        "apply_patch" => "修改文件",
        _ => tool,
    }
    .to_owned()
}
