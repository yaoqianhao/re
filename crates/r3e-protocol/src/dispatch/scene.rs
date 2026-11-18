//! `scene.*` / `anim.*` 臂。
//!
//! 命中不到本组时交给 [`super::media::dispatch_media`]，不在本文件回错误。

use super::*;

pub(super) fn dispatch_scene(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "scene.list" => {
            let filter = param(params, "filter").and_then(Value::as_str);
            let limit = match param(params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.scene_list(filter, limit) {
                Ok(nodes) => Response::ok(id, json!({ "count": nodes.len(), "nodes": nodes })),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.get" => match with_node(params, |node| api.scene_get(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.settransform" => {
            let patch = match (
                opt_f32_array::<3>(params, "position"),
                opt_f32_array::<4>(params, "rotation"),
                opt_f32_array::<3>(params, "scale"),
            ) {
                (Ok(p), Ok(r), Ok(s)) => TransformPatch {
                    position: p,
                    rotation: r,
                    scale: s,
                },
                (Err(f), _, _) | (_, Err(f), _) | (_, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "position / rotation / scale 至少要给一个（否则这次调用没有意义）",
                    ),
                );
            }
            match with_node(params, |node| api.scene_set_transform(node, patch)) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.spawn" => {
            let object = param(params, "object")
                .or_else(|| param(params, "desc"))
                .cloned()
                .unwrap_or(Value::Null);
            if !object.is_object() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params("缺少对象参数 `object`（物件描述）"),
                );
            }
            match api.scene_spawn(object) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.remove" => match with_node(params, |node| api.scene_remove(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        // 材质契约：metallic / roughness / emissive 三个字段可分别给，
        // 一个都不给视为无意义调用；具体收敛由宿主材质结构完成。
        "scene.setmaterial" => {
            let patch = match (
                opt_f32(params, "metallic"),
                opt_f32(params, "roughness"),
                opt_f32_array::<3>(params, "emissive"),
            ) {
                (Ok(m), Ok(r), Ok(e)) => MaterialPatch {
                    metallic: m,
                    roughness: r,
                    emissive: e,
                },
                (Err(f), _, _) | (_, Err(f), _) | (_, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "metallic / roughness / emissive 至少要给一个（否则这次调用没有意义）",
                    ),
                );
            }
            match with_node(params, |node| api.scene_set_material(node, patch)) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 环境探针契约：字段可分别给，一个都不给视为无意义调用；
        // `enabled = false` 是唯一的「清空」入口，与「什么都没给」区分开。
        "scene.setenvironment" => {
            let patch = match (
                opt_bool(params, "enabled"),
                opt_f32_array::<3>(params, "sky_zenith"),
                opt_f32_array::<3>(params, "sky_horizon"),
                opt_f32_array::<3>(params, "ground"),
                opt_f32_array::<3>(params, "sun_dir"),
                opt_f32_array::<3>(params, "sun_color"),
                opt_f32(params, "sun_intensity"),
                opt_f32(params, "intensity"),
            ) {
                (
                    Ok(enabled),
                    Ok(zenith),
                    Ok(horizon),
                    Ok(ground),
                    Ok(dir),
                    Ok(sun),
                    Ok(si),
                    Ok(i),
                ) => EnvironmentPatch {
                    enabled,
                    sky_zenith: zenith,
                    sky_horizon: horizon,
                    ground,
                    sun_dir: dir,
                    sun_color: sun,
                    sun_intensity: si,
                    intensity: i,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), _)
                | (_, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "environment 补丁至少要给一个字段（或 enabled=false 清空）",
                    ),
                );
            }
            match api.scene_set_environment(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 反射探针契约（扩展二十三）：字段可分别给，一个都不给视为无意义调用。
        // 与 set_environment 的区别是「两条独立的线」：这里只动 environment_map 段，
        // 解析天空 / SH 系数一律不碰；`enabled = false` 只翻开关、保留路径。
        // 路径解析（相对场景目录 → 绝对路径）与文件读取都在宿主侧完成，本层只搬字符串。
        "scene.setenvironmentmap" => {
            let patch = match (
                opt_bool(params, "enabled"),
                opt_str(params, "equirect"),
                opt_str_list(params, "faces"),
                opt_i32(params, "face_size"),
                opt_i32(params, "mip_levels"),
                opt_f32(params, "intensity"),
            ) {
                (Ok(enabled), Ok(equirect), Ok(faces), Ok(face_size), Ok(mips), Ok(intensity)) => {
                    EnvironmentMapPatch {
                        enabled,
                        equirect: equirect.map(str::to_string),
                        faces,
                        face_size,
                        mip_levels: mips,
                        intensity,
                    }
                }
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "environment_map 补丁至少要给一个字段（或 enabled=false 临时关闭）",
                    ),
                );
            }
            match api.scene_set_environment_map(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 后处理契约（扩展十三）：字段可分别给，一个都不给视为无意义调用；
        // `enabled = false` 是唯一的「清空」入口。`tonemap` 名字的合法性由宿主判定。
        "scene.setpostfx" => {
            let patch = match (
                opt_bool(params, "enabled"),
                opt_f32(params, "exposure"),
                opt_str(params, "tonemap"),
                opt_bool(params, "bloom_enabled"),
                opt_f32(params, "bloom_threshold"),
                opt_f32(params, "bloom_knee"),
                opt_f32(params, "bloom_intensity"),
                opt_bool(params, "fxaa"),
            ) {
                (
                    Ok(enabled),
                    Ok(exposure),
                    Ok(tonemap),
                    Ok(bloom_enabled),
                    Ok(bloom_threshold),
                    Ok(bloom_knee),
                    Ok(bloom_intensity),
                    Ok(fxaa),
                ) => PostFxPatch {
                    enabled,
                    exposure,
                    tonemap: tonemap.map(str::to_string),
                    bloom_enabled,
                    bloom_threshold,
                    bloom_knee,
                    bloom_intensity,
                    fxaa,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), _)
                | (_, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "postfx 补丁至少要给一个字段（或 enabled=false 清空）",
                    ),
                );
            }
            match api.scene_set_postfx(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 环境光遮蔽契约（扩展十七）：字段可分别给，一个都不给视为无意义调用；
        // 与 set_postfx 不同，`enabled = false` 只关遮蔽，后处理段其余设置保留。
        "scene.setssao" => {
            // 显式 `null` 一律视为「没给」：不少 JSON 客户端会把未填字段序列化成 `null`。
            let enabled = match param(params, "enabled") {
                None | Some(Value::Null) => Ok(None),
                Some(_) => opt_bool(params, "enabled"),
            };
            let patch = match (
                enabled,
                opt_num(params, "radius"),
                opt_num(params, "bias"),
                opt_num(params, "intensity"),
                opt_i32(params, "samples"),
            ) {
                (Ok(enabled), Ok(radius), Ok(bias), Ok(intensity), Ok(samples)) => SsaoPatch {
                    enabled,
                    radius,
                    bias,
                    intensity,
                    samples,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(id, RpcFailure::invalid_params("ssao 补丁至少要给一个字段"));
            }
            match api.scene_set_ssao(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 动画混合树契约（扩展十八 A）：`scene.anim_layers` 只读，`scene.anim_layer_set`
        // 补丁式写入。模式名 / 遮罩串原样透传，认不认识由宿主的 `LayerMode::parse`
        // 与遮罩解析决定——本层同样不认识 `r3e_core` 的任何类型。
        "scene.animlayers" => {
            // `opt_str` 已把显式 null 当「未给」：编辑器面板常把「没选实例」序列化成 null。
            let instance = match opt_str(params, "instance") {
                Ok(v) => v.map(|s| s.to_string()),
                Err(f) => return Response::fail(id, f),
            };
            let limit = match param(params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.scene_anim_layers(instance.as_deref(), limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.animlayerset" => {
            let instance = match require_str(params, "instance") {
                Ok(s) if !s.trim().is_empty() => s.to_string(),
                Ok(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `instance` 不能为空"),
                    )
                }
                Err(f) => return Response::fail(id, f),
            };
            // `layer` 缺省 = 第一层，与脚本侧「空名取第一层」保持一致。
            let layer = match opt_str(params, "layer") {
                Ok(v) => v.unwrap_or_default().to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let patch = match (
                opt_num(params, "weight"),
                opt_str(params, "mode"),
                opt_str(params, "mask"),
                opt_num(params, "speed"),
                opt_str(params, "clip"),
                opt_str(params, "sync"),
                opt_str(params, "sync_source"),
                opt_num(params, "clip_offset"),
                opt_num(params, "clip_speed"),
                opt_bool(params, "clip_looping"),
            ) {
                (
                    Ok(weight),
                    Ok(mode),
                    Ok(mask),
                    Ok(speed),
                    Ok(clip),
                    Ok(sync),
                    Ok(sync_source),
                    Ok(clip_offset),
                    Ok(clip_speed),
                    Ok(clip_looping),
                ) => AnimLayerPatch {
                    instance,
                    layer,
                    weight,
                    mode: mode.map(|s| s.to_string()),
                    mask: mask.map(|s| s.to_string()),
                    speed,
                    clip: clip.map(|s| s.to_string()),
                    sync: sync.map(|s| s.to_string()),
                    sync_source: sync_source.map(|s| s.to_string()),
                    clip_offset,
                    clip_speed,
                    clip_looping,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params("anim_layer_set 补丁至少要给一个字段"),
                );
            }
            match api.scene_anim_layer_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.save" => {
            let path = match param(params, "path") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `path` 应为非空字符串（可省略）"),
                    )
                }
            };
            match api.scene_save(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.shadow" => {
            let patch = match shadow_patch_from(params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_shadow(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 场景栈与存档契约（扩展十八 B）：只做参数校验，压栈 / 换资产库 / 重跑脚本
        // 的顺序全在宿主侧——传输层不认识 Scene，也不该知道"换场景要先换资产"。
        "scene.stack" => {
            let limit = match opt_limit(params) {
                Ok(l) => l,
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_stack(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.switch" => {
            let path = match require_str(params, "path") {
                Ok(s) if !s.trim().is_empty() => s,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `path` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_switch(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.back" => match api.scene_back() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.archive.save" => {
            let path = match param(params, "path") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `path` 应为非空字符串（可省略）"),
                    )
                }
            };
            match api.scene_archive_save(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.archive.load" => {
            let path = match require_str(params, "path") {
                Ok(s) if !s.trim().is_empty() => s,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `path` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_archive_load(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 材质表（M1-S4-D）：`name` 是主键，读写都以「行」为单位。
        // 与上面的 `scene.setMaterial`（节点内联材质补丁）是两件事：那个改某个节点
        // 自己的覆盖值，这两条改全场景共享的材质表。
        "scene.material.list" => match api.material_list() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.material.upsert" => {
            let decl = match material_upsert_from(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.material_upsert(decl) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        _ => media::dispatch_media(api, id, params, method, raw),
    }
}

/// 从 RPC 参数解析一份材质表行（[`MaterialUpsert`]，**整行声明而非补丁**）。
///
/// 没给的字段一律 `None`，由宿主取缺省——也就是说「没给法线贴图」的意思是
/// 「这一行没有法线贴图」，不是「别动原来的法线贴图」。想让现有贴图留下，就把
/// `scene.material.list` 读到的值原样传回来。
fn material_upsert_from(params: &Value) -> Result<MaterialUpsert, RpcFailure> {
    let name = match require_str(params, "name") {
        Ok(s) if !s.trim().is_empty() => s.to_string(),
        Ok(_) => return Err(RpcFailure::invalid_params("参数 `name` 不能为空")),
        Err(f) => return Err(f),
    };
    Ok(MaterialUpsert {
        name,
        metallic: opt_num(params, "metallic")?,
        roughness: opt_num(params, "roughness")?,
        emissive: opt_vec3(params, "emissive")?,
        albedo: opt_str(params, "albedo")?.map(str::to_string),
        normal: opt_str(params, "normal")?.map(str::to_string),
        metallic_roughness: opt_str(params, "metallicRoughness")?.map(str::to_string),
        occlusion: opt_str(params, "occlusion")?.map(str::to_string),
        emissive_map: opt_str(params, "emissiveMap")?.map(str::to_string),
        uv_scale: opt_f32_array::<2>(params, "uvScale")?,
    })
}
