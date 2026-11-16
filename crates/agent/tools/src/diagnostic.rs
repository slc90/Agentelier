#[derive(Clone, Copy)]
pub(crate) enum Diagnostic {
    Calculate,
    Wait,
}
use crate::execution::ToolFailure;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const MAX_WAIT_MILLISECONDS: u64 = 30_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Calculation {
    a: f64,
    b: f64,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Add,
    Subtract,
    Multiply,
    Divide,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    milliseconds: u64,
}

pub(crate) async fn run(implementation: Diagnostic, arguments: &str) -> Result<Value, ToolFailure> {
    let receipt = || uuid::Uuid::new_v4().to_string();
    match implementation {
        Diagnostic::Calculate => {
            let input: Calculation = serde_json::from_str(arguments).map_err(|_| {
                ToolFailure::new("invalid_arguments", "计算参数格式、字段或操作无效")
            })?;
            if !input.a.is_finite() || !input.b.is_finite() {
                return Err(ToolFailure::new(
                    "invalid_arguments",
                    "计算输入必须是有限数",
                ));
            }
            let result = match input.operation {
                Operation::Add => input.a + input.b,
                Operation::Subtract => input.a - input.b,
                Operation::Multiply => input.a * input.b,
                Operation::Divide if input.b == 0.0 => {
                    return Err(ToolFailure::new("division_by_zero", "不能除以零"));
                }
                Operation::Divide => input.a / input.b,
            };
            if !result.is_finite() {
                return Err(ToolFailure::new(
                    "non_finite_result",
                    "计算结果超出有限数范围",
                ));
            }
            Ok(json!({"result":result, "receipt":receipt()}))
        }
        Diagnostic::Wait => {
            let input: Wait = serde_json::from_str(arguments)
                .map_err(|_| ToolFailure::new("invalid_arguments", "等待参数格式或字段无效"))?;
            if input.milliseconds > MAX_WAIT_MILLISECONDS {
                return Err(ToolFailure::new(
                    "invalid_arguments",
                    "等待时间必须在 0–30000 毫秒之间",
                ));
            }
            let started = Instant::now();
            tokio::time::sleep(Duration::from_millis(input.milliseconds)).await;
            Ok(json!({"elapsed_ms":started.elapsed().as_millis(), "receipt":receipt()}))
        }
    }
}
