//! 必填 / 唯一 / 越界 / 数值有限性：场景里「一填错就必然出事」的硬指标。

use std::collections::HashMap;

use r3e_core::{NodeId, Scene, MAX_LOCAL_LIGHTS, MAX_MORPH_TARGETS};

use super::Issue;

/// 局部光源超限的 code。
pub const MAX_LIGHTS_ISSUE: &str = "lights.over_limit";

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    check_nodes(scene, out);
    check_lights(scene, out);
    check_ui(scene, out);
    check_texts(scene, out);
    check_morphs(scene, out);
    check_scene_globals(scene, out);
}

fn check_nodes(scene: &Scene, out: &mut Vec<Issue>) {
    let mut seen: HashMap<&str, NodeId> = HashMap::new();
    for node in scene.iter() {
        if node.name.trim().is_empty() {
            out.push(Issue::error(
                "node.name.empty",
                format!("nodes[{}].name", node.id),
                "节点名为空，按名字寻址的接口全部失效",
            ));
        } else if let Some(first) = seen.get(node.name.as_str()) {
            out.push(Issue::error(
                "node.name.duplicate",
                format!("nodes[{}].name", node.id),
                format!("节点名「{}」与 id={} 重复", node.name, first),
            ));
        } else {
            seen.insert(node.name.as_str(), node.id);
        }

        let t = &node.transform;
        if !t.position.is_finite() || !t.rotation.is_finite() || !t.scale.is_finite() {
            out.push(Issue::error(
                "node.transform.non_finite",
                format!("nodes[{}].transform", node.id),
                format!("节点「{}」的变换含 NaN / Inf", node.name),
            ));
        }
        if !node.color.is_finite() {
            out.push(Issue::error(
                "node.color.non_finite",
                format!("nodes[{}].color", node.id),
                format!("节点「{}」的颜色含 NaN / Inf", node.name),
            ));
        }
        if let Some(m) = node.material {
            if !m.metallic.is_finite()
                || !m.roughness.is_finite()
                || !m.emissive.iter().all(|v| v.is_finite())
            {
                out.push(Issue::error(
                    "node.material.non_finite",
                    format!("nodes[{}].material", node.id),
                    format!("节点「{}」的材质含 NaN / Inf", node.name),
                ));
            }
        }
    }
}

fn check_lights(scene: &Scene, out: &mut Vec<Issue>) {
    if scene.lights.len() > MAX_LOCAL_LIGHTS {
        out.push(Issue::error(
            MAX_LIGHTS_ISSUE,
            "lights",
            format!(
                "局部光源 {} 盏，超过上限 {}",
                scene.lights.len(),
                MAX_LOCAL_LIGHTS
            ),
        ));
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (i, light) in scene.lights.iter().enumerate() {
        if light.key.trim().is_empty() {
            out.push(Issue::error(
                "light.key.empty",
                format!("lights[{i}].key"),
                "局部光源 key 为空，按 key 更新 / 删除都会失效",
            ));
            continue;
        }
        if let Some(first) = seen.get(light.key.as_str()) {
            out.push(Issue::error(
                "light.key.duplicate",
                format!("lights[{i}].key"),
                format!("光源 key「{}」与 lights[{}] 重复", light.key, first),
            ));
        } else {
            seen.insert(light.key.as_str(), i);
        }
        let finite = light.pos.iter().all(|v| v.is_finite())
            && light.color.iter().all(|v| v.is_finite())
            && light.intensity.is_finite()
            && light.range.is_finite();
        if !finite {
            out.push(Issue::error(
                "light.value.non_finite",
                format!("lights[{i}]"),
                format!(
                    "光源「{}」的位置 / 颜色 / 强度 / 范围含 NaN / Inf",
                    light.key
                ),
            ));
        }
    }
}

fn check_ui(scene: &Scene, out: &mut Vec<Issue>) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (i, el) in scene.ui.iter().enumerate() {
        if el.key.trim().is_empty() {
            out.push(Issue::error(
                "ui.key.empty",
                format!("ui[{i}].key"),
                "UI 元素 key 为空，upsert / remove 无法定位",
            ));
            continue;
        }
        if let Some(first) = seen.get(el.key.as_str()) {
            out.push(Issue::error(
                "ui.key.duplicate",
                format!("ui[{i}].key"),
                format!("UI key「{}」与 ui[{}] 重复", el.key, first),
            ));
        } else {
            seen.insert(el.key.as_str(), i);
        }
    }
}

fn check_texts(scene: &Scene, out: &mut Vec<Issue>) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (i, text) in scene.texts.iter().enumerate() {
        if text.key.trim().is_empty() {
            out.push(Issue::error(
                "text.key.empty",
                format!("texts[{i}].key"),
                "文本 key 为空，upsert / remove 无法定位",
            ));
            continue;
        }
        if let Some(first) = seen.get(text.key.as_str()) {
            out.push(Issue::error(
                "text.key.duplicate",
                format!("texts[{i}].key"),
                format!("文本 key「{}」与 texts[{}] 重复", text.key, first),
            ));
        } else {
            seen.insert(text.key.as_str(), i);
        }
    }
}

fn check_morphs(scene: &Scene, out: &mut Vec<Issue>) {
    for (i, morph) in scene.morphs.iter().enumerate() {
        if morph.target_count > MAX_MORPH_TARGETS {
            out.push(Issue::error(
                "morph.target_count.over_max",
                format!("morphs[{i}].target_count"),
                format!(
                    "形变目标数 {} 超过上限 {}",
                    morph.target_count, MAX_MORPH_TARGETS
                ),
            ));
        }
        let bad = morph
            .weights
            .iter()
            .take(morph.target_count.min(MAX_MORPH_TARGETS))
            .any(|w| !w.is_finite());
        if bad {
            out.push(Issue::error(
                "morph.weight.non_finite",
                format!("morphs[{i}].weights"),
                "形变权重含 NaN / Inf",
            ));
        }
    }
}

fn check_scene_globals(scene: &Scene, out: &mut Vec<Issue>) {
    if !scene.background.is_finite() {
        out.push(Issue::error(
            "scene.background.non_finite",
            "background",
            "背景色含 NaN / Inf",
        ));
    }
    let cam = &scene.camera;
    if !cam.position.is_finite() || !cam.target.is_finite() || !cam.fov_deg.is_finite() {
        out.push(Issue::error(
            "camera.non_finite",
            "camera",
            "相机位置 / 目标 / 视角含 NaN / Inf",
        ));
    }
    if scene.physics.enabled == Some(true) {
        if let Some(g) = scene.physics.gravity {
            if !g.iter().all(|v| v.is_finite()) {
                out.push(Issue::error(
                    "physics.gravity.non_finite",
                    "physics.gravity",
                    "物理已启用但重力加速度含 NaN / Inf",
                ));
            }
        }
    }
}
