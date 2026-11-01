//! Markers ▸ Mark Beats…: a density slider with a live count of the markers it will add, a frame
//! offset and an In/Out-only switch. OK runs `markers.markBeats` (one undo step); every change
//! re-runs it with `preview` (the analysis is cached, so this is instant after the first one).
//!
//! Automation ids: `markBeats.density` (slider 0 … 100), `markBeats.offset` (frames),
//! `markBeats.inOut` (checkbox), `markBeats.info` (tempo and marker count), `markBeats.ok`,
//! `markBeats.cancel`.

use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Debug, PartialEq)]
struct Draft {
    clip: u64,
    density: f64,
    offset: f64,
    in_out: bool,
    /// The params `info` was computed for, so the preview runs once per change.
    previewed: Option<Value>,
    info: String,
}

fn draft_id() -> egui::Id {
    egui::Id::unique("mark-beats-draft")
}

fn params(d: &Draft) -> Value {
    json!({
        "clip": d.clip,
        "density": d.density,
        "offsetFrames": d.offset.round() as i64,
        "range": if d.in_out { "inOut" } else { "clip" },
    })
}

fn preview(app: &mut FilmcraftApp, d: &mut Draft) {
    let p = params(d);
    if d.previewed.as_ref() == Some(&p) {
        return;
    }
    let mut q = p.clone();
    q["preview"] = json!(true);
    d.info = match app.session.execute("markers.markBeats", q) {
        Ok(v) => tlf!(
            "{count} markers ({sections} sections)",
            count = v["markers"].as_u64().unwrap_or(0).to_string(),
            sections = v["sections"].as_u64().unwrap_or(0).to_string()
        ),
        Err(e) => e.to_string(),
    };
    d.previewed = Some(p);
}

/// Open the dialog for the selected music clip (the menu entry without parameters).
pub fn open(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    let clip = filmcraft_engine::remix::selected_audio(&app.session).ok_or_else(|| "select a music clip on an audio track".to_string())?;
    let mut d = Draft { clip: clip.0, density: 50.0, offset: 0.0, in_out: false, previewed: None, info: String::new() };
    preview(app, &mut d);
    ctx.data_mut(|m| m.insert_temp(draft_id(), Some(d)));
    Ok(json!({"dialog": "markBeats"}))
}

/// Whether the dialog is open.
pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())).flatten().is_some()
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(Some(mut d)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
    let mut close = false;
    let mut apply = false;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    egui::Window::new(tl!("Mark Beats"))
        .id(egui::Id::unique("Mark Beats"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            egui::Grid::new("mark-beats").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label(tl!("Density:"));
                let r = ui.add(egui::Slider::new(&mut d.density, 0.0..=100.0).step_by(1.0).text(tl!("Fewer · More")));
                elems.push(("markBeats.density".into(), r.rect, format!("{:.0}", d.density)));
                ui.end_row();
                ui.label(tl!("Offset:"));
                let r = ui.add(crate::widgets::number_drag(&mut d.offset).speed(0.2).range(-30.0..=30.0).suffix(tl!(" frames")).max_decimals(0));
                elems.push(("markBeats.offset".into(), r.rect, format!("{:.0}", d.offset)));
                ui.end_row();
                ui.label("");
                let r = ui.checkbox(&mut d.in_out, tl!("Only between In and Out"));
                elems.push(("markBeats.inOut".into(), r.rect, d.in_out.to_string()));
                ui.end_row();
            });
            ui.add_space(6.0);
            let i = ui.weak(&d.info);
            elems.push(("markBeats.info".into(), i.rect, d.info.clone()));
            ui.weak(tl!("Markers land on the strongest sounds of the music. More density adds quieter ones; level changes are always marked."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let c = ui.button(tl!("Cancel"));
                elems.push(("markBeats.cancel".into(), c.rect, "Cancel".into()));
                if c.clicked() {
                    close = true;
                }
                let o = ui.add(egui::Button::new(egui::RichText::new(tl!("OK")).color(egui::Color32::WHITE)).fill(app.tokens.accent));
                elems.push(("markBeats.ok".into(), o.rect, "OK".into()));
                if o.clicked() {
                    apply = true;
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if crate::widgets::escape_closes(ctx) {
        close = true;
    }
    if apply {
        match app.session.execute("markers.markBeats", params(&d)) {
            Ok(v) => app.ui.status = tlf!("Added {count} beat markers", count = v["markers"].as_u64().unwrap_or(0).to_string()),
            Err(e) => app.ui.status = e.to_string(),
        }
        close = true;
    } else if !close {
        preview(app, &mut d);
    }
    ctx.data_mut(|m| m.insert_temp(draft_id(), if close { None } else { Some(d) }));
}
