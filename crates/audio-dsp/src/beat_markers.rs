//! Sound peaks: find the places where the music hits, without assuming a tempo, then pick as many
//! as a density slider asks for (Markers ▸ Mark Beats…). Works on free-time music as well as on
//! steady drums, because no beat grid is involved.
//!
//! ```text
//! mono mix ─► remix::onset_envelope (spectral flux)
//!   peak: a local maximum over ±120 ms, above the mean + 1.5 σ of the ±1.5 s around it
//!   strength: its height relative to the strongest peak, 0…1
//!   sections: where the mean level of the next 8 s differs from the last 8 s by ≥ 3 dB
//! select(density): spacing g = 8 · 2^(−density/25) seconds, keep the strongest peaks at least
//!   g/2 apart until there are duration / g of them; section changes always come first
//! ```
//!
//! | density | spacing | 3 min 40 of music |
//! |---|---|---|
//! | 0 | 8 s | ≈ 27 markers |
//! | 25 | 4 s | ≈ 55 |
//! | 50 | 2 s | ≈ 110 |
//! | 75 | 1 s | ≈ 220 (if the music has that many peaks) |
//! | 100 | 0.5 s | ≈ 440 (same) |
//!
//! Everything is deterministic.

use crate::remix::onset_envelope;

/// Half-width (seconds) of the neighbourhood a peak must dominate.
const PEAK_RADIUS: f64 = 0.12;
/// Half-width (seconds) of the window the peak threshold is measured on.
const THRESHOLD_RADIUS: f64 = 1.5;
/// A peak stands this many standard deviations above its surroundings.
const THRESHOLD_SIGMAS: f32 = 1.5;
/// Below this height (the envelope has unit standard deviation) nothing was really heard.
const MIN_HEIGHT: f32 = 1.0;
/// A peak quieter than this fraction of the loudest one (−30 dB) is background noise.
const MIN_LEVEL: f32 = 0.03;
/// Length of one loudness block (seconds).
const BLOCK: f64 = 0.5;
/// Blocks of loudness compared on each side of a candidate section change (8 s).
const WINDOW: usize = 16;
/// Smallest loudness change (dB) that makes a section change.
const MIN_CHANGE_DB: f32 = 3.0;
/// A section change takes the strongest peak this close to it (seconds).
const SECTION_SNAP: f64 = 1.0;

/// One place where the music hits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    /// Attack position (samples of the analysed signal).
    pub pos: i64,
    /// Height relative to the strongest peak, 0 … 1.
    pub strength: f32,
    /// A section change: the music gets clearly louder or quieter here.
    pub section: bool,
}

/// The peaks of a piece of music, in time order.
#[derive(Clone, Debug, PartialEq)]
pub struct PeakMap {
    pub peaks: Vec<Peak>,
    pub sample_rate: u32,
    pub samples: usize,
}

fn mono(channels: &[&[f32]]) -> Vec<f32> {
    let len = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    let count = channels.len().max(1) as f32;
    let mut out = vec![0f32; len];
    for ch in channels {
        for (o, v) in out.iter_mut().zip(ch.iter()) {
            *o += v / count;
        }
    }
    out
}

/// Frames of `env` that stand out from what surrounds them.
fn peak_frames(env: &[f32], radius: usize, wide: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for (t, &v) in env.iter().enumerate() {
        if v < MIN_HEIGHT {
            continue;
        }
        let near = env.get(t.saturating_sub(radius)..(t + radius + 1).min(env.len())).unwrap_or(&[]);
        if near.iter().any(|&w| w > v) {
            continue;
        }
        // on a plateau, the first frame counts
        if t > 0 && env.get(t - 1).is_some_and(|&w| w == v) {
            continue;
        }
        let around = env.get(t.saturating_sub(wide)..(t + wide + 1).min(env.len())).unwrap_or(&[]);
        let n = around.len().max(1) as f32;
        let mean = around.iter().sum::<f32>() / n;
        let sd = (around.iter().map(|w| (w - mean) * (w - mean)).sum::<f32>() / n).sqrt();
        if v > mean + THRESHOLD_SIGMAS * sd {
            out.push(t);
        }
    }
    out
}

/// Loudness (dB) of consecutive blocks of `x`.
fn block_db(x: &[f32], block: usize) -> Vec<f32> {
    x.chunks(block.max(1))
        .map(|c| {
            let power = c.iter().map(|v| v * v).sum::<f32>() / c.len().max(1) as f32;
            10.0 * power.max(1e-10).log10()
        })
        .collect()
}

