use serde_json::Value;

#[derive(Clone)]
pub enum ProtocolItemSummary {
    User,
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}

#[derive(Clone)]
pub(crate) enum ProtocolItem {
    User(String),
    Assistant { raw: Value, phase: Option<String> },
    Reasoning { raw: Value, encrypted: bool },
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

    pub fn commit(&mut self, input: &str, output: ProtocolOutput) {
        self.0.push(ProtocolItem::User(input.to_owned()));
        self.0.extend(output.0);
    }

    pub fn summary(&self) -> Vec<ProtocolItemSummary> {
        self.0
            .iter()
            .map(|item| match item {
                ProtocolItem::User(_) => ProtocolItemSummary::User,
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
                ProtocolItem::Assistant { raw, .. } | ProtocolItem::Reasoning { raw, .. } => raw,
            })
            .collect()
    }
}
