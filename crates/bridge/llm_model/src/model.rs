use std::fmt;

#[derive(Clone)]
pub struct ModelInfo {
    pub slug: String,
    pub display_name: String,
    pub supported_reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
}

pub enum ModelRequest {
    Catalog,
    Verify {
        slug: String,
        display_name: String,
        effort: Option<String>,
    },
}

pub enum ModelResult {
    Catalog(Vec<ModelInfo>),
    Verified(ModelReply),
}

pub fn request(request: ModelRequest) -> agentelier_llm_model::ModelRequest {
    match request {
        ModelRequest::Catalog => agentelier_llm_model::ModelRequest::Catalog,
        ModelRequest::Verify {
            slug,
            display_name,
            effort,
        } => agentelier_llm_model::ModelRequest::Verify {
            slug,
            display_name,
            effort,
        },
    }
}

pub struct ModelReply {
    pub slug: String,
    pub effort: Option<String>,
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
    Completed(Result<ModelResult, ModelFailure>),
    Cancelled,
}

pub fn convert(
    result: Option<Result<agentelier_llm_model::ModelResult, agentelier_llm_model::ModelFailure>>,
) -> ModelEventKind {
    match result {
        None => ModelEventKind::Cancelled,
        Some(result) => ModelEventKind::Completed(
            result
                .map(|result| match result {
                    agentelier_llm_model::ModelResult::Catalog(models) => ModelResult::Catalog(
                        models
                            .into_iter()
                            .map(|model| ModelInfo {
                                slug: model.slug,
                                display_name: model.display_name,
                                supported_reasoning_levels: model.supported_reasoning_levels,
                                default_reasoning_level: model.default_reasoning_level,
                            })
                            .collect(),
                    ),
                    agentelier_llm_model::ModelResult::Verified(reply) => {
                        ModelResult::Verified(ModelReply {
                            slug: reply.slug,
                            effort: reply.effort,
                            model: reply.model,
                            text: reply.text,
                        })
                    }
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
