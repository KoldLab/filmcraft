//! Tests of [`crate::beat_markers`]: Markers ▸ Mark Beats on generated music (120 BPM, 16 bars,
//! 32 s; bars 8 … 11 three times louder).

use std::sync::Arc;

use filmcraft_audio_dsp::remix as dsp;
use filmcraft_project::{ClipId, Label};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;
use crate::beat_markers::{CMD, TAG};

const SR: u32 = 48_000;

/// A session with the music on A1 at `at` seconds, selected.
fn session(at: f64) -> (Session, ClipId) {
    let (mut m, truth) = dsp::test_music(SR, 120.0, 16, 3);
    let (b8, b12) = (truth[32] as usize, truth[48] as usize);
    for ch in &mut m {
        ch[b8..b12].iter_mut().for_each(|v| *v *= 3.0);
    }
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "beats", "audio": 2, "video": 1})).unwrap();
    let inter: Vec<f32> = (0..m[0].len()).flat_map(|i| [m[0][i], m[1][i]]).collect();
    let bytes: Arc<[u8]> = crate::previews::write_wav_f32(&inter, SR).into();
    let item = crate::commands::import_bytes(&mut s, "/music.wav", bytes, None).unwrap();
    let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": at})).unwrap();
    let c = ClipId(r["clips"][0].as_u64().unwrap());
    s.execute("timeline.select", json!({"clips": [c.0]})).unwrap();
    (s, c)
}

fn markers(s: &Session) -> Vec<filmcraft_project::Marker> {
    s.active_sequence().unwrap().markers.clone()
}

fn secs(v: &Value) -> Vec<f64> {
    v["times"].as_array().unwrap().iter().map(|t| Tick(t.as_i64().unwrap()).seconds()).collect()
}

#[test]
fn preview_changes_nothing_and_shorter_shots_add_markers() {
    let (mut s, _) = session(0.0);
    let few = s.execute(CMD, json!({"seconds": 8, "preview": true})).unwrap();
    let many = s.execute(CMD, json!({"seconds": 0.5, "preview": true})).unwrap();
    assert!(few["markers"].as_u64() < many["markers"].as_u64(), "{few} {many}");
    assert!(markers(&s).is_empty());
}

#[test]
fn markers_land_inside_the_clip_and_follow_its_position() {
    let (mut s, _) = session(0.0);
    let at_zero = secs(&s.execute(CMD, json!({"preview": true})).unwrap());
    let (mut moved, _) = session(5.0);
    let at_five = secs(&moved.execute(CMD, json!({"preview": true})).unwrap());
    assert!(!at_zero.is_empty());
    assert_eq!(at_zero.len(), at_five.len());
    for (a, b) in at_zero.iter().zip(&at_five) {
        assert!((b - a - 5.0).abs() < 0.05, "{a} {b}");
    }
    assert!(at_zero.iter().all(|t| (0.0..32.5).contains(t)), "{at_zero:?}");
}

#[test]
fn marking_is_one_undo_and_replaces_its_own_markers() {
    let (mut s, _) = session(0.0);
    let undo = s.history.undo.len();
    let v = s.execute(CMD, json!({"color": "Rose"})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let n = v["markers"].as_u64().unwrap() as usize;
    assert_eq!(markers(&s).len(), n);
    assert!(markers(&s).iter().all(|m| m.comment.starts_with(TAG)));
    assert!(markers(&s).iter().any(|m| m.color == Label::Rose));
    let again = s.execute(CMD, json!({})).unwrap();
    assert_eq!(again["removed"].as_u64(), Some(n as u64));
    assert_eq!(markers(&s).len(), n);
}

#[test]
fn offset_moves_every_marker_and_in_out_limits_them() {
    let (mut s, _) = session(0.0);
    let base = secs(&s.execute(CMD, json!({"preview": true})).unwrap());
    let later = secs(&s.execute(CMD, json!({"preview": true, "offsetFrames": 10})).unwrap());
    let step = s.sequence_rate().tick_of(10).seconds();
    assert!((later[0] - base[0] - step).abs() < 0.05, "{} {}", later[0], base[0]);
    assert!(s.execute(CMD, json!({"range": "inOut", "preview": true})).is_err());
}

#[test]
fn nothing_selected_is_refused() {
    let (mut s, _) = session(0.0);
    s.execute("timeline.select", json!({"clips": []})).unwrap();
    assert!(s.execute(CMD, json!({})).is_err());
}

#[test]
fn shots_ask_for_one_cut_less() {
    let (mut s, _) = session(0.0);
    let v = s.execute(CMD, json!({"shots": 6, "preview": true})).unwrap();
    assert_eq!(v["markers"].as_u64(), Some(5), "{v}");
}

/// Imports a generated video of `seconds`, selected in the Project panel with the others.
fn add_clip(s: &mut Session, seconds: f64) -> u64 {
    s.execute("file.newBlackVideo", json!({"seconds": seconds})).unwrap()["item"].as_u64().unwrap()
}

#[test]
fn clips_fill_the_time_between_the_markers_and_the_music_stays() {
    let (mut s, music) = session(0.0);
    s.execute(CMD, json!({"shots": 3})).unwrap();
    let cuts: Vec<Tick> = markers(&s).iter().map(|m| m.start).collect();
    assert_eq!(cuts.len(), 2);
    let items: Vec<u64> = vec![add_clip(&mut s, 60.0), add_clip(&mut s, 60.0), add_clip(&mut s, 0.5)];
    s.state.project_selection = items.iter().map(|i| filmcraft_project::ItemId(*i)).collect();
    let undo = s.history.undo.len();
    let v = s.execute("clip.cutToMarkers", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(v["placed"].as_u64(), Some(3), "{v}");
    assert_eq!(v["missing"].as_u64(), Some(0), "{v}");
    assert_eq!(v["slowed"].as_u64(), Some(1), "{v}");
    let q = s.active_sequence().unwrap();
    let video: Vec<(Tick, Tick)> = q.video_tracks[0].items.iter().map(|i| (i.start, Tick(i.start.0 + i.duration.0))).collect();
    assert_eq!(video.len(), 3, "{video:?}");
    let frame = s.sequence_rate().tick_of(2).0;
    assert!((video[0].1.0 - cuts[0].0).abs() <= frame, "{video:?} {cuts:?}");
    assert!((video[1].0.0 - cuts[0].0).abs() <= frame, "{video:?} {cuts:?}");
    assert!((video[2].0.0 - cuts[1].0).abs() <= frame, "{video:?} {cuts:?}");
    let (_, it) = q.find_item(music).unwrap();
    assert!((video[2].1.0 - (it.start.0 + it.duration.0)).abs() <= frame, "the slowed clip reaches the end");
    assert_eq!(it.start, Tick::ZERO, "the music did not move");
}

#[test]
fn missing_clips_are_counted() {
    let (mut s, _) = session(0.0);
    s.execute(CMD, json!({"shots": 4})).unwrap();
    let item = add_clip(&mut s, 60.0);
    s.state.project_selection = vec![filmcraft_project::ItemId(item)];
    let v = s.execute("clip.cutToMarkers", json!({})).unwrap();
    assert_eq!(v["placed"].as_u64(), Some(1));
    assert_eq!(v["missing"].as_u64(), Some(3));
}