/// Sample positions where the level changes for good: a drop, a chorus coming in, a breakdown.
fn section_changes(x: &[f32], sample_rate: u32) -> Vec<i64> {
    let block = (BLOCK * f64::from(sample_rate)) as usize;
    let db = block_db(x, block);
    if db.len() < 2 * WINDOW {
        return Vec::new();
    }
    let mean = |from: usize| db.iter().skip(from).take(WINDOW).sum::<f32>() / WINDOW as f32;
    let mut change = vec![0f32; db.len()];
    for (i, c) in change.iter_mut().enumerate().take(db.len() - WINDOW + 1).skip(WINDOW) {
        *c = (mean(i) - mean(i - WINDOW)).abs();
    }
    let mut out: Vec<usize> = Vec::new();
    for (i, &v) in change.iter().enumerate() {
        if v < MIN_CHANGE_DB {
            continue;
        }
        let near = change.iter().enumerate().skip(i.saturating_sub(WINDOW / 2)).take(WINDOW + 1);
        if near.into_iter().all(|(j, &w)| w < v || (w == v && j >= i)) && out.last().is_none_or(|&l| i >= l + WINDOW) {
            out.push(i);
        }
    }
    out.iter().map(|&i| i.saturating_mul(block) as i64).collect()
}

/// Find the peaks of a piece of music.
pub fn analyze(channels: &[&[f32]], sample_rate: u32) -> PeakMap {
    let x = mono(channels);
    let mut map = PeakMap { peaks: Vec::new(), sample_rate, samples: x.len() };
    if x.is_empty() || sample_rate == 0 {
        return map;
    }
    let (env, n, hop) = onset_envelope(&x, sample_rate);
    let hop = hop.max(1);
    let frames_per_second = f64::from(sample_rate) / hop as f64;
    let radius = ((PEAK_RADIUS * frames_per_second).round() as usize).max(1);
    let wide = ((THRESHOLD_RADIUS * frames_per_second).round() as usize).max(1);
    let frames = peak_frames(&env, radius, wide);
    let top = frames.iter().filter_map(|&t| env.get(t)).copied().fold(0f32, f32::max);
    if top <= 0.0 {
        return map;
    }
    // measured in remix::analyze: an attack sits at t·hop + n − ¾·hop
    let at = |t: usize| t.saturating_mul(hop).saturating_add(n).saturating_sub(3 * hop / 4);
    let level = |t: usize| {
        let from = at(t).min(x.len());
        let part = x.get(from..(from + n).min(x.len())).unwrap_or(&[]);
        (part.iter().map(|v| v * v).sum::<f32>() / part.len().max(1) as f32).sqrt()
    };
    let loudest = frames.iter().map(|&t| level(t)).fold(0f32, f32::max);
    for t in frames {
        if level(t) < loudest * MIN_LEVEL {
            continue;
        }
        let strength = (env.get(t).copied().unwrap_or(0.0) / top).clamp(0.0, 1.0);
        map.peaks.push(Peak { pos: at(t) as i64, strength, section: false });
    }
    let snap = (SECTION_SNAP * f64::from(sample_rate)) as i64;
    for at in section_changes(&x, sample_rate) {
        let near = map.peaks.iter_mut().filter(|p| (p.pos - at).abs() <= snap);
        if let Some(best) = near.max_by(|a, b| a.strength.total_cmp(&b.strength)) {
            best.section = true;
        }
    }
    map
}

/// Spacing (seconds) the density slider asks for: 8 at 0, 2 at 50, 0.5 at 100.
pub fn spacing(density: f64) -> f64 {
    let d = if density.is_finite() { density.clamp(0.0, 100.0) } else { 50.0 };
    8.0 * 2f64.powf(-d / 25.0)
}

