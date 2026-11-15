//! 宿主层内部工具函数：JSON 快照、路径解析、补丁应用与错误构造。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，签名与实现未改动。

use super::*;

/// 后处理快照：字段名与场景 JSON 的 `postfx` 段完全一致，前端拿到即可回写。
pub(super) fn postfx_value(fx: PostFxSettings) -> Value {
    json!({
        "exposure": fx.exposure,
        "tonemap": fx.tonemap.name(),
        "bloom": {
            "enabled": fx.bloom.enabled,
            "threshold": fx.bloom.threshold,
            "knee": fx.bloom.knee,
            "intensity": fx.bloom.intensity,
        },
        "fxaa": fx.fxaa,
        "ssao": ssao_value(fx.ssao),
    })
}

/// 遮蔽快照：字段名与场景 JSON 的 `postfx.ssao` 段一致，前端拿到即可回写。
/// `neutral` 是渲染侧真正用的判据（关闭或强度为 0 时为 true，此时整帧跳过两个 pass），
/// 单看 `enabled` 会把「开了但强度 0」误报成在算遮蔽。
pub(super) fn ssao_value(s: SsaoSettings) -> Value {
    json!({
        "enabled": s.enabled,
        "radius": s.radius,
        "bias": s.bias,
        "intensity": s.intensity,
        "samples": s.samples,
        "neutral": s.is_neutral(),
    })
}

/// 先按名字找，再按 `NodeId` 数字找：编辑器两种句柄都能用。
pub(super) fn find_node<'s>(scene: &'s Scene, key: &str) -> Result<&'s Node, RpcFailure> {
    if let Some(node) = scene.find(key) {
        return Ok(node);
    }
    if let Ok(id) = key.trim().parse::<u64>() {
        if let Some(node) = scene.get(id) {
            return Ok(node);
        }
    }
    Err(RpcFailure::not_found(format!("物件 `{key}` 不存在")))
}

pub(super) fn find_node_id(scene: &Scene, key: &str) -> Result<r3e_core::NodeId, RpcFailure> {
    if let Some(id) = scene.find_id(key) {
        return Ok(id);
    }
    if let Ok(id) = key.trim().parse::<u64>() {
        if scene.get(id).is_some() {
            return Ok(id);
        }
    }
    Err(RpcFailure::not_found(format!("物件 `{key}` 不存在")))
}

/// 单个物件的完整快照（`scene.get` 的结果）。
pub(super) fn node_value(node: &Node) -> Value {
    let t = &node.transform;
    json!({
        "id": node.id,
        "name": node.name,
        "mesh": node.mesh.as_string(),
        "visible": node.visible,
        "parent": node.parent,
        "children": node.children,
        "position": t.position.to_array(),
        "rotation": t.rotation.to_array(),
        "scale": t.scale.to_array(),
        "euler_deg": t.euler_deg().to_array(),
        "color": node.color.to_array(),
        "material": node.material.map(material_value),
        "world": node.world.to_cols_array(),
    })
}

/// 材质快照（读回入口）：未显式设置材质的物件给出 `null`，
/// 显式设置过的给出收敛后的三件套。
/// 环境探针快照：字段名与场景 JSON 的 `environment` 段完全一致，前端拿到即可回写。
pub(super) fn environment_value(env: r3e_core::env::EnvironmentSettings) -> Value {
    json!({
        "sky_zenith": env.sky_zenith,
        "sky_horizon": env.sky_horizon,
        "ground": env.ground,
        "sun_dir": env.sun_dir,
        "sun_color": env.sun_color,
        "sun_intensity": env.sun_intensity,
        "intensity": env.intensity,
    })
}

/// 反射探针快照（扩展二十三）：字段名与场景 JSON 的 `environment_map` 段一致，
/// 前端拿到即可回写；`source` 单独给在响应里，不在这个对象内重复。
pub(super) fn environment_map_value(map: r3e_core::envmap::EnvironmentMapSettings) -> Value {
    json!({
        "enabled": map.enabled,
        "equirect": map.equirect,
        "faces": map.faces,
        "face_size": map.face_size,
        "mip_levels": map.mip_levels,
        "intensity": map.intensity,
    })
}

