//! 地形域（M1-S3-C）：地表格按 name 写 / 删。
//!
//! 与 `env::write_local_light` 同一套语义：**覆盖永远生效、上限只拦新增**。
//! 新增被拒时返回 [`DocError::Rejected`]，而不是「命令显示成功但场景没变」。

use r3e_core::{Scene, TerrainPatch, TERRAIN_MAX_PATCHES};

use crate::{command::CommandKind, DocError};

/// 按 name 写一块地形；同名覆盖，新名走容量检查。
pub(super) fn write_patch(s: &mut Scene, patch: &TerrainPatch) -> Result<(), DocError> {
    let existed = s.terrain.iter().any(|p| p.name == patch.name.trim());
    let before = s.terrain.len();
    if !s.upsert_terrain_patch(patch.clone()) {
        // 两种失败：名字收敛后为空，或表已满且是新名字。前者是调用方的错，后者是容量。
        if patch.name.trim().is_empty() {
            return Err(DocError::Rejected("地形块名字不能为空".to_string()));
        }
        return Err(DocError::Rejected(format!(
            "地形块表已满（{before} 块，上限 {TERRAIN_MAX_PATCHES}），`{}` 未写入",
            patch.name.trim()
        )));
    }
    debug_assert!(existed || s.terrain.len() == before + 1);
    Ok(())
}

/// 按 name 删一块地形。
pub(super) fn remove_patch(s: &mut Scene, name: &str) -> Result<(), DocError> {
    if s.remove_terrain_patch(name) {
        Ok(())
    } else {
        Err(DocError::NotFound(format!("地形块 `{name}`")))
    }
}

impl CommandKind {
    /// 写地形块命令（前像为同名旧块，没有则 `None`）。
    pub fn upsert_terrain_patch(s: &Scene, next: TerrainPatch) -> Self {
        let name = next.name.trim();
        let old = s.terrain.iter().find(|p| p.name == name).cloned();
        Self::UpsertTerrainPatch { next, old }
    }

    /// 删地形块命令（前像即被删的整块；块不存在返回 `None`）。
    ///
    /// 返回 `None` 而不是造一条必然失败的命令：调用方（`terrain.remove` 的宿主实现）
    /// 要把「找不到这块地」与「删成功了」区分开，才能给出 `TERRAIN_PATCH_MISSING`。
    pub fn remove_terrain_patch(s: &Scene, name: &str) -> Option<Self> {
        let key = name.trim();
        let old = s.terrain.iter().find(|p| p.name == key).cloned()?;
        Some(Self::RemoveTerrainPatch { old })
    }
}
