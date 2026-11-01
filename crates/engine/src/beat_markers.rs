//! Markers ▸ Mark Beats…: sequence markers on the peaks of a music clip, a marker every few seconds
//! or exactly what a number of shots needs (analysis in [`filmcraft_audio_dsp::beat_markers`]).
//! Markers ▸ Cut Clips to Markers then fills the time between the markers with the clips selected
//! in the Project panel, without touching the music.
//!
//! | param | default | |
//! |---|---|---|
//! | `clip` | the selected audio clip | the music |
//! | `seconds` | 2 | a shot about every 0.5 … 8 seconds |
//! | `shots` | | instead of `seconds`: the cuts that make this many shots |
//! | `offsetFrames` | 0 | move every marker (negative = before the peak, to cut just ahead of it) |
//! | `range` | `"clip"` | `"inOut"`: only between the sequence In and Out |
//! | `color` / `sectionColor` | Teal / Mango | colours of cuts and transitions; effects are Rose |
//! | `replace` | true | first remove the markers an earlier Mark Beats put in the range |
//! | `preview` | false | report what would be marked without changing anything (the dialog's live count) |
//!
//! Markers are Comment markers snapped to the nearest frame; level changes are named
//! "Transition 1", "Transition 2"…; every marker's comment starts with `Mark Beats`, which is how
//! `replace` finds them. One undo step.

use std::sync::{Arc, Mutex, OnceLock};

use filmcraft_audio_dsp::beat_markers::{self as dsp, PeakMap};
use filmcraft_edit as edit;
use filmcraft_project::{ClipId, ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, ItemPlacement, bad, f64_p, has_seq, marker_color_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

pub(crate) const CMD: &str = "markers.markBeats";
/// Every marker this command adds has a comment starting with this.
pub const TAG: &str = "Mark Beats";
/// Longest music analysed at once.
const MAX_SECONDS: f64 = 20.0 * 60.0;
/// Clip analyses kept, so the dialog's live preview does not analyse again.
const CACHED: usize = 8;
/// Colour of the markers on the strongest peaks.
const EFFECT_COLOR: Label = Label::Rose;
const CUT_CMD: &str = "clip.cutToMarkers";

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

/// One marker to add: sequence time, kind, number (1-based) for transitions.
struct Planned {
    time: Tick,
    kind: &'static str,
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
    let seconds = f64_p(p, "seconds").unwrap_or(2.0);
    if !seconds.is_finite() {
        return Err(bad(CMD, "`seconds` must be a number"));
    }
    let shots = u64_p(p, "shots").map(|n| n.clamp(1, 100_000) as usize);
    let offset_frames = p.get("offsetFrames").and_then(Value::as_i64).unwrap_or(0).clamp(-1_000, 1_000);
    let rate = s.sequence_rate();
    let offset = Tick(rate.tick_of(offset_frames.abs()).0.saturating_mul(offset_frames.signum()));
    let sr = seq.settings.sample_rate.max(1);
    let map = peaks_of(s, item, source_in, dur, sr)?;
    // analysis sample 0 is media time `m0` (the sample at or before the clip's In)
    let m0 = source_in.to_units_floor(i64::from(sr));
    let mut out: Vec<Planned> = Vec::new();
    let mut sections = 0usize;
    let chosen = match shots {
        Some(n) => dsp::select_shots(&map, n),
        None => dsp::select(&map, seconds),
    };
    for b in chosen {
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
        out.push(Planned { time: t, kind: b.kind(), section });
    }
    Ok((out, range, map.peaks.len()))
}

fn report(planned: &[Planned], peaks: usize) -> Value {
    json!({
        "peaks": peaks,
        "markers": planned.len(),
        "sections": planned.iter().filter(|m| m.section.is_some()).count(),
        "effects": planned.iter().filter(|m| m.kind == "effect").count(),
        "times": planned.iter().map(|m| m.time.0).collect::<Vec<_>>(),
    })
}

fn mark(s: &mut Session, p: &Value) -> Result<Value> {
    let (planned, range, peaks) = plan(s, p)?;
    if p.get("preview").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(report(&planned, peaks));
    }
    let color = marker_color_p(p, CMD)?.map_or(Label::Teal, |c| c.0);
    let section_color = match p.get("sectionColor") {
        None | Some(Value::Null) => Label::Mango,
        Some(v) => marker_color_p(&json!({"color": v}), CMD)?.map_or(Label::Mango, |c| c.0),
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
            let name = m.section.map(|n| format!("Transition {n}")).unwrap_or_default();
            let comment = format!("{TAG} · {}", m.kind);
            let color = match m.kind {
                "transition" => section_color,
                "effect" => EFFECT_COLOR,
                _ => color,
            };
            q.markers.push(Marker {
                id,
                start: m.time,
                duration: Tick(0),
                name,
                comment,
                kind: MarkerKind::Comment,
                color,
                custom_color: None,
                author: Default::default(),
                url: Default::default(),
            });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(())
    })?;
    v["removed"] = json!(removed);
    Ok(v)
}