pub(super) fn material_value(material: r3e_core::SurfaceMaterial) -> Value {
    json!({
        "metallic": material.metallic,
        "roughness": material.roughness,
        "emissive": material.emissive,
    })
}

pub(super) fn check_finite<const N: usize>(values: &[f32; N], key: &str) -> Result<(), RpcFailure> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 含 NaN / 无穷大"
        )))
    }
}

/// Rhai 返回值转 JSON：普通标量 / 数组 / Map 走 serde 反序列化，
/// 带自定义类型的返回值退化成字符串，保证 `script.eval` 永远有结果可回。
pub(super) fn dynamic_to_json(value: &rhai::Dynamic) -> Value {
    if value.is_unit() {
        return Value::Null;
    }
    match rhai::serde::from_dynamic::<Value>(value) {
        Ok(v) => v,
        Err(_) => Value::String(value.to_string()),
    }
}

/// 把 Windows 上 `canonicalize` 得到的 `\\?\` 逐字前缀去掉。
///
/// 编辑器客户端拿到 `\\?\D:\a\b.json` 这种路径无法直接展示 / 再拼接，
/// 而 `\\?\UNC\` 前缀则需要还原成 `\\`。
pub(super) fn plain_abs_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

/// 把一条场景命令塞进脚本宿主的命令槽（单槽：一帧一条）。
///
/// 槽被占时如实回 `server busy`，而不是覆盖掉别人那条——脚本刚在 `on_start` 里
/// 申请的切场景如果被编辑器面板冲掉，表现就是"点了没反应"，比报错更难查。
pub(super) fn queue_scene_command(
    scripts: &ScriptHost,
    cmd: SceneCommand,
) -> Result<Value, RpcFailure> {
    let label = cmd.label();
    if !scripts.request_scene_command(cmd) {
        return Err(RpcFailure::new(
            SERVER_BUSY,
            format!("场景命令槽已被占用，`{label}` 未排队，请下一帧重试"),
        ));
    }
    let handle = scripts.scene_stack_handle();
    let depth = handle.borrow().depth();
    Ok(json!({
        "queued": true,
        "command": label,
        "stack": depth,
    }))
}

/// 解析请求里的路径：绝对路径照用；相对路径先按工作目录，再按"当前场景文件所在目录"。
///
/// 后一条是给编辑器用的：面板里填 `lv2.json` 时，用户心里那个"当前目录"是场景目录，
/// 不是进程启动目录。两条都落空时返回原路径（由调用方按"不存在"报错）。
pub(super) fn resolve_request_path(scene_label: &str, path: &str) -> std::path::PathBuf {
    let raw = std::path::PathBuf::from(path);
    if raw.is_absolute() || raw.exists() {
        return raw;
    }
    let beside_scene = std::path::Path::new(scene_label)
        .parent()
        .map(|dir| dir.join(&raw))
        .unwrap_or_else(|| raw.clone());
    if beside_scene.exists() {
        beside_scene
    } else {
        raw
    }
}

/// 把 `UiPatch` 的字段并进一个元素（只覆盖给了的字段）。
///
/// `kind` / `anchor` 已由调用方解析完毕；这里只做搬运与单位解析，不做任何策略判断。
pub(super) fn apply_ui_patch(
    el: &mut UiElement,
    patch: &UiPatch,
    kind: Option<UiKind>,
    anchor: Option<Anchor>,
) {
    if let Some(kind) = kind {
        el.kind = kind;
    }
    if let Some(anchor) = anchor {
        el.anchor = anchor;
    }
    if let Some(text) = patch.x.as_deref() {
        el.offset[0] = parse_ui_value(text);
    }
    if let Some(text) = patch.y.as_deref() {
        el.offset[1] = parse_ui_value(text);
    }
    if let Some(text) = patch.width.as_deref() {
        el.size[0] = parse_ui_value(text);
    }
    if let Some(text) = patch.height.as_deref() {
        el.size[1] = parse_ui_value(text);
    }
    if let Some(color) = patch.color {
        el.color = color;
    }
    if let Some(layer) = patch.layer {
        el.layer = layer;
    }
    if let Some(visible) = patch.visible {
        el.visible = visible;
    }
    if let Some(width) = patch.border_width {
        el.border_width = width;
    }
    if let Some(image) = patch.image.as_deref() {
        el.image = image.to_string();
    }
}

