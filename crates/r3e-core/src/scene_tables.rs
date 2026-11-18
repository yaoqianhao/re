//! `Scene` 上的「表型数据」访问器（局部光源 / 地形块）。
//!
//! 这两张表的语义完全同构：名字即主键、同名原地覆盖、满载只拦新增。
//! 实现随表走（[`crate::light`] / [`crate::terrain`]），这里只做 `Scene` 上的
//! 薄转发，并从 [`crate::scene`] 里分出来，避免场景文件继续膨胀。

use crate::light::{self, LocalLight};
use crate::scene::Scene;
use crate::scene_material::{self, SceneMaterialDecl};
use crate::terrain::{self, TerrainPatch};

impl Scene {
    /// 按 key 覆盖式写入一盏局部光源。
    ///
    /// 语义与 `ui::upsert` 一致：同 key 原地替换（顺序不变），新 key 追加；
    /// 满载时只拒绝新增，同 key 更新仍然生效。返回值即「这次写入是否生效」。
    /// 入参在这里统一跑 `sanitized()`，所以脚本传 NaN 也污染不了场景。
    pub fn upsert_local_light(&mut self, light: LocalLight) -> bool {
        if light.key.is_empty() {
            return false;
        }
        light::upsert_local(&mut self.lights, light)
    }

    /// 按 key 移除一盏局部光源。
    pub fn remove_local_light(&mut self, key: &str) -> bool {
        light::remove_local(&mut self.lights, key)
    }

    /// 按 key 查一盏局部光源。
    pub fn local_light(&self, key: &str) -> Option<&LocalLight> {
        light::find_local(&self.lights, key)
    }

    /// 按 key 改一盏局部光源；key 不存在时返回 `None`（不新建，避免拼错名字后
    /// 悄悄多出一盏灯）。
    pub fn with_local_light<F>(&mut self, key: &str, f: F) -> bool
    where
        F: FnOnce(&mut LocalLight),
    {
        light::with_local(&mut self.lights, key, f).is_some()
    }

    /// 清空全部局部光源。
    pub fn clear_local_lights(&mut self) -> usize {
        let n = self.lights.len();
        self.lights.clear();
        n
    }

    // ---------- 地形（M1-S3-C） ----------

    /// 按 name 覆盖式写入一块地形。
    ///
    /// 语义与 [`Scene::upsert_local_light`] 一致：同名原地替换（顺序不变）、新名追加；
    /// 满载时只拒绝新增，同名更新仍然生效。入参在这里统一 `sanitized()`，
    /// 所以脚本传 NaN、越界分辨率都污染不了场景。返回值即「这次写入是否生效」。
    pub fn upsert_terrain_patch(&mut self, patch: TerrainPatch) -> bool {
        terrain::upsert_patch(&mut self.terrain, patch)
    }

    /// 按 name 移除一块地形。
    pub fn remove_terrain_patch(&mut self, name: &str) -> bool {
        terrain::remove_patch(&mut self.terrain, name)
    }

    /// 按 name 查一块地形。
    pub fn terrain_patch(&self, name: &str) -> Option<&TerrainPatch> {
        terrain::find_patch(&self.terrain, name)
    }

    /// 按 name 覆盖式写入一份场景材质（M1-S4-A）。
    ///
    /// 与地形表同一套语义：同名原地替换（顺序不变）、新名追加、满载只拦新增。
    /// 入参在这里统一 `sanitized()`，脚本传 NaN / 越界 uv 都污染不了场景。
    pub fn upsert_scene_material(&mut self, material: SceneMaterialDecl) -> bool {
        scene_material::upsert_material(&mut self.materials, material)
    }

    /// 按 name 移除一份场景材质。
    ///
    /// 注意：只删表项，**不处理引用它的节点**——悬空引用由校验器报
    /// `material.ref_missing`，是否连带清理由上层（编辑器 / 命令）决定。
    pub fn remove_scene_material(&mut self, name: &str) -> bool {
        scene_material::remove_material(&mut self.materials, name)
    }

    /// 按 name 查一份场景材质。
    pub fn scene_material(&self, name: &str) -> Option<&SceneMaterialDecl> {
        scene_material::find_material(&self.materials, name)
    }
}
