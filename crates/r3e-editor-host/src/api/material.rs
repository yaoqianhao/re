//! `scene.material.*` 两条的宿主实现（M1-S4-D）。
//!
//! 材质表是**场景数据**（`Scene::materials`），所以这两条走 `Document::write`：
//! `upsert` 推版本、置脏、计入写次数，`list` 纯只读。
//!
//! 撤销口径与宿主其余单条写 RPC 同构：**直写不入撤销栈**，要可撤销的写必须走
//! `tx.begin` + `tx.apply`（命令层已有 `CommandKind::UpsertMaterial`，但这条 RPC
//! 还没进 `tx.apply` 的白名单，属已知债，见 `docs/WRITE_SITES.md`）。
//!
//! 与 `scene.setmaterial`（节点内联三标量）的分工：
//!
//! * `scene.setmaterial` 改的是「这个物件表面怎么反光」——写在节点里；
//! * `scene.material.upsert` 改的是「这份材质长什么样」——场景级表的一行，
//!   节点用 `materialRef` 按名字引用，改一行所有引用它的物件一起变。
//!
//! 语义口径（与 `patch.rs` 的 `*Patch` **不是一套**）：`MaterialUpsert` 是
//! **整行声明**而不是打补丁。没给的字段取缺省值，想保住现有贴图就把
//! `scene.material.list` 读到的值原样回传——这样「清掉贴图」与「别动贴图」
//! 不再共用 JSON 的 `null`，调用方不可能手滑清空一堆贴图。

use super::*;
use r3e_core::scene_material::{find_material, upsert_material};
use r3e_core::{SceneMaterialDecl, SCENE_MATERIAL_MAX, TEXTURE_SLOTS};

/// 一行材质的投影（`material.list` 单项与 `material.upsert` 回包同构）。
///
/// `users` 按 `materialRef` 反查物件名——「这份材质有没有人用」决定能不能删，
/// 比让调用方自己再查一遍全部节点省一圈；也顺带回答了「改了会牵连谁」。
fn material_row_value(scene: &Scene, decl: &SceneMaterialDecl) -> Value {
    let users: Vec<&str> = scene
        .iter()
        .filter(|node| node.material_ref.as_deref() == Some(decl.name.as_str()))
        .map(|node| node.name.as_str())
        .collect();
    // 贴图只列**声明了的**槽：`null` 和「这个槽没贴图」是同一件事，不必各写一遍。
    let mut textures = serde_json::Map::new();
    for (slot, path) in [
        (TEXTURE_SLOTS[0], decl.albedo.as_deref()),
        (TEXTURE_SLOTS[1], decl.normal.as_deref()),
        (TEXTURE_SLOTS[2], decl.metallic_roughness.as_deref()),
        (TEXTURE_SLOTS[3], decl.occlusion.as_deref()),
        (TEXTURE_SLOTS[4], decl.emissive_map.as_deref()),
    ] {
        if let Some(path) = path {
            textures.insert(slot.to_string(), json!(path));
        }
    }
    json!({
        "name": decl.name,
        "metallic": decl.metallic,
        "roughness": decl.roughness,
        "emissive": decl.emissive,
        "textures": Value::Object(textures),
        "uvScale": decl.uv_scale,
        "users": users,
    })
}

/// `scene.material.list`：整表只读投影。
pub(super) fn material_list(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let document = api.scene.borrow();
    let scene: &Scene = &document;
    let materials: Vec<Value> = scene
        .materials
        .iter()
        .map(|decl| material_row_value(scene, decl))
        .collect();
    Ok(json!({
        "count": materials.len(),
        "materials": materials,
    }))
}

/// 整行声明 → 表行。缺省取 [`SceneMaterialDecl::default`]，不引入第二套默认值。
fn decl_from_upsert(decl: MaterialUpsert) -> SceneMaterialDecl {
    let defaults = SceneMaterialDecl::default();
    SceneMaterialDecl {
        name: decl.name,
        metallic: decl.metallic.unwrap_or(defaults.metallic),
        roughness: decl.roughness.unwrap_or(defaults.roughness),
        emissive: decl.emissive.unwrap_or(defaults.emissive),
        albedo: decl.albedo,
        normal: decl.normal,
        metallic_roughness: decl.metallic_roughness,
        occlusion: decl.occlusion,
        emissive_map: decl.emissive_map,
        uv_scale: decl.uv_scale.unwrap_or(defaults.uv_scale),
    }
}

/// `scene.material.upsert`：同名整体覆盖、顺序不变，上限只拦新增。
pub(super) fn material_upsert(
    api: &mut LocalEngineApi<'_>,
    decl: MaterialUpsert,
) -> Result<Value, RpcFailure> {
    // 收敛后才判空：名字只有空格 = 空名，这种「看着有名字实际没有」早报比塞进表好。
    let row = decl_from_upsert(decl).sanitized();
    if row.name.is_empty() {
        return Err(RpcFailure::invalid_params("材质名不能为空"));
    }
    let name = row.name.clone();
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let created = find_material(&scene.materials, &name).is_none();
            if !upsert_material(&mut scene.materials, row) {
                // 覆盖同名不会走到这里（上限只拦新增），所以失败必然是「表满」。
                return Err(RpcFailure::engine(format!(
                    "材质表已满（上限 {SCENE_MATERIAL_MAX} 条）：`{name}` 未写入，先删掉没人用的材质"
                )));
            }
            let material = find_material(&scene.materials, &name)
                .map(|decl| material_row_value(scene, decl));
            Ok(json!({
                "name": name,
                "created": created,
                "count": scene.materials.len(),
                "material": material,
            }))
        })
}
