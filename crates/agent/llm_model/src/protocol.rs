use serde_json::Value;

#[derive(Clone)]
pub enum ProtocolItemSummary {
    User,
    FunctionCall { call_id: String, name: String },
    FunctionResult { call_id: String },
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}

#[derive(Clone)]
pub(crate) enum ProtocolItem {
    User(String),
    FunctionCall {
        raw: Value,
        call_id: String,
        name: String,
    },
    FunctionResult {
        call_id: String,
        output: String,
    },
    Assistant {
        raw: Value,
        phase: Option<String>,
    },
    Reasoning {
        raw: Value,
        encrypted: bool,
    },
}

pub struct ProtocolOutput(pub(crate) Vec<ProtocolItem>);

impl ProtocolOutput {
    pub(crate) fn is_replayable(&self) -> bool {
        self.0.iter().all(|item| {
            !matches!(
                item,
                ProtocolItem::Reasoning {
                    encrypted: false,
                    ..
                }
            )
        })
    }
}

#[derive(Clone, Default)]
pub struct ProtocolHistory(Vec<ProtocolItem>);

impl ProtocolHistory {
    pub fn with_user(&self, input: &str) -> Self {
        let mut history = self.clone();
        history.0.push(ProtocolItem::User(input.to_owned()));
        history
    }

    pub fn seal(
        &mut self,
        input: Option<&str>,
        output: ProtocolOutput,
        results: Vec<(String, String)>,
    ) -> Result<(), crate::ModelFailure> {
        let calls: Vec<_> = output
            .0
            .iter()
            .filter_map(|item| match item {
                ProtocolItem::FunctionCall { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        if calls.len() != results.len()
            || calls
                .iter()
                .zip(&results)
                .any(|(expected, (actual, _))| *expected != actual)
        {
            return Err(crate::model::protocol("工具调用与结果未封闭"));
        }
        if let Some(input) = input {
            self.0.push(ProtocolItem::User(input.to_owned()));
        }
        self.0.extend(output.0);
        self.0.extend(
            results
                .into_iter()
                .map(|(call_id, output)| ProtocolItem::FunctionResult { call_id, output }),
        );
        Ok(())
    }

    pub fn summary(&self) -> Vec<ProtocolItemSummary> {
        self.0
            .iter()
            .map(|item| match item {
                ProtocolItem::User(_) => ProtocolItemSummary::User,
                ProtocolItem::FunctionCall { call_id, name, .. } => {
                    ProtocolItemSummary::FunctionCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                    }
                }
                ProtocolItem::FunctionResult { call_id, .. } => {
                    ProtocolItemSummary::FunctionResult {
                        call_id: call_id.clone(),
                    }
                }
                ProtocolItem::Assistant { phase, .. } => ProtocolItemSummary::Assistant {
                    phase: phase.clone(),
                },
                ProtocolItem::Reasoning { encrypted, .. } => ProtocolItemSummary::Reasoning {
                    encrypted: *encrypted,
                },
            })
            .collect()
    }

    pub(crate) fn into_input(self) -> Vec<Value> {
        self.0
            .into_iter()
            .map(|item| match item {
                ProtocolItem::User(input) => serde_json::json!({"role": "user", "content": input}),
                ProtocolItem::Assistant { raw, .. } | ProtocolItem::Reasoning { raw, .. } | ProtocolItem::FunctionCall {raw, ..} => raw,
                ProtocolItem::FunctionResult {call_id, output} => serde_json::json!({"type":"function_call_output","call_id":call_id,"output":output}),
            })
            .collect()
    }
}
