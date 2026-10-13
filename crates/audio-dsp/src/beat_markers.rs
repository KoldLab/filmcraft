//! Beat markers: rank the beats of a piece of music by how good a place to cut each one is, then
//! pick as many as a density slider asks for (Markers ▸ Mark Beats…).
//!
//! ```text
//! remix::analyze ─► beats, beat self-similarity, onset strength and loudness per beat
//!   downbeat: the bar phase (0…3) where the harmony / timbre changes most (chords change on bars)
//!   sections: where the loudness of the two bars after a beat differs from the two bars before
//!             by ≥ 3 dB (a drop, a chorus coming in, a breakdown), snapped to the nearest downbeat
//!   level of each beat, counting bars from the last section start:
//!     Section > Phrase (every 4 bars) > TwoBars > Bar > HalfBar (beat 3) > Beat
//!   score = level + accent (the beat's onset strength, 0…1)
//! select(density): spacing g = 16 · 2^(−density/25) beats; take the beats of every level at
//!   least as coarse as the nearest power of two ≤ g, best score first, until there are
//!   beats / g of them
//! ```
//!
//! | density | spacing | at 120 BPM, 60 s (120 beats) |
//! |---|---|---|
//! | 0 | 16 beats (4 bars) | ≈ 8 markers |
//! | 25 | 8 beats (2 bars) | ≈ 15 |
//! | 50 | 4 beats (1 bar) | ≈ 30 |
//! | 75 | 2 beats | ≈ 60 |
//! | 100 | every beat | ≈ 120 |
//!
//! Between two stops the coarser level is kept whole and the beats of the finer level are added
//! strongest first, so moving the slider adds cuts where the music hits hardest. Section starts
//! are always eligible. Everything is deterministic.

use crate::remix::{Analysis, beat_lead};

/// Beats per bar (4/4 is assumed).
pub const BEATS_PER_BAR: usize = 4;
/// Beats of loudness compared on each side of a candidate section start (two bars).
const WINDOW: usize = 8;
/// Smallest loudness change (dB) that starts a section: a drop, a chorus coming in, a breakdown.
const MIN_CHANGE_DB: f32 = 3.0;

/// How structurally important a beat is (higher is coarser).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Beat = 0,
    HalfBar = 1,
    Bar = 2,
    TwoBars = 3,
    Phrase = 4,
    Section = 5,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Beat => "beat",
            Level::HalfBar => "halfBar",
            Level::Bar => "bar",
            Level::TwoBars => "twoBars",
            Level::Phrase => "phrase",
            Level::Section => "section",
        }
    }
}

/// One ranked beat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RankedBeat {
    /// Index in [`Analysis::beats`].
    pub index: usize,
    /// Attack position (samples of the analysed signal).
    pub pos: i64,
    pub level: Level,
    /// Onset strength relative to the strongest beat, 0 … 1.
    pub accent: f32,
    /// `level + 0.99 · accent`: the order beats are picked in.
    pub score: f32,
}

/// The ranked beats of a piece of music.
#[derive(Clone, Debug, PartialEq)]
pub struct BeatGrid {
    pub beats: Vec<RankedBeat>,
    /// Index of the first downbeat (0 … 3).
    pub downbeat: usize,
    /// Beat indices where a new section starts.
    pub sections: Vec<usize>,
    pub bpm: f64,
}

fn sim(a: &Analysis, i: usize, j: usize) -> f32 {
    let m = a.beats.len();
    i.checked_mul(m).and_then(|r| r.checked_add(j)).and_then(|k| a.sim.get(k)).copied().unwrap_or(0.0)
}

/// Bar phase of the downbeats: where the harmony changes most, onset strength as a tie-break.
fn downbeat_phase(a: &Analysis, accent: &[f32]) -> usize {
    let m = a.beats.len();
    let mut best = (f32::MIN, 0usize);
    for p in 0..BEATS_PER_BAR {
        let (mut s, mut n) = (0f32, 0f32);
        for i in (p..m).step_by(BEATS_PER_BAR) {
            let change = if i > 0 { 1.0 - sim(a, i - 1, i) } else { 0.0 };
            s += change + 0.25 * accent.get(i).copied().unwrap_or(0.0);
            n += 1.0;
        }
        let v = if n > 0.0 { s / n } else { 0.0 };
        if v > best.0 {
            best = (v, p);
        }
    }
    best.1
}

