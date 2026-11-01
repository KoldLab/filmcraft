//! Sequence presets (File ▸ New ▸ Sequence… ▸ Presets): named starting points for a new sequence
//! (frame size, timebase, timecode, audio, render quality and track counts), saved as small JSON
//! files in `<data dir>/Sequence Presets` so they can be shared between editors.
//!
//! The built-in presets can be overridden: saving one under a built-in name writes a user file
//! that shadows it (listed as `modified`), and deleting that file ("Restore Original") brings the
//! built-in values back. Built-in presets themselves can't be deleted.
//!
//! Commands: `sequence.presets`, `sequence.savePreset`, `sequence.deletePreset`,
//! `sequence.importPreset`, `sequence.exportPreset`.

use std::path::{Path, PathBuf};

use filmcraft_project::MAX_FRAME_SIDE;
use filmcraft_time::FrameRate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p};
use crate::{Result, Session};

/// The folder of sequence presets, beside `preferences.json`.
pub const SEQUENCE_PRESETS_DIR: &str = "Sequence Presets";
/// Largest preset file read; a real one is under 1 KiB.
const PRESET_MAX_BYTES: u64 = 64 * 1024;
/// Longest preset name, in characters.
const NAME_MAX: usize = 64;
/// Longest description, in characters.
const DESCRIPTION_MAX: usize = 500;
/// Most files looked at in the presets folder.
const PRESETS_MAX: usize = 500;
/// Most tracks of each kind a preset starts a sequence with (as `file.newSequence`).
const TRACKS_MAX: u32 = 256;
const MIXES: [&str; 4] = ["Stereo", "Mono", "5.1", "Adaptive"];

/// A sequence preset: what New Sequence starts from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SequencePreset {
    pub name: String,
    pub description: String,
    pub width: u32,
    pub height: u32,
    /// The timebase, frames per second as `fps_num / fps_den`.
    pub fps_num: i64,
    pub fps_den: i64,
    /// Drop-frame timecode (NTSC rates only).
    pub drop_frame: bool,
    /// `Stereo`, `Mono`, `5.1` or `Adaptive`.
    pub mix: String,
    pub sample_rate: u32,
    pub max_render_quality: bool,
    pub video_tracks: u32,
    pub audio_tracks: u32,
}

impl Default for SequencePreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            width: 1920,
            height: 1080,
            fps_num: 24_000,
            fps_den: 1001,
            drop_frame: false,
            mix: "Stereo".into(),
            sample_rate: 48_000,
            max_render_quality: false,
            video_tracks: 3,
            audio_tracks: 3,
        }
    }
}

/// The built-in presets: (name, description, width, height, fps num, fps den). Stereo 48 kHz, three
/// video and three audio tracks. The first is New Sequence's starting point.
const BUILTINS: [(&str, &str, u32, u32, i64, i64); 6] = [
    ("1080p 23.976 fps", "Full HD 1920 × 1080 at 23.976 fps, the usual film-look rate.", 1920, 1080, 24_000, 1001),
    ("1080p 25 fps", "Full HD 1920 × 1080 at 25 fps (PAL regions).", 1920, 1080, 25, 1),
    ("1080p 30 fps", "Full HD 1920 × 1080 at 30 fps, common for web and screen recordings.", 1920, 1080, 30, 1),
    ("4K UHD 25 fps", "Ultra HD 3840 × 2160 at 25 fps.", 3840, 2160, 25, 1),
    ("Vertical 1080 × 1920 30 fps", "Vertical 9:16 for Reels, Shorts and TikTok, at 30 fps.", 1080, 1920, 30, 1),
    ("Square 1080 × 1080 30 fps", "Square 1:1 for social feeds, at 30 fps.", 1080, 1080, 30, 1),
];

impl SequencePreset {
    /// Every built-in preset, New Sequence's starting point first.
    pub fn builtins() -> Vec<Self> {
        BUILTINS
            .iter()
            .map(|&(name, description, width, height, fps_num, fps_den)| Self {
                name: name.into(),
                description: description.into(),
                width,
                height,
                fps_num,
                fps_den,
                ..Self::default()
            })
            .collect()
    }

