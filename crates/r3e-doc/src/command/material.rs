//! 场景材质域（M1-S4-A）：材质表按 name 写 / 删 + 节点的材质引用。
//!
//! 与 `terrain::write_patch` 同一套语义：**覆盖永远生效、上限只拦新增**。
//! 新增被拒时返回 [`DocError::Rejected`]，而不是「命令显示成功但场景没变」。
//!
//! 代码里有两层「材质」概念，别混：
//!
//! - **材质表项**（`materials[]` 的一行，[`SceneMaterialDecl`]）= 本模块 `write_material` 管；
//! - **节点内联材质**（`Node::material`，三个标量）= `scene::write_material` 管（既有命令
//!   `SetNodeMaterial`）。
//!
//! 节点引用（`materialRef`）只写名字，**不校验表里有没有、也不校验与内联材质是否冲突**：
//! 那是校验器的活（`material.ref_missing` / `material.ref_conflict`），写点只保证类型与收敛。

use r3e_core::{Scene, SceneMaterialDecl, SCENE_MATERIAL_MAX};

use crate::{command::CommandKind, DocError};

/// 按 name 写一份场景材质；同名覆盖，新名走容量检查。
pub(super) fn write_material(s: &mut Scene, decl: &SceneMaterialDecl) -> Result<(), DocError> {
    let name = decl.name.trim();
    let before = s.materials.len();
    if !s.upsert_scene_material(decl.clone()) {
        // 两种失败：名字收敛后为空，或表已满且是新名字。前者是调用方的错，后者是容量。
        if name.is_empty() {
            return Err(DocError::Rejected("材质名字不能为空".to_string()));
        }
        return Err(DocError::Rejected(format!(
            "材质表已满（{before} 份，上限 {SCENE_MATERIAL_MAX}），`{name}` 未写入"
        )));
    }
    Ok(())
}

/// 按 name 删一份场景材质（不动引用它的节点，悬空由校验器报）。
pub(super) fn remove_material(s: &mut Scene, name: &str) -> Result<(), DocError> {
    if s.remove_scene_material(name) {
        Ok(())
    } else {
        Err(DocError::NotFound(format!("材质 `{name}`")))
    }
}

/// 写节点的材质引用；`None` 表示清除引用。
///
/// 「值没变」不算错误（空转成功）；只有节点不存在才报 [`DocError::NotFound`]。
pub(super) fn write_node_material_ref(
    s: &mut Scene,
    name: &str,
    new: &Option<String>,
) -> Result<(), DocError> {
    let changed = s.set_node_material_ref(name, new.as_deref());
    if changed || s.find(name).is_some() {
        Ok(())
    } else {
        Err(DocError::NotFound(format!("节点 `{name}`")))
    }
}

impl CommandKind {
    /// 写材质命令（前像为同名旧材质，没有则 `None`）。
    pub fn upsert_scene_material(s: &Scene, next: SceneMaterialDecl) -> Self {
        let name = next.name.trim().to_string();
        let old = s.scene_material(&name).cloned();
        Self::UpsertMaterial { next, old }
    }

    /// 删材质命令（前像为整份材质；不存在即报错，不造空命令）。
    pub fn remove_scene_material(s: &Scene, name: &str) -> Result<Self, DocError> {
        let old = s
            .scene_material(name)
            .cloned()
            .ok_or_else(|| DocError::NotFound(format!("材质 `{name}`")))?;
        Ok(Self::RemoveMaterial { old })
    }

    /// 改节点材质引用命令（`new = None` 表示清除引用）。
    ///
    /// 前像取 `node.material_ref` 本身（而不是「解析后的材质」）：撤销必须精确还原
    /// 「有没有引用」这件事，否则清除引用的撤销会凭空写回一份引用。
    pub fn set_node_material_ref(
        s: &Scene,
        name: &str,
        new: Option<String>,
    ) -> Result<Self, DocError> {
        let node = s
            .find(name)
            .ok_or_else(|| DocError::NotFound(format!("节点 `{name}`")))?;
        Ok(Self::SetNodeMaterialRef {
            name: name.to_string(),
            old: node.material_ref.clone(),
            new: r3e_core::normalize_material_ref(new),
        })
    }
}
