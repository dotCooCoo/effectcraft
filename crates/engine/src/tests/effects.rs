//! Effect Controls operations: copy/paste, duplicate, reorder, remove (Delete), reset, and the
//! Edit menu routing to them when effects are selected, all with undo.

use effectcraft_keyframe::{Ease, Interp, Value as KV};
use effectcraft_project::{Layer, LayerId, PropGroup};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

fn comp_with_two_solids() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 400, "height": 300})).unwrap()["layer"].as_u64().unwrap();
    (s, a, b)
}

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn fx_names(s: &Session, id: u64) -> Vec<String> {
    layer(s, id).effects().map(|f| f.groups().map(|g| g.name.clone()).collect()).unwrap_or_default()
}

fn apply(s: &mut Session, lid: u64, effect: &str) -> u64 {
    s.execute("effect.apply", json!({"layers": [lid], "effect": effect})).unwrap()["effects"][0].as_u64().unwrap()
}

fn select_effect(s: &mut Session, lid: u64, uid: u64) {
    s.state.selected_layers = vec![LayerId(lid)];
    s.state.selected_props = vec![(LayerId(lid), uid)];
}

fn effect_group(s: &Session, lid: u64, uid: u64) -> PropGroup {
    layer(s, lid).effects().unwrap().groups().find(|g| g.uid == uid).unwrap().clone()
}

fn animate_effect(s: &mut Session, lid: u64, uid: u64, path: &str, keys: &[(f64, f64)]) {
    let prop = effect_group(s, lid, uid).prop(path).unwrap().uid;
    for &(time, value) in keys {
        s.execute("prop.addKey", json!({"layer": lid, "prop": prop, "time": time, "timeBase": "comp", "value": value})).unwrap();
    }
}

fn set_layer_clock(s: &mut Session, lid: u64, start: f64, stretch: f64) {
    let cid = s.active_comp_id().unwrap();
    let l = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(lid)).unwrap();
    l.start_time = Tick::from_seconds_f64(start);
    l.stretch = stretch;
}

#[test]
fn copy_paste_effects_between_layers_with_undo() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let gid = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    s.execute("prop.set", json!({"layer": a, "prop": gid, "value": 12.5})).unwrap();
    select_effect(&mut s, a, blur);
    // Edit ▸ Copy with an effect selected copies the effect, not the layer.
    s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(s.state.effect_clipboard.len(), 1);
    assert!(s.state.clipboard.is_empty());
    // Paste onto the other layer.
    s.state.selected_layers = vec![LayerId(b)];
    s.state.selected_props.clear();
    let n_layers = s.active_comp().unwrap().layers.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n_layers, "no layer pasted");
    assert_eq!(fx_names(&s, b), vec!["Gaussian Blur"]);
    let pasted = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    assert_ne!(pasted.uid, blur, "fresh uids");
    assert_eq!(pasted.get("blurriness").unwrap().value, KV::Scalar(12.5));
    // Pasting again names the second instance uniquely.
    s.execute("effect.paste", json!({"layers": [b]})).unwrap();
    assert_eq!(fx_names(&s, b), vec!["Gaussian Blur", "Gaussian Blur 2"]);
    s.undo();
    s.undo();
    assert!(fx_names(&s, b).is_empty());
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur"]);
}

/// #989: copying an animated effect aligns its first key to the destination CTI.
#[test]
fn pasted_effect_keys_start_at_the_playhead() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let uid = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    for (time, value) in [(1.0, 10.0), (3.0, 30.0)] {
        s.execute("prop.addKey", json!({"layer": a, "prop": uid, "time": time, "value": value})).unwrap();
    }
    let source = layer(&s, a);
    select_effect(&mut s, a, blur);
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    let undo = s.history.undo.len();
    s.execute("edit.paste", json!({})).unwrap();
    let pasted = layer(&s, b);
    let keys = &pasted.effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().keys;
    assert_eq!(keys.iter().map(|k| pasted.comp_time(k.time).seconds()).collect::<Vec<_>>(), vec![5.0, 7.0]);
    assert_eq!(keys.iter().map(|k| k.value.as_f64()).collect::<Vec<_>>(), vec![10.0, 30.0]);
    assert_eq!(layer(&s, a), source, "copy and paste do not edit the source");
    assert_eq!(s.history.undo.len(), undo + 1, "the whole paste is one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(fx_names(&s, b).is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(layer(&s, b), pasted);
}

#[test]
fn pasted_effects_share_one_anchor_including_nested_parameters() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let fractal = apply(&mut s, a, "Fractal");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    animate_effect(&mut s, a, fractal, "mandelbrot/mandelbrotX", &[(2.0, 1.25), (4.0, 2.5)]);
    animate_effect(&mut s, a, fractal, "mandelbrot/mandelbrotY", &[(3.0, 0.5), (5.0, 2.0)]);
    let static_value = effect_group(&s, a, fractal).get("setChoice").unwrap().value.clone();
    s.state.selected_props = vec![(LayerId(a), blur), (LayerId(a), fractal)];
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    let reply = s.execute("effect.paste", json!({"layers": [b]})).unwrap();
    for (index, path, times) in
        [(0, "blurriness", vec![5.0, 7.0]), (1, "mandelbrot/mandelbrotX", vec![6.0, 8.0]), (1, "mandelbrot/mandelbrotY", vec![7.0, 9.0])]
    {
        let group = effect_group(&s, b, reply["effects"][index].as_u64().unwrap());
        assert_eq!(group.prop(path).unwrap().keys.iter().map(|k| k.time.seconds()).collect::<Vec<_>>(), times, "{path}");
    }
    let group = effect_group(&s, b, reply["effects"][1].as_u64().unwrap());
    assert_eq!(group.get("setChoice").unwrap().value, static_value, "unanimated controls stay unchanged");
}