    /// The built-in preset called `name` (any case).
    pub fn builtin(name: &str) -> Option<Self> {
        Self::builtins().into_iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn rate(&self) -> FrameRate {
        FrameRate { num: self.fps_num, den: self.fps_den }.sane()
    }

    /// Whether the settings (not the name or description) are the same as `other`'s.
    pub fn same_settings(&self, other: &Self) -> bool {
        Self { name: String::new(), description: String::new(), ..self.clone() } == Self { name: String::new(), description: String::new(), ..other.clone() }
    }

    /// Bring every value in range: a name of 1 to 64 characters (else an error), the description
    /// cut to 500, a frame side of 1 to 32 768, a sane timebase (drop-frame only where it exists),
    /// a known channel format, 8–192 kHz and at most 256 tracks of each kind.
    pub fn sanitize(mut self) -> std::result::Result<Self, String> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() || self.name.chars().count() > NAME_MAX {
            return Err(format!("the preset needs a name of 1 to {NAME_MAX} characters"));
        }
        self.description = self.description.trim().chars().take(DESCRIPTION_MAX).collect();
        self.width = self.width.clamp(1, MAX_FRAME_SIDE);
        self.height = self.height.clamp(1, MAX_FRAME_SIDE);
        filmcraft_project::validate_frame_size(self.width, self.height)?;
        let r = self.rate();
        (self.fps_num, self.fps_den) = (r.num, r.den);
        self.drop_frame &= r.supports_drop_frame();
        if !MIXES.contains(&self.mix.as_str()) {
            self.mix = "Stereo".into();
        }
        self.sample_rate = self.sample_rate.clamp(8_000, 192_000);
        self.video_tracks = self.video_tracks.min(TRACKS_MAX);
        self.audio_tracks = self.audio_tracks.min(TRACKS_MAX);
        Ok(self)
    }

    /// Check a preset file: at most 64 KiB of JSON with a name; the rest is [`Self::sanitize`]d.
    pub fn parse(bytes: &[u8]) -> std::result::Result<Self, String> {
        if bytes.len() as u64 > PRESET_MAX_BYTES {
            return Err("the file is too large to be a sequence preset".into());
        }
        let p: Self = serde_json::from_slice(bytes).map_err(|e| format!("not a sequence preset: {e}"))?;
        p.sanitize()
    }

    pub fn read(path: &Path) -> std::result::Result<Self, String> {
        use std::io::Read as _;
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(PRESET_MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes)
    }

    pub fn write(&self, path: &Path) -> std::result::Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        filmcraft_format::atomic_write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// One preset as listed: `builtin` for a built-in name, `modified` when a user file overrides that
/// built-in, `file` the user file (None for an unmodified built-in).
#[derive(Clone, Debug, PartialEq)]
pub struct SequencePresetEntry {
    pub preset: SequencePreset,
    pub builtin: bool,
    pub modified: bool,
    pub file: Option<PathBuf>,
}

impl Session {
    /// The sequence presets folder (None without a data directory, e.g. tests and the web).
    pub fn sequence_presets_dir(&self) -> Option<PathBuf> {
        self.prefs_path.as_deref().and_then(Path::parent).map(|d| d.join(SEQUENCE_PRESETS_DIR))
    }

    /// The built-in presets (overridden ones with the user's values), then the user's presets by
    /// name. Files that fail [`SequencePreset::parse`] and names already taken are left out.
    pub fn sequence_presets(&self) -> Vec<SequencePresetEntry> {
        let mut found: Vec<(SequencePreset, PathBuf)> = self
            .sequence_presets_dir()
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .flatten()
            .take(PRESETS_MAX)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
            .filter_map(|p| SequencePreset::read(&p).ok().map(|preset| (preset, p)))
            .collect();
        found.sort_by(|a, b| (a.0.name.to_lowercase(), &a.1).cmp(&(b.0.name.to_lowercase(), &b.1)));
        let mut out: Vec<SequencePresetEntry> = Vec::new();
        for b in SequencePreset::builtins() {
            let entry = match found.iter().position(|(p, _)| p.name.eq_ignore_ascii_case(&b.name)) {
                Some(i) => {
                    let (mut p, file) = found.remove(i);
                    p.name = b.name.clone();
                    SequencePresetEntry { preset: p, builtin: true, modified: true, file: Some(file) }
                }
                None => SequencePresetEntry { preset: b, builtin: true, modified: false, file: None },
            };
            out.push(entry);
        }
        for (p, file) in found {
            if !out.iter().any(|e| e.preset.name.eq_ignore_ascii_case(&p.name)) {
                out.push(SequencePresetEntry { preset: p, builtin: false, modified: false, file: Some(file) });
            }
        }
        out
    }