/// Loudness novelty at every beat: the change (dB) between the mean loudness of the [`WINDOW`]
/// beats after and the ones before. 0 near the edges.
fn novelty(a: &Analysis) -> Vec<f32> {
    let m = a.beats.len();
    let mut out = vec![0f32; m];
    if m < 2 * WINDOW {
        return out;
    }
    let mean = |from: usize| a.loudness.iter().skip(from).take(WINDOW).sum::<f32>() / WINDOW as f32;
    for (i, o) in out.iter_mut().enumerate().take(m - WINDOW + 1).skip(WINDOW) {
        *o = (mean(i) - mean(i - WINDOW)).abs();
    }
    out
}

/// Section starts: novelty peaks of at least [`MIN_CHANGE_DB`] (local maxima over ±one bar),
/// snapped to the nearest downbeat, at least two bars apart.
fn sections(nov: &[f32], downbeat: usize) -> Vec<usize> {
    let r = BEATS_PER_BAR;
    let mut out: Vec<usize> = Vec::new();
    for (i, &v) in nov.iter().enumerate() {
        if v < MIN_CHANGE_DB {
            continue;
        }
        let lo = i.saturating_sub(r);
        let is_peak = nov.iter().enumerate().skip(lo).take(2 * r + 1).all(|(j, &w)| w < v || (w == v && j >= i));
        if !is_peak {
            continue;
        }
        // nearest downbeat (ties go earlier)
        let off = (i + BEATS_PER_BAR - downbeat % BEATS_PER_BAR) % BEATS_PER_BAR;
        let snapped = if off <= BEATS_PER_BAR / 2 { i.saturating_sub(off) } else { i + (BEATS_PER_BAR - off) };
        if snapped >= nov.len() {
            continue;
        }
        if out.last().is_none_or(|&l| snapped >= l + 2 * BEATS_PER_BAR) {
            out.push(snapped);
        }
    }
    out
}

/// Rank the beats of analysed music.
pub fn grid(a: &Analysis) -> BeatGrid {
    let m = a.beats.len();
    let top = a.strength.iter().copied().fold(0f32, f32::max);
    let accent: Vec<f32> = (0..m).map(|i| if top > 0.0 { (a.strength.get(i).copied().unwrap_or(0.0) / top).clamp(0.0, 1.0) } else { 0.0 }).collect();
    let downbeat = downbeat_phase(a, &accent);
    let sections = sections(&novelty(a), downbeat);
    let lead = beat_lead(a.sample_rate);
    let mut beats = Vec::with_capacity(m);
    let mut origin = downbeat; // bars count from the last section start (or the first downbeat)
    for (i, &b) in a.beats.iter().enumerate() {
        let is_section = sections.contains(&i);
        if is_section {
            origin = i;
        }
        let level = if is_section {
            Level::Section
        } else if i < origin {
            // pickup beats before the first downbeat
            if (origin - i).is_multiple_of(2) { Level::HalfBar } else { Level::Beat }
        } else {
            let rel = i - origin;
            let (bar, pos) = (rel / BEATS_PER_BAR, rel % BEATS_PER_BAR);
            match pos {
                0 if bar.is_multiple_of(4) => Level::Phrase,
                0 if bar.is_multiple_of(2) => Level::TwoBars,
                0 => Level::Bar,
                2 => Level::HalfBar,
                _ => Level::Beat,
            }
        };
        let acc = accent.get(i).copied().unwrap_or(0.0);
        beats.push(RankedBeat { index: i, pos: b.saturating_add(lead), level, accent: acc, score: level as u8 as f32 + 0.99 * acc });
    }
    BeatGrid { beats, downbeat, sections, bpm: a.bpm }
}

/// Spacing (in beats) the density slider asks for: 16 at 0, 4 at 50, 1 at 100.
pub fn spacing(density: f64) -> f64 {
    let d = if density.is_finite() { density.clamp(0.0, 100.0) } else { 50.0 };
    16.0 * 2f64.powf(-d / 25.0)
}

/// The coarsest level a spacing (beats) keeps whole.
fn level_for(spacing: f64) -> (Level, usize) {
    if spacing >= 16.0 {
        (Level::Phrase, 16)
    } else if spacing >= 8.0 {
        (Level::TwoBars, 8)
    } else if spacing >= 4.0 {
        (Level::Bar, 4)
    } else if spacing >= 2.0 {
        (Level::HalfBar, 2)
    } else {
        (Level::Beat, 1)
    }
}

