//! 场景列表与节点读写（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

pub(super) fn scene_list(
    api: &mut LocalEngineApi<'_>,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<NodeBrief>, RpcFailure> {
    let needle = filter
        .map(|f| f.trim().to_ascii_lowercase())
        .filter(|f| !f.is_empty());
    let scene = api.scene.borrow();
    let mut out = Vec::new();
    for node in scene.iter() {
        if let Some(needle) = &needle {
            if !node.name.to_ascii_lowercase().contains(needle) {
                continue;
            }
        }
        if let Some(limit) = limit {
            if out.len() >= limit {
                break;
            }
        }
        out.push(NodeBrief {
            name: node.name.clone(),
            mesh: Some(node.mesh.as_string()),
            position: node.transform.position.to_array(),
            visible: node.visible,
        });
    }
    Ok(out)
}

pub(super) fn scene_get(api: &mut LocalEngineApi<'_>, node: &str) -> Result<Value, RpcFailure> {
    let scene = api.scene.borrow();
    Ok(node_value(find_node(&scene, node)?))
}

pub(super) fn scene_set_transform(
    api: &mut LocalEngineApi<'_>,
    node: &str,
    patch: TransformPatch,
) -> Result<Value, RpcFailure> {
    if patch.is_empty() {
        return Err(RpcFailure::invalid_params("变换补丁为空"));
    }
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let id = find_node_id(&scene, node)?;
            let snapshot = {
                let target = scene
                    .get_mut(id)
                    .ok_or_else(|| RpcFailure::not_found(format!("物件 `{node}` 不存在")))?;
                if let Some(position) = patch.position {
                    check_finite(&position, "position")?;
                    target.transform.position = Vec3::from_array(position);
                }
                if let Some(rotation) = patch.rotation {
                    check_finite(&rotation, "rotation")?;
                    let quat = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);
                    if quat.length_squared() < 1e-12 {
                        return Err(RpcFailure::invalid_params(
                            "rotation 需要非零四元数 [x,y,z,w]（不能全为 0）",
                        ));
                    }
                    target.transform.rotation = quat.normalize();
                }
                if let Some(scale) = patch.scale {
                    check_finite(&scale, "scale")?;
                    target.transform.scale = Vec3::from_array(scale);
                }
                node_value(target)
            };
            // 局部变换改完立刻刷新世界矩阵：编辑器下一次 `scene.get` 应看到新的 world
            scene.update_world_transforms();
            Ok(snapshot)
        })
}

pub(super) fn scene_spawn(
    api: &mut LocalEngineApi<'_>,
    object: Value,
) -> Result<Value, RpcFailure> {
    let desc: ObjectDesc = serde_json::from_value(object)
        .map_err(|e| RpcFailure::invalid_params(format!("物件描述不合法: {e}")))?;
    let name = desc.name.clone();
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let id = scene.spawn(desc).map_err(RpcFailure::engine)?;
            scene.update_world_transforms();
            Ok(json!({ "id": id, "name": name, "nodes": scene.len() }))
        })
}

pub(super) fn scene_remove(api: &mut LocalEngineApi<'_>, node: &str) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let id = find_node_id(&scene, node)?;
            let name = scene
                .get(id)
                .map(|n| n.name.clone())
                .ok_or_else(|| RpcFailure::not_found(format!("物件 `{node}` 不存在")))?;
            scene.despawn_id(id);
            scene.update_world_transforms();
            Ok(json!({ "removed": name, "nodes": scene.len() }))
        })
}

/// 材质补丁：在现有材质上叠加，再交给 `SurfaceMaterial` 统一收敛。
/// 返回合并后的完整材质（读回入口），调用方据此确认非法值已被夹取。
pub(super) fn scene_set_material(
    api: &mut LocalEngineApi<'_>,
    node: &str,
    patch: MaterialPatch,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let mut material = scene.material_of(node).unwrap_or_default();
            if let Some(metallic) = patch.metallic {
                material.metallic = metallic;
            }
            if let Some(roughness) = patch.roughness {
                material.roughness = roughness;
            }
            if let Some(emissive) = patch.emissive {
                material.emissive = emissive;
            }
            if !scene.set_material(node, material) {
                return Err(RpcFailure::not_found(format!("物件 `{node}` 不存在")));
            }
            let applied = scene.material_of(node).unwrap_or_default();
            let snapshot = find_node(&scene, node)
                .map(node_value)
                .unwrap_or(Value::Null);
            Ok(json!({
                "material": material_value(applied),
                "node": snapshot,
            }))
        })
}
