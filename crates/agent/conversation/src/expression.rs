use crate::{ActiveTurn, TurnMemory, TurnStatus, TurnView, behavior};
use agentelier_llm_model::{
    ExpressionContext, ExpressionExcerpt, MemorySnapshot, ProjectPublication, expression_excerpt,
};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

const MAX_SOURCE_TURNS: usize = 32;
const MAX_EXCERPTS: usize = 5;
const MAX_REFERENCES: usize = 8;
const MAX_REFERENCE_CHARS: usize = 160;

pub(crate) fn capture(active: &ActiveTurn<'_>, turn: &TurnView) -> Option<ExpressionContext> {
    let view = &active.view;
    let window = view.context.window;
    if window == 0
        || !view.settings.kind.is_user()
        || view.settings.background.is_some()
        || active.child.is_some()
        || !behavior::has_origin(view)
        || turn.result_continuation
        || !turn.selected_skills.is_empty()
        || matches!(turn.memory, Some(TurnMemory::Withdrawn { .. }))
        || !active.history.has_compaction()
    {
        return None;
    }
    if !turn.tools.is_empty() {
        return None;
    }
    let quotes = references(turn);
    if quotes.is_empty() {
        return None;
    }
    if let Some(saved) = turn
        .requests
        .iter()
        .rev()
        .find(|request| request.window == window)
    {
        return saved
            .projection
            .expression
            .as_ref()
            .filter(|material| {
                !material
                    .context
                    .excerpts
                    .iter()
                    .any(|excerpt| already_quoted(&excerpt.text, &quotes))
            })
            .map(|material| material.context.clone());
    }
    let resident = active.history.assistant_ids()?;
    let directory = &view.settings.working_directory;
    let mut excerpts = Vec::new();
    for prior in view.previous_turns.iter().rev().take(MAX_SOURCE_TURNS) {
        if let Some(excerpt) = from_turn(
            prior,
            window,
            directory,
            &view.project_publications,
            &resident,
            active.state.memory.as_deref(),
            &quotes,
        ) {
            excerpts.push(excerpt);
            if excerpts.len() == MAX_EXCERPTS {
                break;
            }
        }
    }
    (!excerpts.is_empty()).then(|| ExpressionContext {
        session: view.id,
        window,
        directory: directory.clone(),
        excerpts,
    })
}

pub(crate) fn from_turn(
    turn: &TurnView,
    window: u64,
    directory: &Path,
    publications: &[Arc<ProjectPublication>],
    resident: &HashSet<&str>,
    memory: Option<&MemorySnapshot>,
    quotes: &[&str],
) -> Option<ExpressionExcerpt> {
    if !matches!(turn.status, TurnStatus::Completed)
        || !turn.tools.is_empty()
        || turn.audio.is_some()
        || turn.transcript.is_some()
        || turn.result_continuation
        || !turn.selected_skills.is_empty()
        || turn.input.lines().nth(1).is_some()
        || expression_excerpt(&turn.input).is_none()
        || turn.requests.iter().any(|request| {
            request.projection.memory.as_ref().is_some_and(|old| {
                let old = &old.snapshot;
                let scope = old.scope.as_ref();
                !memory.is_some_and(|current| {
                    current.epoch == old.epoch
                        && current
                            .scope
                            .as_ref()
                            .zip(scope)
                            .is_some_and(|(current, old)| {
                                current.generation == old.generation
                                    && current.directory == old.directory
                            })
                })
            })
        })
    {
        return None;
    }
    let message = turn
        .messages
        .iter()
        .rev()
        .find(|message| match message.phase.as_deref() {
            Some("final_answer") => true,
            None => message.request_index == turn.request_index,
            _ => false,
        })?;
    let request = turn
        .requests
        .iter()
        .find(|request| request.request == message.request_index)?;
    let project = request
        .publication
        .and_then(|id| publications.iter().find(|project| project.id == id))?;
    if request.window >= window
        || request.response_accepted != Some(true)
        || message.id.is_empty()
        || resident.contains(message.id.as_str())
        || project.snapshot.working_directory != directory
    {
        return None;
    }
    let part = message
        .content
        .iter()
        .find(|part| !part.text.trim().is_empty())?;
    let text = expression_excerpt(&part.text)?;
    if already_quoted(&text, quotes)
        || !quotes
            .iter()
            .any(|&quote| message.content.iter().any(|part| part.text.contains(quote)))
    {
        return None;
    }
    Some(ExpressionExcerpt {
        turn: turn.id.0,
        message: message.id.clone(),
        part: part.content_index,
        text,
    })
}

fn already_quoted(opening: &str, quotes: &[&str]) -> bool {
    quotes
        .iter()
        .any(|&quote| quote.contains(opening) || opening.contains(quote))
}

pub(crate) fn references(turn: &TurnView) -> Vec<&str> {
    std::iter::once(turn.input.as_str())
        .chain(
            turn.steers
                .iter()
                .filter(|steer| matches!(steer.state, crate::SteerState::Applied))
                .map(|steer| steer.text.as_str()),
        )
        .flat_map(quoted_passages)
        .take(MAX_REFERENCES)
        .collect()
}

fn quoted_passages(input: &str) -> impl Iterator<Item = &str> {
    let mut open = None;
    input.char_indices().filter_map(move |(offset, character)| {
        if let Some((start, end)) = open {
            if character == end {
                open = None;
                let quote = input[start..offset].trim();
                return substantial_quote(quote).then_some(quote);
            }
            if character == '\n' {
                open = None;
            }
            return None;
        }
        let end = match character {
            '"' => '"',
            '“' => '”',
            '‘' => '’',
            '「' => '」',
            '『' => '』',
            '«' => '»',
            _ => return None,
        };
        open = Some((offset + character.len_utf8(), end));
        None
    })
}

fn substantial_quote(quote: &str) -> bool {
    quote.chars().take(MAX_REFERENCE_CHARS + 1).count() <= MAX_REFERENCE_CHARS
        && expression_excerpt(quote).is_some()
        && (quote
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphabetic))
            .take(4)
            .count()
            >= 4
            && quote
                .chars()
                .filter(|character| character.is_alphabetic())
                .take(16)
                .count()
                >= 16
            || quote
                .chars()
                .filter(|character| character.is_alphabetic() && !character.is_ascii())
                .take(12)
                .count()
                >= 12)
}
