//! 引擎统计 / 截图 / 引擎信息（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::*;

/// P0-4：一帧的「数字画像」——帧时间分位 + 渲染统计 + 场景规模。
///
/// AI 改完场景后先看这里：`draw_calls` / `instances` / `shadow_draws` 有没有按预期
/// 涨跌，`ssao.passes`、`shadow_cascades` 有没有真的打开，比肉眼看画面更可靠。
/// 渲染器缺席时 `render` 直接给 `null`（`renderer: false`），绝不填零冒充正常。
pub(super) fn engine_stats(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let (scene_name, objects, visible, mesh_kinds, material_overrides, physics_bodies) = {
        let scene = api.scene.borrow();
        let mut mesh_kinds = std::collections::HashSet::new();
        let mut visible = 0usize;
        let mut material_overrides = 0usize;
        let mut physics_bodies = 0usize;
        for node in scene.iter() {
            mesh_kinds.insert(node.mesh);
            visible += usize::from(node.visible);
            material_overrides += usize::from(node.material.is_some());
            physics_bodies += usize::from(node.physics.is_some());
        }
        (
            scene.name.clone(),
            scene.len(),
            visible,
            mesh_kinds.len(),
            material_overrides,
            physics_bodies,
        )
    };

    let frame = api.frame_ms;
    let (size, stats) = match api.renderer.as_deref() {
        Some(renderer) => (renderer.size(), Some(renderer.stats())),
        None => ((0u32, 0u32), None),
    };
    let render = match stats {
        Some(s) => json!({
            "draw_calls": s.draw_calls,
            "instances": s.instances,
            "culled": s.culled,
            "cull_unknown": s.cull_unknown,
            "shadow_draws": s.shadow_draws,
            "shadow_cascades": s.shadow_cascades,
            // 扩展二十四：按级裁剪遮挡体的判定量与裁掉量。`culled` 为 0 且
            // `tested` 为 0 = 裁剪没开（或阴影关闭），此时 `shadow_draws` 会回到
            // 扩展二十三的「批次数 × 级数」。
            "shadow_casters": s.shadow_casters,
            "shadow_culled": s.shadow_culled,
            // 扩展二十五：层级剔除。`shadow_pruned` 是整枝跳过省下的实例判定数
            // （与 `shadow_casters` 相加才是逐实例时代的总量），
            // `shadow_box_tests` 是被下探的树节点数 × 级数，
            // `caster_tree_builds` 稳态不变 = 层级树缓存生效。
            "shadow_pruned": s.shadow_pruned,
            "shadow_branch_pruned": s.shadow_branch_pruned,
            "shadow_box_tests": s.shadow_box_tests,
            "caster_tree_builds": s.caster_tree_builds,
            // 扩展二十九：主通道视锥的层级剔除。`pruned` 是「没做实例判定就被
            // 整枝剪掉」的实例数，`instance_tests + pruned` 应等于可见节点数；
            // 只有 `branch_pruned` 是相对逐实例判定的净收益。
            "cull": {
                "box_tests": s.cull.box_tests,
                "instance_tests": s.cull.instance_tests,
                "pruned": s.cull.pruned,
                "branch_pruned": s.cull.branch_pruned,
                "tree_builds": s.cull.tree_builds,
            },
            // 扩展十九：半透明批次与 MSAA。`transparent_draws` 为 0 = 本帧没有
            // 半透明物件；`depth_prepass_draws` 应恒等于不透明批次的 draw call 数，
            // 掉成 0 说明遮蔽通道的单采样深度来源断了。
            "transparent_draws": s.transparent_draws,
            "transparent_instances": s.transparent_instances,
            "depth_prepass_draws": s.depth_prepass_draws,
            // 扩展二十二：遮挡剔除。`enabled` 是「真的在判」（开关开 + 金字塔就绪），
            // `occluded` 是本帧因此少画的实例数，`readback` 表示本帧刚取回一份新深度。
            // 扩展三十：`hierarchy` 为 true 时走的是按子树预筛（`box_tests` /
            // `pruned` / `branch_pruned` 才有意义），`tested + pruned` = 过视锥的
            // 不透明实例数；`hierarchy` 为 false 时退回逐实例，`tested` 就是全部。
            "occlusion": {
                "enabled": s.occlusion.enabled,
                "tested": s.occlusion.tested,
                "occluded": s.occlusion.occluded,
                "readback": s.occlusion.readback,
                "levels": s.occlusion.levels,
                "hierarchy": s.occlusion.hierarchy,
                "box_tests": s.occlusion.box_tests,
                "pruned": s.occlusion.pruned,
                "branch_pruned": s.occlusion.branch_pruned,
            },
            "msaa_samples": r3e_render::MSAA_SAMPLE_COUNT,
            // 扩展三十一：实例缓冲的脏段上传。`recorded + skipped` = 本帧实例
            // 记录总数；`bytes` 是真正传上 GPU 的字节，`full_bytes` 是全量上传
            // 本该写的字节，相除就是本帧省下的比例。`full` 在首帧、扩容换代、
            // 实例数变化或脏段碎到超限时为 true；`resets` 累计增长说明容量估算失效。
            "instance_upload": {
                "ranges": s.instance_upload.ranges,
                "recorded": s.instance_upload.recorded,
                "skipped": s.instance_upload.skipped,
                "bytes": s.instance_upload.bytes,
                "full_bytes": s.instance_upload.full_bytes,
                "full": s.instance_upload.full,
                "resets": s.instance_upload.resets,
            },
            // 扩展三十二：关节矩阵的脏段上传。口径同上，记录换成 mat4（64 B）；
            // 关节缓冲容量固定不换代，`resets` 恒为 0。
            "joint_upload": {
                "ranges": s.joint_upload.ranges,
                "recorded": s.joint_upload.recorded,
                "skipped": s.joint_upload.skipped,
                "bytes": s.joint_upload.bytes,
                "full_bytes": s.joint_upload.full_bytes,
                "full": s.joint_upload.full,
                "resets": s.joint_upload.resets,
            },
            // 扩展三十五：暂存环（持久映射槽 + 多帧飞行）。`writes` 是队列级
            // write_buffer 次数，`saves` 是被合掉的次数，`copies` 是块拷贝条数。
            "upload_staging": {
                "enabled": s.upload_staging.enabled,
                "slots": s.upload_staging.slots,
                "capacity": s.upload_staging.capacity,
                "frames": s.upload_staging.frames,
                "writes": s.upload_staging.writes,
                "copies": s.upload_staging.copies,
                "bytes": s.upload_staging.bytes,
                "saves": s.upload_staging.saves,
                "rebuilds": s.upload_staging.rebuilds,
                "fallbacks": s.upload_staging.fallbacks,
            },
            "render_frame_index": s.frame_index,
            "ssao": {
                "enabled": s.ssao.enabled,
                "passes": s.ssao.passes,
                "samples": s.ssao.samples,
                "width": s.ssao.size.0,
                "height": s.ssao.size.1,
            },
            "postfx": {
                "bloom_passes": s.postfx.bloom_passes,
                "composite_passes": s.postfx.composite_passes,
                "bloom_levels": s.postfx.bloom_levels,
            },
            "local_lights": s.local_lights,
            "local_shadow_casters": s.local_shadow_casters,
            "local_shadow_layers": s.local_shadow_layers,
            "text_glyphs": s.text_glyphs,
            "text_draws": s.text_draws,
            "text_missing": s.text_missing,
            "ui_quads": s.ui_quads,
            "ui_draws": s.ui_draws,
        }),
        None => Value::Null,
    };

    Ok(json!({
        "renderer": stats.is_some(),
        "frame": {
            "index": api.frames,
            "fps": api.fps,
            "uptime_ms": api.uptime_ms,
            "paused": api.paused,
            "frame_ms": {
                "samples": frame.samples,
                "last": frame.last_ms,
                "p50": frame.p50_ms,
                "p95": frame.p95_ms,
                "p99": frame.p99_ms,
                "max": frame.max_ms,
            },
        },
        "scene": {
            "name": scene_name,
            "objects": objects,
            "visible": visible,
            "mesh_kinds": mesh_kinds,
            "material_overrides": material_overrides,
            "physics_bodies": physics_bodies,
        },
        "render": render,
        "size": { "width": size.0, "height": size.1 },
    }))
}