/// 单位串 → `UiValue`：`"12"` / `"12px"` 为像素，`"50%"` 为屏幕百分比。
///
/// 解析失败折成 0 与脚本层同一口径——RPC 调用方拿到的是「渲染结果」而非异常，
/// 与 `scene.set_material` 对非法数值的处理保持一致。
pub(super) fn parse_ui_value(text: &str) -> UiValue {
    ui_text::parse_ui_value(text)
}

/// `UiValue` → 单位串（`"12"` / `"50%"`），供 RPC 回读。
/// 唯一实现在 [`r3e_core::ui_text::format_ui_value`]（M0-S2 收口）。
pub(super) fn fmt_ui_value(value: UiValue) -> String {
    ui_text::format_ui_value(value)
}

/// 单个 UI 元素的 RPC 表示（声明层，未折算像素）。
/// RPC 回包用的「一套阴影设置」JSON 形态。
///
/// 字段名沿用场景文件里的驼峰写法（`orthoSize`），`cascades` / `maxCascades` 是
/// 扩展十六新增项；`maxCascades` 让编辑器不必自己硬编码上限就能画对滑块范围。
pub(super) fn shadow_value(shadow: &ShadowSettings) -> Value {
    json!({
        "enabled": shadow.enabled,
        "size": shadow.size,
        "strength": shadow.strength,
        "orthoSize": shadow.ortho_size,
        "bias": shadow.bias,
        "normalBias": shadow.normal_bias,
        "cascades": shadow.cascade.count,
        "maxCascades": MAX_CASCADES,
        "blend": shadow.cascade.blend,
        "distance": shadow.cascade.max_distance,
        "lambda": shadow.cascade.lambda,
    })
}

/// 层不存在时的统一报错（`layer` 给空串表示第一层）。
pub(super) fn layer_missing(instance: &str, layer: &str) -> RpcFailure {
    let which = if layer.trim().is_empty() {
        "（空名 = 第一层）".to_string()
    } else {
        format!("`{layer}`")
    };
    RpcFailure::invalid_params(format!("实例 `{instance}` 上没有名为 {which} 的层"))
}

/// 遮罩字符串 → [`BoneMask`]，规则与脚本侧 `parse_bone_mask` 完全一致：
/// `"arm,hand"` 只作用这几处，`"!root"` 除 root 外全部，空串 / `"*"` / 只有分隔符
/// 表示整副骨架——归一成 `pass_all` 而不是空遮罩，空遮罩会让整层都不出效果。
pub(super) fn bone_mask_of(spec: &str) -> BoneMask {
    let trimmed = spec.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return BoneMask::pass_all();
    }
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    for part in trimmed.split(',') {
        let rule = part.trim();
        if rule.is_empty() {
            continue;
        }
        match rule.strip_prefix('!') {
            Some(rest) => {
                let name = rest.trim();
                // 只有 `!` 没有名字的规则直接丢掉，不造一条空排除规则。
                if !name.is_empty() {
                    exclude.push(name.to_string());
                }
            }
            None => include.push(rule.to_string()),
        }
    }
    if include.is_empty() && exclude.is_empty() {
        BoneMask::pass_all()
    } else {
        BoneMask { include, exclude }
    }
}