/// The peaks to mark at `density` (0 … 100), in time order.
pub fn select(map: &PeakMap, density: f64) -> Vec<Peak> {
    let sr = f64::from(map.sample_rate.max(1));
    let sp = spacing(density);
    let target = ((map.samples as f64 / sr / sp).round() as usize).max(1);
    let gap = (sp / 2.0 * sr) as i64;
    let mut order: Vec<&Peak> = map.peaks.iter().collect();
    // section changes first, then the strongest; equal scores in time order
    order.sort_by(|a, b| b.section.cmp(&a.section).then(b.strength.total_cmp(&a.strength)).then(a.pos.cmp(&b.pos)));
    let mut taken: Vec<Peak> = Vec::new();
    for p in order {
        if taken.len() >= target && !p.section {
            break;
        }
        if taken.iter().all(|t| (t.pos - p.pos).abs() >= gap) {
            taken.push(*p);
        }
    }
    taken.sort_by_key(|p| p.pos);
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 22_050;

    /// Quiet noise with a decaying hit at each of `hits` (seconds, height).
    fn hits(seconds: f64, hits: &[(f64, f32)]) -> Vec<f32> {
        let len = (seconds * f64::from(SR)) as usize;
        let mut seed = 12_345u32;
        let mut x: Vec<f32> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((seed >> 16) & 0x7fff) as f32 / 32_768.0 * 0.002 - 0.001
            })
            .collect();
        for &(at, height) in hits {
            let start = (at * f64::from(SR)) as usize;
            for i in 0..(SR as usize / 5) {
                let t = i as f32 / SR as f32;
                if let Some(v) = x.get_mut(start + i) {
                    *v += height * (t * 2_000.0).sin() * (-t * 25.0).exp();
                }
            }
        }
        x
    }

    fn near(map: &[Peak], at: f64) -> bool {
        map.iter().any(|p| (p.pos as f64 / f64::from(SR) - at).abs() < 0.06)
    }

    #[test]
    fn spacing_follows_the_slider() {
        assert_eq!(spacing(0.0), 8.0);
        assert_eq!(spacing(50.0), 2.0);
        assert_eq!(spacing(100.0), 0.5);
        assert_eq!(spacing(-5.0), 8.0);
        assert_eq!(spacing(f64::NAN), 2.0);
    }

    #[test]
    fn markers_follow_irregular_hits() {
        // no tempo at all: gaps of 3.1, 1.7, 5.3, 2.2 s
        let times = [2.0, 5.1, 6.8, 12.1, 14.3];
        let list: Vec<(f64, f32)> = times.iter().map(|&t| (t, 0.5)).collect();
        let x = hits(18.0, &list);
        let map = analyze(&[&x], SR);
        let chosen = select(&map, 50.0);
        for t in times {
            assert!(near(&chosen, t), "no marker at {t} s: {chosen:?}");
        }
        assert_eq!(chosen.len(), times.len(), "{chosen:?}");
    }

    #[test]
    fn the_strongest_hits_win_at_low_density() {
        let mut list: Vec<(f64, f32)> = (0..12).map(|i| (1.0 + f64::from(i) * 1.9, 0.08)).collect();
        list[3].1 = 0.8;
        list[9].1 = 0.8;
        let x = hits(24.0, &list);
        let map = analyze(&[&x], SR);
        let sparse = select(&map, 0.0);
        assert!(sparse.len() <= 4, "{sparse:?}");
        assert!(near(&sparse, list[3].0) && near(&sparse, list[9].0), "{sparse:?}");
    }

    #[test]
    fn a_lasting_level_change_is_a_section() {
        let mut list: Vec<(f64, f32)> = (0..15).map(|i| (1.0 + f64::from(i), 0.05)).collect();
        list.extend((0..15).map(|i| (16.0 + f64::from(i), 0.5)));
        let x = hits(32.0, &list);
        let map = analyze(&[&x], SR);
        let sections: Vec<&Peak> = map.peaks.iter().filter(|p| p.section).collect();
        assert_eq!(sections.len(), 1, "{sections:?}");
        let at = sections[0].pos as f64 / f64::from(SR);
        assert!((at - 16.0).abs() < 1.5, "section at {at} s");
        assert!(select(&map, 0.0).iter().any(|p| p.section));
    }

    #[test]
    fn more_density_never_means_fewer_markers() {
        let list: Vec<(f64, f32)> = (0..40).map(|i| (0.5 + f64::from(i) * 0.61, 0.1 + 0.01 * i as f32)).collect();
        let x = hits(26.0, &list);
        let map = analyze(&[&x], SR);
        let (few, many) = (select(&map, 0.0).len(), select(&map, 100.0).len());
        assert!(few < many, "{few} !< {many}");
    }

    #[test]
    fn silence_and_nonsense_are_safe() {
        assert!(analyze(&[], SR).peaks.is_empty());
        let quiet = vec![0f32; SR as usize * 3];
        assert!(analyze(&[&quiet], SR).peaks.is_empty());
        assert!(analyze(&[&quiet], 0).peaks.is_empty());
        let map = PeakMap { peaks: Vec::new(), sample_rate: 0, samples: 0 };
        assert!(select(&map, 50.0).is_empty());
    }
}
