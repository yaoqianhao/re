//! 引用存在性：动画绑定 / 蒙皮关节 / 形变实例指向的节点必须真的在场景里。
//!
//! 这类悬空引用是删除节点后最容易留下的残留：`despawn` 会剪 skins 引用（实测已做），
//! 但热重载、脚本手动写 Vec、以及外部反序列化都可能绕过那层保护，所以校验器要兜住。

use r3e_core::Scene;

use super::Issue;

/// 悬空节点引用的统一 code。
pub const MISSING_NODE_ISSUE: &str = "ref.node_missing";

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    check_animations(scene, out);
    check_skins(scene, out);
    check_morphs(scene, out);
    check_asset_refs(scene, out);
}

fn check_animations(scene: &Scene, out: &mut Vec<Issue>) {
    for (i, anim) in scene.animations.iter().enumerate() {
        if anim.name.trim().is_empty() {
            out.push(Issue::error(
                "anim.name.empty",
                format!("animations[{i}].name"),
                "动画实例名为空，脚本无法按名字定位",
            ));
        }
        for (j, binding) in anim.bindings.iter().enumerate() {
            if scene.get(binding.node).is_none() {
                out.push(Issue::error(
                    MISSING_NODE_ISSUE,
                    format!("animations[{i}].bindings[{j}].node"),
                    format!(
                        "动画「{}」的绑定指向不存在的节点 {}",
                        anim.name, binding.node
                    ),
                ));
            }
        }
    }
}

fn check_skins(scene: &Scene, out: &mut Vec<Issue>) {
    for (i, skin) in scene.skins.iter().enumerate() {
        if skin.name.trim().is_empty() {
            out.push(Issue::error(
                "skin.name.empty",
                format!("skins[{i}].name"),
                "蒙皮实例名为空",
            ));
        }
        if scene.get(skin.mesh_node).is_none() {
            out.push(Issue::error(
                MISSING_NODE_ISSUE,
                format!("skins[{i}].mesh_node"),
                format!("蒙皮「{}」的网格节点 {} 不存在", skin.name, skin.mesh_node),
            ));
        }
        for (j, joint) in skin.bindings.iter().enumerate() {
            if scene.get(joint.node).is_none() {
                out.push(Issue::error(
                    MISSING_NODE_ISSUE,
                    format!("skins[{i}].bindings[{j}].node"),
                    format!(
                        "蒙皮「{}」的关节「{}」指向不存在的节点 {}",
                        skin.name, joint.path, joint.node
                    ),
                ));
            }
        }
        // 已求解过就必须一一对应：长度不等意味着关节矩阵与关节表错位，
        // 传给渲染层会让整具骨架错乱。全 0 长度是「还没求解」，只算可疑。
        if skin.joint_matrices.is_empty() {
            if !skin.bindings.is_empty() {
                out.push(Issue::warn(
                    "skin.joint_matrices.unsolved",
                    format!("skins[{i}].joint_matrices"),
                    format!("蒙皮「{}」尚未求解关节矩阵", skin.name),
                ));
            }
        } else if skin.joint_matrices.len() != skin.bindings.len() {
            out.push(Issue::error(
                "skin.joint_matrices.len",
                format!("skins[{i}].joint_matrices"),
                format!(
                    "蒙皮「{}」关节矩阵 {} 个与关节表 {} 个不匹配",
                    skin.name,
                    skin.joint_matrices.len(),
                    skin.bindings.len()
                ),
            ));
        }
    }
}

fn check_morphs(scene: &Scene, out: &mut Vec<Issue>) {
    for (i, morph) in scene.morphs.iter().enumerate() {
        if scene.get(morph.mesh_node).is_none() {
            out.push(Issue::error(
                MISSING_NODE_ISSUE,
                format!("morphs[{i}].mesh_node"),
                format!("形变实例的网格节点 {} 不存在", morph.mesh_node),
            ));
        }
    }
}

/// 物件 `asset:` 引用：`MeshRef::Asset` 带的是资产路径哈希，这里只能查「场景里是不是
/// 声明过对应资产」——用 `assets[].path` 反推哈希比对，未命中即悬空。
fn check_asset_refs(scene: &Scene, out: &mut Vec<Issue>) {
    if scene.assets.is_empty() {
        return;
    }
    let declared: Vec<r3e_core::AssetId> = scene
        .assets
        .iter()
        .map(|a| r3e_core::AssetId::from_path(&a.path))
        .collect();
    for node in scene.iter() {
        if let r3e_core::MeshRef::Asset { asset, .. } = node.mesh {
            if !declared.contains(&asset) {
                out.push(Issue::error(
                    "ref.asset_missing",
                    format!("nodes[{}].mesh", node.id),
                    format!("节点「{}」引用的资产未在 assets 段声明", node.name),
                ));
            }
        }
    }
}
