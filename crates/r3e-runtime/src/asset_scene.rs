//! 场景侧的资产装配（从 `app.rs` 拆出）：
//! `spawn_asset_nodes` / 形变绑定 / 蒙皮绑定 / 动画实例登记。
//!
//! 这里的函数都接收 `&mut Scene`，是 S3 doc 改造后的**受控出口**之一——
//! 调用方必须处于 `Document::write` 闭包内，`cargo xtask doc-gate` 规则 2 白名单已登记。

use std::path::Path;
use std::sync::Arc;

use r3e_asset::AssetLibrary;
use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::{MeshRef, NodeId, ObjectDesc, Scene, SkinInstance};

use crate::app::AssetSpec;

pub(crate) fn spawn_asset_nodes(
    scene: &mut Scene,
    library: &AssetLibrary,
    specs: &[AssetSpec],
    out: &mut Vec<NodeId>,
    show_bones: bool,
) -> usize {
    let mut spawned = 0usize;
    for spec in specs {
        if !spec.spawn {
            continue;
        }
        let path_str = spec.path.display().to_string();
        let id = r3e_core::AssetId::from_path(&path_str);
        let Some(entry) = library.get(id) else {
            continue;
        };
        // 网格节点按资产内 mesh 下标对齐（P4 蒙皮要拿它当被驱动对象）
        let mut mesh_nodes: Vec<NodeId> = Vec::new();
        for (index, mesh) in entry.meshes.iter().enumerate() {
            let base = if mesh.node_path.trim().is_empty() {
                if mesh.name.trim().is_empty() {
                    format!("asset_{index}")
                } else {
                    mesh.name.clone()
                }
            } else {
                mesh.node_path.clone()
            };

            let mut name = base.clone();
            let mut suffix = 0u32;
            while scene.find_id(&name).is_some() {
                suffix += 1;
                name = format!("{base}#{suffix}");
            }

            // P2 起颜色由材质 bind group 负责（贴图 + 材质因子），节点色保持白色，
            // 否则会和材质 base_color 相乘造成二次压暗。
            let color = [1.0, 1.0, 1.0, 1.0];

            let desc = ObjectDesc {
                name: name.clone(),
                mesh: Some(format!("asset:{}#{}", path_str, index)),
                position: spec.position,
                rotation: spec.rotation,
                scale: spec.scale,
                color: Some(color),
                parent: None,
                visible: Some(true),
                physics: None,
                material: None,
            };
            match scene.spawn(desc) {
                Ok(node_id) => {
                    out.push(node_id);
                    mesh_nodes.push(node_id);
                    spawned += 1;
                }
                Err(e) => log::warn!("资产节点 `{name}` spawn 失败: {e}"),
            }
        }

        // 有节点树时建骨架（动画片段顺带登记）；纯静态资产不额外造节点
        if !entry.nodes.is_empty() && (!entry.animations.is_empty() || !entry.skins.is_empty()) {
            let skel = spawn_asset_anim(scene, entry, spec, &path_str, show_bones);

            // P4：骨架就位后立刻以「静止姿态」绑定蒙皮 —— 逆绑定矩阵必须取这一刻的
            // 世界矩阵，任何动画推进之后再绑都会把当前姿态误当绑定姿态。
            if let Some(skel) = skel {
                scene.update_world_transforms();
                bind_asset_skins(scene, entry, &path_str, &skel, &mesh_nodes);
            }
        }
    }
    spawned
}

/// 扩展三十六：给「网格自带形变形状」的资产物件补登记形变绑定。
///
/// 形状数是网格的属性（glTF `primitive.targets` 的个数），脚本无法可靠地自己数 ——
/// 让用户手写 `morph_bind(name, 2)` 早晚会写错并静默形变失败。所以场景就位后在这里
/// 按网格数据统一登记，脚本只剩「调权重」一件事。已绑定的节点不重绑（热重载补挂会
/// 重复进来），权重初值全 0，与没有形变能力时像素逐位相同。
///
/// 返回新登记的数量，供调用方决定要不要打日志。
pub(crate) fn attach_morph_bindings(scene: &mut Scene, library: &r3e_asset::AssetLibrary) -> usize {
    let bound = attach_morph_bindings_with(scene, |mesh| match mesh {
        MeshRef::Asset { asset, mesh: index } => library
            .get(asset)
            .and_then(|entry| entry.meshes.get(index as usize))
            .map(|mesh| mesh.morph.len())
            .filter(|count| *count > 0),
        // 内置图元不带形变形状；用通配分支保持「只认资产网格」这一条路径
        _ => None,
    });
    if bound > 0 {
        for node in scene.iter() {
            if let Some(morph) = scene.morph_of(node.id) {
                if morph.weights.iter().all(|w| *w == 0.0) && morph.target_count > 0 {
                    log::info!(
                        "形变 `{}`：绑 {} 个形状，权重初值全 0（脚本用 morph_set 调）",
                        node.name,
                        morph.target_count
                    );
                }
            }
        }
    }
    bound
}

