//! 场景装配：把声明了 `physics` 段的节点绑成刚体，产出「节点 → 句柄」表。
//!
//! S4-C 把它从运行时的私有装配逻辑（原 `r3e-runtime/src/app.rs::sync_physics_bindings`）
//! 下沉到这里，运行时 / 编辑器 / 内容验证器共用同一个入口。
//!
//! 单独成文件而不是并回 `lib.rs`：`lib.rs` 已在大文件债务白名单里，只许减不许增。
//!
//! **只读场景**是这里的关键约束——装配只认 `&Scene`，句柄通过 `BindReport::bindings`
//! 交回，写回由持有写通道的一方完成。这样只读的调用方（内容验证器）也能用同一个入口。

use glam::Vec3;
use r3e_core::{MeshData, MeshProvider, Node, NodeId, Scene};

use crate::{
    half_extents_from, infer_shape, BackendCaps, BodyDesc, BodyHandle, BodyKind, ColliderShape,
    Physics,
};

// ---------------------------------------------------------------------------
// 装配报告
// ---------------------------------------------------------------------------

/// 一次 [`Physics::bind_scene`] 的结果。
///
/// 为什么要有它：装配是「尽力而为」的批量动作，失败原因各不相同——物理没开、节点没网格、
/// `physics` 段推不出参数、后端刚被换掉。只返回「绑了几个」的话，上层（运行时日志 /
/// 编辑器面板 / 内容验证器）分不清「场景本来就没有刚体」与「本该有却一个没绑上」，
/// 只能靠猜，而猜错的方向通常是把问题静默掉。
///
/// `created + reused == bound()`，`skipped` 单独计——它不是失败，是「这个节点不该有刚体」
/// 或「声明不完整」，验证器据此报问题，运行时不报错。
#[derive(Debug, Clone, PartialEq)]
pub struct BindReport {
    /// 后端名（`null` / `builtin` / `rapier`）。
    pub backend: &'static str,
    /// 装配时物理是否开着。`false` 时其余计数全为 0——不是失败，是没有入口。
    pub enabled: bool,
    /// 后端能力位，照抄 `Physics::capabilities()`，省得调用方再取一次。
    pub capabilities: BackendCaps,
    /// 场景里声明了 `physics` 段的节点数。
    pub candidates: usize,
    /// 本次**新建**刚体的节点数。
    pub created: usize,
    /// 句柄仍活着、直接复用的节点数（幂等的证据）。
    pub reused: usize,
    /// 推不出 `BodyDesc` 而跳过的节点数。
    pub skipped: usize,
    /// 本次装配认可的「节点 → 刚体句柄」，调用方按它写回 `Node::body`。
    ///
    /// 为什么不让 `bind_scene` 自己写回：那会把签名定成 `&mut Scene`，于是
    /// **只读的调用方（内容验证器）用不了这个入口**——而验证器恰恰是最需要
    /// 「按运行时的同一套规则推一遍碰撞体」的地方。改成「物理只读场景、产出
    /// 句柄表」，写回由持有写通道的一方（运行时的 `write` 闭包）完成。
    ///
    /// 表里同时含新建与复用的节点：调用方无脑覆盖即可，不必自己区分。
    pub bindings: Vec<(NodeId, u64)>,
}

impl BindReport {
    fn untouched(backend: &'static str, enabled: bool, capabilities: BackendCaps) -> Self {
        Self {
            backend,
            enabled,
            capabilities,
            candidates: 0,
            created: 0,
            reused: 0,
            skipped: 0,
            bindings: Vec::new(),
        }
    }

    /// 本次实际挂上刚体的节点数（新建 + 复用）。
    pub fn bound(&self) -> usize {
        self.created + self.reused
    }

    /// 后端是否真的会模拟这些刚体。
    ///
    /// 关着、或后端一位能力都不申报（`null` 空转）时为 `false`：此时节点上的 `body`
    /// 只是句柄记账，物件不会动。上层要据此如实告知，而不是看到 `bound() > 0` 就以为
    /// 物理生效了。
    pub fn simulated(&self) -> bool {
        self.enabled && self.capabilities != BackendCaps::NONE
    }

    /// 一行摘要，直接进日志 / 面板。
    pub fn summary(&self) -> String {
        if !self.enabled {
            return format!("物理未开启（后端 {}），未做装配", self.backend);
        }
        let tail = if self.simulated() {
            String::new()
        } else {
            format!("；后端 `{}` 不模拟刚体，句柄仅记账", self.backend)
        };
        format!(
            "装配 {} 个节点：新建 {} / 复用 {} / 跳过 {}{}",
            self.candidates, self.created, self.reused, self.skipped, tail
        )
    }
}

impl Physics {
    /// 句柄在本后端是否还活着。
    ///
    /// 与 [`Physics::body_count`] 不同，**这里不看开关**：它回答的是后端状态，
    /// 不是「物理是否在跑」。装配（[`Physics::bind_scene`]）幂等就靠它。
    pub fn has_body(&self, handle: BodyHandle) -> bool {
        self.backend.has_body(handle)
    }

