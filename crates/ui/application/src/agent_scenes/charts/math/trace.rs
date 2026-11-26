use super::super::plot;
use bevy::math::DVec2;
use bevy_widgetry::canvas::CanvasPathCommand;

pub(super) fn trace(
    commands: &mut Vec<CanvasPathCommand>,
    domain: [f64; 2],
    steps: usize,
    mut evaluate: impl FnMut(f64) -> Option<DVec2>,
) {
    let mut run = Vec::new();
    let mut previous = Sample {
        parameter: domain[0],
        at: evaluate(domain[0]),
    };
    if let Some(point) = previous.at {
        run.push(point);
    }
    for index in 1..=steps {
        let mix = index as f64 / steps as f64;
        let parameter = domain[0] * (1.0 - mix) + domain[1] * mix;
        let next = Sample {
            parameter,
            at: evaluate(parameter),
        };
        refine(commands, &mut run, previous, next, &mut evaluate, 0);
        previous = next;
    }
    plot::append_line(commands, &run);
}

#[derive(Clone, Copy)]
struct Sample {
    parameter: f64,
    at: Option<DVec2>,
}

fn refine(
    commands: &mut Vec<CanvasPathCommand>,
    run: &mut Vec<DVec2>,
    from: Sample,
    to: Sample,
    evaluate: &mut impl FnMut(f64) -> Option<DVec2>,
    depth: usize,
) {
    let parameter = from.parameter.midpoint(to.parameter);
    let halfway = Sample {
        parameter,
        at: evaluate(parameter),
    };
    if let (Some(start), Some(mid), Some(end)) = (from.at, halfway.at, to.at) {
        let chord = start * 0.5 + end * 0.5;
        let error = (mid - chord).abs().max_element();
        let span = (start - end).abs().max_element();
        if error <= 0.003 || (depth == 4 && error <= (0.05 + span * 0.04).min(0.2)) {
            run.push(end);
            return;
        }
    }
    if depth < 4
        && parameter != from.parameter
        && parameter != to.parameter
        && (from.at.is_some() || halfway.at.is_some() || to.at.is_some())
    {
        refine(commands, run, from, halfway, evaluate, depth + 1);
        refine(commands, run, halfway, to, evaluate, depth + 1);
        return;
    }
    plot::append_line(commands, run);
    run.clear();
    if let Some(end) = to.at {
        run.push(end);
    }
}
