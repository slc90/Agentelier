use bevy::prelude::Result;
use serde::{Deserialize, Serialize};
use std::io;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AxisKind {
    #[default]
    Number,
    Time,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum Coordinate {
    Number(f64),
    Timestamp(String),
}

impl AxisKind {
    pub(super) fn value(self, coordinate: Coordinate) -> Result<f64> {
        match (self, coordinate) {
            (Self::Number, Coordinate::Number(value)) if value.is_finite() => Ok(value),
            (Self::Time, Coordinate::Timestamp(value)) => OffsetDateTime::parse(&value, &Rfc3339)
                .map(|date| (date.unix_timestamp_nanos() as f64) / 1e9)
                .map_err(|_| io::Error::other("时间坐标必须是带时区的 RFC 3339 时间戳").into()),
            (Self::Number, _) => Err(io::Error::other("数值坐标必须是有限数字").into()),
            (Self::Time, _) => Err(io::Error::other("时间坐标必须是 RFC 3339 字符串").into()),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AxisSpec<T> {
    pub label: String,
    pub unit: Option<String>,
    pub range: Option<[T; 2]>,
}

#[derive(Clone, PartialEq)]
pub(super) struct Axis {
    pub range: Range,
    pub name: String,
    pub ticks: Vec<Tick>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Range {
    min: f64,
    span: f64,
}

#[derive(Clone, PartialEq)]
pub(super) struct Tick {
    pub fraction: f64,
    pub label: String,
}

impl Range {
    pub(super) fn new(
        explicit: Option<[f64; 2]>,
        values: impl IntoIterator<Item = f64>,
        kind: AxisKind,
    ) -> Result<Self> {
        let [min, max] = if let Some(range) = explicit {
            range
        } else {
            let extent = values
                .into_iter()
                .filter(|value| value.is_finite())
                .fold(None, |extent: Option<[f64; 2]>, value| {
                    Some(extent.map_or([value, value], |[low, high]| {
                        [low.min(value), high.max(value)]
                    }))
                })
                .ok_or_else(|| io::Error::other("无可用于自动确定范围的观测值，请提供轴范围"))?;
            if extent[0] == extent[1] {
                let value = extent[0];
                let pad = match kind {
                    AxisKind::Number => (value.abs() * 0.05).max(0.5),
                    AxisKind::Time => 30.0,
                };
                [value - pad, value + pad]
            } else {
                extent
            }
        };
        let span = max - min;
        if !min.is_finite() || !span.is_finite() || span <= 0.0 {
            return Err(io::Error::other("轴范围必须有限且从小到大").into());
        }
        Ok(Self { min, span })
    }

    pub(super) fn fraction(self, value: f64) -> f64 {
        (value - self.min) / self.span
    }
}

impl Axis {
    pub(super) fn new(
        label: String,
        unit: Option<String>,
        range: Range,
        kind: AxisKind,
        divisions: u8,
    ) -> Result<Self> {
        if label.trim().is_empty()
            || label.len() > 96
            || unit
                .as_ref()
                .is_some_and(|unit| unit.trim().is_empty() || unit.len() > 32)
        {
            return Err(io::Error::other("轴标签必须非空且最多 96 字节，单位最多 32 字节").into());
        }
        let name = match (kind, unit) {
            (AxisKind::Time, Some(unit)) => format!("{label} ({unit}, UTC)"),
            (AxisKind::Time, None) => format!("{label} (UTC)"),
            (AxisKind::Number, Some(unit)) => format!("{label} ({unit})"),
            (AxisKind::Number, None) => label,
        };
        let target = range.span / f64::from(divisions);
        let step = match kind {
            AxisKind::Number => nice_step(target),
            AxisKind::Time => time_step(target),
        };
        let start = (range.min / step - 1e-9).ceil() * step;
        let mut ticks: Vec<Tick> = Vec::new();
        for index in 0..=u16::from(divisions) + 1 {
            let value = start + f64::from(index) * step;
            let fraction = range.fraction(value);
            if !(-1e-8..=1.0 + 1e-8).contains(&fraction) {
                continue;
            }
            if let Some(last) = ticks.last()
                && fraction <= last.fraction
            {
                continue;
            }
            let label = match kind {
                AxisKind::Number => numeric_label(value, step),
                AxisKind::Time => time_label(value, step, range.span)?,
            };
            ticks.push(Tick {
                fraction: fraction.clamp(0.0, 1.0),
                label,
            });
        }
        if ticks.len() < 2 {
            for fraction in [0.0, 1.0] {
                let value = range.min + range.span * fraction;
                let label = match kind {
                    AxisKind::Number => numeric_label(value, range.span),
                    AxisKind::Time => time_label(value, range.span, range.span)?,
                };
                if fraction == 0.0 {
                    ticks.clear();
                }
                ticks.push(Tick { fraction, label });
            }
        }
        Ok(Self { range, name, ticks })
    }
}

fn nice_step(target: f64) -> f64 {
    if !target.is_normal() {
        return target.max(f64::MIN_POSITIVE);
    }
    let factor = 10.0_f64.powf(target.log10().floor());
    for unit in [1.0, 2.0, 2.5, 5.0, 10.0] {
        let step = factor * unit;
        if step >= target && step.is_finite() {
            return step;
        }
    }
    target
}

fn time_step(target: f64) -> f64 {
    if target < 1.0 {
        return nice_step(target);
    }
    const SECONDS: [f64; 24] = [
        1.,
        2.,
        5.,
        10.,
        15.,
        30.,
        60.,
        120.,
        300.,
        600.,
        900.,
        1_800.,
        3_600.,
        7_200.,
        10_800.,
        21_600.,
        43_200.,
        86_400.,
        172_800.,
        604_800.,
        1_209_600.,
        2_592_000.,
        7_776_000.,
        31_536_000.,
    ];
    SECONDS
        .into_iter()
        .find(|step| *step >= target)
        .unwrap_or_else(|| nice_step(target))
}

fn numeric_label(value: f64, step: f64) -> String {
    if value.abs() < step * 1e-8 {
        return "0".into();
    }
    if !(1e-4..1e7).contains(&value.abs()) {
        let precision = (value.abs().log10() - step.log10()).ceil().clamp(2.0, 12.0) as usize;
        return format!("{value:.precision$e}");
    }
    let mut precision = (-step.log10().floor()).clamp(0.0, 12.0) as usize;
    while precision < 12 {
        let scaled = step * 10.0_f64.powf(precision as f64);
        if (scaled - scaled.round()).abs() < 1e-6 {
            break;
        }
        precision += 1;
    }
    let label = format!("{value:.precision$}");
    if precision == 0 {
        label
    } else {
        label.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn time_label(value: f64, step: f64, span: f64) -> Result<String> {
    let date = OffsetDateTime::from_unix_timestamp_nanos((value * 1e9).round() as i128)
        .map_err(|_| io::Error::other("时间轴范围无法显示为 UTC 时间"))?;
    let (year, month, day) = (date.year(), u8::from(date.month()), date.day());
    let (hour, minute, second) = (date.hour(), date.minute(), date.second());
    if step >= 86_400.0 {
        Ok(format!("{year:04}-{month:02}-{day:02}"))
    } else if span >= 86_400.0 {
        Ok(format!("{month:02}-{day:02} {hour:02}:{minute:02}"))
    } else if step >= 60.0 {
        Ok(format!("{hour:02}:{minute:02}"))
    } else if step >= 1.0 {
        Ok(format!("{hour:02}:{minute:02}:{second:02}"))
    } else {
        let digits = (-step.log10().floor()).clamp(1.0, 9.0) as usize;
        let fraction = date.nanosecond() / 10_u32.pow((9 - digits) as u32);
        Ok(format!(
            "{hour:02}:{minute:02}:{second:02}.{fraction:0digits$}"
        ))
    }
}
