use super::atlas::Entry;
use bevy::prelude::*;
use std::{f32::consts::PI, io};

const SAMPLE_WIDTH: u32 = 480;
const LANDMARKS: [(Vec2, f32, u8); 11] = [
    (Vec2::new(0.678, 0.265), 0.08, 1),
    (Vec2::new(0.748, 0.233), 0.21, 2),
    (Vec2::new(0.692, 0.075), 0.27, 1),
    (Vec2::new(0.812, 0.151), 0.16, 2),
    (Vec2::new(0.603, 0.769), 0.11, 3),
    (Vec2::new(0.777, 0.526), 0.26, 2),
    (Vec2::new(0.505, 0.435), 0.19, 0),
    (Vec2::new(0.386, 0.655), 0.39, 0),
    (Vec2::new(0.917, 0.615), 0.30, 2),
    (Vec2::new(0.746, 0.887), 0.20, 3),
    (Vec2::new(0.900, 0.893), 0.36, 3),
];

pub(super) struct Target {
    pub entry: Entry,
    pub uv: Vec2,
    pub height: f32,
    pub angle: f32,
    pub weight: f32,
    pub tone: f32,
    pub arrival: f32,
    pub route: u8,
}

pub(super) struct Rng(pub u32);

impl Rng {
    pub fn sample(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1_u32 << 24) as f32
    }

    pub fn index(&mut self, count: usize) -> usize {
        (self.sample() * count as f32) as usize
    }
}

struct Artwork {
    size: UVec2,
    luma: Vec<f32>,
}

impl Artwork {
    fn sample(image: &Image) -> Result<Self> {
        let source = image.size();
        if source.x == 0 || source.y == 0 {
            return Err(io::Error::other("Splash 背景尺寸为空").into());
        }
        let height = (SAMPLE_WIDTH * source.y).div_ceil(source.x);
        let size = UVec2::new(SAMPLE_WIDTH, height);
        let mut luma = Vec::with_capacity((size.x * size.y) as usize);
        let weights = Vec3::new(0.45, 0.46, 0.09);
        for y in 0..size.y {
            for x in 0..size.x {
                let uv = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) / size.as_vec2();
                let texel = (uv * source.as_vec2()).as_uvec2().min(source - UVec2::ONE);
                let srgba = image.get_color_at(texel.x, texel.y)?.to_srgba();
                let rgb = Vec3::new(srgba.red, srgba.green, srgba.blue);
                luma.push(rgb.dot(weights));
            }
        }
        Ok(Self { size, luma })
    }

    fn at(&self, coord: IVec2) -> f32 {
        let tile = coord.clamp(IVec2::ZERO, self.size.as_ivec2() - IVec2::ONE);
        self.luma[tile.y as usize * self.size.x as usize + tile.x as usize]
    }

    fn detail_at(&self, point: IVec2, uv: Vec2) -> (f32, f32, f32) {
        let center = self.at(point);
        let x = IVec2::new(3, 0);
        let y = IVec2::new(0, 3);
        let mean =
            (self.at(point - x) + self.at(point + x) + self.at(point - y) + self.at(point + y))
                * 0.25;
        let paper = 0.865 + 0.043 * ((uv.y * 1.28).min(1.0) * PI * 0.5).sin() - uv.x * 0.021;
        let edge = (((center - mean).abs() - 0.004) * 15.0).clamp(0.0, 1.0);
        let ink = ((paper - center - 0.012) * 7.3).clamp(0.0, 1.0);
        let highlight = ((center - paper - 0.016) * 7.7).clamp(0.0, 1.0);
        let face = region(uv, Vec2::new(0.715, 0.290), Vec2::new(0.109, 0.114));
        let hand = region(uv, Vec2::new(0.608, 0.763), Vec2::new(0.075, 0.100));
        let pale_stroke = ((center - 0.891) / 0.045).clamp(0.0, 1.0) * face.max(hand) * 1.28;
        let weight = (ink * 0.88).max(highlight * 0.80).max(pale_stroke) + edge * 0.58;
        let tone = ((center - 0.784) / 0.146).clamp(0.0, 1.0);
        (weight.clamp(0.0, 1.0), edge, tone)
    }

    fn tangent(&self, point: IVec2) -> f32 {
        let x = IVec2::new(2, 0);
        let y = IVec2::new(0, 2);
        let dx = self.at(point + x) - self.at(point - x);
        let dy = self.at(point + y) - self.at(point - y);
        ((-dx).atan2(dy) + PI * 0.5).rem_euclid(PI) - PI * 0.5
    }
}