/// 把一层混合树序列化成编辑器契约的 JSON。
///
/// `weight` 是层自己的目标权重，`effective_weight` 才含淡入淡出的当前值——
/// 编辑器画「目标 vs 实际」两条线时两个都要。
pub(super) fn anim_layer_value(instance: &str, layer: &AnimLayer) -> Value {
    let mask = if layer.mask.is_pass_all() {
        "*".to_string()
    } else {
        let mut parts = layer.mask.include.clone();
        parts.extend(layer.mask.exclude.iter().map(|rule| format!("!{rule}")));
        parts.join(",")
    };
    let weights = layer.normalized_weights();
    let clips: Vec<Value> = layer
        .clips
        .iter()
        .zip(weights)
        .map(|(lc, w)| {
            json!({
                "name": lc.clip.name,
                "base_weight": lc.weight,
                "weight": w,
                // 扩展四十七：这一路的相位偏移（没设过是 0，编辑器画错步标尺用）
                "phase_offset": lc.phase_offset,
                // 扩展四十八：这一路自带的播放参数（没设过为 null，编辑器据此显示「跟层走」）
                "clip_speed": lc.playback.map(|pb| pb.speed),
                "clip_looping": lc.playback.map(|pb| pb.looping),
            })
        })
        .collect();
    json!({
        "instance": instance,
        "layer": layer.name,
        "mode": layer.mode.as_str(),
        "weight": layer.weight,
        "effective_weight": layer.effective_weight(),
        "fading": layer.fade.is_some(),
        "mask": mask,
        "mask_include": layer.mask.include.clone(),
        "mask_exclude": layer.mask.exclude.clone(),
        "clips": clips,
        "time": layer.sample_time(),
        "duration": layer.duration(),
        "speed": layer.speed,
        "looping": layer.looping,
        "playing": layer.playing,
        // 扩展四十六：采样策略 + 当前相位（`phase` 不受 `off` 影响，始终可读，
        // 编辑器画相位标尺时不用管策略开关）。
        "sync": layer.sync.as_str(),
        "phase": layer.phase(),
        // 扩展四十七：跨层跟随的源层名（没设是空串；成环/解析不到的情况在核心层就被挡掉了）
        "sync_source": layer.sync_source.clone().unwrap_or_default(),
    })
}

/// RPC / 脚本共用的「一盏灯」JSON 形态。
///
/// 字段与 `LocalLight` 一一对应，`kind` 用规范名而不是数字码——编辑器面板要能
/// 直接显示 `spot`，也不想为了一个枚举在两侧同步一份常量表。
pub(super) fn local_light_value(light: &LocalLight) -> Value {
    json!({
        "key": light.key,
        "kind": light.kind.name(),
        "pos": light.pos,
        "dir": light.dir,
        "color": light.color,
        "intensity": light.intensity,
        "range": light.range,
        "inner_deg": light.inner_deg,
        "outer_deg": light.outer_deg,
        "cast_shadow": light.cast_shadow,
        "shadow_bias": light.shadow_bias,
    })
}

/// 一个声源的完整快照：声明态（编辑器改的）+ 结算态（上一帧算出来的）。
///
/// 结算字段缺失时用 `null`，不伪造 `1.0`——「这一帧没算过」和「算出来正好是 1」
/// 对排查未注册 / 静默后端这两类问题完全是两回事。
pub(super) fn spatial_source_value(audio: &Audio, key: &str) -> Value {
    let settings = audio.spatial_settings(key);
    json!({
        "key": key,
        "position": audio.source_position(key),
        "node": audio.spatial_node(key),
        "distance": audio.spatial_distance(key),
        "gain": settings.map(|_| audio.spatial_gain(key)),
        "rate": settings.map(|_| audio.spatial_rate(key)),
        "min_distance": settings.map(|s| s.min_distance),
        "max_distance": settings.map(|s| s.max_distance),
        "model": settings.map(|s| s.model.name()),
        "doppler": settings.map(|s| s.doppler_factor),
        "speed_of_sound": settings.map(|s| s.speed_of_sound),
    })
}

pub(super) fn ui_element_value(el: &UiElement) -> Value {
    json!({
        "key": el.key,
        "kind": el.kind.name(),
        "anchor": el.anchor.name(),
        "x": fmt_ui_value(el.offset[0]),
        "y": fmt_ui_value(el.offset[1]),
        "width": fmt_ui_value(el.size[0]),
        "height": fmt_ui_value(el.size[1]),
        "color": el.color,
        "border_width": el.border_width,
        "image": el.image,
        "layer": el.layer,
        "visible": el.visible,
    })
}
