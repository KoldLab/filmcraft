//! Tests of [`crate::beat_markers`]: Markers ▸ Mark Beats on generated music (120 BPM, 16 bars,
//! 32 s; bars 8 … 11 three times louder, so sections start at 16 s and 24 s).

use std::sync::Arc;

use filmcraft_audio_dsp::remix as dsp;
use filmcraft_project::{ClipId, Label};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::*;

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

/// Distance (s) from `t` to the nearest multiple of `step` after `origin`.
fn off_grid(t: f64, origin: f64, step: f64) -> f64 {
    let k = ((t - origin) / step).round();
    (t - origin - k * step).abs()
}

#[test]
fn menu_entry_and_disabled_reason() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "empty"})).unwrap();
    let c = crate::commands::find("markers.markBeats").unwrap();
    assert_eq!(c.menu, &["Markers"]);
    assert_eq!((c.enabled)(&s).unwrap_err(), "select a music clip on an audio track");
    let pos = |id: &str| crate::commands::command_specs().iter().position(|c| c.id == id).unwrap();
    assert_eq!(pos("markers.markBeats"), pos("markers.addFlashCue") + 1);
    let (s, _) = session(0.0);
    assert!((c.enabled)(&s).is_ok());
}

#[test]
fn marks_bars_on_the_beat_with_sections_named() {
    let (mut s, _) = session(2.0);
    let v = s.execute("markers.markBeats", json!({})).unwrap();
    assert!((v["bpm"].as_f64().unwrap() - 120.0).abs() < 0.5, "{v}");
    let t = secs(&v);
    // one per bar (2 s), on the bar grid of the clip placed at 2 s, within a frame
    assert!((13..=17).contains(&t.len()), "{} markers: {t:?}", t.len());
    for x in &t {
        assert!(off_grid(*x, 2.0, 2.0) < 0.045, "marker at {x} s is off the bars");
    }
    let ms = markers(&s);
    assert_eq!(ms.len(), t.len());
    let sections: Vec<(f64, String)> = ms.iter().filter(|m| m.color == Label::Mango).map(|m| (m.start.seconds(), m.name.clone())).collect();
    assert_eq!(sections.len(), 2, "{sections:?}");
    assert!((sections[0].0 - 18.0).abs() < 0.05 && sections[0].1 == "Section 1", "{sections:?}");
    assert!((sections[1].0 - 26.0).abs() < 0.05 && sections[1].1 == "Section 2", "{sections:?}");
    assert!(ms.iter().all(|m| m.comment.starts_with("Mark Beats")));
    // one undo step
    s.execute("edit.undo", json!({})).unwrap();
    assert!(markers(&s).is_empty());
}

#[test]
fn density_preview_and_replace() {
    let (mut s, _) = session(0.0);
    let counts: Vec<u64> = [0.0, 25.0, 50.0, 75.0, 100.0]
        .iter()
        .map(|d| s.execute("markers.markBeats", json!({"density": d, "preview": true})).unwrap()["markers"].as_u64().unwrap())
        .collect();
    assert!(counts.windows(2).all(|w| w[0] < w[1]), "{counts:?}");
    assert!(markers(&s).is_empty(), "preview changes nothing");
    s.execute("markers.add", json!({"seconds": 5.3, "name": "mine"})).unwrap();
    s.execute("markers.markBeats", json!({"density": 100})).unwrap();
    let dense = markers(&s).len();
    let v = s.execute("markers.markBeats", json!({"density": 0})).unwrap();
    let ms = markers(&s);
    assert_eq!(v["removed"].as_u64().unwrap() as usize, dense - 1);
    assert_eq!(ms.len(), v["markers"].as_u64().unwrap() as usize + 1, "the user's marker is kept");
    assert!(ms.iter().any(|m| m.name == "mine"));
}

#[test]
fn offset_and_in_out_range() {
    let (mut s, _) = session(0.0);
    let base = secs(&s.execute("markers.markBeats", json!({"preview": true})).unwrap());
    let early = secs(&s.execute("markers.markBeats", json!({"preview": true, "offsetFrames": -2})).unwrap());
    let fps = s.sequence_rate();
    let two = fps.tick_of(2).seconds();
    // the first marker may fall before the clip once moved; compare the rest
    for (b, e) in base.iter().rev().zip(early.iter().rev()) {
        assert!((b - e - two).abs() < 0.001, "{b} → {e}");
    }
    assert!(s.execute("markers.markBeats", json!({"range": "inOut"})).unwrap_err().to_string().contains("In and Out"));
    s.execute("markers.markIn", json!({"seconds": 8.0})).unwrap();
    s.execute("markers.markOut", json!({"seconds": 16.0})).unwrap();
    let t = secs(&s.execute("markers.markBeats", json!({"range": "inOut", "density": 100})).unwrap());
    assert!(t.iter().all(|x| (8.0..16.0).contains(x)), "{t:?}");
    assert!((14..=16).contains(&t.len()), "{t:?}");
}

#[test]
fn hostile_params_and_unsupported_clips() {
    let (mut s, c) = session(0.0);
    for p in [json!({"density": -50}), json!({"density": 1e300}), json!({"offsetFrames": i64::MIN}), json!({"offsetFrames": i64::MAX})] {
        s.execute("markers.markBeats", p.clone()).unwrap_or_else(|e| panic!("{p}: {e}"));
    }
    assert!(s.execute("markers.markBeats", json!({"range": "everything"})).is_err());
    assert!(s.execute("markers.markBeats", json!({"color": "Plaid"})).is_err());
    assert!(s.execute("markers.markBeats", json!({"clip": 999_999})).is_err());
    s.execute("clip.speedDuration", json!({"clips": [c.0], "speed": 200.0})).unwrap();
    assert!(s.execute("markers.markBeats", json!({})).unwrap_err().to_string().contains("100 % speed"));
}