fn region(uv: Vec2, center: Vec2, extent: Vec2) -> f32 {
    let distance = ((uv - center) / extent).length();
    ((1.0 - distance) * 2.0).clamp(0.0, 1.0)
}

pub(super) fn sample(image: &Image, entries: &[Entry]) -> Result<Vec<Target>> {
    let artwork = Artwork::sample(image)?;
    let compact: Vec<_> = entries
        .iter()
        .copied()
        .filter(|entry| entry.ratio <= 1.3)
        .collect();
    if entries.is_empty() || compact.is_empty() {
        return Err(io::Error::other("Splash 字形缺少人物成像所需的短碎片").into());
    }
    let size = image.size();
    let aspect = size.x as f32 / size.y as f32;
    let mut rng = Rng(13_491);
    let mut targets = Vec::new();
    let mut v = 0.004_f32;
    while v < 0.999 {
        let fine_rows = 1.0 - ((v - 0.255).abs() / 0.135).clamp(0.0, 1.0);
        let row_height = 0.0057 + rng.sample() * 0.0014 - fine_rows * 0.0018;
        let mut u = rng.sample() * 0.004;
        while u < 0.998 {
            let eye_region = (Vec2::new(u, v) - Vec2::new(0.719, 0.253)) / Vec2::new(0.078, 0.098);
            let eye = (1.0 - eye_region.length()).max(0.0);
            let pool = if rng.sample() < eye * 0.92 {
                &compact
            } else {
                entries
            };
            let entry = pool[rng.index(pool.len())];
            let height = (0.0091 - 0.0032 * eye) * (0.92 + rng.sample() * 0.19);
            let stride = height * (entry.ratio * 0.84 + 0.10) / aspect;
            let uv = Vec2::new(
                (u + stride * (0.29 + rng.sample() * 0.46)).min(0.999),
                (v + row_height * (0.24 + rng.sample() * 0.52)).min(0.999),
            );
            u += stride;
            let texel = (uv * artwork.size.as_vec2()).as_ivec2();
            let (weight, edge, tone) = artwork.detail_at(texel, uv);
            let weight = weight.max(eye * 0.67);
            if weight < 0.087 || rng.sample() > (0.21 + weight * 2.04 + eye * 0.36).min(1.0) {
                continue;
            }
            let angle = artwork.tangent(texel) * (0.34 + 0.42 * edge) * (1.0 - eye * 0.55);
            let (delay, route) = arrival_region(uv);
            targets.push(Target {
                entry,
                uv,
                height,
                angle,
                weight,
                tone,
                arrival: 1.47 + delay * 0.94 + rng.sample() * 0.62 - weight * 0.20 - eye * 0.16,
                route,
            });
        }
        v += row_height;
    }
    if targets.is_empty() {
        return Err(io::Error::other("Splash 背景未生成有效人物字形").into());
    }
    targets.sort_by(|a, b| a.arrival.total_cmp(&b.arrival));
    Ok(targets)
}

fn arrival_region(uv: Vec2) -> (f32, u8) {
    let mut delay = 1.0_f32;
    let mut route = 0;
    for (center, offset, branch) in LANDMARKS {
        let distance = ((uv - center) * Vec2::new(1.52, 0.83)).length();
        let candidate = offset + distance * 1.58;
        if candidate < delay {
            delay = candidate;
            route = branch;
        }
    }
    let jitter = 0.071 * (uv.x * 72.0 + uv.y * 45.0).sin() * (uv.y * 61.0 - uv.x * 17.0).cos();
    ((delay + jitter).clamp(0.0, 1.0), route)
}