    /// Save `preset` to the presets folder and return it as saved. A preset of the same name (any
    /// case) is replaced only with `overwrite`; under a built-in name it overrides the built-in,
    /// and saving a built-in's own values back restores it.
    pub fn save_sequence_preset(&self, preset: SequencePreset, overwrite: bool) -> std::result::Result<SequencePreset, String> {
        let mut p = preset.sanitize()?;
        let existing = self.sequence_presets().into_iter().find(|e| e.preset.name.eq_ignore_ascii_case(&p.name));
        if existing.is_some() && !overwrite {
            return Err(format!("a sequence preset named “{}” already exists", p.name));
        }
        // replacing keeps the name as it was spelled; a built-in keeps its own
        if let Some(e) = &existing {
            p.name = e.preset.name.clone();
        }
        let builtin = SequencePreset::builtin(&p.name);
        let dir = self.sequence_presets_dir().ok_or("there is no presets folder in this session")?;
        if let Some(file) = existing.and_then(|e| e.file) {
            std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        if builtin.is_some_and(|b| b.same_settings(&p) && b.description == p.description) {
            return Ok(p);
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let safe: String = p.name.chars().map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' }).collect();
        let safe = safe.trim();
        let mut path = dir.join(format!("{safe}.json"));
        // two names can make the same file name ("a/b" and "a_b"): don't clobber the other preset
        let mut n = 2;
        while path.exists() && n < 1000 {
            path = dir.join(format!("{safe} {n}.json"));
            n += 1;
        }
        p.write(&path)?;
        Ok(p)
    }

    /// Delete a user preset, or restore a built-in one the user overrode. An unmodified built-in
    /// preset can't be deleted.
    pub fn delete_sequence_preset(&self, name: &str) -> std::result::Result<(), String> {
        let e = self
            .sequence_presets()
            .into_iter()
            .find(|e| e.preset.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("there is no sequence preset “{name}”"))?;
        let file = e.file.ok_or("built-in presets can't be deleted")?;
        std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))
    }
}

fn entry_json(e: &SequencePresetEntry) -> Value {
    let mut v = serde_json::to_value(&e.preset).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.insert("builtin".into(), json!(e.builtin));
        o.insert("modified".into(), json!(e.modified));
    }
    v
}

fn presets_json(s: &Session) -> Value {
    let presets: Vec<Value> = s.sequence_presets().iter().map(entry_json).collect();
    json!({"dir": s.sequence_presets_dir(), "presets": presets})
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(presets_json(s))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let preset: SequencePreset = serde_json::from_value(p.clone()).map_err(|e| bad("sequence.savePreset", e.to_string()))?;
    let saved = s.save_sequence_preset(preset, bool_p(p, "overwrite") == Some(true)).map_err(|e| bad("sequence.savePreset", e))?;
    Ok(json!({"name": saved.name, "presets": presets_json(s)}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").ok_or_else(|| bad("sequence.deletePreset", "need `name`"))?;
    s.delete_sequence_preset(name).map_err(|e| bad("sequence.deletePreset", e))?;
    Ok(presets_json(s))
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("sequence.importPreset", "need `path`"))?;
    let preset = SequencePreset::read(Path::new(path)).map_err(|e| bad("sequence.importPreset", e))?;
    let saved = s.save_sequence_preset(preset, true).map_err(|e| bad("sequence.importPreset", e))?;
    Ok(json!({"name": saved.name, "presets": presets_json(s)}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").ok_or_else(|| bad("sequence.exportPreset", "need `name`"))?;
    let path = str_p(p, "path").ok_or_else(|| bad("sequence.exportPreset", "need `path`"))?;
    let e = s
        .sequence_presets()
        .into_iter()
        .find(|e| e.preset.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| bad("sequence.exportPreset", format!("there is no sequence preset “{name}”")))?;
    e.preset.write(Path::new(path)).map_err(|e| bad("sequence.exportPreset", e))?;
    Ok(json!({"name": e.preset.name, "path": path}))
}

fn spec(id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>, journal: bool) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec("sequence.presets", "Sequence Presets", "{}", list, false),
        spec(
            "sequence.savePreset",
            "Save Sequence Preset",
            r#"{"name":str,"description":str?,"width":u32=1920,"height":u32=1080,"fpsNum":i64=24000,"fpsDen":i64=1001,"dropFrame":bool?,"mix":"Stereo|Mono|5.1|Adaptive"?,"sampleRate":u32=48000,"maxRenderQuality":bool?,"videoTracks":n=3,"audioTracks":n=3,"overwrite":bool?}"#,
            save,
            true,
        ),
        spec("sequence.deletePreset", "Delete Sequence Preset", r#"{"name":str}"#, delete, true),
        spec("sequence.importPreset", "Import Sequence Preset", r#"{"path":str}"#, import, true),
        spec("sequence.exportPreset", "Export Sequence Preset", r#"{"name":str,"path":str}"#, export, false),
    ]
}
