use crate::{SessionView, SteerState, TurnView};
use agentelier_prompts::InstructionSlot;

const SCENARIOS: &[(&[&str], &str)] = &[
    (
        &[
            "不想分析",
            "只想说",
            "有点烦",
            "难过",
            "just vent",
            "don't want advice",
            "feeling upset",
        ],
        "For emotional sharing, respond to what the user is expressing. Provide analysis or advice only if they want it.",
    ),
    (
        &[
            "代码",
            "系统设计",
            "架构",
            "bevy",
            "rust",
            "code",
            "system design",
            "architecture",
        ],
        "In design discussions, clarify responsibilities and trade-offs. Keep the answer at the design level when requested, and wait for an implementation request before writing code or changing files.",
    ),
    (
        &[
            "数学",
            "整除",
            "反例",
            "证明",
            "定理",
            "prime number",
            "divisible",
            "counterexample",
            "prove that",
            "theorem",
        ],
        "For mathematical reasoning, make the definitions and reasoning complete. Check claims and use a counterexample when disproving one.",
    ),
    (
        &[
            "科研",
            "合成资料",
            "方差",
            "因果",
            "相关性",
            "研究数据",
            "variance",
            "causal",
            "random assignment",
            "research evidence",
        ],
        "For research, distinguish evidence, method, inference, and what is unknown. Use actual sources and qualify statistical or causal conclusions.",
    ),
    (
        &[
            "动漫",
            "分镜",
            "镜头",
            "审美",
            "anime",
            "storyboard",
            "shot composition",
        ],
        "For aesthetic discussion, state your own preference with concrete reasons and respect the user's requested scope. Describe the provided work without inventing personal experiences.",
    ),
    (
        &[
            "正式文档",
            "设计说明",
            "整理成",
            "写一份",
            "撰写",
            "design document",
            "design note",
            "write a report",
            "formal document",
        ],
        "For a formal artifact, write for its intended reader and use its appropriate register. Preserve precise terminology and evidence.",
    ),
    (
        &[
            "创作",
            "小册子",
            "创意",
            "生成草稿",
            "create a draft",
            "booklet",
            "creative concept",
        ],
        "For creative work, use the requested medium and scope. Keep discussion exploratory until the user explicitly asks for an artifact.",
    ),
];

const REPORT_TERMS: &[&str] = &[
    "后台",
    "任务进度",
    "执行状态",
    "任务结果",
    "background task",
    "task progress",
    "task status",
];
const REPORT_GUIDANCE: &str = "Report progress, blockers, completion and deliverables from recorded tool results and actual state. For unfinished work, name the current stage and the decision needed. Verify an artifact before claiming delivery.";
const INTENT_GUIDANCE: &str = "Adapt this request to the user's current intent and the applicable project rules. Apply the following guidance only where it fits:";

pub(crate) fn for_request(view: &SessionView, turn: &TurnView) -> Option<String> {
    has_origin(view).then_some(())?;
    let input = turn
        .steers
        .iter()
        .rev()
        .find(|input| matches!(input.state, SteerState::Applied) && !input.text.trim().is_empty())
        .map_or(turn.input.as_str(), |input| input.text.as_str())
        .to_lowercase();
    let mut guidance = SCENARIOS
        .iter()
        .filter_map(|(terms, hint)| contains_term(&input, terms).then_some(*hint))
        .collect::<Vec<_>>();
    if turn.result_continuation || !turn.tools.is_empty() || contains_term(&input, REPORT_TERMS) {
        guidance.push(REPORT_GUIDANCE);
    }
    if guidance.is_empty() {
        return None;
    }
    let guidance = guidance.join("\n");
    Some(format!("{INTENT_GUIDANCE}\n{guidance}"))
}

pub(crate) fn has_origin(view: &SessionView) -> bool {
    view.settings
        .instructions
        .instructions
        .iter()
        .any(|instruction| instruction.slot == InstructionSlot::Origin)
}

fn contains_term(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| {
        text.match_indices(*term).any(|(start, value)| {
            let end = start + value.len();
            !term.is_ascii()
                || (!text[..start].ends_with(|character: char| character.is_ascii_alphanumeric())
                    && !text[end..]
                        .starts_with(|character: char| character.is_ascii_alphanumeric()))
        })
    })
}
