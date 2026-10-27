//! Project files store each composition's poster frame (`posters`): a PNG of the comp at its
//! poster time, longest side at most 320 pixels. Reading a project ignores it.

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, Project};
use effectcraft_time::Tick;
use serde_json::{Map, Value, json};

use crate::Session;

fn out_dir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// "Wide" (640×360, blue): a red solid from 1 s on, poster time 1.5 s. "Tall" (90×200): a green
/// solid, poster time 0.
fn two_comps() -> (Session, ItemId, ItemId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Wide", "width": 640, "height": 360, "frameRate": 30, "duration": 3})).unwrap();
    let wide = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000"})).unwrap();
    let p = std::sync::Arc::make_mut(&mut s.project);
    let c = p.comp_mut(wide).unwrap();
    c.background = [0.0, 0.0, 1.0];
    c.layers[0].in_point = Tick::from_seconds_f64(1.0);
    s.set_time(Tick::from_seconds_f64(1.5));
    s.execute("comp.setPosterTime", json!({})).unwrap();
    s.execute("comp.new", json!({"name": "Tall", "width": 90, "height": 200, "frameRate": 30, "duration": 3})).unwrap();
    let tall = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"name": "Green", "color": "#00ff00"})).unwrap();
    (s, wide, tall)
}

fn posters(json: &str) -> Map<String, Value> {
    let v: Value = serde_json::from_str(json).unwrap();
    v.get("posters").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// A stored poster decoded to (width, height, RGBA).
fn decode(posters: &Map<String, Value>, id: ItemId) -> (u32, u32, Vec<u8>) {
    let b64 = posters[&id.0.to_string()].as_str().unwrap();
    let png = effectcraft_track::roto::rle::base64_decode(b64).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    (img.width(), img.height(), img.into_raw())
}

#[test]
fn a_save_stores_each_comps_poster_frame() {
    let dir = out_dir("project-posters");
    let (mut s, wide, tall) = two_comps();
    let path = dir.join("Posters.ecproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let stored = posters(&text);
    assert_eq!(stored.len(), 2, "{:?}", stored.keys().collect::<Vec<_>>());
    // The longest side is 320 at most and a smaller comp keeps its size; the pixels are the comp's
    // render at its poster time, where the red solid has started.
    let (w, h, px) = decode(&stored, wide);
    assert_eq!((w, h), (320, 180));
    assert_eq!((w, h, px.clone()), s.render_rgba8(wide, Tick::from_seconds_f64(1.5), 320).unwrap());
    let mid = ((h / 2 * w + w / 2) * 4) as usize;
    assert_eq!(px[mid..mid + 4], [255, 0, 0, 255]);
    let (w, h, px) = decode(&stored, tall);
    assert_eq!((w, h), (90, 200));
    assert_eq!((w, h, px), s.render_rgba8(tall, Tick::ZERO, 320).unwrap());
    // The posters come after the project, and opening the file gives the same project.
    assert!(text.find("\"posters\"").unwrap() > text.find("\"next_id\"").unwrap());
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(t.project.to_json(), s.project.to_json());
    // An XML copy carries them too.
    let xml_path = dir.join("Posters.ecprojx").to_string_lossy().to_string();
    s.execute("file.saveCopyAsXml", json!({"path": xml_path})).unwrap();
    let xml = std::fs::read_to_string(&xml_path).unwrap();
    assert_eq!(posters(&crate::xml_project::from_xml(&xml).unwrap()), stored);
}

#[test]
fn a_project_saved_without_posters_opens_and_saves_with_them() {
    let dir = out_dir("project-posters-old");
    let (s, wide, tall) = two_comps();
    let path = dir.join("Old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, s.project.to_file_json_with_view(&s.saved_view()).unwrap()).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(t.project.to_json(), s.project.to_json());
    t.execute("file.save", json!({})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let stored = posters(&text);
    assert!(stored.contains_key(&wide.0.to_string()) && stored.contains_key(&tall.0.to_string()), "{text:.200}");
    assert_eq!(Project::from_json(&text).unwrap(), *s.project);
}

#[test]
fn a_comp_with_missing_footage_saves_without_a_poster() {
    let dir = out_dir("project-posters-missing");
    let (mut s, wide, tall) = two_comps();
    let gone = std::sync::Arc::make_mut(&mut s.project).add_item(
        "gone.mov",
        Default::default(),
        None,
        ItemKind::Footage(Footage {
            path: "/offline/gone.mov".into(),
            kind: FootageKind::Video,
            width: 64,
            height: 64,
            has_video: true,
            duration: Tick::from_seconds_f64(3.0),
            missing: true,
            ..Default::default()
        }),
    );
    s.execute("layer.addItem", json!({"comp": tall.0, "item": gone.0, "time": 0.0})).unwrap();
    // A comp that nests it uses the missing footage too.
    s.execute("comp.new", json!({"name": "Outer", "width": 100, "height": 100, "frameRate": 30, "duration": 3})).unwrap();
    let outer = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"comp": outer.0, "item": tall.0, "time": 0.0})).unwrap();
    let path = dir.join("Missing.ecproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let keys: Vec<String> = posters(&text).keys().cloned().collect();
    assert_eq!(keys, [wide.0.to_string()]);
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(t.project.to_json(), s.project.to_json());
}