/// [`attach_morph_bindings`] 的可测内核：形状数由调用方按网格给。
///
/// 与具体资产库解耦后，纯 CPU 就能断言「哪些节点被绑、重复调用不重绑、内置网格
/// 不受影响」——这些是自动绑定最容易写错的地方，不该依赖显卡才能验证。
pub(crate) fn attach_morph_bindings_with(
    scene: &mut Scene,
    morph_count_of: impl Fn(MeshRef) -> Option<usize>,
) -> usize {
    // 先收集再改：`scene.iter()` 与 `scene.add_morph` 不能同时借同一个 `Scene`
    let candidates: Vec<(NodeId, MeshRef)> =
        scene.iter().map(|node| (node.id, node.mesh)).collect();
    let mut bound = 0usize;
    for (id, mesh) in candidates {
        if scene.morph_of(id).is_some() {
            continue;
        }
        let Some(raw) = morph_count_of(mesh) else {
            continue;
        };
        let count = raw.min(r3e_core::MAX_MORPH_TARGETS);
        if count == 0 {
            continue;
        }
        scene.add_morph(r3e_core::MorphInstance::new(id, count));
        bound += 1;
    }
    bound
}

/// 一个资产实例建出来的骨架句柄：节点下标 → 场景节点，供蒙皮按 `node_index` 对齐。
pub(crate) struct SkeletonSpawn {
    prefix: String,
    ids: Vec<Option<NodeId>>,
}

/// 把资产的蒙皮数据绑到网格节点上（P4）。
///
/// 关节按 `node_index` 对齐场景节点（与动画轨道同一套对齐方式），路径只作日志；
/// 绑定姿态取当前世界矩阵，交给 `SkinInstance` 求逆绑定矩阵。
pub(crate) fn bind_asset_skins(
    scene: &mut Scene,
    entry: &r3e_asset::AssetEntry,
    path_str: &str,
    skel: &SkeletonSpawn,
    mesh_nodes: &[NodeId],
) {
    for (mi, mesh) in entry.meshes.iter().enumerate() {
        let Some(skin_index) = mesh.skin else {
            continue;
        };
        let Some(data) = entry.skin(skin_index) else {
            log::warn!("资产 `{path_str}` 的网格 #{mi} 引用了不存在的蒙皮 #{skin_index}");
            continue;
        };
        let Some(&mesh_node) = mesh_nodes.get(mi) else {
            continue;
        };
        let Some(mesh_world) = scene.get(mesh_node).map(|n| n.world) else {
            continue;
        };
        let mut inst = SkinInstance::new(format!("{}/skin#{mi}", skel.prefix), mesh_node);
        for joint in &data.joints {
            let Some(Some(node)) = skel.ids.get(joint.node_index as usize) else {
                continue;
            };
            let Some(n) = scene.get(*node) else {
                continue;
            };
            inst.add_joint(
                *node,
                joint.node_index,
                joint.path.clone(),
                n.world,
                mesh_world,
            );
        }
        if inst.is_empty() {
            log::warn!("蒙皮 `{}` 无有效关节，跳过", inst.name);
            continue;
        }
        log::info!(
            "蒙皮 `{}`：{} 个关节 / {} 顶点带权重，绑定到网格节点 `{}`",
            inst.name,
            inst.joint_count(),
            mesh.joints.len(),
            scene
                .get(mesh_node)
                .map(|n| n.name.clone())
                .unwrap_or_default()
        );
        let skin_index = scene.add_skin(inst);
        // 扩展三十三：以静止姿态固定「骨骼树顶点 → 网格节点」的常量基准，
        // 之后整体刚体运动不再让关节矩阵产生浮点伪脏。
        if !scene.align_skin_frame(skin_index) {
            log::warn!(
                "蒙皮 `{}` 未能固定相对基准（骨骼树顶点缺失），关节矩阵回退世界矩阵路径",
                scene
                    .skins
                    .get(skin_index)
                    .map(|s| s.name.clone())
                    .unwrap_or_default()
            );
        }
    }
}

