//! 节点域：变换 / 可见性 / 颜色 / 材质。
//!
//! 一律按 [`NodeId`] 寻址（id 单调递增、永不复用），只有材质按名字走 `Scene` 已有的
//! 收敛入口 `set_material`（拒绝绕过 `sanitized`，否则 NaN 会从编辑命令漏进场景）。
//! 撤销 = 把前像按同一个写函数写回去，写函数的幂等性保证 round-trip 精确。

use r3e_core::{Node, NodeId, ObjectDesc, Scene, SurfaceMaterial, Transform, Vec4};

use crate::{command::CommandKind, DocError};

/// 新建物件。`spawn` 重名即失败，因此撤销可以安全地按名字删。
pub(super) fn spawn(s: &mut Scene, desc: &ObjectDesc) -> Result<(), DocError> {
    s.spawn(desc.clone()).map(|_| ()).map_err(DocError::Scene)
}

/// 按名字删物件。
pub(super) fn despawn(s: &mut Scene, name: &str) -> Result<(), DocError> {
    if s.despawn(name) {
        Ok(())
    } else {
        Err(DocError::NotFound(format!("节点 `{name}`")))
    }
}

fn node_mut(s: &mut Scene, id: NodeId) -> Result<&mut Node, DocError> {
    s.get_mut(id)
        .ok_or_else(|| DocError::NotFound(format!("节点 #{id}")))
}

/// 写变换。
pub(super) fn write_transform(s: &mut Scene, id: NodeId, t: Transform) -> Result<(), DocError> {
    node_mut(s, id)?.transform = t;
    Ok(())
}

/// 写可见性。
pub(super) fn write_visible(s: &mut Scene, id: NodeId, visible: bool) -> Result<(), DocError> {
    node_mut(s, id)?.visible = visible;
    Ok(())
}

/// 写颜色。
pub(super) fn write_color(s: &mut Scene, id: NodeId, color: Vec4) -> Result<(), DocError> {
    node_mut(s, id)?.color = color;
    Ok(())
}

/// 写材质覆盖；`None` 表示清掉覆盖、回退网格默认材质。
pub(super) fn write_material(
    s: &mut Scene,
    name: &str,
    material: &Option<SurfaceMaterial>,
) -> Result<(), DocError> {
    let ok = match material {
        Some(m) => s.set_material(name, *m),
        None => s.clear_material(name),
    };
    if ok {
        Ok(())
    } else {
        Err(DocError::NotFound(format!("节点 `{name}`")))
    }
}

impl CommandKind {
    /// 新建物件命令。
    pub fn spawn_node(desc: ObjectDesc) -> Self {
        Self::SpawnNode { desc }
    }

    /// 改变换命令；`new` 为要写入的变换，前像从当前场景抓。
    pub fn set_transform(s: &Scene, id: NodeId, new: Transform) -> Result<Self, DocError> {
        let old = node_ref(s, id)?.transform;
        Ok(Self::SetTransform { id, old, new })
    }

    /// 改可见性命令。
    pub fn set_visible(s: &Scene, id: NodeId, new: bool) -> Result<Self, DocError> {
        let old = node_ref(s, id)?.visible;
        Ok(Self::SetVisible { id, old, new })
    }

    /// 改颜色命令。
    pub fn set_node_color(s: &Scene, id: NodeId, new: Vec4) -> Result<Self, DocError> {
        let old = node_ref(s, id)?.color;
        Ok(Self::SetNodeColor { id, old, new })
    }

    /// 改材质命令；`new = None` 表示清掉覆盖。
    pub fn set_node_material(
        s: &Scene,
        name: &str,
        new: Option<SurfaceMaterial>,
    ) -> Result<Self, DocError> {
        // 前像必须取 `node.material`（覆盖位本身），不能用 `Scene::material_of`：
        // 后者是「有效材质」，没有覆盖时会回落成默认值 `Some(default)`，据此撤销会把
        // 一个原本没有覆盖的节点写出 `material: {metallic: 0}` 来（实测踩过）。
        let old = node_ref_by_name(s, name)?.material;
        Ok(Self::SetNodeMaterial {
            name: name.to_string(),
            old,
            new,
        })
    }
}

fn node_ref(s: &Scene, id: NodeId) -> Result<&Node, DocError> {
    s.get(id)
        .ok_or_else(|| DocError::NotFound(format!("节点 #{id}")))
}

fn node_ref_by_name<'a>(s: &'a Scene, name: &str) -> Result<&'a Node, DocError> {
    s.find(name)
        .ok_or_else(|| DocError::NotFound(format!("节点 `{name}`")))
}