/// P0-4：把当前一帧渲染到离屏纹理并落成 PNG，返回绝对路径。
///
/// 这是「看画面」的唯一入口：AI 拿到路径后可以直接看图，不必依赖窗口。
/// 落盘后回读文件长度做一次自检——空文件当失败上报，避免下游拿到一张坏图。
pub(super) fn capture_frame(
    api: &mut LocalEngineApi<'_>,
    path: Option<&str>,
) -> Result<Value, RpcFailure> {
    let Some(renderer) = api.renderer.as_deref_mut() else {
        return Err(RpcFailure::engine(
            "渲染器未初始化（窗口 / GPU 未就绪），当前进程无法截图",
        ));
    };
    let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(raw) => absolute_path(Path::new(raw)).map_err(RpcFailure::engine)?,
        None => api.capture_dir.join(default_capture_name()),
    };
    // 落盘链路与 `--capture-frame` 共用 runtime 里的那一份：回归基线图与
    // 编辑器截的图必须是同一口径（同一段离屏渲染 + 空文件自检），否则
    // 「基准」和「实际」拍的根本不是一件事。
    let info = {
        let scene = api.scene.borrow();
        capture_to_file(renderer, &scene, &target).map_err(RpcFailure::engine)?
    };
    Ok(json!({
        "path": info.path.display().to_string(),
        "width": info.width,
        "height": info.height,
        "bytes": info.bytes,
        "pixels": info.pixel_bytes(),
        "format": info.format,
    }))
}

pub(super) fn info(api: &mut LocalEngineApi<'_>) -> EngineInfo {
    let (scene_name, nodes) = {
        let scene = api.scene.borrow();
        (scene.name.clone(), scene.len())
    };
    let physics = api.physics.borrow();
    EngineInfo {
        name: ENGINE_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: PROTOCOL_VERSION,
        scene: if api.scene_label.is_empty() {
            scene_name
        } else {
            api.scene_label.clone()
        },
        script: api.script_label.clone(),
        frames: api.frames,
        fps: api.fps,
        uptime_ms: api.uptime_ms,
        paused: api.paused,
        nodes,
        physics: PhysicsInfo {
            enabled: physics.enabled(),
            backend: physics.backend_name().to_string(),
            capabilities: physics
                .capabilities()
                .enabled_keys()
                .into_iter()
                .map(str::to_string)
                .collect(),
            bodies: physics.body_count(),
            ccd_default: physics.ccd_default(),
        },
    }
}