    /// 把场景里声明了 `physics` 段的节点绑成刚体，产出「节点 → 句柄」表。
    ///
    /// 这是**装配的唯一入口**：运行时、编辑器、验证器都走这里，不要在各自那侧再写
    /// 一遍「遍历节点 → 取网格 → 推 `BodyDesc` → create」。同一段装配写三份，
    /// 迟早让「编辑器里看到的碰撞体」与「运行时撞到的东西」不是同一个。
    ///
    /// **只读场景**：句柄通过 [`BindReport::bindings`] 交回，由持有写通道的一方
    /// （运行时的 `Document::write` 闭包）写进 `Node::body`。这样验证器这类只读
    /// 调用方也能用同一个入口，而不必为了调它去伪造可变场景。
    ///
    /// 三条口径（都是踩过的坑）：
    /// * **网格几何是唯一来源**：碰撞体形状与尺寸从 `meshes` 给的 `MeshData` 推
    ///   （[`BodyDesc::from_node_with`]），不按「内置图元 / 外部资产」分支——资产导入
    ///   尺寸常常远不是 1×1×1，退化成 `scale * 0.5` 会得到看不见的错尺寸碰撞体。
    /// * **幂等靠后端句柄表**：节点上已有 `body` 且后端仍认这条句柄 → 记 `reused`
    ///   并原样交回句柄。所以脚本用 `attach_body` 手动绑的刚体不会被这里覆盖。
    ///   反过来，后端不认的句柄（切后端 / 清空过）会先销毁再重建，不留孤儿。
    /// * **关闭就是关闭**：物理没开时返回 `enabled = false` 的报告并且**不产出**任何
    ///   绑定——不要留一条「看起来绑过、其实没有」的痕迹给上层误判。
    pub fn bind_scene(&mut self, scene: &Scene, meshes: &dyn MeshProvider) -> BindReport {
        let backend = self.backend_name();
        let caps = self.capabilities();
        if !self.enabled {
            return BindReport::untouched(backend, false, caps);
        }

        let ids: Vec<NodeId> = scene
            .iter()
            .filter(|n| n.physics.is_some())
            .map(|n| n.id)
            .collect();

        let mut report = BindReport {
            candidates: ids.len(),
            ..BindReport::untouched(backend, true, caps)
        };

        for id in ids {
            let Some(node) = scene.get(id) else {
                report.skipped += 1;
                continue;
            };
            let mesh = meshes.mesh_data(node.mesh);
            let Some(desc) = BodyDesc::from_node_with(node, mesh.as_ref()) else {
                // `physics` 段在但推不出来（字段被清空 / 类型非法）。如实记账，
                // 静默 continue 会让「验证器说该有刚体、运行时没有」变成悬案。
                report.skipped += 1;
                continue;
            };

            // 复用顺序：场景里记着的句柄优先（脚本 / 上次装配写回的那条），
            // 其次是上一轮只读装配自己记下的句柄（只读调用方不会写回场景）。
            let known = node.body.or_else(|| self.last_handle(id));
            match known {
                Some(raw) if self.backend.has_body(BodyHandle(raw)) => {
                    report.reused += 1;
                    report.bindings.push((id, raw));
                    continue;
                }
                Some(raw) => {
                    // 旧句柄后端不认：先销毁（契约要求幂等），避免留下不可达的孤儿刚体。
                    self.backend.destroy_body(BodyHandle(raw));
                }
                None => {}
            }

            let handle = self.backend.create_body(id, desc);
            report.bindings.push((id, handle.0));
            report.created += 1;
        }
        // 记下本轮结果：只读调用方靠它实现「连调两次不重建刚体」的幂等，
        // 写入同时把上一轮已经消失的节点自然淘汰掉（整表替换而不是增量合并）。
        self.last_bindings = report.bindings.clone();
        report
    }

    /// 上一轮装配给该节点分配的句柄（没有则 `None`）。
    fn last_handle(&self, id: NodeId) -> Option<u64> {
        self.last_bindings
            .iter()
            .find(|(node, _)| *node == id)
            .map(|(_, handle)| *handle)
    }
}

// ---------------------------------------------------------------------------
// 从场景节点推导刚体描述（装配与验证器共用的唯一算法）
// ---------------------------------------------------------------------------

impl BodyDesc {
    /// 从场景节点推导刚体描述：节点需带 `physics` 段，否则返回 `None`。
    ///
    /// 缺省规则与场景文件约定一致：碰撞形状由 `collider` 指定、否则由 mesh 推导；
    /// 半尺寸由 `half_extents` 指定、否则取 scale * 0.5；质量缺省 1.0。
    pub fn from_node(node: &Node) -> Option<Self> {
        Self::from_node_with(node, None)
    }

    /// 同 [`BodyDesc::from_node`]，但可用网格真实包围盒推导碰撞体尺寸。
    ///
    /// 外部导入网格的真实尺寸往往远不是 1×1×1，退化成
    /// `scale * 0.5` 会让碰撞体明显偏小；这里让 AABB 参与推导。
    /// 显式写了 `half_extents` 的场景仍然以场景为准（人工调过的值优先）。
    pub fn from_node_with(node: &Node, mesh: Option<&MeshData>) -> Option<Self> {
        let p = node.physics.as_ref()?;
        let shape = p
            .collider
            .as_deref()
            .map(ColliderShape::parse)
            .unwrap_or_else(|| mesh.map(infer_shape).unwrap_or(ColliderShape::Box));
        let scale = node.transform.scale.abs();
        let half_extents = p
            .half_extents
            .map(|h| Vec3::from(h).abs())
            .unwrap_or_else(|| half_extents_from(mesh.map(|m| m.aabb_box()), scale));
        Some(Self {
            kind: BodyKind::parse(&p.body),
            shape,
            half_extents,
            mass: if p.mass > 1e-4 { p.mass } else { 1.0 },
            restitution: p.restitution.unwrap_or(0.2),
            friction: p.friction.unwrap_or(0.6),
            position: node.world_position(),
            rotation: node.transform.rotation,
            scale,
            continuous: p.ccd,
        })
    }
}
