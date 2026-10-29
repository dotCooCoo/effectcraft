//! Retained shared controls preserve their public numbers and individual target units.
use super::{EgControl, EgLink};
use crate::{ItemId, Layer, LayerSource, Project, Property, Value};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
pub const SCALE: f64 = 1.7;
/// An instance's entry point into a shared, compact binding graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub graph: Arc<OverrideMap>,
    pub node: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverrideMap {
    pub nodes: Vec<OverrideNode>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverrideNode {
    pub comp: ItemId,
    pub control: u64,
    pub units: bool,
    pub gamma_targets: Vec<EgLink>,
    pub forward_targets: Vec<Forward>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forward {
    pub target: EgLink,
    pub node: usize,
}
impl Binding {
    pub fn policy(&self, comp: ItemId, control: u64) -> Option<&OverrideNode> {
        self.graph.nodes.get(self.node).filter(|n| n.comp == comp && n.control == control)
    }
    pub fn child(&self, comp: ItemId, control: u64, target: EgLink) -> Option<Self> {
        let node = self.policy(comp, control)?.forward_targets.iter().find(|e| e.target == target)?.node;
        self.graph.nodes.get(node)?;
        Some(Self { graph: self.graph.clone(), node })
    }
    pub fn offset(&mut self, offset: u64) {
        for n in &mut Arc::make_mut(&mut self.graph).nodes {
            n.comp.0 = n.comp.0.saturating_add(offset);
            n.control = n.control.saturating_add(offset);
            for e in n.gamma_targets.iter_mut().chain(n.forward_targets.iter_mut().map(|f| &mut f.target)) {
                e.layer.0 = e.layer.0.saturating_add(offset);
                e.prop = e.prop.saturating_add(offset);
            }
        }
    }
}
pub fn scale_value(v: &mut Value, factor: f64) {
    if let Value::Scalar(x) = v {
        *x *= factor;
    }
}
pub fn scale_property(p: &mut Property, factor: f64) {
    scale_value(&mut p.value, factor);
    if let crate::ParamUi::Slider { min, max, slider_min, slider_max, .. } = &mut p.ui {
        *min *= factor;
        *max *= factor;
        *slider_min *= factor;
        *slider_max *= factor;
    }
    for k in &mut p.keys {
        scale_value(&mut k.value, factor);
        for e in k.in_ease.iter_mut().chain(&mut k.out_ease) {
            e.speed *= factor;
        }
    }
}
pub fn factor(c: &EgControl, layer: crate::LayerId, prop: u64) -> f64 {
    if c.legacy_gamma_targets.contains(&EgLink { layer, prop }) { 1.0 / SCALE } else { 1.0 }
}
pub fn direct(l: &Layer, uid: u64, legacy: bool) -> bool {
    l.props.sub("effects").is_some_and(|fx| {
        fx.groups().any(|g| {
            matches!(&g.kind, crate::GroupKind::Effect { effect, version } if effect == "ec.utility.cineon" && (!legacy || *version == 0))
                && g.get("gamma").is_some_and(|p| p.uid == uid)
        })
    })
}
pub fn nested(l: &Layer, uid: u64) -> Option<(ItemId, u64)> {
    let LayerSource::Comp { item } = l.source else { return None };
    let mut found = None;
    super::group(l)?.walk("", &mut |_, p| {
        if p.uid == uid {
            found = super::control_of(&p.match_id).map(|id| (item, id));
        }
    });
    found
}
/// Controls already expressed entirely in current Gamma units. Retained mixed controls
/// keep their public units even if a non-Gamma link is subsequently removed.
pub fn exclusive(p: &Project, legacy: bool) -> HashSet<(ItemId, u64)> {
    let mut out = HashSet::new();
    loop {
        let mut add = vec![];
        for (cid, cm) in p.comps() {
            let Some(eg) = &cm.essential else { continue };
            for c in eg.flat() {
                let Some(canonical) = eg.resolve(c.id) else { continue };
                let targets = canonical.kind.targets();
                if !out.contains(&(*cid, c.id))
                    && canonical.legacy_gamma_targets.is_empty()
                    && !targets.is_empty()
                    && targets
                        .iter()
                        .all(|(lid, uid)| cm.layer(*lid).is_some_and(|l| direct(l, *uid, legacy) || nested(l, *uid).is_some_and(|k| out.contains(&k))))
                {
                    add.push((*cid, c.id));
                }
            }
        }
        if add.is_empty() {
            break;
        }
        out.extend(add);
    }
    out
}

/// Capture old preset semantics without changing the shared definition or sibling instances.
pub fn override_map(p: &Project, comp: ItemId, control: u64, legacy: bool) -> Option<Binding> {
    graph(p, comp, control, Some(legacy), false)
}
pub fn definition_map(p: &Project, comp: ItemId, control: u64) -> Option<Binding> {
    graph(p, comp, control, None, false)
}
/// Includes explicit identity nodes, used to preserve existing overrides when source policy changes.
pub fn snapshot(p: &Project, comp: ItemId, control: u64) -> Option<Binding> {
    graph(p, comp, control, None, true)
}
// Compare reachable policies rather than Arc addresses. Repeated identical precomp
// instances share one output node even when earlier synchronization allocated new graphs.
fn same_policy(a: &Option<Binding>, b: &Option<Binding>) -> bool {
    let (Some(a), Some(b)) = (a, b) else { return a.is_none() && b.is_none() };
    if Arc::ptr_eq(&a.graph, &b.graph) && a.node == b.node {
        return true;
    }
    let mut pending = vec![(a.node, b.node)];
    let mut seen = HashSet::new();
    while let Some((ai, bi)) = pending.pop() {
        if !seen.insert((ai, bi)) {
            continue;
        }
        let (Some(an), Some(bn)) = (a.graph.nodes.get(ai), b.graph.nodes.get(bi)) else { return false };
        if an.comp != bn.comp
            || an.control != bn.control
            || an.units != bn.units
            || an.gamma_targets != bn.gamma_targets
            || an.forward_targets.len() != bn.forward_targets.len()
        {
            return false;
        }
        for (af, bf) in an.forward_targets.iter().zip(&bn.forward_targets) {
            if af.target != bf.target {
                return false;
            }
            pending.push((af.node, bf.node));
        }
    }
    true
}
fn graph(p: &Project, comp: ItemId, control: u64, legacy: Option<bool>, keep_identity: bool) -> Option<Binding> {
    let pure = exclusive(p, legacy.unwrap_or(false));
    type Key = (ItemId, u64, bool);
    let mut seen: HashMap<Key, Vec<(Option<Binding>, usize)>> = HashMap::new();
    let mut nodes = vec![];
    let mut pending = vec![];
    let add = |cid: ItemId,
               id: u64,
               context: Option<Binding>,
               inherited_units: Option<bool>,
               seen: &mut HashMap<Key, Vec<(Option<Binding>, usize)>>,
               nodes: &mut Vec<OverrideNode>,
               pending: &mut Vec<(usize, Option<Binding>)>|
     -> Option<usize> {
        let c = p.comp(cid)?.essential.as_ref()?.resolve(id)?;
        let context = context.filter(|b| b.policy(cid, c.id).is_some());
        let units = if legacy.is_some() {
            false
        } else {
            inherited_units.or_else(|| context.as_ref().and_then(|b| b.policy(cid, c.id)).map(|n| n.units)).unwrap_or(c.cineon_gamma_units)
        };
        let key = (cid, c.id, units);
        if let Some((_, index)) = seen.get(&key).and_then(|candidates| candidates.iter().find(|(previous, _)| same_policy(previous, &context))) {
            return Some(*index);
        }
        let index = nodes.len();
        seen.entry(key).or_default().push((context.clone(), index));
        nodes.push(OverrideNode { comp: cid, control: c.id, units, gamma_targets: vec![], forward_targets: vec![] });
        pending.push((index, context));
        Some(index)
    };
    let root = add(comp, control, None, None, &mut seen, &mut nodes, &mut pending)?;
    while let Some((index, context)) = pending.pop() {
        let (cid, id) = (nodes[index].comp, nodes[index].control);
        let Some(cm) = p.comp(cid) else { continue };
        let Some(c) = cm.essential.as_ref().and_then(|eg| eg.resolve(id)) else { continue };
        for (lid, uid) in c.kind.targets() {
            let Some(l) = cm.layer(lid) else { continue };
            let edge = EgLink { layer: lid, prop: uid };
            let scaled = legacy.map_or_else(
                || context.as_ref().and_then(|b| b.policy(cid, id)).map_or_else(|| c.legacy_gamma_targets.contains(&edge), |n| n.gamma_targets.contains(&edge)),
                |old| direct(l, uid, old) || nested(l, uid).is_some_and(|k| pure.contains(&k)),
            );
            if scaled {
                nodes[index].gamma_targets.push(edge);
                continue;
            }
            if let Some((child_comp, child_control)) = nested(l, uid) {
                let child_context = if legacy.is_some() {
                    None
                } else {
                    context.as_ref().and_then(|b| b.child(cid, id, edge)).or_else(|| l.props.find(uid).and_then(|pr| pr.cineon_gamma_map.clone()))
                };
                let canonical =
                    p.comp(child_comp).and_then(|cm| cm.essential.as_ref()).and_then(|eg| eg.resolve(child_control)).map(|c| c.id).unwrap_or(child_control);
                let child_units = child_context
                    .as_ref()
                    .and_then(|b| b.policy(child_comp, canonical))
                    .map(|n| n.units)
                    .or_else(|| l.props.find(uid).map(|pr| pr.cineon_gamma_units));
                if let Some(child) = add(child_comp, child_control, child_context, child_units, &mut seen, &mut nodes, &mut pending) {
                    nodes[index].forward_targets.push(Forward { target: edge, node: child });
                }
            }
        }
    }
    if !keep_identity && nodes.iter().all(|n| n.gamma_targets.is_empty()) { None } else { Some(Binding { graph: Arc::new(OverrideMap { nodes }), node: root }) }
}
/// Store stable control units: adding an unrelated link must not change retained reader units.
pub fn mark_units(p: &mut Project) {
    let mut units = exclusive(p, false);
    for (cid, cm) in p.comps() {
        if let Some(eg) = &cm.essential {
            for c in eg.flat() {
                let Some(master) = eg.resolve(c.id) else { continue };
                if master.legacy_gamma_targets.is_empty() && super::source_prop(p, *cid, c).is_some_and(|(_, pr)| pr.cineon_gamma_units) {
                    units.insert((*cid, c.id));
                }
            }
        }
    }
    for (cid, id) in units {
        if p.comp(cid).and_then(|cm| cm.essential.as_ref()).and_then(|eg| eg.find(id)).is_some_and(|c| c.cineon_gamma_units) {
            continue;
        }
        if let Some(c) = p.comp_mut(cid).and_then(|cm| cm.essential.as_mut()).and_then(|eg| eg.find_mut(id)) {
            c.cineon_gamma_units = true;
        }
    }
}

pub fn migrate_bindings(p: &mut Project) {
    let pure = exclusive(p, true);
    let mut writes = vec![];
    for (cid, cm) in p.comps() {
        let Some(eg) = &cm.essential else { continue };
        for c in eg.flat() {
            if pure.contains(&(*cid, c.id)) || !c.legacy_gamma_targets.is_empty() {
                continue;
            }
            let targets = c
                .kind
                .targets()
                .into_iter()
                .filter_map(|(lid, uid)| {
                    let l = cm.layer(lid)?;
                    (direct(l, uid, true) || nested(l, uid).is_some_and(|k| pure.contains(&k))).then_some(EgLink { layer: lid, prop: uid })
                })
                .collect::<Vec<_>>();
            if !targets.is_empty() {
                writes.push((*cid, c.id, targets));
            }
        }
    }
    for (cid, id, targets) in writes {
        if let Some(c) = p.comp_mut(cid).and_then(|cm| cm.essential.as_mut()).and_then(|eg| eg.find_mut(id)) {
            c.legacy_gamma_targets = targets;
        }
    }
    for (cid, id) in pure {
        if let Some(c) = p.comp_mut(cid).and_then(|cm| cm.essential.as_mut()).and_then(|eg| eg.find_mut(id)) {
            c.cineon_gamma_units = true;
        }
    }
    let maps: HashMap<_, _> = p
        .comps()
        .flat_map(|(cid, cm)| cm.essential.iter().flat_map(|eg| eg.flat()).map(move |c| (*cid, c.id)))
        .filter_map(|(cid, id)| definition_map(p, cid, id).map(|m| ((cid, id), m)))
        .collect();
    for it in p.items.values_mut() {
        let crate::ItemKind::Comp(cm) = &mut it.kind else { continue };
        for l in &mut Arc::make_mut(cm).layers {
            let LayerSource::Comp { item } = l.source else { continue };
            if let Some(g) = l.props.sub_mut(super::GROUP) {
                g.walk_mut(&mut |pr| {
                    if pr.cineon_gamma_map.is_none() {
                        pr.cineon_gamma_map = super::control_of(&pr.match_id).and_then(|id| maps.get(&(item, id))).cloned();
                    }
                });
            }
        }
    }
}
pub fn target_units(c: &EgControl, comp: ItemId, layer: crate::LayerId, uid: u64, source: &Property) -> Option<bool> {
    let edge = EgLink { layer, prop: uid };
    let node = source.cineon_gamma_map.as_ref().and_then(|b| b.policy(comp, c.id));
    if node.map_or_else(|| c.legacy_gamma_targets.contains(&edge), |n| n.gamma_targets.contains(&edge)) {
        return Some(true);
    }
    let child = source.cineon_gamma_map.as_ref()?.child(comp, c.id, edge)?;
    child.graph.nodes.get(child.node).map(|n| n.units)
}
pub fn to_target(c: &EgControl, comp: ItemId, layer: crate::LayerId, uid: u64, source: &Property) -> Property {
    let mut out = source.clone();
    let edge = EgLink { layer, prop: uid };
    let node = source.cineon_gamma_map.as_ref().and_then(|b| b.policy(comp, c.id));
    if node.map_or_else(|| c.legacy_gamma_targets.contains(&edge), |n| n.gamma_targets.contains(&edge)) {
        scale_property(&mut out, 1.0 / SCALE);
        out.cineon_gamma_units = true;
    }
    out.cineon_gamma_map = source.cineon_gamma_map.as_ref().and_then(|b| b.child(comp, c.id, edge));
    if let Some(b) = &out.cineon_gamma_map
        && let Some(n) = b.graph.nodes.get(b.node)
    {
        out.cineon_gamma_units = n.units;
    }
    out
}
pub fn prune(p: &mut Project) {
    let graph = p.clone();
    prune_against(p, &graph);
}
pub fn prune_against(p: &mut Project, graph: &Project) {
    let targets: HashMap<_, _> = graph
        .comps()
        .flat_map(|(cid, cm)| cm.essential.iter().flat_map(move |eg| eg.flat().into_iter().map(move |c| ((*cid, c.id), c.kind.targets()))))
        .collect();
    let valid = |cid: ItemId, id: u64, e: &EgLink| targets.get(&(cid, id)).is_some_and(|ts| ts.contains(&(e.layer, e.prop)));
    let forward_valid = |cid: ItemId, id: u64, f: &Forward, map: &OverrideMap| {
        if !valid(cid, id, &f.target) {
            return false;
        }
        let Some(target) = map.nodes.get(f.node) else {
            return false;
        };
        let child = graph.comp(cid).and_then(|cm| cm.layer(f.target.layer)).and_then(|l| nested(l, f.target.prop));
        child.is_some_and(|(comp, control)| {
            comp == target.comp
                && graph.comp(comp).and_then(|cm| cm.essential.as_ref()).and_then(|eg| eg.resolve(control)).is_some_and(|c| c.id == target.control)
        })
    };
    let invalid_binding = |binding: &Binding| {
        binding.graph.nodes.get(binding.node).is_none_or(|n| !targets.contains_key(&(n.comp, n.control)))
            || binding.graph.nodes.iter().any(|n| {
                n.gamma_targets.iter().any(|e| !valid(n.comp, n.control, e))
                    || n.forward_targets.iter().any(|f| !forward_valid(n.comp, n.control, f, &binding.graph))
            })
    };
    for it in p.items.values_mut() {
        let crate::ItemKind::Comp(cm) = &mut it.kind else { continue };
        let cid = it.id;
        let invalid_controls = cm.essential.as_ref().is_some_and(|eg| eg.flat().iter().any(|c| c.legacy_gamma_targets.iter().any(|e| !valid(cid, c.id, e))));
        let invalid_instances = cm.layers.iter().any(|l| {
            let Some(g) = super::group(l) else { return false };
            let mut found = false;
            g.walk("", &mut |_, pr| {
                found |= pr.cineon_gamma_map.as_ref().is_some_and(invalid_binding);
            });
            found
        });
        if !invalid_controls && !invalid_instances {
            continue;
        }
        let cm = Arc::make_mut(cm);
        if let Some(eg) = &mut cm.essential {
            fn go(cs: &mut [EgControl], cid: ItemId, valid: &impl Fn(ItemId, u64, &EgLink) -> bool) {
                for c in cs {
                    c.legacy_gamma_targets.retain(|e| valid(cid, c.id, e));
                    if let super::EgKind::Group { children } = &mut c.kind {
                        go(children, cid, valid);
                    }
                }
            }
            go(&mut eg.controls, cid, &valid);
        }
        for l in &mut cm.layers {
            let Some(group) = l.props.sub_mut(super::GROUP) else { continue };
            group.walk_mut(&mut |pr| {
                if let Some(binding) = &mut pr.cineon_gamma_map {
                    if binding.graph.nodes.get(binding.node).is_none_or(|n| !targets.contains_key(&(n.comp, n.control))) {
                        pr.cineon_gamma_map = None;
                        return;
                    }
                    let invalid = binding.graph.nodes.iter().any(|n| {
                        n.gamma_targets.iter().any(|e| !valid(n.comp, n.control, e))
                            || n.forward_targets.iter().any(|f| !forward_valid(n.comp, n.control, f, &binding.graph))
                    });
                    if invalid {
                        let allowed: Vec<Vec<_>> = binding
                            .graph
                            .nodes
                            .iter()
                            .map(|n| n.forward_targets.iter().filter(|f| forward_valid(n.comp, n.control, f, &binding.graph)).cloned().collect())
                            .collect();
                        for (n, forward) in Arc::make_mut(&mut binding.graph).nodes.iter_mut().zip(allowed) {
                            n.gamma_targets.retain(|e| valid(n.comp, n.control, e));
                            n.forward_targets = forward;
                        }
                    }
                }
            });
        }
    }
}

/// Push adopts this instance's binding units as the source policy. Already-overridden
/// siblings keep their previous policies; following instances take the new source defaults.
pub fn adopt_source_policy(p: &mut Project, comp: ItemId, control: u64, binding: &Binding, units: bool) {
    let canonical = p.comp(comp).and_then(|cm| cm.essential.as_ref()).and_then(|eg| eg.resolve(control)).map(|c| c.id);
    let Some(control) = canonical else { return };
    let Some(policy) = binding.policy(comp, control) else { return };
    let mapped = policy.gamma_targets.clone();
    let old = snapshot(p, comp, control);
    let siblings: Vec<_> = p
        .comps()
        .flat_map(|(cid, cm)| {
            cm.layers.iter().filter_map(move |l| {
                if !matches!(l.source,LayerSource::Comp {item} if item==comp) {
                    return None;
                }
                Some((*cid, l.id, super::overridden(l)))
            })
        })
        .collect();
    let mirrors: HashSet<_> = p
        .comp(comp)
        .and_then(|cm| cm.essential.as_ref())
        .map(|eg| eg.flat().iter().filter(|c| eg.resolve(c.id).is_some_and(|m| m.id == control)).map(|c| c.id).collect())
        .unwrap_or_default();
    for (cid, lid, overridden) in siblings {
        if let Some(g) = p.comp_mut(cid).and_then(|cm| cm.layer_mut(lid)).and_then(|l| l.props.sub_mut(super::GROUP)) {
            g.walk_mut(&mut |pr| {
                if overridden.contains(&pr.uid) && pr.cineon_gamma_map.is_none() && super::control_of(&pr.match_id).is_some_and(|id| mirrors.contains(&id)) {
                    pr.cineon_gamma_map = old.clone();
                }
            });
        }
    }
    if let Some(c) = p.comp_mut(comp).and_then(|cm| cm.essential.as_mut()).and_then(|eg| eg.find_mut(control)) {
        c.legacy_gamma_targets = mapped;
        c.cineon_gamma_units = units;
    }
}