/// 为一个资产实例建立「骨架节点树 + 播放器」（P3）。
///
/// 场景节点命名规则 `<资产名>/<FBX 节点路径>`，例如
/// `tirger/RootNode/bip01/Spine`。网格顶点在导入时已烘焙到世界空间（P2 约定），
/// 因此骨架节点只承载变换、不挂网格（`visible: false` 纯粹是让渲染层跳过），
/// 真正决定形状的蒙皮要等 P4；P3 验证的是「轨道采样 → 节点局部 TRS」这条链路。
///
/// 返回骨架句柄（根节点创建失败时为 `None`）；无可用片段时只建骨架不挂播放器。
pub(crate) fn spawn_asset_anim(
    scene: &mut Scene,
    entry: &r3e_asset::AssetEntry,
    spec: &AssetSpec,
    path_str: &str,
    show_bones: bool,
) -> Option<SkeletonSpawn> {
    let stem = Path::new(path_str)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset")
        .to_string();
    // 同一 FBX 可能被声明多次（不同位移），骨架名字要能区分
    let prefix = {
        let mut p = stem.clone();
        let mut n = 0u32;
        while !scene.find_all(&format!("{p}/")).is_empty() && n < 100 {
            n += 1;
            p = format!("{stem}#{n}");
        }
        p
    };

    // 资产实例根：承载 `assets[].position/rotation/scale`，骨架各根节点挂在它下面
    let root_name = prefix.clone();
    if let Err(e) = scene.spawn(ObjectDesc {
        name: root_name.clone(),
        mesh: None,
        position: spec.position,
        rotation: spec.rotation,
        scale: spec.scale,
        color: None,
        parent: None,
        visible: Some(false),
        physics: None,
        material: None,
    }) {
        log::warn!("资产实例根 `{root_name}` 创建失败: {e}");
        return None;
    }

    // 建节点：父节点先建，父下标缺失时挂到实例根
    let mut ids: Vec<Option<NodeId>> = vec![None; entry.nodes.len()];
    let mut pending: Vec<usize> = (0..entry.nodes.len()).collect();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|&i| {
            let node = &entry.nodes[i];
            let parent_name = match node.parent {
                Some(pi) if (pi as usize) < ids.len() => match ids[pi as usize] {
                    Some(_) => {
                        let p = &entry.nodes[pi as usize];
                        format!(
                            "{prefix}/{}",
                            if p.path.is_empty() {
                                p.name.as_str()
                            } else {
                                p.path.as_str()
                            }
                        )
                    }
                    None => return true, // 父还没建，留到下一轮
                },
                _ => root_name.clone(),
            };
            let name = format!(
                "{prefix}/{}",
                if node.path.is_empty() {
                    node.name.as_str()
                } else {
                    node.path.as_str()
                }
            );
            let desc = ObjectDesc {
                name: name.clone(),
                mesh: None,
                position: Some(node.position),
                rotation: None,
                scale: Some(node.scale),
                color: None,
                parent: Some(parent_name),
                visible: Some(false),
                physics: None,
                material: None,
            };
            match scene.spawn(desc) {
                Ok(id) => {
                    // 四元数直接用绑定姿态，避免 euler 往返丢精度
                    if let Some(n) = scene.get_mut(id) {
                        n.transform.rotation = glam::Quat::from_array(node.rotation);
                    }
                    ids[i] = Some(id);
                    false
                }
                Err(e) => {
                    log::warn!("骨架节点 `{name}` 创建失败: {e}");
                    false
                }
            }
        });
        if pending.len() == before {
            // 父链有环或父下标越界：剩下的全挂实例根，避免死循环
            for i in std::mem::take(&mut pending) {
                let node = &entry.nodes[i];
                let name = format!("{prefix}/{}", node.path);
                if let Ok(id) = scene.spawn(ObjectDesc {
                    name,
                    mesh: None,
                    position: Some(node.position),
                    rotation: None,
                    scale: Some(node.scale),
                    color: None,
                    parent: Some(root_name.clone()),
                    visible: Some(false),
                    physics: None,
                    material: None,
                }) {
                    if let Some(n) = scene.get_mut(id) {
                        n.transform.rotation = glam::Quat::from_array(node.rotation);
                    }
                    ids[i] = Some(id);
                }
            }
            break;
        }
    }
    debug_assert!(!ids.is_empty() || entry.nodes.is_empty());

    // `--show-bones`：每个骨架节点挂一个小方块，动画推进时方块跟着动。
    // P4 蒙皮上线前，这是唯一能用肉眼确认「节点 TRS 采样生效」的手段。
    if show_bones {
        let root_scale = spec
            .scale
            .map(|s| s[0].abs())
            .filter(|v| *v > 1e-6)
            .unwrap_or(1.0);
        let local = 0.25 / root_scale;
        let mut markers = 0usize;
        for (i, slot) in ids.iter().enumerate() {
            if slot.is_none() {
                continue;
            }
            let node = &entry.nodes[i];
            let node_name = format!(
                "{prefix}/{}",
                if node.path.is_empty() {
                    node.name.as_str()
                } else {
                    node.path.as_str()
                }
            );
            match scene.spawn(ObjectDesc {
                name: format!("{prefix}/bone#{i}"),
                mesh: Some("cube".to_string()),
                position: Some([0.0, 0.0, 0.0]),
                rotation: None,
                scale: Some([local, local, local]),
                color: Some([1.0, 0.42, 0.12, 1.0]),
                parent: Some(node_name),
                visible: Some(true),
                physics: None,
                material: None,
            }) {
                Ok(_) => markers += 1,
                Err(e) => log::warn!("骨架标记 `{prefix}/bone#{i}` 创建失败: {e}"),
            }
        }
        log::info!("骨架可视化: `{prefix}` 共 {markers} 个标记方块");
    }

    // 把该资产的全部片段登记到场景，供脚本 `set_anim_clip` 切换（idle → run → atk）
    for anim in &entry.animations {
        if !anim.is_empty() {
            scene.register_anim_clip(format!("{prefix}/{}", anim.name), Arc::new(anim.clone()));
        }
    }

    // 选片段：声明里的 clip（大小写不敏感）优先，否则第一个
    let decl = spec.anim.clone().unwrap_or_default();
    let clip = match decl
        .clip
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(want) => entry
            .animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(want))
            .or_else(|| entry.default_animation()),
        None => entry.default_animation(),
    };
    let Some(clip) = clip else {
        // 只有蒙皮没有动画的资产：骨架已建好，回去绑蒙皮即可
        log::info!(
            "资产 `{path_str}` 无动画片段，仅建立骨架（{} 个节点）",
            ids.len()
        );
        return Some(SkeletonSpawn {
            prefix: prefix.clone(),
            ids,
        });
    };

    // 轨道 → 场景节点（按资产内节点下标对齐，比路径匹配更稳；路径留作切片段时的兜底）
    let mut bindings = Vec::new();
    for (ti, track) in clip.tracks.iter().enumerate() {
        if let Some(Some(id)) = ids.get(track.node_index as usize) {
            bindings.push(AnimBinding {
                node: *id,
                node_index: track.node_index,
                path: track.node_path.clone(),
                track: ti as u32,
            });
        }
    }

    let mut inst_name = format!("{prefix}/{}", clip.name);
    if scene.anim_instance(&inst_name).is_some() {
        inst_name = format!("{prefix}#{}", clip.name);
    }
    let mut inst = AnimInstance::new(inst_name.clone(), Arc::new(clip.clone()));
    inst.bindings = bindings;
    inst.speed = decl.speed.unwrap_or(1.0);
    inst.looping = decl.looping.unwrap_or(true);
    inst.time = decl.time.unwrap_or(0.0);
    inst.playing = decl.play.unwrap_or(true);
    let tracks = inst.track_count();
    let duration = inst.clip.duration;
    scene.add_anim_instance(inst);
    log::info!(
        "动画实例 `{inst_name}`：{} 条轨道 / {:.2}s / {}",
        tracks,
        duration,
        if scene
            .anim_instance(&inst_name)
            .map(|a| a.playing)
            .unwrap_or(false)
        {
            "播放中"
        } else {
            "已暂停"
        }
    );
    let _ = inst_name;
    Some(SkeletonSpawn { prefix, ids })
}
