//! Markers ▸ Mark Beats…: sequence markers on the peaks of a music clip, as many as a density
//! slider asks for, the strongest first (analysis in [`filmcraft_audio_dsp::beat_markers`]).
//!
//! | param | default | |
//! |---|---|---|
//! | `clip` | the selected audio clip | the music |
//! | `density` | 50 | 0 = one peak per 8 s … 50 = one per 2 s … 100 = one per 0.5 s |
//! | `offsetFrames` | 0 | move every marker (negative = before the peak, to cut just ahead of it) |
//! | `range` | `"clip"` | `"inOut"`: only between the sequence In and Out |
//! | `color` / `sectionColor` | Teal / Mango | marker colours (section changes get their own) |
//! | `replace` | true | first remove the markers an earlier Mark Beats put in the range |
//! | `preview` | false | report what would be marked without changing anything (the dialog's live count) |
//!
//! Markers are Comment markers snapped to the nearest frame; section changes are named
//! "Section 1", "Section 2"…; every marker's comment starts with `Mark Beats`, which is how
//! `replace` finds them. One undo step. Pair with Clip ▸ Automate to Sequence (placement: unnumbered
//! markers) to cut clips on the music.

use std::sync::{Arc, Mutex, OnceLock};

use filmcraft_audio_dsp::beat_markers::{self as dsp, PeakMap};
use filmcraft_project::{ClipId, ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, marker_color_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

pub(crate) const CMD: &str = "markers.markBeats";
/// Every marker this command adds has a comment starting with this.
pub const TAG: &str = "Mark Beats";
/// Longest music analysed at once.
const MAX_SECONDS: f64 = 20.0 * 60.0;
/// Clip analyses kept, so the dialog's live preview does not analyse again.
const CACHED: usize = 8;

type Key = (u64, i64, i64, u32);

fn cache() -> &'static Mutex<Vec<(Key, Arc<PeakMap>)>> {
    static CACHE: OnceLock<Mutex<Vec<(Key, Arc<PeakMap>)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

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
        return Err(bad(CMD, "the clip is remixed; revert the remix to mark it"));
    }
    if it.reverse || (it.speed - 1.0).abs() > 1e-9 || it.frame_hold.is_some() {
        return Err(bad(CMD, "Mark Beats works on clips playing forward at 100 % speed"));
    }
    if it.duration.seconds() > MAX_SECONDS {
        return Err(bad(CMD, format!("the clip is longer than {} minutes; trim it first", MAX_SECONDS / 60.0)));
    }
    Ok(c)
}

/// The peaks of `[source_in, source_in + dur)` of the clip's media at the sequence rate (cached).
fn peaks_of(s: &Session, item: ItemId, source_in: Tick, dur: Tick, sr: u32) -> Result<Arc<PeakMap>> {
    let m0 = source_in.to_units_floor(i64::from(sr));
    let len = (source_in + dur).to_units_floor(i64::from(sr)).saturating_sub(m0);
    if len < 0 {
        return Err(bad(CMD, "the clip has no audio to analyse"));
    }
    let key = (item.0, m0, len, sr);
    if let Some((_, a)) = cache().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return Ok(a.clone());
    }
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let src = filmcraft_render::SourceProvider::source(&provider, item).ok_or_else(|| EngineError::Other("the clip's media is offline".into()))?;
    let buf = src.audio(m0, len as usize, sr).map_err(|e| EngineError::Other(format!("reading the clip's audio: {e}")))?;
    let refs: Vec<&[f32]> = buf.channels.iter().map(Vec::as_slice).collect();
    let map = Arc::new(dsp::analyze(&refs, sr));
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.push((key, map.clone()));
    if c.len() > CACHED {
        c.remove(0);
    }
    Ok(map)
}

/// One marker to add: sequence time, section number (1-based) for section changes.
struct Planned {
    time: Tick,
    section: Option<usize>,
}

/// What would be marked: the markers, the time range they fill, the peaks found.
fn plan(s: &Session, p: &Value) -> Result<(Vec<Planned>, (Tick, Tick), usize)> {
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
    let sr = seq.settings.sample_rate.max(1);
    let map = peaks_of(s, item, source_in, dur, sr)?;
    // analysis sample 0 is media time `m0` (the sample at or before the clip's In)
    let m0 = source_in.to_units_floor(i64::from(sr));
    let mut out: Vec<Planned> = Vec::new();
    let mut sections = 0usize;
    for b in dsp::select(&map, density) {
        let media = Tick::from_units(m0.saturating_add(b.pos), i64::from(sr));
        let t = Tick(start.0.saturating_add(media.0.saturating_sub(source_in.0)).saturating_add(offset.0));
        let t = rate.snap_nearest(t);
        if t < range.0 || t >= range.1 || out.last().is_some_and(|l| l.time == t) {
            continue;
        }
        let section = b.section.then(|| {
            sections += 1;
            sections
        });
        out.push(Planned { time: t, section });
    }
    Ok((out, range, map.peaks.len()))
}

fn report(planned: &[Planned], peaks: usize) -> Value {
    json!({
        "peaks": peaks,
        "markers": planned.len(),
        "sections": planned.iter().filter(|m| m.section.is_some()).count(),
        "times": planned.iter().map(|m| m.time.0).collect::<Vec<_>>(),
    })
}

fn mark(s: &mut Session, p: &Value) -> Result<Value> {
    let (planned, range, peaks) = plan(s, p)?;
    if p.get("preview").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(report(&planned, peaks));
    }
    let color = marker_color_p(p, CMD)?.unwrap_or(Label::Teal);
    let section_color = match p.get("sectionColor") {
        None | Some(Value::Null) => Label::Mango,
        Some(v) => marker_color_p(&json!({"color": v}), CMD)?.unwrap_or(Label::Mango),
    };
    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(true);
    let mut v = report(&planned, peaks);
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
            let kind = if m.section.is_some() { "section" } else { "peak" };
            let comment = format!("{TAG} · {kind}");
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