#[test]
fn pasted_effects_preserve_comp_timing_and_eased_values_on_stretched_layers() {
    for (source_start, source_stretch) in [(2.0, 100.0), (1.0, 200.0), (8.0, -200.0)] {
        for (target_start, target_stretch) in [(0.0, 100.0), (3.0, 200.0), (8.0, -100.0)] {
            let (mut s, a, b) = comp_with_two_solids();
            set_layer_clock(&mut s, a, source_start, source_stretch);
            set_layer_clock(&mut s, b, target_start, target_stretch);
            let blur = apply(&mut s, a, "Gaussian Blur");
            animate_effect(&mut s, a, blur, "blurriness", &[(3.0, 10.0), (5.0, 30.0)]);
            let uid = effect_group(&s, a, blur).get("blurriness").unwrap().uid;
            let cid = s.active_comp_id().unwrap();
            let pr = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_mut(uid).unwrap();
            for key in &mut pr.keys {
                key.in_interp = Interp::Bezier;
                key.out_interp = Interp::Bezier;
                key.in_ease = vec![Ease { speed: 7.0, influence: 0.25 }];
                key.out_ease = vec![Ease { speed: 3.0, influence: 0.6 }];
                key.label = 4;
            }
            let source = layer(&s, a);
            let source_pr = source.props.find(uid).unwrap();
            s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
            s.execute("time.set", json!({"time": 5.0})).unwrap();
            let pasted = s.execute("effect.paste", json!({"layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
            let target = layer(&s, b);
            let target_group = target.effects().unwrap().groups().find(|g| g.uid == pasted).unwrap();
            let target_pr = target_group.get("blurriness").unwrap();
            let mut times: Vec<Tick> = target_pr.keys.iter().map(|k| target.comp_time(k.time)).collect();
            times.sort();
            assert_eq!(times, vec![Tick::from_seconds_f64(5.0), Tick::from_seconds_f64(7.0)]);
            for offset in [0.0, 0.25, 0.75, 1.5, 2.0] {
                let expected = source_pr.value_at(source.layer_time(Tick::from_seconds_f64(3.0 + offset))).as_f64();
                let actual = target_pr.value_at(target.layer_time(Tick::from_seconds_f64(5.0 + offset))).as_f64();
                assert!((expected - actual).abs() < 1e-8, "source {source_stretch} %, target {target_stretch} %, t={offset}: {expected} != {actual}");
            }
            assert!(target_pr.keys.iter().all(|k| k.label == 4));
            assert_eq!(layer(&s, a), source);
        }
    }
}

#[test]
fn pasted_spatial_effect_keys_preserve_the_eased_path_on_reversed_layers() {
    for (source_stretch, target_stretch) in [(200.0, 100.0), (-200.0, 100.0), (100.0, -200.0)] {
        let (mut s, a, b) = comp_with_two_solids();
        set_layer_clock(&mut s, a, if source_stretch < 0.0 { 8.0 } else { 1.0 }, source_stretch);
        set_layer_clock(&mut s, b, if target_stretch < 0.0 { 8.0 } else { 0.0 }, target_stretch);
        let ramp = apply(&mut s, a, "Gradient Ramp");
        let uid = effect_group(&s, a, ramp).get("start").unwrap().uid;
        for (time, value) in [(3.0, [50.0, 25.0]), (5.0, [180.0, 75.0])] {
            s.execute("prop.addKey", json!({"layer": a, "prop": uid, "time": time, "timeBase": "comp", "value": value})).unwrap();
        }
        let cid = s.active_comp_id().unwrap();
        let pr = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_mut(uid).unwrap();
        pr.spatial = true;
        for key in &mut pr.keys {
            key.in_interp = Interp::Bezier;
            key.out_interp = Interp::Bezier;
            key.in_ease = vec![Ease { speed: 15.0, influence: 0.25 }];
            key.out_ease = vec![Ease { speed: 30.0, influence: 0.6 }];
            key.spatial_auto = false;
            key.spatial_in = [-10.0, -20.0, 0.0];
            key.spatial_out = [20.0, 5.0, 0.0];
        }
        let source = layer(&s, a);
        let source_pr = source.props.find(uid).unwrap();
        s.execute("effect.copy", json!({"layer": a, "effect": ramp})).unwrap();
        s.execute("time.set", json!({"time": 5.0})).unwrap();
        let pasted = s.execute("effect.paste", json!({"layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
        let target = layer(&s, b);
        let group = target.effects().unwrap().groups().find(|g| g.uid == pasted).unwrap();
        let target_pr = group.get("start").unwrap();
        for offset in [0.0, 0.25, 0.75, 1.5, 2.0] {
            let expected = source_pr.value_at(source.layer_time(Tick::from_seconds_f64(3.0 + offset))).components();
            let actual = target_pr.value_at(target.layer_time(Tick::from_seconds_f64(5.0 + offset))).components();
            assert_eq!(expected.len(), actual.len());
            assert!(expected.iter().zip(&actual).all(|(a, b)| (a - b).abs() < 0.05), "path at {offset}: {expected:?} != {actual:?}");
        }
    }
}

#[test]
fn effect_clipboard_is_a_snapshot_and_repeated_paste_uses_the_target_comps_cti() {
    let (mut s, a, _) = comp_with_two_solids();
    let original_comp = s.active_comp_id().unwrap();
    let blur = apply(&mut s, a, "Gaussian Blur");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let clipboard = s.state.effect_clipboard.clone();
    // Changing the source after Copy must not change what the clipboard will paste.
    animate_effect(&mut s, a, blur, "blurriness", &[(4.0, 99.0)]);
    s.execute("time.set", json!({"time": 9.0})).unwrap();
    let destination = s.execute("comp.new", json!({"name": "Destination", "frameRate": 24, "duration": 10})).unwrap()["comp"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 4.0})).unwrap();
    s.open_comp(original_comp);
    for (cti, expected) in [(4.0, vec![4.0, 6.0]), (2.0, vec![2.0, 4.0])] {
        s.state.times.insert(effectcraft_project::ItemId(destination), Tick::from_seconds_f64(cti));
        let uid = s.execute("effect.paste", json!({"comp": destination, "layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
        let l = s.project.comp(effectcraft_project::ItemId(destination)).unwrap().layer(LayerId(b)).unwrap();
        let pr = l.effects().unwrap().groups().find(|g| g.uid == uid).unwrap().get("blurriness").unwrap();
        assert_eq!(pr.keys.iter().map(|k| l.comp_time(k.time).seconds()).collect::<Vec<_>>(), expected);
        assert_eq!(pr.keys.iter().map(|k| k.value.as_f64()).collect::<Vec<_>>(), vec![10.0, 30.0]);
        assert_eq!(s.state.effect_clipboard, clipboard);
    }
}

#[test]
fn invalid_effect_paste_timing_leaves_every_target_and_history_unchanged() {
    for stretch in [0.0, f64::NAN, f64::INFINITY, 1e-300, 1e300] {
        let (mut s, a, b) = comp_with_two_solids();
        let blur = apply(&mut s, a, "Gaussian Blur");
        animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
        s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
        set_layer_clock(&mut s, b, 0.0, stretch);
        s.execute("time.set", json!({"time": 5.0})).unwrap();
        let project = s.project.clone();
        let clipboard = s.state.effect_clipboard.clone();
        let undo = s.history.undo.len();
        // A is valid and comes first; failure on B must roll back A's new effect too.
        assert!(s.execute("effect.paste", json!({"layers": [a, b]})).is_err(), "{stretch}");
        assert!(std::sync::Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.state.effect_clipboard, clipboard);
        assert_eq!(s.history.undo.len(), undo);
    }
}

#[test]
fn invalid_effect_copy_keeps_the_previous_clipboard() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let clipboard = s.state.effect_clipboard.clone();
    set_layer_clock(&mut s, a, 0.0, 0.0);
    assert!(s.execute("effect.copy", json!({"layer": a, "effect": blur})).is_err());
    assert_eq!(s.state.effect_clipboard, clipboard);
}

#[test]
fn effect_paste_keeps_exact_ntsc_frame_offsets() {
    let (mut s, a, b) = comp_with_two_solids();
    let cid = s.active_comp_id().unwrap();
    let frame = effectcraft_time::FrameRate::new(30000, 1001).frame_duration();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let uid = effect_group(&s, a, blur).get("blurriness").unwrap().uid;
    let pr = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_mut(uid).unwrap();
    pr.keys = vec![
        effectcraft_keyframe::Keyframe::new(frame.mul_ratio(11, 1), KV::Scalar(10.0)),
        effectcraft_keyframe::Keyframe::new(frame.mul_ratio(43, 1), KV::Scalar(30.0)),
    ];
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let cti = frame.mul_ratio(71, 1);
    s.state.times.insert(cid, cti);
    let pasted = s.execute("effect.paste", json!({"layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
    let group = effect_group(&s, b, pasted);
    assert_eq!(group.get("blurriness").unwrap().keys.iter().map(|k| k.time).collect::<Vec<_>>(), vec![cti, cti + frame.mul_ratio(32, 1)]);
}

#[test]
fn overflowing_effect_paste_times_return_an_error_without_an_edit() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let cid = s.active_comp_id().unwrap();
    for cti in [Tick::MAX, Tick(i64::MAX)] {
        s.state.times.insert(cid, cti);
        let project = s.project.clone();
        let undo = s.history.undo.len();
        assert!(s.execute("effect.paste", json!({"layers": [b]})).is_err());
        assert!(std::sync::Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.history.undo.len(), undo);
    }
}

#[test]
fn duplicate_reorder_and_delete_selected_effect() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    apply(&mut s, a, "Invert");
    select_effect(&mut s, a, blur);
    // Edit ▸ Duplicate (Cmd+D) duplicates the selected effect right after it and selects the copy.
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur", "Gaussian Blur 2", "Invert"]);
    let dup = s.state.selected_props[0].1;
    assert_ne!(dup, blur);
    // Reorder: move Invert to the top.
    s.execute("effect.reorder", json!({"layer": a, "effect": "Invert", "index": 1})).unwrap();
    assert_eq!(fx_names(&s, a), vec!["Invert", "Gaussian Blur", "Gaussian Blur 2"]);
    // Delete with the duplicate selected removes just that effect (not the layer).
    s.execute("edit.clear", json!({})).unwrap();
    assert_eq!(fx_names(&s, a), vec!["Invert", "Gaussian Blur"]);
    assert!(s.active_comp().unwrap().layer(LayerId(a)).is_some());
    s.undo();
    assert_eq!(fx_names(&s, a), vec!["Invert", "Gaussian Blur", "Gaussian Blur 2"]);
    s.undo();
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur", "Gaussian Blur 2", "Invert"]);
    s.undo();
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur", "Invert"]);
}

#[test]
fn reset_effect_restores_defaults_in_one_undo_step() {
    let (mut s, _, b) = comp_with_two_solids();
    let ramp = apply(&mut s, b, "Gradient Ramp");
    let g = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    let start = g.get("start").unwrap().uid;
    let shape = g.get("shape").unwrap().uid;
    // Point defaults are fractions of the layer (400 × 300).
    assert_eq!(g.get("start").unwrap().value, KV::Vec2([200.0, 0.0]));
    s.execute("prop.set", json!({"layer": b, "prop": start, "value": [10.0, 20.0]})).unwrap();
    s.execute("prop.set", json!({"layer": b, "prop": shape, "value": 1})).unwrap();
    let undo_before = s.history.undo.len();
    s.execute("effect.reset", json!({"layer": b, "effect": ramp})).unwrap();
    assert_eq!(s.history.undo.len(), undo_before + 1);
    let g = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    assert_eq!(g.get("start").unwrap().value, KV::Vec2([200.0, 0.0]));
    assert_eq!(g.get("shape").unwrap().value, KV::Enum(0));
    s.undo();
    let g = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    assert_eq!(g.get("start").unwrap().value, KV::Vec2([10.0, 20.0]));
}

#[test]
fn reset_restores_parameters_inside_twirl_down_groups() {
    let (mut s, a, _) = comp_with_two_solids();
    // Fractal and Scribble keep most parameters in groups (`mandelbrot/…`, `edgeOptions/…`).
    for (effect, changes) in [
        ("Fractal", [("setChoice", json!(1)), ("mandelbrot/mandelbrotX", json!(1.25)), ("fractalColor/hue", json!(90.0))]),
        ("Scribble", [("scribble", json!(2)), ("edgeOptions/edgeWidth", json!(33.0)), ("strokeOptions/spacing", json!(9.0))]),
    ] {
        let fx = apply(&mut s, a, effect);
        let group = |s: &Session| layer(s, a).effects().unwrap().groups().find(|g| g.uid == fx).unwrap().clone();
        let defaults: Vec<_> = changes.iter().map(|(id, _)| group(&s).prop(id).unwrap().value.clone()).collect();
        for (id, v) in &changes {
            let uid = group(&s).prop(id).unwrap().uid;
            s.execute("prop.set", json!({"layer": a, "prop": uid, "value": v})).unwrap();
            assert_ne!(group(&s).prop(id).unwrap().value, defaults[changes.iter().position(|(i, _)| i == id).unwrap()], "{effect} {id} changed");
        }
        s.execute("effect.reset", json!({"layer": a, "effect": fx})).unwrap();
        for ((id, _), d) in changes.iter().zip(&defaults) {
            assert_eq!(&group(&s).prop(id).unwrap().value, d, "{effect} {id} back to its default");
        }
    }
}

#[test]
fn reset_keys_animated_params_at_the_cti() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let uid = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    s.execute("prop.toggleAnimation", json!({"layer": a, "prop": uid})).unwrap();
    s.execute("prop.set", json!({"layer": a, "prop": uid, "value": 30.0})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("effect.reset", json!({"layer": a, "effect": blur})).unwrap();
    let pr = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().clone();
    assert_eq!(pr.keys.len(), 2, "a key at the CTI with the default value");
    assert_eq!(pr.keys[1].value, KV::Scalar(0.0));
}

#[test]
fn tint_swaps_only_its_colors_in_one_undo_step() {
    let (mut s, a, b) = comp_with_two_solids();
    let first = apply(&mut s, a, "Tint");
    let target = apply(&mut s, a, "Tint");
    let other = apply(&mut s, b, "Tint");
    for (path, value) in [("black", json!([1.0, 0.0, 0.0, 1.0])), ("white", json!([0.0, 0.25, 1.0, 1.0])), ("amount", json!(45.0))] {
        s.execute("prop.set", json!({"layer": a, "path": format!("effects/#2/{path}"), "value": value})).unwrap();
    }
    let before_a = layer(&s, a);
    let before_b = layer(&s, b);
    let n = s.history.undo.len();
    s.execute("effect.swapTintColors", json!({"layer": a, "effect": target})).unwrap();
    let swapped = layer(&s, a);
    let g = swapped.props.find_group(target).unwrap();
    assert_eq!(g.prop("black").unwrap().value, KV::Color([0.0, 0.25, 1.0, 1.0]));
    assert_eq!(g.prop("white").unwrap().value, KV::Color([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(g.prop("amount").unwrap().value, KV::Scalar(45.0));
    assert_eq!(swapped.props.find_group(first), before_a.props.find_group(first));
    assert_eq!(layer(&s, b).props.find_group(other), before_b.props.find_group(other));
    assert_eq!(s.history.undo.len(), n + 1);
    assert!(s.undo());
    assert_eq!(layer(&s, a).props, before_a.props);
    assert!(s.redo());
    assert_eq!(layer(&s, a).props, swapped.props);
}

#[test]
fn tint_swap_keys_animated_colors_at_layer_time() {
    let (mut s, a, _) = comp_with_two_solids();
    let target = apply(&mut s, a, "Tint");
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/black", "value": [1.0, 0.0, 0.0, 1.0]})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/white", "value": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    s.execute("prop.toggleAnimation", json!({"layer": a, "path": "effects/#1/black"})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/black", "value": [0.0, 1.0, 0.0, 1.0]})).unwrap();
    s.execute("layer.timeStretch", json!({"layers": [a], "percent": 200.0})).unwrap();
    // At comp time 2s the stretched layer is at 1s, between its unchanged 0s and 2s keys.
    let before = layer(&s, a);
    let g = before.props.find_group(target).unwrap();
    let lt = before.layer_time(s.time());
    let expected = g.prop("black").unwrap().value_at(lt);
    let keys = g.prop("black").unwrap().keys.clone();
    select_effect(&mut s, a, target);
    s.execute("effect.swapTintColors", json!({})).unwrap();
    let after = layer(&s, a);
    let g = after.props.find_group(target).unwrap();
    let black = g.prop("black").unwrap();
    assert_eq!(black.keys.len(), 3);
    assert_eq!(black.keys[0], keys[0]);
    assert_eq!(black.keys[2], keys[1]);
    assert_eq!(black.keys[1].time, lt);
    assert_eq!(black.keys[1].value, KV::Color([0.0, 0.0, 1.0, 1.0]));
    assert_eq!(g.prop("white").unwrap().value, expected);
    assert!(g.prop("white").unwrap().keys.is_empty());
    assert!(s.undo());
    assert_eq!(layer(&s, a).props, before.props);
}

#[test]
fn tint_swap_rejects_invalid_targets_without_editing() {
    let mut empty = Session::default();
    assert!(empty.execute("effect.swapTintColors", json!({})).is_err());
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let tint = apply(&mut s, a, "Tint");
    for params in [
        json!({"layer": a, "effect": blur}),
        json!({"layer": a, "effect": "missing"}),
        json!({"layer": a, "effect": []}),
        json!({"layer": a, "effect": 1.5}),
        json!({"layer": a, "effect": 0}),
        json!(false),
    ] {
        let before = layer(&s, a);
        let n = s.history.undo.len();
        assert!(s.execute("effect.swapTintColors", params).is_err());
        assert_eq!(layer(&s, a).props, before.props);
        assert_eq!(s.history.undo.len(), n);
    }
    // A damaged project's colour parameter must also fail atomically.
    let cid = s.active_comp_id().unwrap();
    s.edit("Break Tint", None, |p, _| {
        p.comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_group_mut(tint).unwrap().prop_mut("white").unwrap().value = KV::Scalar(1.0);
        Ok(())
    })
    .unwrap();
    let before = layer(&s, a);
    let n = s.history.undo.len();
    assert!(s.execute("effect.swapTintColors", json!({"layer": a, "effect": tint})).is_err());
    assert_eq!(layer(&s, a).props, before.props);
    assert_eq!(s.history.undo.len(), n);
}

#[test]
fn copy_layers_clears_effect_clipboard() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    select_effect(&mut s, a, blur);
    s.execute("effect.copy", json!({})).unwrap();
    s.state.selected_props.clear();
    s.state.selected_layers = vec![LayerId(b)];
    s.execute("edit.copy", json!({})).unwrap();
    assert!(s.state.effect_clipboard.is_empty());
    let n = s.active_comp().unwrap().layers.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n + 1, "layer pasted");
}

#[test]
fn layer_params_get_a_source_companion() {
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "ec.channel.blend");
    let g = layer(&s, a).effects().unwrap().groups().next().unwrap().clone();
    let layer_params: Vec<_> = g.props().filter(|p| matches!(p.ui, effectcraft_project::ParamUi::Layer)).map(|p| p.match_id.clone()).collect();
    assert!(!layer_params.is_empty());
    for id in layer_params {
        let src = g.get(&effectcraft_effects::layer_source_id(&id)).expect("companion");
        assert_eq!(src.value, KV::Enum(2), "Effects & Masks by default");
        assert!(matches!(src.ui, effectcraft_project::ParamUi::Hidden));
    }
}

/// Projects saved before parameters were renamed, regrouped, added or had their popups
/// reordered open with their instances brought up to date (`effectcraft_effects::migrate`).
#[test]
fn opening_an_old_project_upgrades_effect_instances() {
    use effectcraft_effects::migrate::PARAM_ID_ALIASES;
    use effectcraft_project::{Node, ParamUi, PropGroup};
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "ec.blur.gaussian");
    // One instance of every effect with renamed parameter ids.
    let mut aliased: Vec<&str> = PARAM_ID_ALIASES.iter().map(|(e, _, _)| *e).collect();
    aliased.dedup();
    for e in &aliased {
        apply(&mut s, a, e);
    }
    // Rewrite the saved instances the way an older version wrote them.
    fn take(g: &mut PropGroup, path: &str) -> Option<effectcraft_project::Property> {
        match path.split_once('/') {
            None => {
                let i = g.children.iter().position(|c| matches!(c, Node::Prop(p) if p.match_id == path))?;
                match g.children.remove(i) {
                    Node::Prop(p) => Some(p),
                    Node::Group(_) => None,
                }
            }
            Some((first, rest)) => take(g.sub_mut(first)?, rest),
        }
    }
    let mut old = (*s.project).clone();
    let cid = s.active_comp_id().unwrap();
    {
        let comp = std::sync::Arc::make_mut(match &mut old.items.get_mut(&cid).unwrap().kind {
            effectcraft_project::ItemKind::Comp(c) => c,
            _ => panic!("not a composition"),
        });
        let l = comp.layers.iter_mut().find(|l| l.id == LayerId(a)).unwrap();
        for n in &mut l.props.sub_mut("effects").unwrap().children {
            let Node::Group(g) = n else { continue };
            if g.match_id == "ec.blur.gaussian" {
                // Repeat Edge Pixels did not exist yet, Blurriness had another name, the popup
                // listed its options in reverse.
                take(g, "repeatEdge").unwrap();
                g.get_mut("blurriness").unwrap().name = "Old Blurriness".into();
                let d = g.get_mut("dimensions").unwrap();
                let ParamUi::Popup { options } = &d.ui else { panic!("dimensions is a popup") };
                d.ui = ParamUi::Popup { options: options.iter().rev().cloned().collect() };
                d.value = KV::Enum(0);
            }
            for (e, old_id, new_id) in PARAM_ID_ALIASES {
                if g.match_id == *e {
                    let mut p = take(g, new_id).unwrap_or_else(|| panic!("{e}: no {new_id}"));
                    // Put it back at its old path (which may be inside a group).
                    let (path, leaf) = old_id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, old_id));
                    p.match_id = leaf.to_string();
                    let mut at: &mut PropGroup = g;
                    for seg in path.into_iter().flat_map(|p| p.split('/')) {
                        if at.sub(seg).is_none() {
                            at.children.push(PropGroup::new(1_000_000 + at.children.len() as u64, seg, seg).into());
                        }
                        at = at.sub_mut(seg).unwrap();
                    }
                    at.children.push(p.into());
                }
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("ec-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.to_json()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let l = s2.project.comp(cid).unwrap().layer(LayerId(a)).unwrap().clone();
    let fx = l.effects().unwrap();
    let spec = effectcraft_effects::find("ec.blur.gaussian").unwrap();
    let sp = |id: &str| spec.params.iter().find(|p| p.id == id).unwrap();
    let blur = fx.groups().find(|g| g.match_id == "ec.blur.gaussian").unwrap();
    assert_eq!(blur.get("blurriness").unwrap().name, sp("blurriness").name);
    assert!(blur.get("repeatEdge").is_some(), "missing parameter added");
    let dims = blur.get("dimensions").unwrap();
    let ParamUi::Popup { options } = &sp("dimensions").ui else { panic!() };
    assert_eq!(dims.value, KV::Enum(options.len() as u32 - 1), "popup value remapped by label");
    assert_eq!(dims.ui, sp("dimensions").ui);
    for (e, old_id, new_id) in PARAM_ID_ALIASES {
        let g = fx.groups().find(|g| g.match_id == *e).unwrap();
        let at = |id: &str| {
            let (path, leaf) = id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, id));
            let mut node = Some(g);
            for seg in path.into_iter().flat_map(|p| p.split('/')) {
                node = node.and_then(|n| n.sub(seg));
            }
            node.and_then(|n| n.get(leaf)).is_some()
        };
        assert!(!at(old_id), "{e}: {old_id} still present");
        let (path, leaf) = new_id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, new_id));
        let mut node = Some(g);
        for seg in path.into_iter().flat_map(|p| p.split('/')) {
            node = node.and_then(|n| n.sub(seg));
        }
        assert!(node.and_then(|n| n.get(leaf)).is_some(), "{e}: {old_id} not moved to {new_id}");
    }
    // The upgraded project renders.
    let _ = s2.render(cid, s2.time(), Default::default());
}

/// A project saved before schema 2 (#227): effect points on a shape layer were measured from
/// its origin. Opening converts them to effect space so points that were set keep their place
/// in the comp; points at their default become the layer centre; layers with a source
/// rectangle, adjustment layers (their effects see the comp below as it is) and expression
/// controls keep their values. Saving writes schema 2, and opening that again changes nothing.
#[test]
fn opening_a_schema_1_project_moves_shape_layer_effect_points_into_effect_space() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 200, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let shape = s.execute("layer.newShape", json!({"kind": "rect", "size": [180, 80]})).unwrap()["layer"].as_u64().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#0000ff", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let point = |s: &Session, lid: u64, fx: u64, id: &str| -> KV {
        let l = layer(s, lid);
        let g = l.effects().unwrap().groups().find(|g| g.uid == fx).unwrap();
        g.props().find(|p| p.match_id == id || (id.is_empty() && p.ui == effectcraft_project::ParamUi::Point)).unwrap().value.clone()
    };
    let set = |s: &mut Session, lid: u64, fx: u64, id: &str, v: [f64; 2]| {
        let l = layer(s, lid);
        let g = l.effects().unwrap().groups().find(|g| g.uid == fx).unwrap();
        let uid = g.props().find(|p| p.match_id == id || (id.is_empty() && p.ui == effectcraft_project::ParamUi::Point)).unwrap().uid;
        s.execute("prop.set", json!({"layer": lid, "prop": uid, "value": v})).unwrap();
    };
    let twirl = apply(&mut s, shape, "Twirl");
    set(&mut s, shape, twirl, "center", [-30.0, -10.0]);
    let bulge = apply(&mut s, shape, "Bulge");
    let control = apply(&mut s, shape, "Point Control");
    set(&mut s, shape, control, "", [5.0, 5.0]);
    let on_solid = apply(&mut s, solid, "Twirl");
    set(&mut s, solid, on_solid, "center", [30.0, 20.0]);
    let adjust = s.execute("layer.newShape", json!({"kind": "rect", "size": [50, 50]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [adjust], "switch": "adjustment", "value": true})).unwrap();
    let on_adjust = apply(&mut s, adjust, "Twirl");
    set(&mut s, adjust, on_adjust, "center", [40.0, 30.0]);
    assert_eq!(point(&s, shape, bulge, "center"), KV::Vec2([100.0, 50.0]), "Bulge's default centre");

    let old = s.project.to_file_json().unwrap().replacen("\"schema\": 2", "\"schema\": 1", 1);
    assert!(old.contains("\"schema\": 1"));
    let dir = std::env::temp_dir().join(format!("ec-effect-space-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(s2.project.schema, effectcraft_project::SCHEMA_VERSION);
    let check = |s2: &Session| {
        assert_eq!(point(s2, shape, twirl, "center"), KV::Vec2([70.0, 40.0]), "a set point keeps its place in the comp");
        assert_eq!(point(s2, shape, bulge, "center"), KV::Vec2([100.0, 50.0]), "a default point becomes the layer centre");
        assert_eq!(point(s2, shape, control, ""), KV::Vec2([5.0, 5.0]), "expression controls keep their values");
        assert_eq!(point(s2, solid, on_solid, "center"), KV::Vec2([30.0, 20.0]), "a solid's effect space is its layer space");
        assert_eq!(point(s2, adjust, on_adjust, "center"), KV::Vec2([40.0, 30.0]), "an adjustment layer's effects see the comp");
    };
    check(&s2);
    // Saved as schema 2 and opened again: nothing moves twice.
    s2.execute("file.save", json!({"path": path})).unwrap();
    let mut s3 = Session::default();
    s3.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    check(&s3);
}

/// Remove the per-effect version to model a file written by the released Gamma scale.
fn unversioned_cineon(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(fields) => {
            fields.remove("cineon_gamma_units");
            fields.remove("cineon_gamma_map");
            fields.remove("legacy_gamma_targets");
            if fields.get("effect").and_then(serde_json::Value::as_str) == Some("ec.utility.cineon") {
                fields.remove("version");
            }
            for child in fields.values_mut() {
                unversioned_cineon(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                unversioned_cineon(child);
            }
        }
        _ => {}
    }
}

/// Cineon Converter's original Log to Linear: the linear value raised to `1.7 / gamma`
/// (no Highlight Rolloff).
fn cineon_on_legacy_scale(v: f32, gamma: f32) -> f32 {
    let off = 10f32.powf((95.0 - 685.0) * 0.002 / 0.6);
    ((10f32.powf((v * 1023.0 - 685.0) * 0.002 / 0.6) - off) / (1.0 - off)).max(0.0).powf(1.7 / gamma)
}

/// A project saved before effect version 1 holds Cineon Converter's Gamma on the old scale, where 1.7
/// gave linear output. Opening it divides the Gamma, its keyframes and their speeds by 1.7, so
/// the frames render as they did; saving and opening again converts nothing a second time.
#[test]
fn opening_a_schema_2_project_keeps_what_cineon_converter_renders() {
    let mut s = Session::default();
    // 32 bpc, so frames are not rounded to 8-bit steps.
    s.execute("file.projectSettings", json!({"bitDepth": 32})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let animated = s.execute("layer.newSolid", json!({"color": [0.4, 0.5, 0.6], "width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let input = s.render(cid, Tick::ZERO, Default::default()).get(4, 4);
    let fx = apply(&mut s, animated, "Cineon Converter");
    s.execute("prop.set", json!({"layer": animated, "path": "effects/#1/highlightRolloff", "value": 0.0})).unwrap();
    // Old-scale values: 2.2 at 0 s and 3.4 at 1 s.
    animate_effect(&mut s, animated, fx, "gamma", &[(0.0, 2.2), (1.0, 3.4)]);
    let still = s.execute("layer.newSolid", json!({"color": [0.4, 0.5, 0.6], "width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    let still_fx = apply(&mut s, still, "Cineon Converter");
    s.execute("prop.set", json!({"layer": still, "path": "effects/#1/gamma", "value": 2.2})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [still], "switch": "video", "value": false})).unwrap();

    let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    old["schema"] = json!(2);
    unversioned_cineon(&mut old);
    let dir = std::env::temp_dir().join(format!("ec-cineon-gamma-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.to_string()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(s2.project.schema, effectcraft_project::SCHEMA_VERSION);
    let check = |s2: &Session| {
        let gamma = |lid: u64, uid: u64| effect_group(s2, lid, uid).prop("gamma").unwrap().clone();
        let g = gamma(animated, fx);
        let keys: Vec<f64> = g.keys.iter().map(|k| k.value.as_f64()).collect();
        assert!((keys[0] - 2.2 / 1.7).abs() < 1e-9 && (keys[1] - 3.4 / 1.7).abs() < 1e-9, "{keys:?}");
        assert!((gamma(still, still_fx).value.as_f64() - 2.2 / 1.7).abs() < 1e-9, "a static Gamma is converted too");
        for (t, old_gamma) in [(0.0, 2.2), (0.5, 2.8), (1.0, 3.4)] {
            let got = s2.render(cid, Tick::from_seconds_f64(t), Default::default()).get(4, 4);
            for c in 0..3 {
                let want = cineon_on_legacy_scale(input[c], old_gamma);
                assert!((got[c] - want).abs() < 1e-4, "{t} s, channel {c}: {} rendered, {want} before", got[c]);
            }
        }
    };
    check(&s2);
    s2.execute("file.save", json!({"path": path})).unwrap();
    let mut s3 = Session::default();
    s3.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    check(&s3);
}

/// A Cineon Converter saved before effect version 1 without a Gamma property gets today's default
/// (1, linear output) when opened, not that default divided by 1.7.
#[test]
fn opening_a_schema_2_cineon_converter_without_gamma_gives_linear_output() {
    let mut s = Session::default();
    s.execute("file.projectSettings", json!({"bitDepth": 32})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": [0.4, 0.5, 0.6], "width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let input = s.render(cid, Tick::ZERO, Default::default()).get(4, 4);
    let fx = apply(&mut s, l, "Cineon Converter");
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/highlightRolloff", "value": 0.0})).unwrap();

    fn drop_gamma(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(o) => {
                if let Some(serde_json::Value::Array(children)) = o.get_mut("children") {
                    children.retain(|c| !(c["node"] == json!("Prop") && c["match"] == json!("gamma")));
                }
                o.values_mut().for_each(drop_gamma);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(drop_gamma),
            _ => {}
        }
    }
    let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    old["schema"] = json!(2);
    unversioned_cineon(&mut old);
    drop_gamma(&mut old);
    assert!(!old.to_string().contains("\"match\":\"gamma\""));
    let dir = std::env::temp_dir().join(format!("ec-cineon-no-gamma-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.to_string()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let gamma = effect_group(&s2, l, fx).prop("gamma").unwrap().value.as_f64();
    assert!((gamma - 1.0).abs() < 1e-9, "the missing Gamma gets today's default: {gamma}");
    let got = s2.render(cid, Tick::ZERO, Default::default()).get(4, 4);
    for c in 0..3 {
        let want = cineon_on_legacy_scale(input[c], 1.7);
        assert!((got[c] - want).abs() < 1e-4, "channel {c}: {} rendered, {want} linear", got[c]);
    }
}

/// A precomp instance that overrides an exposed Cineon Converter Gamma in a project saved
/// before effect version 1 has its Essential Properties value converted too, so it renders as before.
#[test]
fn opening_a_schema_2_project_converts_an_overridden_cineon_gamma() {
    let mut s = Session::default();
    s.execute("file.projectSettings", json!({"bitDepth": 32})).unwrap();
    s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": [0.4, 0.5, 0.6], "width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    let input = s.render(inner, Tick::ZERO, Default::default()).get(4, 4);
    apply(&mut s, solid, "Cineon Converter");
    s.execute("prop.set", json!({"layer": solid, "path": "effects/#1/highlightRolloff", "value": 0.0})).unwrap();
    s.execute("prop.set", json!({"layer": solid, "path": "effects/#1/gamma", "value": 1.7})).unwrap();
    let control = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
    // This visible Gamma+Opacity control keeps its public number and both rendered targets.
    let mixed_layer = s.execute("layer.newSolid", json!({"color": [0.4, 0.5, 0.6], "width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    apply(&mut s, mixed_layer, "Cineon Converter");
    let mixed = s.execute("essential.addProperty", json!({"layer": mixed_layer, "path": "effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("essential.linkProperty", json!({"control": mixed, "layer": mixed_layer, "path": "transform/opacity"})).unwrap();
    s.execute("prop.set", json!({"layer": mixed_layer, "path": "effects/#1/highlightRolloff", "value": 0.0})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let inst = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    // Old-scale value: 2.2.
    s.execute("essential.set", json!({"layer": inst, "control": control, "value": 2.2})).unwrap();
    s.execute("essential.set", json!({"layer": inst, "control": mixed, "value": 2.2})).unwrap();

    let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    old["schema"] = json!(2);
    unversioned_cineon(&mut old);
    let dir = std::env::temp_dir().join(format!("ec-cineon-essential-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.to_string()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let controls = s2.execute("essential.instance", json!({"comp": main.0, "layer": inst})).unwrap()["controls"].clone();
    let find = |id: u64| controls.as_array().unwrap().iter().find(|c| c["control"] == id).unwrap().clone();
    let c = find(control);
    assert_eq!(c["overridden"], json!(true), "{c}");
    assert!((c["value"].as_f64().unwrap() - 2.2 / 1.7).abs() < 1e-9, "{c}");
    let m = find(mixed);
    assert!((m["value"].as_f64().unwrap() - 2.2).abs() < 1e-9, "a control that also drives Opacity keeps its value: {m}");
    let got = s2.render(main, Tick::ZERO, Default::default()).get(4, 4);
    for ch in 0..3 {
        let want = cineon_on_legacy_scale(input[ch], 2.2);
        assert!((got[ch] - want).abs() < 1e-4, "channel {ch}: {} rendered, {want} before", got[ch]);
    }
    s2.execute("essential.linkProperty", json!({"comp":inner.0,"control":control,"layer":solid,"path":"transform/opacity"})).unwrap();
    let p = s2.project.comp(main).unwrap().layer(LayerId(inst)).unwrap().props.prop(&format!("essential/eg{control}")).unwrap();
    assert!(p.cineon_gamma_units, "adding a mixed link must retain converted public units for old expression readers");
}

/// A Gamma exposed again by a middle comp, from its precomp layer's Essential Properties, is
/// converted in the outer instance that overrides it too.
#[test]
fn opening_a_schema_2_project_converts_a_cineon_gamma_exposed_through_a_nested_precomp() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": [0.4, 0.5, 0.6], "width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    apply(&mut s, solid, "Cineon Converter");
    s.execute("prop.set", json!({"layer": solid, "path": "effects/#1/gamma", "value": 1.7})).unwrap();
    let inner_control = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Middle", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let middle = s.active_comp_id().unwrap();
    let mid_layer = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    let path = format!("essential/{}", effectcraft_project::essential::match_id(inner_control));
    let control = s.execute("essential.addProperty", json!({"layer": mid_layer, "path": path})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let inst = s.execute("layer.addItem", json!({"item": middle.0})).unwrap()["layer"].as_u64().unwrap();
    // Old-scale value: 2.2.
    s.execute("essential.set", json!({"layer": inst, "control": control, "value": 2.2})).unwrap();

    let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    old["schema"] = json!(2);
    unversioned_cineon(&mut old);
    let dir = std::env::temp_dir().join(format!("ec-cineon-nested-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.to_string()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let c = s2.execute("essential.instance", json!({"comp": main.0, "layer": inst})).unwrap()["controls"][0].clone();
    assert!((c["value"].as_f64().unwrap() - 2.2 / 1.7).abs() < 1e-9, "{c}");
}

/// An animation preset saved before Cineon Converter's Gamma scale changed (format version 1)
/// is converted when applied, for a saved effect and for a saved Gamma property alone; a preset
/// saved now is applied as it is.
#[test]
fn version_1_presets_convert_cineon_converter_gamma() {
    let (mut s, a, _) = comp_with_two_solids();
    let fx = apply(&mut s, a, "Cineon Converter");
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/gamma", "value": 2.2})).unwrap();
    let gamma = |s: &Session, n: usize| layer(s, a).effects().unwrap().groups().nth(n).unwrap().get("gamma").unwrap().value.as_f64();
    let tag = "effectcraftAnimationPreset";
    let dir = std::env::temp_dir().join(format!("ec-cineon-preset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let effect_path = dir.join("effect.ecpreset").to_string_lossy().to_string();
    select_effect(&mut s, a, fx);
    s.execute("anim.savePreset", json!({"path": effect_path})).unwrap();
    let prop_path = dir.join("gamma.ecpreset").to_string_lossy().to_string();
    s.execute("prop.select", json!({"layer": a, "path": "effects/#1/gamma"})).unwrap();
    s.execute("anim.savePreset", json!({"path": prop_path})).unwrap();
    let read = |p: &str| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(p).unwrap()).unwrap();
    let (effect_preset, prop_preset) = (read(&effect_path), read(&prop_path));
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(effect_preset[tag], json!(2));

    s.execute("anim.applyPreset", json!({"preset": effect_preset.clone(), "layers": [a]})).unwrap();
    assert!((gamma(&s, 1) - 2.2).abs() < 1e-9, "a current preset is applied as saved");
    let mut v1 = effect_preset;
    v1[tag] = json!(1);
    unversioned_cineon(&mut v1);
    s.execute("anim.applyPreset", json!({"preset": v1, "layers": [a]})).unwrap();
    assert!((gamma(&s, 2) - 2.2 / 1.7).abs() < 1e-9, "a version 1 effect is converted");

    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/gamma", "value": 1.0})).unwrap();
    let mut v1 = prop_preset;
    v1[tag] = json!(1);
    unversioned_cineon(&mut v1);
    let r = s.execute("anim.applyPreset", json!({"preset": v1, "layers": [a]})).unwrap();
    assert_eq!(r["skipped"], json!([]), "{r}");
    assert!((gamma(&s, 0) - 2.2 / 1.7).abs() < 1e-9, "a version 1 Gamma property is converted");
}

#[test]
fn version_1_presets_read_cineon_gamma_in_other_expressions_as_saved() {
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "Cineon Converter");
    let text = "effect('Cineon Converter')('Gamma') * 100";
    s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "expression": text})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "transform/rotation"})).unwrap();
    let file = std::env::temp_dir().join(format!("ec-cineon-reader-{}.ecpreset", std::process::id()));
    s.execute("anim.savePreset", json!({"path": file})).unwrap();
    let preset: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    std::fs::remove_file(file).unwrap();
    let expression = |s: &Session| layer(s, a).props.prop("transform/rotation").and_then(|p| p.expr.clone()).map(|e| e.text).unwrap_or_default();
    for version in [2, 1] {
        let mut preset = preset.clone();
        preset["effectcraftAnimationPreset"] = json!(version);
        s.execute("anim.applyPreset", json!({"preset": preset, "layers": [a]})).unwrap();
        let applied = expression(&s);
        assert_eq!(applied, text, "version {version}: original editable text");
        let metadata = layer(&s, a).props.prop("transform/rotation").unwrap().expr.as_ref().unwrap().compat;
        assert_eq!(metadata.is_some(), version == 1);
        assert!(applied.contains("Gamma"), "{applied}");
    }
}

#[test]
fn presets_convert_exposed_cineon_gamma_for_direct_and_nested_controls() {
    for nested in [false, true] {
        for mixed in [false, true] {
            let mut s = Session::default();
            s.execute("file.projectSettings", json!({"bitDepth":32})).unwrap();
            s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
            let mut source = s.active_comp_id().unwrap();
            let leaf = source;
            let solid = s.execute("layer.newSolid", json!({"color":[0.4,0.5,0.6],"width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
            apply(&mut s, solid, "Cineon Converter");
            s.execute("prop.set", json!({"layer":solid,"path":"effects/#1/highlightRolloff","value":0})).unwrap();
            let mut control = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
            if mixed {
                s.execute("essential.linkProperty", json!({"control": control, "layer": solid, "path": "transform/opacity"})).unwrap();
            }
            if nested {
                control = s.execute("essential.addMirror", json!({"control":control})).unwrap()["control"].as_u64().unwrap();
                s.execute("comp.new", json!({"name": "Middle", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
                let middle = s.active_comp_id().unwrap();
                let instance = s.execute("layer.addItem", json!({"item": source.0})).unwrap()["layer"].as_u64().unwrap();
                let path = format!("essential/{}", effectcraft_project::essential::match_id(control));
                control = s.execute("essential.addProperty", json!({"layer": instance, "path": path})).unwrap()["controls"][0].as_u64().unwrap();
                source = middle;
            }
            s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
            let main = s.active_comp_id().unwrap();
            let instance = s.execute("layer.addItem", json!({"item": source.0})).unwrap()["layer"].as_u64().unwrap();
            let path = format!("essential/{}", effectcraft_project::essential::match_id(control));
            s.execute("essential.set", json!({"layer": instance, "control": control, "value": 2.2})).unwrap();
            s.execute("prop.select", json!({"layer": instance, "path": path})).unwrap();
            let file = std::env::temp_dir().join(format!("ec-cineon-exposed-preset-{}-{nested}-{mixed}.ecpreset", std::process::id()));
            s.execute("anim.savePreset", json!({"path": file})).unwrap();
            let preset: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            std::fs::remove_file(file).unwrap();
            assert_eq!(preset["properties"][0]["path"], path);
            for version in [2, 1] {
                let mut preset = preset.clone();
                preset["effectcraftAnimationPreset"] = json!(version);
                s.execute("essential.set", json!({"layer": instance, "control": control, "value": 0.5})).unwrap();
                let result = s.execute("anim.applyPreset", json!({"preset": preset, "layers": [instance]})).unwrap();
                assert_eq!(result["skipped"], json!([]));
                let value = s.execute("prop.get", json!({"layer": instance, "path": path})).unwrap()["value"].as_f64().unwrap();
                let expected = if version == 1 && !mixed { 2.2 / 1.7 } else { 2.2 };
                assert!((value - expected).abs() < 1e-9, "nested={nested}, mixed={mixed}, version={version}: {value} != {expected}");
                // Render the visible nested mixed control against explicit independent leaf values.
                let mut reference = Session { project: s.project.clone(), ..Session::default() };
                let leaf_layer = std::sync::Arc::make_mut(&mut reference.project).comp_mut(leaf).unwrap().layer_mut(LayerId(solid)).unwrap();
                leaf_layer.props.prop_mut("effects/#1/gamma").unwrap().value = KV::Scalar(if version == 1 { 2.2 / 1.7 } else { 2.2 });
                leaf_layer.props.prop_mut("transform/opacity").unwrap().value = KV::Scalar(if mixed { 2.2 } else { 100.0 });
                let want = reference.render(leaf, Tick::ZERO, Default::default()).get(4, 4);
                let got = s.render(main, Tick::ZERO, Default::default()).get(4, 4);
                for ch in 0..4 {
                    assert!((got[ch] - want[ch]).abs() < 1e-5, "nested={nested}, mixed={mixed}, version={version}: {got:?} != {want:?}");
                }
            }
            // Applying the old preset is local to that instance; a fresh sibling and the
            // shared definition retain current semantics.
            let sibling = s.execute("layer.addItem", json!({"item":source.0})).unwrap()["layer"].as_u64().unwrap();
            let sibling_prop = s.project.comp(main).unwrap().layer(LayerId(sibling)).unwrap().props.prop(&path).unwrap();
            assert!(sibling_prop.cineon_gamma_map.is_none());
            assert!(s.project.comp(source).unwrap().essential.as_ref().unwrap().resolve(control).unwrap().legacy_gamma_targets.is_empty());
        }
    }
}

#[test]
fn version_1_presets_leave_an_exposed_gamma_expression_they_do_not_set() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let source = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    apply(&mut s, solid, "Cineon Converter");
    let control = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let instance = s.execute("layer.addItem", json!({"item": source.0})).unwrap()["layer"].as_u64().unwrap();
    let path = format!("essential/{}", effectcraft_project::essential::match_id(control));
    s.execute("essential.set", json!({"layer": instance, "control": control, "value": 2.2})).unwrap();
    s.execute("prop.select", json!({"layer": instance, "path": path})).unwrap();
    let file = std::env::temp_dir().join(format!("ec-cineon-kept-expression-{}.ecpreset", std::process::id()));
    s.execute("anim.savePreset", json!({"path": file})).unwrap();
    let mut preset: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    std::fs::remove_file(file).unwrap();
    preset["effectcraftAnimationPreset"] = json!(1);
    s.execute("prop.setExpression", json!({"layer": instance, "path": path, "expression": "1"})).unwrap();
    s.execute("anim.applyPreset", json!({"preset": preset, "layers": [instance]})).unwrap();
    let kept = layer(&s, instance).props.prop(&path).and_then(|p| p.expr.clone()).map(|e| e.text);
    assert_eq!(kept.as_deref(), Some("1"));
}

#[test]
fn cineon_migration_preserves_text_disabled_expressions_and_unrelated_wiggle() {
    let (mut s, a, b) = comp_with_two_solids();
    apply(&mut s, a, "Cineon Converter");
    s.execute("prop.set", json!({"layer":a,"path":"effects/#1/gamma","value":1.7})).unwrap();
    let source = "  // retained source\n2.2";
    s.execute("prop.setExpression", json!({"layer":a,"path":"effects/#1/gamma","expression":source,"enabled":false})).unwrap();
    s.execute("prop.setExpression", json!({"layer":b,"path":"transform/rotation","expression":"wiggle(2, 5)"})).unwrap();
    let untouched = layer(&s, b).props.prop("transform/rotation").unwrap().expr.clone();
    let reader = "thisComp.layer(2).effect('Cineon Converter')('Gamma') * 100";
    s.execute("prop.setExpression", json!({"layer":b,"path":"transform/opacity","expression":reader,"enabled":false})).unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    unversioned_cineon(&mut old);
    s.project = effectcraft_project::Project::from_json(&old.to_string()).unwrap().into();
    crate::session::upgrade_effects(std::sync::Arc::make_mut(&mut s.project));
    assert_eq!(s.project.schema, 2);
    let expr = layer(&s, a).props.prop("effects/#1/gamma").unwrap().expr.clone().unwrap();
    assert_eq!(expr.text, source);
    assert!(!expr.enabled);
    assert_eq!(expr.compat, Some(effectcraft_project::ExpressionCompat::CineonGamma));
    assert_eq!(layer(&s, b).props.prop("transform/rotation").unwrap().expr, untouched);
    assert_eq!(
        layer(&s, b).props.prop("transform/opacity").unwrap().expr.as_ref().unwrap().compat,
        Some(effectcraft_project::ExpressionCompat::CineonGammaReader)
    );
    s.execute("prop.setExpression", json!({"layer":b,"path":"transform/opacity","enabled":true})).unwrap();
    s.execute("prop.setExpression", json!({"layer":b,"path":"transform/opacity","expression":reader})).unwrap();
    assert_eq!(
        layer(&s, b).props.prop("transform/opacity").unwrap().expr.as_ref().unwrap().compat,
        Some(effectcraft_project::ExpressionCompat::CineonGammaReader)
    );
    s.execute("prop.setExpression", json!({"layer":a,"path":"effects/#1/gamma","enabled":true})).unwrap();
    assert!(layer(&s, a).props.prop("effects/#1/gamma").unwrap().expr.as_ref().unwrap().compat.is_some());
    s.execute("prop.setExpression", json!({"layer":a,"path":"effects/#1/gamma","expression":source})).unwrap();
    assert!(layer(&s, a).props.prop("effects/#1/gamma").unwrap().expr.as_ref().unwrap().compat.is_some(), "same text is not an edit");
    s.execute("prop.setExpression", json!({"layer":a,"path":"effects/#1/gamma","expression":"1"})).unwrap();
    assert_eq!(layer(&s, a).props.prop("effects/#1/gamma").unwrap().expr.as_ref().unwrap().compat, None);
    let saved = s.project.to_file_json().unwrap();
    let mut reopened = effectcraft_project::Project::from_json(&saved).unwrap();
    crate::session::upgrade_effects(&mut reopened);
    assert_eq!(&reopened, &*s.project, "no second conversion");
}

#[test]
fn prototype_schema_three_cineon_opens_without_another_gamma_conversion() {
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "Cineon Converter");
    s.execute("prop.set", json!({"layer":a,"path":"effects/#1/gamma","value":1.3})).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    unversioned_cineon(&mut json);
    json["schema"] = json!(3);
    let mut project = effectcraft_project::Project::from_json(&json.to_string()).unwrap();
    crate::session::upgrade_effects(&mut project);
    assert_eq!(project.schema, 2);
    let l = project.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap();
    assert_eq!(l.props.prop("effects/#1/gamma").unwrap().value.as_f64(), 1.3);
}

#[test]
fn cineon_saved_gamma_at_the_old_guard_keeps_a_visible_near_white_render() {
    let mut s = Session::default();
    s.execute("file.projectSettings", json!({"bitDepth":32})).unwrap();
    s.execute("comp.new", json!({"name":"Boundary","width":8,"height":8,"frameRate":30,"duration":1})).unwrap();
    let sample = 684.5f32 / 1023.0;
    let a = s.execute("layer.newSolid", json!({"color":[sample,sample,sample],"width":8,"height":8})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let input = s.render(cid, Tick::ZERO, Default::default()).get(4, 4)[0];
    apply(&mut s, a, "Cineon Converter");
    s.execute("prop.set", json!({"layer":a,"path":"effects/#1/highlightRolloff","value":0})).unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    unversioned_cineon(&mut old);
    let mut project = effectcraft_project::Project::from_json(&old.to_string()).unwrap();
    project.comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.prop_mut("effects/#1/gamma").unwrap().value = KV::Scalar(0.01);
    crate::session::upgrade_effects(&mut project);
    s.project = project.into();
    let expected = cineon_on_legacy_scale(input, 0.01);
    assert!(expected > 0.1, "the boundary sample must not underflow");
    let got = s.render(cid, Tick::ZERO, Default::default()).get(4, 4)[0];
    assert!((got - expected).abs() < 1e-4, "{got} vs {expected}");
}

/// #410: `effect.warning` reports what an OCIO effect can't render as asked (Effect Controls
/// shows it under the effect's name): a transform of its custom config that passes colours
/// through, a colour space the config lacks or a config it can't read; nothing for the
/// built-in config.
#[test]
fn ocio_effect_warns_about_unsupported_config_transforms() {
    let (mut s, a, _) = comp_with_two_solids();
    let fx = apply(&mut s, a, "OCIO Color Space Transform");
    let warning = |s: &mut Session| s.execute("effect.warning", json!({"layer": a, "effect": fx})).unwrap()["warning"].clone();
    assert!(warning(&mut s).is_null(), "built-in config");
    let set = |s: &mut Session, param: &str, value: serde_json::Value| {
        let uid = layer(s, a).effects().unwrap().groups().next().unwrap().get(param).unwrap().uid;
        s.execute("prop.set", json!({"layer": a, "prop": uid, "value": value})).unwrap();
    };
    let config = "ocio_profile_version: 2\ncolorspaces:\n  - !<ColorSpace>\n    name: lin\n  - !<ColorSpace>\n    name: graded\n    from_scene_reference: !<GradingToneTransform> {}\n";
    set(&mut s, "config", json!(1));
    set(&mut s, "configFile", json!(config));
    set(&mut s, "sourceName", json!("lin"));
    set(&mut s, "destinationName", json!("graded"));
    let w = warning(&mut s);
    assert!(w.as_str().is_some_and(|w| w.contains("GradingToneTransform")), "{w}");
    set(&mut s, "destinationName", json!("lin"));
    assert!(warning(&mut s).is_null());
    set(&mut s, "destinationName", json!("nowhere"));
    assert!(warning(&mut s).as_str().is_some_and(|w| w.contains("no color space `nowhere`")));
    set(&mut s, "configFile", json!("/no/such/config.ocio"));
    assert!(warning(&mut s).as_str().is_some_and(|w| w.starts_with("Can't read the OCIO config")));
}

#[test]
fn effect_colour_labels_set_clear_undo_and_round_trip() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let label = |s: &Session| layer(s, a).effects().unwrap().groups().next().unwrap().label;
    assert_eq!(label(&s), None);
    s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": "Blue"})).unwrap();
    assert_eq!(label(&s), Some(effectcraft_color::Label::Blue));
    // Duplicates keep the label.
    s.execute("effect.duplicate", json!({"layer": a, "effect": blur})).unwrap();
    assert_eq!(layer(&s, a).effects().unwrap().groups().filter(|g| g.label.is_some()).count(), 2);
    s.undo();
    // It saves with the project and survives a reload.
    let json = s.project.to_file_json().unwrap();
    let back = effectcraft_project::Project::from_json(&json).unwrap();
    let l = back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap().clone();
    assert_eq!(l.effects().unwrap().groups().next().unwrap().label, Some(effectcraft_color::Label::Blue));
    // None (or index 0) clears it; the change undoes.
    s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": 0})).unwrap();
    assert_eq!(label(&s), None);
    s.undo();
    assert_eq!(label(&s), Some(effectcraft_color::Label::Blue));
    assert!(s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": "Chartreuse"})).is_err());
    assert!(s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": 99})).is_err());
    // A project saved before labels existed has none.
    let old = json.replace("\"label\":\"Blue\"", "\"x\":0").replace("\"label\": \"Blue\"", "\"x\": 0");
    assert_ne!(old, json, "the replace removed the effect's label");
    let old = effectcraft_project::Project::from_json(&old).unwrap();
    let l = old.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap().clone();
    assert_eq!(l.effects().unwrap().groups().next().unwrap().label, None);
}

/// #1746: a preset saved from an effect property (`effects/ec.control.slider/slider`, `#2` for the second
/// instance) applies back to the same layer instead of landing in `skipped`.
#[test]
fn preset_round_trips_effect_properties_by_match_name() {
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "Slider Control");
    apply(&mut s, a, "Slider Control");
    let slider = |s: &Session, n: usize| match layer(s, a).effects().unwrap().groups().nth(n).unwrap().get("slider").unwrap().value {
        KV::Scalar(v) => v,
        ref other => panic!("not a scalar: {other:?}"),
    };
    for (n, value) in [(1, 30.0), (2, 70.0)] {
        s.execute("prop.set", json!({"layer": a, "path": format!("effects/#{n}/slider"), "value": value})).unwrap();
    }
    for n in [1, 2] {
        let path = std::env::temp_dir().join(format!("ec-1746-{}-{n}.ecpreset", std::process::id())).to_string_lossy().to_string();
        s.execute("prop.select", json!({"layer": a, "path": format!("effects/#{n}/slider")})).unwrap();
        s.execute("anim.savePreset", json!({"path": path})).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        let want = if n == 1 { "effects/ec.control.slider/slider" } else { "effects/ec.control.slider#2/slider" };
        assert!(saved.contains(want), "{saved}");
        s.execute("prop.set", json!({"layer": a, "path": format!("effects/#{n}/slider"), "value": 0.0})).unwrap();
        let r = s.execute("anim.applyPreset", json!({"path": path, "layers": [a]})).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(r["applied"], 1, "{r}");
        assert_eq!(r["skipped"], json!([]), "{r}");
        assert_eq!(slider(&s, n - 1), if n == 1 { 30.0 } else { 70.0 });
    }
}

/// #2064: `prop.addKey` accepts the `effects/<match name>[#n]/<property>` path that
/// `essential.list` reports.
#[test]
fn add_key_resolves_effect_match_name_paths() {
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "Slider Control");
    apply(&mut s, a, "Slider Control");
    let slider = |s: &Session, n: usize| match layer(s, a).effects().unwrap().groups().nth(n).unwrap().get("slider").unwrap().value {
        KV::Scalar(v) => v,
        ref other => panic!("not a scalar: {other:?}"),
    };
    for (path, n, value) in [("effects/ec.control.slider/slider", 0, 30.0), ("effects/ec.control.slider#2/slider", 1, 70.0)] {
        let reply = s.execute("prop.addKey", json!({"layer": a, "path": path, "time": 0, "value": value})).unwrap();
        assert_eq!(reply, json!(1), "{path}");
        let got = s.execute("prop.get", json!({"layer": a, "path": path, "time": 0})).unwrap();
        assert_eq!(got["value"], json!(value), "{path}");
        assert_eq!(got["keys"].as_array().unwrap().len(), 1, "{path}");
        assert_eq!(slider(&s, n), 0.0, "the static value stays put; the key carries {value}");
    }
}

/// #1746: Edit ▸ Copy of an effect property writes the same `effects/ec.blur.gaussian/blurriness` path,
/// and Paste (the value, or Copy Expression Only) sets that property of the other layer's effect.
#[test]
fn copied_effect_property_pastes_onto_the_same_effect() {
    let (mut s, a, b) = comp_with_two_solids();
    apply(&mut s, a, "Gaussian Blur");
    apply(&mut s, b, "Gaussian Blur");
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/blurriness", "value": 12.5})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "effects/#1/blurriness"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    let r = s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(r["values"], 1, "{r}");
    assert_eq!(layer(&s, b).props.prop("effects/#1/blurriness").unwrap().value, KV::Scalar(12.5));
    s.execute("prop.setExpression", json!({"layer": a, "path": "effects/#1/blurriness", "expression": "time * 10"})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "effects/#1/blurriness"})).unwrap();
    s.execute("edit.copyExpressionOnly", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    let r = s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(r["expressions"], 1, "{r}");
    assert_eq!(layer(&s, b).props.prop("effects/#1/blurriness").unwrap().expr.as_ref().unwrap().text, "time * 10");
}

#[test]
fn legacy_mixed_cineon_controls_preserve_visible_output_and_inverse_source_edits() {
    use effectcraft_project::{LayerId, essential};
    for gamma_primary in [false, true] {
        let mut s = Session::default();
        s.execute("file.projectSettings", json!({"bitDepth":32})).unwrap();
        s.execute("comp.new", json!({"name":"Mixed", "width":8,"height":8,"duration":2})).unwrap();
        let inner = s.active_comp_id().unwrap();
        let solid = s.execute("layer.newSolid", json!({"color":[0.4,0.5,0.6],"width":8,"height":8})).unwrap()["layer"].as_u64().unwrap();
        apply(&mut s, solid, "Cineon Converter");
        s.execute("prop.set", json!({"layer":solid,"path":"effects/#1/highlightRolloff","value":0})).unwrap();
        let (primary, linked) = if gamma_primary { ("effects/#1/gamma", "transform/opacity") } else { ("transform/opacity", "effects/#1/gamma") };
        let control = s.execute("essential.addProperty", json!({"layer":solid,"path":primary})).unwrap()["controls"][0].as_u64().unwrap();
        s.execute("essential.linkProperty", json!({"control":control,"layer":solid,"path":linked})).unwrap();
        s.execute("prop.set", json!({"layer":solid,"path":primary,"value":2.2})).unwrap();
        s.execute("comp.new", json!({"name":"Main","width":8,"height":8,"duration":2})).unwrap();
        let main = s.active_comp_id().unwrap();
        let instance = s.execute("layer.addItem", json!({"item":inner.0})).unwrap()["layer"].as_u64().unwrap();
        s.execute("essential.set", json!({"layer":instance,"control":control,"value":3.4})).unwrap();
        let mut old: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
        unversioned_cineon(&mut old);
        // Independent current-unit render: assign the two target values explicitly.
        let explicit = essential::with_overrides(&s.project, inner, &[essential::Override { control, value: KV::Scalar(3.4), gamma_map: None }]).unwrap();
        let mut expected = explicit;
        expected.comp_mut(inner).unwrap().layer_mut(LayerId(solid)).unwrap().props.prop_mut("effects/#1/gamma").unwrap().value = KV::Scalar(2.0);
        let reference = Session { project: std::sync::Arc::new(expected), ..Session::default() };
        let expected_pixel = reference.render(inner, Tick::ZERO, Default::default()).get(4, 4);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.ecproj");
        std::fs::write(&path, old.to_string()).unwrap();
        let mut restored = Session::default();
        restored.execute("file.open", json!({"path":path})).unwrap();
        let check = |s: &Session| {
            let got = s.render(main, Tick::ZERO, Default::default()).get(4, 4);
            for c in 0..4 {
                assert!((got[c] - expected_pixel[c]).abs() < 1e-5, "primary={gamma_primary}, channel{c}: {got:?} != {expected_pixel:?}");
            }
            let pr = s.project.comp(main).unwrap().layer(LayerId(instance)).unwrap().props.prop(&format!("essential/eg{control}")).unwrap();
            assert_eq!(pr.value, KV::Scalar(3.4));
            assert!(pr.cineon_gamma_map.is_some());
        };
        check(&restored);
        restored.execute("file.save", json!({"path":path})).unwrap();
        let mut reopened = Session::default();
        reopened.execute("file.open", json!({"path":path})).unwrap();
        check(&reopened);
        let mut shifted = (*reopened.project).clone();
        essential::offset_ids(&mut shifted, 100000);
        let imported = Session { project: shifted.into(), ..Session::default() };
        let shifted_pixel = imported.render(effectcraft_project::ItemId(main.0 + 100000), Tick::ZERO, Default::default()).get(4, 4);
        for c in 0..4 {
            assert!((shifted_pixel[c] - expected_pixel[c]).abs() < 1e-5, "remapped bindings: {shifted_pixel:?}");
        }
        // A Gamma source edit is inverted into shared control units, including keys and speeds.
        reopened.execute("comp.open", json!({"comp":inner.0})).unwrap();
        reopened.execute("prop.set", json!({"layer":solid,"path":"effects/#1/gamma","value":2.0})).unwrap();
        let l = reopened.project.comp(inner).unwrap().layer(LayerId(solid)).unwrap();
        assert!((l.props.prop("transform/opacity").unwrap().value.as_f64() - 3.4).abs() < 1e-9);
        let listing = reopened.execute("essential.list", json!({})).unwrap();
        assert!((listing["controls"][0]["value"].as_f64().unwrap() - 3.4).abs() < 1e-9, "{listing}");
        reopened.execute("essential.pushToComp", json!({"comp":main.0,"layer":instance,"control":control})).unwrap();
        let l = reopened.project.comp(inner).unwrap().layer(LayerId(solid)).unwrap();
        assert!((l.props.prop("effects/#1/gamma").unwrap().value.as_f64() - 2.0).abs() < 1e-9);
        assert!((l.props.prop("transform/opacity").unwrap().value.as_f64() - 3.4).abs() < 1e-9);
        reopened
            .edit("Gamma keys", None, |p, _| {
                let gamma = p.comp_mut(inner).unwrap().layer_mut(LayerId(solid)).unwrap().props.prop_mut("effects/#1/gamma").unwrap();
                let mut k = effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(0.5), KV::Scalar(2.0));
                k.in_ease = vec![Ease { speed: 0.4, influence: 0.3 }];
                k.out_ease = vec![Ease { speed: 0.8, influence: 0.6 }];
                gamma.keys = vec![k];
                Ok(())
            })
            .unwrap();
        let keys = &reopened.project.comp(inner).unwrap().layer(LayerId(solid)).unwrap().props.prop("transform/opacity").unwrap().keys;
        assert_eq!(keys[0].time, Tick::from_seconds_f64(0.5));
        assert!((keys[0].value.as_f64() - 3.4).abs() < 1e-9);
        assert!((keys[0].in_ease[0].speed - 0.68).abs() < 1e-9);
        assert!((keys[0].out_ease[0].speed - 1.36).abs() < 1e-9);
        assert_eq!(keys[0].in_ease[0].influence, 0.3);
        if !gamma_primary {
            let following = reopened.execute("layer.addItem", json!({"comp":main.0,"item":inner.0})).unwrap()["layer"].as_u64().unwrap();
            assert!(essential::overridden(reopened.project.comp(main).unwrap().layer(LayerId(following)).unwrap()).is_empty());
            reopened.execute("essential.unlinkProperty", json!({"comp":inner.0,"control":control,"layer":solid,"path":"effects/#1/gamma"})).unwrap();
            assert!(
                essential::overridden(reopened.project.comp(main).unwrap().layer(LayerId(following)).unwrap()).is_empty(),
                "maintenance must not freeze a following instance"
            );
            reopened.execute("essential.linkProperty", json!({"comp":inner.0,"control":control,"layer":solid,"path":"effects/#1/gamma"})).unwrap();
            let ctl = reopened.project.comp(inner).unwrap().essential.as_ref().unwrap().find(control).unwrap();
            assert!(ctl.legacy_gamma_targets.is_empty(), "a new link starts in current units");
            let l = reopened.project.comp(inner).unwrap().layer(LayerId(solid)).unwrap();
            assert_eq!(l.props.prop("effects/#1/gamma").unwrap().value, l.props.prop("transform/opacity").unwrap().value);
        }
    }
}

#[test]
fn mixed_cineon_push_and_nested_bindings_keep_distinct_instance_policies() {
    let mut s = Session::default();
    s.execute("file.projectSettings", json!({"bitDepth":32})).unwrap();
    s.execute("comp.new", json!({"name":"Leaf","width":8,"height":8,"duration":2})).unwrap();
    let leaf = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color":[0.4,0.5,0.6],"width":8,"height":8})).unwrap()["layer"].as_u64().unwrap();
    apply(&mut s, solid, "Cineon Converter");
    s.execute("prop.set", json!({"layer":solid,"path":"effects/#1/highlightRolloff","value":0})).unwrap();
    let control = s.execute("essential.addProperty", json!({"layer":solid,"path":"effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("essential.linkProperty", json!({"control":control,"layer":solid,"path":"transform/opacity"})).unwrap();
    s.execute("comp.new", json!({"name":"Middle","width":16,"height":8,"duration":2})).unwrap();
    let middle = s.active_comp_id().unwrap();
    let left = s.execute("layer.addItem", json!({"item":leaf.0})).unwrap()["layer"].as_u64().unwrap();
    let right = s.execute("layer.addItem", json!({"item":leaf.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer":left,"path":"transform/position","value":[4,4,0]})).unwrap();
    s.execute("prop.set", json!({"layer":right,"path":"transform/position","value":[12,4,0]})).unwrap();
    let path = format!("essential/eg{control}");
    let preset = json!({"effectcraftAnimationPreset":1,"properties":[{"path":path,"value":{"Scalar":3.4},"keys":[]}],"effects":[]});
    // Value's externally tagged representation is generated rather than assumed.
    let mut preset = preset;
    preset["properties"][0]["value"] = serde_json::to_value(KV::Scalar(3.4)).unwrap();
    s.execute("anim.applyPreset", json!({"preset":preset,"layers":[left]})).unwrap();
    s.execute("essential.set", json!({"layer":right,"control":control,"value":1.9})).unwrap();
    let leaf_pixel = |s: &Session, gamma: f64, opacity: f64| {
        let mut reference = Session { project: s.project.clone(), ..Session::default() };
        let l = std::sync::Arc::make_mut(&mut reference.project).comp_mut(leaf).unwrap().layer_mut(LayerId(solid)).unwrap();
        l.props.prop_mut("effects/#1/gamma").unwrap().value = KV::Scalar(gamma);
        l.props.prop_mut("transform/opacity").unwrap().value = KV::Scalar(opacity);
        reference.render(leaf, Tick::ZERO, Default::default()).get(4, 4)
    };
    let compare = |got: [f32; 4], want: [f32; 4]| {
        for ch in 0..4 {
            assert!((got[ch] - want[ch]).abs() < 1e-5, "{got:?} != {want:?}");
        }
    };
    compare(s.render(middle, Tick::ZERO, Default::default()).get(4, 4), leaf_pixel(&s, 2.0, 3.4));
    compare(s.render(middle, Tick::ZERO, Default::default()).get(12, 4), leaf_pixel(&s, 1.9, 1.9));
    s.execute("essential.pushToComp", json!({"layer":left,"control":control})).unwrap();
    compare(s.render(middle, Tick::ZERO, Default::default()).get(4, 4), leaf_pixel(&s, 2.0, 3.4));
    compare(s.render(middle, Tick::ZERO, Default::default()).get(12, 4), leaf_pixel(&s, 1.9, 1.9));
    let old = s.project.comp(middle).unwrap().layer(LayerId(right)).unwrap().props.prop(&path).unwrap();
    assert!(old.cineon_gamma_map.is_some(), "the sibling's prior identity policy is explicit after Push");
    let outer_control = s.execute("essential.addProperty", json!({"layer":left,"path":path})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("essential.linkProperty", json!({"comp":middle.0,"control":outer_control,"layer":right,"path":path})).unwrap();
    s.execute("comp.new", json!({"name":"Main","width":16,"height":8,"duration":2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let outer = s.execute("layer.addItem", json!({"item":middle.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.set", json!({"layer":outer,"control":outer_control,"value":2.2})).unwrap();
    let check = |s: &Session| {
        compare(s.render(main, Tick::ZERO, Default::default()).get(4, 4), leaf_pixel(s, 2.2 / 1.7, 2.2));
        compare(s.render(main, Tick::ZERO, Default::default()).get(12, 4), leaf_pixel(s, 2.2, 2.2));
    };
    check(&s);
    let restored = Session { project: effectcraft_project::Project::from_json(&s.project.to_file_json().unwrap()).unwrap().into(), ..Session::default() };
    check(&restored);

    s.execute("comp.new", json!({"name":"Repeated","width":16,"height":8,"duration":2})).unwrap();
    let repeated = s.active_comp_id().unwrap();
    let mut shared = None;
    for _ in 0..12 {
        let instance = s.execute("layer.addItem", json!({"item":leaf.0})).unwrap()["layer"].as_u64().unwrap();
        if let Some(control) = shared {
            s.execute("essential.linkProperty", json!({"comp":repeated.0,"control":control,"layer":instance,"path":path})).unwrap();
        } else {
            shared = Some(s.execute("essential.addProperty", json!({"layer":instance,"path":path})).unwrap()["controls"][0].as_u64().unwrap());
        }
    }
    let graph = effectcraft_project::essential::gamma::definition_map(&s.project, repeated, shared.unwrap()).unwrap();
    assert_eq!(graph.graph.nodes.len(), 2, "identical source-instance policies must share a node");
    let mut project = (*s.project).clone();
    let original = match &project.item(repeated).unwrap().kind {
        effectcraft_project::ItemKind::Comp(c) => c.clone(),
        _ => panic!("expected composition fixture"),
    };
    effectcraft_project::essential::gamma::prune(&mut project);
    effectcraft_project::essential::gamma::mark_units(&mut project);
    let unchanged = match &project.item(repeated).unwrap().kind {
        effectcraft_project::ItemKind::Comp(c) => c,
        _ => panic!("expected composition fixture"),
    };
    assert!(std::sync::Arc::ptr_eq(&original, unchanged), "valid compatibility metadata must not clone whole comps on ordinary edits");
}

#[test]
fn version_1_mixed_preset_adopts_shared_units_for_a_retained_current_expression() {
    struct LiteralExpression;
    impl effectcraft_render::ExprHost for LiteralExpression {
        fn eval(&self, _ctx: &effectcraft_render::EvalCtx, _layer: &Layer, prop: &effectcraft_project::Property, _value: &KV) -> Result<KV, String> {
            prop.expr.as_ref().map(|e| e.text.trim()).unwrap_or_default().parse::<f64>().map(KV::Scalar).map_err(|e| e.to_string())
        }
    }
    let mut s = Session { expr: Some(std::sync::Arc::new(LiteralExpression)), ..Session::default() };
    s.execute("file.projectSettings", json!({"bitDepth":32})).unwrap();
    s.execute("comp.new", json!({"name":"Inner","width":8,"height":8,"duration":2})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color":[0.4,0.5,0.6],"width":8,"height":8})).unwrap()["layer"].as_u64().unwrap();
    apply(&mut s, solid, "Cineon Converter");
    s.execute("prop.set", json!({"layer":solid,"path":"effects/#1/highlightRolloff","value":0})).unwrap();
    let control = s.execute("essential.addProperty", json!({"layer":solid,"path":"effects/#1/gamma"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("essential.linkProperty", json!({"control":control,"layer":solid,"path":"transform/opacity"})).unwrap();
    s.execute("comp.new", json!({"name":"Main","width":8,"height":8,"duration":2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let instance = s.execute("layer.addItem", json!({"item":inner.0})).unwrap()["layer"].as_u64().unwrap();
    let path = format!("essential/eg{control}");
    let text = " 4.4 ";
    s.execute("prop.setExpression", json!({"layer":instance,"path":path,"expression":text})).unwrap();
    let kept = s.project.comp(main).unwrap().layer(LayerId(instance)).unwrap().props.prop(&path).unwrap().expr.clone();
    assert!(kept.as_ref().unwrap().compat.is_none());
    assert_eq!(kept.as_ref().unwrap().text, text);
    let preset = json!({"effectcraftAnimationPreset":1,"properties":[{"path":path,"value":serde_json::to_value(KV::Scalar(2.2)).unwrap(),"keys":[],"expression":null}],"effects":[]});
    s.execute("anim.applyPreset", json!({"preset":preset,"layers":[instance]})).unwrap();
    let prop = s.project.comp(main).unwrap().layer(LayerId(instance)).unwrap().props.prop(&path).unwrap();
    assert_eq!(prop.expr, kept, "preset application retains the existing expression text and flags");
    assert!(prop.cineon_gamma_map.is_some());
    assert!(!prop.cineon_gamma_units);
    let mut reference = Session { project: s.project.clone(), ..Session::default() };
    let leaf = std::sync::Arc::make_mut(&mut reference.project).comp_mut(inner).unwrap().layer_mut(LayerId(solid)).unwrap();
    leaf.props.prop_mut("effects/#1/gamma").unwrap().value = KV::Scalar(4.4 / 1.7);
    leaf.props.prop_mut("transform/opacity").unwrap().value = KV::Scalar(4.4);
    let want = reference.render(inner, Tick::ZERO, Default::default()).get(4, 4);
    let got = s.render(main, Tick::ZERO, Default::default()).get(4, 4);
    for ch in 0..4 {
        assert!((got[ch] - want[ch]).abs() < 1e-5, "adopted shared-unit output: {got:?} != {want:?}");
    }
}
