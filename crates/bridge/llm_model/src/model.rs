use std::fmt;

pub struct ModelReply {
    pub model: String,
    pub text: String,
}
#[derive(Clone, Copy, Debug)]
pub enum ModelFailureKind {
    Authentication,
    Permission,
    UsageLimit,
    Service,
    Network,
    Protocol,
    Incomplete,
}
pub struct ModelFailure {
    pub kind: ModelFailureKind,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub request_id: Option<String>,
    pub message: String,
}
impl fmt::Display for ModelFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(status) = self.status {
            write!(f, "；HTTP {status}")?;
        }
        if let Some(code) = &self.code {
            write!(f, "；{code}")?;
        }
        if let Some(id) = &self.request_id {
            write!(f, "；请求 {id}")?;
        }
        Ok(())
    }
}
pub enum ModelEventKind {
    Completed(Result<ModelReply, ModelFailure>),
    Cancelled,
}

pub fn convert(
    result: Option<Result<agentelier_llm_model::ModelReply, agentelier_llm_model::ModelFailure>>,
) -> ModelEventKind {
    match result {
        None => ModelEventKind::Cancelled,
        Some(result) => ModelEventKind::Completed(
            result
                .map(|reply| ModelReply {
                    model: reply.model,
                    text: reply.text,
                })
                .map_err(|failure| ModelFailure {
                    kind: match failure.kind {
                        agentelier_llm_model::ModelFailureKind::Authentication => {
                            ModelFailureKind::Authentication
                        }
                        agentelier_llm_model::ModelFailureKind::Permission => {
                            ModelFailureKind::Permission
                        }
                        agentelier_llm_model::ModelFailureKind::UsageLimit => {
                            ModelFailureKind::UsageLimit
                        }
                        agentelier_llm_model::ModelFailureKind::Service => {
                            ModelFailureKind::Service
                        }
                        agentelier_llm_model::ModelFailureKind::Network => {
                            ModelFailureKind::Network
                        }
                        agentelier_llm_model::ModelFailureKind::Protocol => {
                            ModelFailureKind::Protocol
                        }
                        agentelier_llm_model::ModelFailureKind::Incomplete => {
                            ModelFailureKind::Incomplete
                        }
                    },
                    status: failure.status,
                    code: failure.code,
                    request_id: failure.request_id,
                    message: failure.message,
                }),
        ),
    }
}
