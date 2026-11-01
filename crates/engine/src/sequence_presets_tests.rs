use serde_json::json;

use crate::Session;
use crate::media_test_util::tmp_dir;
use crate::sequence_presets::SequencePreset;

fn session(name: &str) -> (Session, std::path::PathBuf) {
    let dir = tmp_dir(name);
    (Session { prefs_path: Some(dir.join("preferences.json")), ..Session::default() }, dir)
}

fn names(s: &mut Session) -> Vec<String> {
    let v = s.execute("sequence.presets", json!({})).unwrap();
    v["presets"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect()
}

fn find(s: &mut Session, name: &str) -> serde_json::Value {
    let v = s.execute("sequence.presets", json!({})).unwrap();
    v["presets"].as_array().unwrap().iter().find(|p| p["name"] == name).cloned().unwrap()
}

/// The built-in presets: New Sequence's first one is the default sequence settings, every one is
/// valid as shipped, and names are distinct.
#[test]
fn builtins_are_valid_and_start_from_the_defaults() {
    let b = SequencePreset::builtins();
    assert_eq!(b.len(), 6);
    let d = filmcraft_project::SequenceSettings::default();
    assert_eq!((b[0].width, b[0].height, b[0].rate(), b[0].sample_rate), (d.width, d.height, d.frame_rate, d.sample_rate));
    for p in &b {
        assert_eq!(p.clone().sanitize().as_ref(), Ok(p), "{} is in range", p.name);
        assert_eq!(b.iter().filter(|q| q.name.eq_ignore_ascii_case(&p.name)).count(), 1);
    }
    assert!(b.iter().any(|p| (p.width, p.height) == (1080, 1920)), "a vertical preset");
}

/// Save writes a user preset; saving the same name again needs `overwrite`, which replaces it.
#[test]
fn save_and_overwrite_a_user_preset() {
    let (mut s, dir) = session("seq-presets-save");
    let builtins = names(&mut s);
    let r = s
        .execute(
            "sequence.savePreset",
            json!({"name": " Client 4K ", "width": 3840, "height": 2160, "fpsNum": 50, "fpsDen": 1, "videoTracks": 6, "audioTracks": 2}),
        )
        .unwrap();
    assert_eq!(r["name"], "Client 4K");
    assert!(dir.join("Sequence Presets").join("Client 4K.json").is_file());
    let mut want = builtins.clone();
    want.push("Client 4K".into());
    assert_eq!(names(&mut s), want);
    let p = find(&mut s, "Client 4K");
    assert_eq!((p["width"].as_u64(), p["fpsNum"].as_i64(), p["videoTracks"].as_u64(), p["builtin"].as_bool()), (Some(3840), Some(50), Some(6), Some(false)));

    // same name (any case) without overwrite: refused, the preset is unchanged
    let e = s.execute("sequence.savePreset", json!({"name": "client 4k", "width": 1280, "height": 720})).unwrap_err();
    assert!(e.to_string().contains("already exists"), "{e}");
    assert_eq!(find(&mut s, "Client 4K")["width"], 3840);
    // with overwrite: replaced, still one preset
    s.execute("sequence.savePreset", json!({"name": "client 4k", "width": 1280, "height": 720, "overwrite": true})).unwrap();
    assert_eq!(names(&mut s).len(), want.len());
    assert_eq!(find(&mut s, "Client 4K")["width"], 1280, "replacing keeps the name");
    assert_eq!(std::fs::read_dir(dir.join("Sequence Presets")).unwrap().count(), 1, "the old file is gone");

    s.execute("sequence.deletePreset", json!({"name": "Client 4K"})).unwrap();
    assert_eq!(names(&mut s), builtins);
}

/// A built-in preset can be overridden (listed as modified, at its place) and restored by
/// deleting the override; the built-in itself can't be deleted.
#[test]
fn override_and_restore_a_builtin() {
    let (mut s, _dir) = session("seq-presets-override");
    let before = names(&mut s);
    assert!(s.execute("sequence.savePreset", json!({"name": "1080p 25 fps", "videoTracks": 1})).is_err(), "needs overwrite");
    s.execute("sequence.savePreset", json!({"name": "1080P 25 FPS", "fpsNum": 25, "fpsDen": 1, "videoTracks": 1, "audioTracks": 8, "overwrite": true}))
        .unwrap();
    assert_eq!(names(&mut s), before, "same list and order, the built-in's name kept");
    let p = find(&mut s, "1080p 25 fps");
    assert_eq!(
        (p["videoTracks"].as_u64(), p["audioTracks"].as_u64(), p["modified"].as_bool(), p["builtin"].as_bool()),
        (Some(1), Some(8), Some(true), Some(true))
    );

    // Restore Original
    s.execute("sequence.deletePreset", json!({"name": "1080p 25 fps"})).unwrap();
    let p = find(&mut s, "1080p 25 fps");
    assert_eq!((p["videoTracks"].as_u64(), p["modified"].as_bool()), (Some(3), Some(false)));
    assert!(s.execute("sequence.deletePreset", json!({"name": "1080p 25 fps"})).is_err(), "built-ins stay");

    // saving a built-in's own values back is the same as restoring it
    let b = SequencePreset::builtin("4K UHD 25 fps").unwrap();
    let mut v = serde_json::to_value(&b).unwrap();
    v["overwrite"] = json!(true);
    s.execute("sequence.savePreset", v).unwrap();
    assert_eq!(find(&mut s, "4K UHD 25 fps")["modified"], false);
}

/// Hostile files and parameters: out-of-range values are brought in range, nameless or oversized
/// files are refused, junk in the folder is skipped; import and export round-trip.
#[test]
fn hostile_values_import_and_export() {
    let (mut s, dir) = session("seq-presets-hostile");
    let r = s.execute(
        "sequence.savePreset",
        json!({"name": "Odd", "width": 0, "height": 99_999_999u64, "fpsNum": -5, "fpsDen": 0, "dropFrame": true, "mix": "7.1", "sampleRate": 1, "videoTracks": 1_000_000, "audioTracks": 4_000_000_000u64}),
    );
    let p = find(&mut s, "Odd");
    assert!(r.is_ok(), "{r:?}");
    assert_eq!((p["width"].as_u64(), p["height"].as_u64()), (Some(1), Some(32_768)));
    assert!(p["fpsDen"].as_i64().unwrap() > 0 && p["fpsNum"].as_i64().unwrap() > 0);
    assert_eq!(
        (p["mix"].as_str(), p["sampleRate"].as_u64(), p["videoTracks"].as_u64(), p["audioTracks"].as_u64()),
        (Some("Stereo"), Some(8_000), Some(256), Some(256))
    );
    assert!(s.execute("sequence.savePreset", json!({"name": "   "})).is_err());
    assert!(s.execute("sequence.savePreset", json!({"name": "x".repeat(65)})).is_err());

    assert!(SequencePreset::parse(&vec![b' '; 70 * 1024]).is_err());
    std::fs::write(dir.join("Sequence Presets").join("junk.json"), b"{not json").unwrap();
    std::fs::write(dir.join("Sequence Presets").join("nameless.json"), b"{\"width\": 10}").unwrap();
    assert_eq!(names(&mut s).len(), 7, "unreadable files are skipped");

    // export, delete, import back
    let out = dir.join("shared.json");
    s.execute("sequence.exportPreset", json!({"name": "Odd", "path": out.to_string_lossy()})).unwrap();
    s.execute("sequence.deletePreset", json!({"name": "Odd"})).unwrap();
    assert_eq!(s.execute("sequence.importPreset", json!({"path": out.to_string_lossy()})).unwrap()["name"], "Odd");
    assert_eq!(find(&mut s, "Odd")["height"], 32_768);
    assert!(s.execute("sequence.importPreset", json!({"path": dir.join("missing.json").to_string_lossy()})).is_err());

    // names that make the same file name don't clobber each other
    s.execute("sequence.savePreset", json!({"name": "a/b"})).unwrap();
    s.execute("sequence.savePreset", json!({"name": "a_b"})).unwrap();
    assert!(names(&mut s).iter().any(|n| n == "a/b") && names(&mut s).iter().any(|n| n == "a_b"));
}

/// Without a data folder (tests, the web) the built-ins still list and saving is an error.
#[test]
fn no_data_folder() {
    let mut s = Session::default();
    assert_eq!(names(&mut s).len(), 6);
    assert!(s.execute("sequence.savePreset", json!({"name": "X"})).is_err());
}
