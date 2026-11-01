//! Transcribe: download Whisper if it is missing (after asking), then transcribe in the
//! background with a progress bar, so the app stays usable and the work can be cancelled.
//!
//! Automation ids: `transcribe.download`, `transcribe.cancel`, `transcribe.status`,
//! `transcribe.close`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_engine::transcript::{Finished, Prepared};

use crate::FilmcraftApp;

#[derive(Default)]
pub struct Shared {
    status: String,
    fraction: f32,
    result: Option<Result<Finished, String>>,
}

/// What the dialog shows.
pub enum Job {
    /// The model must be downloaded first: its name and size in bytes.
    Confirm(Box<Prepared>, String, u64),
    Running {
        shared: Arc<Mutex<Shared>>,
        cancel: Arc<AtomicBool>,
    },
    Failed(String),
}

/// Called from the Transcribe button.
pub fn start(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if matches!(app.transcribe_job, Some(Job::Running { .. })) {
        return;
    }
    match filmcraft_engine::transcript::prepare(&app.session, &serde_json::json!({})) {
        Ok(job) => match job.missing_model() {
            Some((name, size)) => app.transcribe_job = Some(Job::Confirm(Box::new(job), name, size)),
            None => run(app, ctx, job),
        },
        Err(e) => app.transcribe_job = Some(Job::Failed(e.to_string())),
    }
}

fn run(app: &mut FilmcraftApp, ctx: &egui::Context, job: Prepared) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    let (out, stop, repaint) = (shared.clone(), cancel.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("filmcraft-transcribe".into()).spawn(move || {
        let progress_to = out.clone();
        let wake = repaint.clone();
        let mut progress = move |fraction: f32, status: &str| {
            let mut s = progress_to.lock().unwrap_or_else(PoisonError::into_inner);
            s.fraction = fraction;
            s.status = status.to_string();
            drop(s);
            wake.request_repaint();
            !stop.load(Ordering::Relaxed)
        };
        let result = catch_unwind(AssertUnwindSafe(|| job.run(&mut progress)));
        let result = match result {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err("the transcription stopped unexpectedly".to_string()),
        };
        out.lock().unwrap_or_else(PoisonError::into_inner).result = Some(result);
        repaint.request_repaint();
    });
    app.transcribe_job = Some(match spawned {
        Ok(_) => Job::Running { shared, cancel },
        Err(e) => Job::Failed(e.to_string()),
    });
}

/// `bytes` as megabytes, for the confirmation.
fn megabytes(bytes: u64) -> String {
    (bytes / 1_000_000).to_string()
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // a finished job is applied here, whichever panel is showing
    if let Some(Job::Running { shared, .. }) = &app.transcribe_job {
        let finished = shared.lock().unwrap_or_else(PoisonError::into_inner).result.take();
        match finished {
            Some(Ok(done)) => {
                app.transcribe_job = match filmcraft_engine::transcript::finish(&mut app.session, done) {
                    Ok(_) => None,
                    Err(e) => Some(Job::Failed(e.to_string())),
                };
            }
            Some(Err(e)) => app.transcribe_job = Some(Job::Failed(e)),
            None => {}
        }
    }
    let Some(job) = app.transcribe_job.take() else { return };
    let mut keep = Some(job);
    let mut download = None;
    let mut elems: Vec<(&str, egui::Rect, String)> = Vec::new();
    egui::Window::new(tl!("Transcribe"))
        .id(egui::Id::unique("Transcribe job"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            match keep.take() {
                Some(Job::Confirm(job, name, size)) => {
                    let text = tlf!("{model} is needed to transcribe. Download it now ({size} MB)?", model = name.clone(), size = megabytes(size));
                    let l = ui.label(&text);
                    elems.push(("transcribe.status", l.rect, text));
                    ui.weak(tl!("It comes from huggingface.co and is checked before use. This happens once."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let c = ui.button(tl!("Cancel"));
                        elems.push(("transcribe.cancel", c.rect, "Cancel".into()));
                        let d = ui.button(tl!("Download"));
                        elems.push(("transcribe.download", d.rect, "Download".into()));
                        if d.clicked() {
                            download = Some(*job);
                        } else if !c.clicked() {
                            keep = Some(Job::Confirm(job, name, size));
                        }
                    });
                }
                Some(Job::Running { shared, cancel }) => {
                    let (status, fraction) = {
                        let s = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        (s.status.clone(), s.fraction)
                    };
                    let shown = if status.is_empty() { tl!("Starting…").to_string() } else { status };
                    let l = ui.label(&shown);
                    elems.push(("transcribe.status", l.rect, shown));
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    ui.add_space(8.0);
                    let c = ui.button(tl!("Cancel"));
                    elems.push(("transcribe.cancel", c.rect, "Cancel".into()));
                    if c.clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    keep = Some(Job::Running { shared, cancel });
                }
                Some(Job::Failed(error)) => {
                    let l = ui.label(&error);
                    elems.push(("transcribe.status", l.rect, error.clone()));
                    ui.add_space(8.0);
                    let c = ui.button(tl!("Close"));
                    elems.push(("transcribe.close", c.rect, "Close".into()));
                    if !c.clicked() {
                        keep = Some(Job::Failed(error));
                    }
                }
                None => {}
            }
        });
    app.transcribe_job = keep;
    for (id, r, l) in elems {
        app.auto.add(id, r, &l);
    }
    if let Some(job) = download {
        run(app, ctx, job);
    }
}
