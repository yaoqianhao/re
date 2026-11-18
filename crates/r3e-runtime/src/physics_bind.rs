//! 物理装配的运行时半边：网格来源 + 句柄写回。
//!
//! 装配本体在 [`r3e_physics::Physics::bind_scene`]，与编辑器、内容验证器同一个入口；
//! 这里只承担运行时独有的部分——把「资产优先、内置图元兜底」的网格来源递进去，
//! 并把 `bindings` 表通过场景写通道落回节点。
//!
//! 单独成文件而不是塞在 `app.rs`：`app.rs` 已在大文件债务白名单里，只许减不许增。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use r3e_asset::AssetLibrary;
use r3e_core::{MeshData, MeshProvider, MeshRef};
use r3e_doc::Document;
use r3e_physics::{BindReport, Physics};

use crate::app::node_mesh_data;

/// 把「资产优先、内置图元兜底」这套顺序包成 `MeshProvider`，交给物理装配用。
///
/// 存在的意义只有一个：**让装配与渲染看同一个网格**。物理侧不该知道
/// `AssetLibrary` 与 `ProceduralProvider` 的存在，更不该在库里复制一份
/// 「先查资产再退图元」的判定——复制出来迟早与 [`node_mesh_data`] 分叉。
pub(crate) struct SceneMeshProvider {
    /// 拿一份 `Arc` 快照而不是借用 `assets`：装配发生在 `write` 闭包里，
    /// 借用会与外层 `RefCell` 冲突。`Arc` 克隆是 O(1)，装配又是低频动作。
    assets: Arc<AssetLibrary>,
}

impl SceneMeshProvider {
    /// 每次装配一份即可：构造只包一层 `Arc`，不碰资产库。
    pub(crate) fn new(assets: Arc<AssetLibrary>) -> Self {
        Self { assets }
    }
}

impl MeshProvider for SceneMeshProvider {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData> {
        node_mesh_data(&self.assets, key)
    }
}

/// 装配一次场景刚体，并把句柄写回节点。
///
/// `bind_scene` 只读场景（验证器也要能调），所以写回必须在持有写通道的这一侧完成：
/// 这里把「装配 + 落盘」收在一个 `write` 闭包里，中途不 yield，避免拿到半装配的场景。
pub(crate) fn rebind(
    physics: &mut Physics,
    scene: &Rc<RefCell<Document>>,
    assets: &Arc<AssetLibrary>,
) -> BindReport {
    let meshes = SceneMeshProvider::new(Arc::clone(assets));
    scene.borrow_mut().write(|scene| {
        let report = physics.bind_scene(scene, &meshes);
        for &(id, handle) in &report.bindings {
            if let Some(node) = scene.get_mut(id) {
                node.body = Some(handle);
            }
        }
        report
    })
}
