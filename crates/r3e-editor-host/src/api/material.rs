//! `scene.material.*` 两条的宿主实现（M1-S4-D，写路径事务化于 M1-S4-E 前置补债）。
//!
//! 材质表是**场景数据**（`Scene::materials`），写入只经命令层：
//! `upsert` 构造 [`CommandKind::UpsertMaterial`] 后根据当前是否处于事务分两条路走——
//!
//! * **无未决事务**：`Document::apply` —— 推版本、置脏、计写次，并**入撤销栈**。
//!   写场景的 RPC 不许绕开命令层直写：同一次会话里「别的写都能撤销、这一条不能」
//!   是明摆着的洞（S4-D 已知债，此处补齐）。
//! * **有未决事务**：改走 `tx.apply` 的同名 `op`，进影子场景。回包带
//!   `txId` / `staged` / `committed: false`，主场景一字节不动——事务未决时绝不提交。
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

use super::support::{optional_array, optional_f32, optional_str, required_str};
use super::tx::map_doc;
use super::*;
use r3e_core::{SceneMaterialDecl, SCENE_MATERIAL_MAX, TEXTURE_SLOTS};
use r3e_doc::CommandKind;

/// `tx.apply` 里这条写点的 `op` 名（与 RPC 方法名同名，事务里外一眼对得上）。
pub(super) const MATERIAL_UPSERT_OP: &str = "scene.material.upsert";

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

/// RPC 参数 → 整行声明（键名口径与 `scene.material.upsert` 的协议层解析一致）。
///
/// 协议层在派发前已按同一套键名解析过一遍；这里再解析一次是给 `tx.apply` 用的：
/// 那条路进来的是原始 `args`，而 `MaterialUpsert` 是纯数据（没有 serde 实现），
/// 只能自己搬。两处键名（`metallicRoughness` / `emissiveMap` / `uvScale`）保持一致，
/// 否则同一份参数在「直写」和「事务」两条路上会解析成两行不同的材质。
pub(super) fn upsert_from_args(args: &Value) -> Result<MaterialUpsert, RpcFailure> {
    Ok(MaterialUpsert {
        name: required_str(args, "name")?,
        metallic: optional_f32(args, "metallic")?,
        roughness: optional_f32(args, "roughness")?,
        emissive: optional_array::<3>(args, "emissive")?,
        albedo: optional_str(args, "albedo")?,
        normal: optional_str(args, "normal")?,
        metallic_roughness: optional_str(args, "metallicRoughness")?,
        occlusion: optional_str(args, "occlusion")?,
        emissive_map: optional_str(args, "emissiveMap")?,
        uv_scale: optional_array::<2>(args, "uvScale")?,
    })
}

/// 整行声明 → 收敛后的表行；名字收敛后为空即参数错误。
///
/// 两条入口（本文件的 `material_upsert`、`tx.rs` 的 `tx.apply` 白名单）共用本函数：
/// 各写一遍收敛逻辑，迟早一边 trim 一边不 trim。收敛后才判空——名字只有空格 =
/// 空名，这种「看着有名字实际没有」早报比塞进表好。
pub(super) fn decl_from_upsert(decl: MaterialUpsert) -> Result<SceneMaterialDecl, RpcFailure> {
    let defaults = SceneMaterialDecl::default();
    let row = SceneMaterialDecl {
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
    .sanitized();
    if row.name.is_empty() {
        return Err(RpcFailure::invalid_params("材质名不能为空"));
    }
    Ok(row)
}

/// `scene.material.upsert`：同名整体覆盖、顺序不变，上限只拦新增。
///
/// 两条路都经命令层，区别只在「提交到主场景」还是「暂存到影子场景」。
pub(super) fn material_upsert(
    api: &mut LocalEngineApi<'_>,
    decl: MaterialUpsert,
) -> Result<Value, RpcFailure> {
    let row = decl_from_upsert(decl)?;
    let name = row.name.clone();
    let mut document = api.scene.borrow_mut();

    // 事务未决：进影子。这里**不**碰主场景，回包形状与 `tx.apply` 对齐，
    // 调用方一眼能看出「这条写还没落地」。
    if let Some(tx_id) = document.tx_session().map(|tx| tx.id().to_string()) {
        let staged = document
            .tx_apply(&tx_id, MATERIAL_UPSERT_OP, move |scene| {
                Ok(CommandKind::upsert_scene_material(scene, row))
            })
            .map_err(map_doc)?;
        return Ok(json!({
            "name": name,
            "txId": tx_id,
            "op": MATERIAL_UPSERT_OP,
            "staged": staged.staged,
            "committed": false,
        }));
    }

    // 表满先在宿主把话说清：命令层的 `Rejected` 会被映射成 `TX_REJECTED`，
    // 而这条 RPC 并没有在事务里，回事务错误码只会把调用方带偏。
    let created = document.scene_material(&name).is_none();
    if created && document.materials.len() >= SCENE_MATERIAL_MAX {
        return Err(RpcFailure::engine(format!(
            "材质表已满（上限 {SCENE_MATERIAL_MAX} 条）：`{name}` 未写入，先删掉没人用的材质"
        )));
    }

    let cmd = CommandKind::upsert_scene_material(&document, row);
    document.apply(&cmd).map_err(map_doc)?;
    let material = document
        .scene_material(&name)
        .map(|decl| material_row_value(&document, decl));
    Ok(json!({
        "name": name,
        "created": created,
        "count": document.materials.len(),
        "material": material,
    }))
}
