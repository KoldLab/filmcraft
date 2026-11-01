//! Headless UI test of Markers ▸ Mark Beats…: the dialog's live count, the density slider and OK,
//! driven by automation id through the control channel.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

const SR: u32 = 48_000;

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn new(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    fn element(&mut self, id: &str) -> Value {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().find(|e| e["id"] == id).cloned().unwrap_or_else(|| panic!("{id} not on screen"))
    }

    fn markers(&self) -> usize {
        self.harness.state().session.active_sequence().unwrap().markers.len()
    }
}

fn session() -> Session {
    let (m, _) = filmcraft_audio_dsp::remix::test_music(SR, 120.0, 16, 5);
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "beats", "audio": 2, "video": 1})).unwrap();
    let inter: Vec<f32> = (0..m[0].len()).flat_map(|i| [m[0][i], m[1][i]]).collect();
    let bytes: Arc<[u8]> = filmcraft_engine::previews::write_wav_f32(&inter, SR).into();
    let item = filmcraft_engine::commands::import_bytes(&mut s, "/music.wav", bytes, None).unwrap();
    let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    let c = r["clips"][0].as_u64().unwrap();
    s.execute("timeline.select", json!({"clips": [c]})).unwrap();
    s
}

/// The marker count the dialog's info line announces.
fn announced(info: &str) -> usize {
    info.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no count in {info:?}"))
}

#[test]
fn mark_beats_dialog_counts_live_and_marks() {
    let mut d = Driver::new(session());
    let items = d.ok("ui.menu.list", json!({}));
    assert!(items.as_array().unwrap().iter().any(|i| i["id"] == "markers.markBeats"), "Mark Beats… missing from the menus");
    let r = d.ok("ui.menu.invoke", json!({"id": "markers.markBeats"}));
    assert_eq!(r["dialog"], "markBeats");
    d.frames(3);
    for id in ["density", "offset", "inOut", "info", "ok", "cancel"] {
        d.element(&format!("markBeats.{id}"));
    }
    // default density: one peak per 2 s of the 32 s of music
    let info = d.element("markBeats.info")["label"].as_str().unwrap().to_string();
    let per_bar = announced(&info);
    assert!((8..=20).contains(&per_bar), "{info}");
    // Cancel changes nothing
    d.ok("ui.click", json!({"id": "markBeats.cancel"}));
    d.frames(3);
    assert!(d.ids("markBeats.").is_empty());
    assert_eq!(d.markers(), 0);
    // reopen, push the slider to the far right: the count follows live, OK adds exactly that many
    d.ok("ui.menu.invoke", json!({"id": "markers.markBeats"}));
    d.frames(3);
    let r = d.element("markBeats.density")["rect"].clone();
    let (x, y, w, h) = (r[0].as_f64().unwrap(), r[1].as_f64().unwrap(), r[2].as_f64().unwrap(), r[3].as_f64().unwrap());
    // the track is the first 100 px of the widget (the "Fewer · More" text follows it)
    d.ok("ui.click", json!({"x": x + 90.0_f64.min(w), "y": y + h / 2.0}));
    d.frames(3);
    assert!(d.element("markBeats.density")["label"].as_str().unwrap().parse::<f64>().unwrap() > 50.0);
    let info = d.element("markBeats.info")["label"].as_str().unwrap().to_string();
    let dense = announced(&info);
    assert!(dense > per_bar, "{info}");
    let undo_before = d.harness.state().session.history.undo.len();
    d.ok("ui.click", json!({"id": "markBeats.ok"}));
    d.frames(3);
    assert!(d.ids("markBeats.").is_empty());
    assert_eq!(d.markers(), dense);
    assert_eq!(d.harness.state().session.history.undo.len(), undo_before + 1);
}