/// The beats to mark at `density` (0 … 100), in time order.
pub fn select(g: &BeatGrid, density: f64) -> Vec<RankedBeat> {
    let m = g.beats.len();
    if m == 0 {
        return Vec::new();
    }
    let sp = spacing(density);
    let (min_level, step) = level_for(sp);
    let target = ((m as f64 / sp).round() as usize).clamp(1, m);
    let mut cands: Vec<&RankedBeat> = g.beats.iter().filter(|b| b.level >= min_level).collect();
    // best first; equal scores in time order
    cands.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.index.cmp(&b.index)));
    // ¾ of the step: a section start may fall off the bar grid of the previous section
    let min_gap = (step * 3).div_ceil(4).max(1);
    let mut taken: Vec<usize> = Vec::new();
    for c in cands {
        if taken.len() >= target {
            break;
        }
        if taken.iter().all(|&t| t.abs_diff(c.index) >= min_gap) {
            taken.push(c.index);
        }
    }
    taken.sort_unstable();
    taken.iter().filter_map(|&i| g.beats.get(i).copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remix::{analyze, test_music};

    const SR: u32 = 22_050;

    fn grid_of(bpm: f64, bars: usize) -> (BeatGrid, Vec<i64>) {
        let (m, truth) = test_music(SR, bpm, bars, 7);
        let refs: Vec<&[f32]> = m.iter().map(Vec::as_slice).collect();
        (grid(&analyze(&refs, SR).unwrap()), truth)
    }

    /// Index in `truth` of the true beat nearest `pos`, and the distance in seconds.
    fn nearest(truth: &[i64], pos: i64) -> (usize, f64) {
        let (i, d) = truth.iter().enumerate().map(|(i, &t)| (i, (t - pos).abs())).min_by_key(|&(_, d)| d).unwrap();
        (i, d as f64 / SR as f64)
    }

    #[test]
    fn spacing_follows_the_slider() {
        assert_eq!(spacing(0.0), 16.0);
        assert_eq!(spacing(50.0), 4.0);
        assert_eq!(spacing(100.0), 1.0);
        assert_eq!(spacing(-5.0), 16.0);
        assert_eq!(spacing(f64::NAN), 4.0);
    }

    #[test]
    fn markers_land_on_beats_and_bars() {
        let (g, truth) = grid_of(120.0, 24);
        // the downbeat is a true bar start
        let first_down = g.beats.iter().find(|b| b.level >= Level::Bar).unwrap();
        let (i, d) = nearest(&truth, first_down.pos);
        assert!(d < 0.03, "first downbeat {d} s off");
        assert_eq!(i % 4, 0, "downbeat on beat {i}");
        // density 50: one marker per bar, every one on a true bar start
        let bars = select(&g, 50.0);
        assert!((20..=25).contains(&bars.len()), "{} markers", bars.len());
        for b in &bars {
            let (i, d) = nearest(&truth, b.pos);
            assert!(d < 0.03 && i % 4 == 0, "marker {i} ({d} s)");
        }
    }

    #[test]
    fn loudness_changes_start_sections() {
        // bars 8 … 15 three times louder (+9.5 dB): sections at bars 8 and 16
        let (mut m, truth) = test_music(SR, 120.0, 24, 7);
        let (b8, b16) = (truth[32] as usize, truth[64] as usize);
        for ch in &mut m {
            ch[b8..b16].iter_mut().for_each(|v| *v *= 3.0);
        }
        let refs: Vec<&[f32]> = m.iter().map(Vec::as_slice).collect();
        let g = grid(&analyze(&refs, SR).unwrap());
        let found: Vec<usize> = g.sections.iter().map(|&i| nearest(&truth, g.beats[i].pos).0).collect();
        assert_eq!(found, vec![32, 64], "section starts (true beat index)");
        // the sparsest density still marks them
        let sparse = select(&g, 0.0);
        for &s in &g.sections {
            assert!(sparse.iter().any(|b| b.index == s), "section beat {s} not marked at density 0");
        }
        // music at a steady level has no sections
        let (steady, _) = grid_of(120.0, 24);
        assert!(steady.sections.is_empty(), "{:?}", steady.sections);
    }

    #[test]
    fn density_adds_markers_monotonically() {
        let (g, _) = grid_of(110.0, 20);
        let mut last: Vec<usize> = Vec::new();
        for d in (0..=100).step_by(5) {
            let now: Vec<usize> = select(&g, d as f64).iter().map(|b| b.index).collect();
            assert!(now.len() >= last.len(), "density {d}: {} < {}", now.len(), last.len());
            assert!(now.windows(2).all(|w| w[0] < w[1]));
            last = now;
        }
        assert_eq!(last.len(), g.beats.len(), "100 marks every beat");
    }

    #[test]
    fn deterministic_and_empty_safe() {
        let (g, _) = grid_of(128.0, 16);
        assert_eq!(select(&g, 37.0), select(&g, 37.0));
        let empty = BeatGrid { beats: Vec::new(), downbeat: 0, sections: Vec::new(), bpm: 120.0 };
        assert!(select(&empty, 50.0).is_empty());
    }
}
