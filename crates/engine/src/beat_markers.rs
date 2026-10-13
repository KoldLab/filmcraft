//! Markers ▸ Mark Beats…: sequence markers on the beats of a music clip, as many as a density
//! slider asks for, the musically important ones first (ranking in
//! [`filmcraft_audio_dsp::beat_markers`], analysis shared with Clip ▸ Remix).
//!
//! | param | default | |
//! |---|---|---|
//! | `clip` | the selected audio clip | the music |
//! | `density` | 50 | 0 = every 4 bars and section starts … 50 = every bar … 100 = every beat |
//! | `offsetFrames` | 0 | move every marker (negative = before the beat, to cut just ahead of it) |
//! | `range` | `"clip"` | `"inOut"`: only between the sequence In and Out |
//! | `color` / `sectionColor` | Teal / Mango | marker colours (section starts get their own) |
//! | `replace` | true | first remove the markers an earlier Mark Beats put in the range |
//! | `preview` | false | report what would be marked without changing anything (the dialog's live count) |
//!
//! Markers are Comment markers snapped to the nearest frame; section starts are named
//! "Section 1", "Section 2"…; every marker's comment starts with `Mark Beats`, which is how
//! `replace` finds them. One undo step. Pair with Clip ▸ Automate to Sequence (placement: unnumbered
//! markers) to cut clips on the beat.