fn can_cut(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.project_selection.is_empty() {
        return Err("select your clips in the Project panel".into());
    }
    if s.active_sequence().is_none_or(|q| q.markers.is_empty()) {
        return Err("the sequence has no markers: run Mark Beats first".into());
    }
    Ok(())
}

/// The shots the markers make: from the start of the music to its end, cut at every marker.
fn shots(s: &Session) -> Result<Vec<(Tick, Tick)>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = s.sequence_rate();
    let first = q.audio_tracks.iter().flat_map(|t| t.items.iter()).map(|i| i.start).min().unwrap_or(Tick::ZERO);
    let end = q.audio_tracks.iter().map(|t| t.end()).max().filter(|e| *e > first).unwrap_or_else(|| q.duration());
    let mut cuts: Vec<Tick> =
        q.markers.iter().filter(|m| m.kind == MarkerKind::Comment).map(|m| rate.snap(m.start)).filter(|t| *t > first && *t < end).collect();
    cuts.push(first);
    cuts.push(end);
    cuts.sort();
    cuts.dedup();
    Ok(cuts.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect())
}

/// Each selected clip fills one shot on the video track: a long clip is cut at the next marker, a
/// short one is slowed down until it reaches it. The audio tracks do not change. One undo step.
fn cut_to_markers(s: &mut Session, _: &Value) -> Result<Value> {
    let vdest = s.targeting().video_dest.ok_or_else(|| bad(CUT_CMD, "no video track is targeted"))?;
    let mut order = Vec::new();
    s.project.root.all_items(&mut order);
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let mut items: Vec<ItemId> = s.state.project_selection.iter().copied().filter(|i| *i != seq && s.project.source_streams(*i).0).collect();
    items.sort_by_key(|i| order.iter().position(|x| x == i).unwrap_or(usize::MAX));
    if items.is_empty() {
        return Err(bad(CUT_CMD, "select video clips in the Project panel"));
    }
    let segments = shots(s)?;
    let (Some(start), Some(end)) = (segments.first().map(|x| x.0), segments.last().map(|x| x.1)) else {
        return Err(bad(CUT_CMD, "the sequence has no room between its markers"));
    };
    let n0 = s.history.undo.len();
    // a new attempt replaces the previous one instead of colliding with it
    s.edit_sequence("Cut Clips to Markers", |q, _, _| {
        if let Some(t) = q.video_tracks.iter_mut().find(|t| t.id == vdest) {
            t.items.retain(|i| i.start < start || i.start >= end);
        }
        Ok(())
    })?;
    let still_duration = s.prefs.timeline.still_duration(s.sequence_rate());
    let mut placed: Vec<ClipId> = Vec::new();
    let mut slowed = 0usize;
    let mut failure = None;
    for (item, (from, to)) in items.iter().zip(&segments) {
        let wanted = Tick(to.0.saturating_sub(from.0));
        let Some(r) = crate::clip_ops::item_range(&s.project, *item, still_duration) else { continue };
        let still = s.project.item(*item).and_then(|i| i.as_media()).is_some_and(|m| matches!(m.info.kind, filmcraft_media::MediaKind::Still));
        let range = if still || r.duration >= wanted { TimeRange::new(r.start, wanted) } else { r };
        let short = !still && r.duration < wanted;
        let ids = match crate::commands::place_item(
            s,
            ItemPlacement { item: *item, range, at: *from, vdest: Some(vdest), adest: None, insert: false, label: "Cut Clips to Markers", audio_split: None },
        ) {
            Ok(ids) => ids,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        if short {
            let speed = r.duration.0 as f64 / wanted.0.max(1) as f64;
            let edited = s.edit_sequence("Cut Clips to Markers", |q, ctx, _| {
                edit::set_speed_group(q, &ids, speed, false, false, ctx)?;
                Ok(())
            });
            if let Err(e) = edited {
                failure = Some(e);
                break;
            }
            slowed += 1;
        }
        placed.extend(ids);
    }
    if let Some(e) = failure {
        while s.history.undo.len() > n0 {
            s.undo();
        }
        s.history.redo.clear();
        return Err(e);
    }
    crate::clip_ops::collapse_history(s, n0, "Cut Clips to Markers");
    s.state.selection = placed.clone();
    Ok(json!({
        "placed": placed.len(),
        "shots": segments.len(),
        "missing": segments.len().saturating_sub(placed.len()),
        "slowed": slowed,
        "clips": placed.iter().map(|c| c.0).collect::<Vec<_>>(),
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: CMD,
            label: "Mark Beats…",
            menu: &["Markers"],
            shortcut: None,
            params: r#"{"clip":id?,"seconds":0.5..8?,"shots":u64?,"offsetFrames":i64?,"range":"clip"|"inOut"?,"color":label?,"sectionColor":label?,"replace":bool?,"preview":bool?}"#,
            enabled: can_mark,
            run: mark,
            journal: true,
        },
        CommandSpec {
            id: CUT_CMD,
            label: "Cut Clips to Markers",
            menu: &["Markers"],
            shortcut: None,
            params: "{}",
            enabled: can_cut,
            run: cut_to_markers,
            journal: true,
        },
    ]
}