use filmcraft_audio_dsp::beat_markers::{self as dsp, Level};
use filmcraft_project::{ClipId, ItemKind, Label, Marker, MarkerId, MarkerKind, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, marker_color_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "markers.markBeats";
/// Every marker this command adds has a comment starting with this.
pub const TAG: &str = "Mark Beats";
/// Longest music analysed at once (the analysis holds a beats² similarity matrix).
const MAX_SECONDS: f64 = 20.0 * 60.0;

fn can_mark(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    crate::remix::selected_audio(s).map(|_| ()).ok_or_else(|| "select a music clip on an audio track".into())
}

/// The clip to analyse and why it can't be, checked up front.
fn music_clip(s: &Session, p: &Value) -> Result<ClipId> {
    let c = match u64_p(p, "clip") {
        Some(c) => ClipId(c),
        None => crate::remix::selected_audio(s).ok_or_else(|| bad(CMD, "select a music clip on an audio track or pass `clip`"))?,
    };
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (t, it) = seq.find_item(c).ok_or_else(|| bad(CMD, "no such clip"))?;
    if seq.track(t).is_none_or(|t| t.kind != TrackKind::Audio) {
        return Err(bad(CMD, "Mark Beats works on audio clips"));
    }
    let item = s.project.item(it.item).ok_or_else(|| bad(CMD, "the clip has no media"))?;
    if !matches!(item.kind, ItemKind::Media(_)) || !item.has_audio() {
        return Err(bad(CMD, "the clip has no audio media"));
    }
    if filmcraft_render::remix::is_remixed(it) {
        return Err(bad(CMD, "the clip is remixed; revert the remix to mark its beats"));
    }
    if it.reverse || (it.speed - 1.0).abs() > 1e-9 || it.frame_hold.is_some() {
        return Err(bad(CMD, "Mark Beats works on clips playing forward at 100 % speed"));
    }
    if it.duration.seconds() > MAX_SECONDS {
        return Err(bad(CMD, format!("the clip is longer than {} minutes; trim it first", MAX_SECONDS / 60.0)));
    }
    Ok(c)
}

/// One marker to add: sequence time, level, section number (1-based) for section starts.
struct Planned {
    time: Tick,
    level: Level,
    section: Option<usize>,
}

/// What would be marked: the markers, the time range they fill, the tempo and the beat count.
fn plan(s: &Session, p: &Value) -> Result<(Vec<Planned>, (Tick, Tick), f64, usize)> {
    let c = music_clip(s, p)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(c).ok_or_else(|| bad(CMD, "no such clip"))?;
    let (start, source_in, dur, item) = (it.start, it.source_in, it.duration, it.item);
    let mut range = (start, Tick(start.0.saturating_add(dur.0)));
    match str_p(p, "range").unwrap_or("clip") {
        "clip" => {}
        "inOut" => {
            let (Some(i), Some(o)) = (seq.mark_in, seq.mark_out) else {
                return Err(bad(CMD, "set the sequence In and Out first, or use range \"clip\""));
            };
            range = (range.0.max(i), range.1.min(o));
            if range.0 >= range.1 {
                return Err(bad(CMD, "the In/Out range does not overlap the clip"));
            }
        }
        other => return Err(bad(CMD, format!("unknown range {other:?}; use \"clip\" or \"inOut\""))),
    }
    let density = f64_p(p, "density").unwrap_or(50.0);
    if !density.is_finite() {
        return Err(bad(CMD, "`density` must be a number from 0 to 100"));
    }
    let offset_frames = p.get("offsetFrames").and_then(Value::as_i64).unwrap_or(0).clamp(-1_000, 1_000);
    let rate = s.sequence_rate();
    let offset = Tick(rate.tick_of(offset_frames.abs()).0.saturating_mul(offset_frames.signum()));
    let sr = seq.settings.sample_rate.max(1) as i64;
    let a = crate::remix::analysis_or(s, item, source_in, dur, |e| format!("cannot mark beats: {e}"))?;
    let grid = dsp::grid(&a);
    // analysis sample 0 is media time `m0` (the sample at or before the clip's In)
    let m0 = source_in.to_units_floor(sr);
    let mut out: Vec<Planned> = Vec::new();
    for b in dsp::select(&grid, density) {
        let media = Tick::from_units(m0.saturating_add(b.pos), sr);
        let t = Tick(start.0.saturating_add(media.0.saturating_sub(source_in.0)).saturating_add(offset.0));
        let t = rate.snap_nearest(t);
        if t < range.0 || t >= range.1 || out.last().is_some_and(|l| l.time == t) {
            continue;
        }
        let section = grid.sections.iter().position(|&i| i == b.index).map(|n| n + 1);
        out.push(Planned { time: t, level: b.level, section });
    }
    Ok((out, range, grid.bpm, grid.beats.len()))
}

fn report(planned: &[Planned], bpm: f64, beats: usize) -> Value {
    json!({
        "bpm": bpm,
        "beats": beats,
        "markers": planned.len(),
        "sections": planned.iter().filter(|m| m.section.is_some()).count(),
        "times": planned.iter().map(|m| m.time.0).collect::<Vec<_>>(),
        "levels": planned.iter().map(|m| m.level.name()).collect::<Vec<_>>(),
    })
}

fn mark(s: &mut Session, p: &Value) -> Result<Value> {
    let (planned, range, bpm, beats) = plan(s, p)?;
    if p.get("preview").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(report(&planned, bpm, beats));
    }
    let color = marker_color_p(p, CMD)?.unwrap_or(Label::Teal);
    let section_color = match p.get("sectionColor") {
        None | Some(Value::Null) => Label::Mango,
        Some(v) => marker_color_p(&json!({"color": v}), CMD)?.unwrap_or(Label::Mango),
    };
    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(true);
    let mut v = report(&planned, bpm, beats);
    let mut removed = 0usize;
    s.edit_sequence("Mark Beats", |q, ctx, _| {
        if replace {
            let before = q.markers.len();
            q.markers.retain(|m| !(m.comment.starts_with(TAG) && m.start >= range.0 && m.start < range.1));
            removed = before.saturating_sub(q.markers.len());
        }
        for m in &planned {
            let id = MarkerId(ctx.alloc());
            let name = m.section.map(|n| format!("Section {n}")).unwrap_or_default();
            let comment = format!("{TAG} · {} · {bpm:.0} BPM", m.level.name());
            let color = if m.section.is_some() { section_color } else { color };
            q.markers.push(Marker { id, start: m.time, duration: Tick(0), name, comment, kind: MarkerKind::Comment, color });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(())
    })?;
    v["removed"] = json!(removed);
    Ok(v)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Mark Beats…",
        menu: &["Markers"],
        shortcut: None,
        params: r#"{"clip":id?,"density":0..100?,"offsetFrames":i64?,"range":"clip"|"inOut"?,"color":label?,"sectionColor":label?,"replace":bool?,"preview":bool?}"#,
        enabled: can_mark,
        run: mark,
        journal: true,
    }]
}
