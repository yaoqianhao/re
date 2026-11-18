//! 场景材质资源引用体系（M1-S4-A）。
//!
//! 与 [`crate::SurfaceMaterial`] 的分工：
//!
//! - `SurfaceMaterial` 是**节点内联**的三个标量旋钮（metallic / roughness / emissive），
//!   语义是「这个物件表面怎么反光」，随物件一起写在 `objects[]` 里；
//! - `SceneMaterialDecl` 是**场景级材质表**的一行：名字 + 同样的三标量 + 五张贴图路径
//!   + uv 缩放。节点通过 `materialRef` 按名字引用它，一份材质可被多个节点复用。
//!
//! 为什么不复用网格的 `asset:path#mesh` 语法：`asset:` 指的是「资产文件里的网格/材质槽」，
//! 走资产导入通道、解析结果是运行期 `AssetId`；`materialRef` 指的是「本场景 `materials[]`
//! 里的一行」，纯文档层寻址、不需要导入。两套语义混在一个字符串里，改一次路径就会同时
//! 影响「资产重导」与「文档校验」两条链路，所以这里**独立命名空间**。
//!
//! 表语义与 [`crate::TerrainPatch`] / [`crate::LocalLight`] 完全同构：
//!
//! - `name` 是唯一主键，重复写同名 = 整体覆盖（顺序不变）；
//! - 上限 [`SCENE_MATERIAL_MAX`] **只拦新增、不拦覆盖**；
//! - 入参一律经 [`SceneMaterialDecl::sanitized`] 收敛，NaN / 越界 / 名字拼空格污染不了场景。

use serde::{Deserialize, Serialize};

use std::path::Path;

use crate::material::DEFAULT_ROUGHNESS;
use crate::{AssetId, Scene, SurfaceMaterial};

/// 材质表上限（只拦新增）。64 份材质足够覆盖一个关卡的表面种类，也让快照与哈希有界。
pub const SCENE_MATERIAL_MAX: usize = 64;

/// 材质名长度上限（字符数，按 `char` 边界截断，不切坏 UTF-8）。
pub const SCENE_MATERIAL_NAME_MAX_LEN: usize = 64;

/// 贴图路径长度上限（字符数）。超长路径按字符截断——这类路径几乎必然是拼错了。
pub const TEXTURE_PATH_MAX_LEN: usize = 260;

/// uv 缩放下限。`<= 0` 与 NaN 一律按「不缩放」（`1.0`）处理。
pub const UV_SCALE_MIN: f32 = 0.01;

/// uv 缩放上限。再大的平铺意味着贴图已经糊成一片噪点，直接夹住。
pub const UV_SCALE_MAX: f32 = 1024.0;

/// 材质行的五个贴图槽名（顺序即文件里的书写顺序，也是校验 / 哈希的遍历顺序）。
pub const TEXTURE_SLOTS: [&str; 5] = [
    "albedo",
    "normal",
    "metallicRoughness",
    "occlusion",
    "emissiveMap",
];

fn default_uv_scale() -> [f32; 2] {
    [1.0, 1.0]
}

/// 场景材质表的一行：名字 + 三标量 + 五张贴图路径 + uv 缩放。
///
/// 标量口径与 [`SurfaceMaterial`] 严格一致（同一个 `sanitized` 实现），这样「内联材质」
/// 与「引用材质」在数值语义上不会漂移：改一个地方，两条路径同时生效。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SceneMaterialDecl {
    /// 唯一主键，`materialRef` 按它寻址。重复写同名 = 整体覆盖。
    pub name: String,
    /// 金属度 0..1。
    pub metallic: f32,
    /// 粗糙度 [`crate::MIN_ROUGHNESS`]`..=1`。
    pub roughness: f32,
    /// 自发光（线性 RGB），逐通道夹到 `0..=`[`crate::MAX_EMISSIVE`]。
    pub emissive: [f32; 3],
    /// 反照率贴图路径（相对场景文件所在目录）。
    pub albedo: Option<String>,
    /// 法线贴图路径。
    pub normal: Option<String>,
    /// 金属度 / 粗糙度打包贴图路径（glTF 口径：G 通道粗糙度、B 通道金属度）。
    pub metallic_roughness: Option<String>,
    /// 环境光遮蔽贴图路径。
    pub occlusion: Option<String>,
    /// 自发光贴图路径。
    pub emissive_map: Option<String>,
    /// uv 缩放，逐轴夹到 [`UV_SCALE_MIN`]`..=`[`UV_SCALE_MAX`]。
    pub uv_scale: [f32; 2],
}

impl Default for SceneMaterialDecl {
    fn default() -> Self {
        Self {
            name: String::new(),
            metallic: 0.0,
            roughness: DEFAULT_ROUGHNESS,
            emissive: [0.0, 0.0, 0.0],
            albedo: None,
            normal: None,
            metallic_roughness: None,
            occlusion: None,
            emissive_map: None,
            uv_scale: default_uv_scale(),
        }
    }
}

impl SceneMaterialDecl {
    /// 新建一份只有标量的材质（贴图全空、uv 不缩放）。
    pub fn from_params(name: impl Into<String>, params: SurfaceMaterial) -> Self {
        Self {
            name: name.into(),
            metallic: params.metallic,
            roughness: params.roughness,
            emissive: params.emissive,
            ..Self::default()
        }
    }

    /// 只取三标量（内联材质口径）。渲染层把它当 `SurfaceMaterial` 用。
    pub fn params(&self) -> SurfaceMaterial {
        SurfaceMaterial {
            metallic: self.metallic,
            roughness: self.roughness,
            emissive: self.emissive,
        }
    }

    /// 参数收敛：名字去空白并截断、三标量走 [`SurfaceMaterial::sanitized`]、
    /// 路径去空白（空串 = 不声明）并截断、uv 缩放夹到合法区间。
    ///
    /// 这是唯一收敛入口：场景文件、doc 命令、运行期解析三条路径都必须经过它。
    pub fn sanitized(self) -> Self {
        let params = self.params().sanitized();
        Self {
            name: truncate_chars(self.name.trim(), SCENE_MATERIAL_NAME_MAX_LEN),
            metallic: params.metallic,
            roughness: params.roughness,
            emissive: params.emissive,
            albedo: sanitize_path(self.albedo),
            normal: sanitize_path(self.normal),
            metallic_roughness: sanitize_path(self.metallic_roughness),
            occlusion: sanitize_path(self.occlusion),
            emissive_map: sanitize_path(self.emissive_map),
            uv_scale: [
                sanitize_uv_scale(self.uv_scale[0]),
                sanitize_uv_scale(self.uv_scale[1]),
            ],
        }
    }

    /// 名字是否为空（收敛后）。空名材质不可被引用，等同无效。
    pub fn is_unnamed(&self) -> bool {
        self.name.is_empty()
    }

    /// 五个槽位里声明了路径的那些，按 [`TEXTURE_SLOTS`] 的顺序产出 `(槽名, 路径)`。
    ///
    /// 校验器判「文件是否存在」、哈希算指纹、运行期解析资产都用这一个迭代器，
    /// 保证「加了槽位就自动被三处消费」，不用分别改三份 matcher。
    pub fn textures(&self) -> impl Iterator<Item = (&'static str, &str)> {
        let slots = [
            (TEXTURE_SLOTS[0], self.albedo.as_deref()),
            (TEXTURE_SLOTS[1], self.normal.as_deref()),
            (TEXTURE_SLOTS[2], self.metallic_roughness.as_deref()),
            (TEXTURE_SLOTS[3], self.occlusion.as_deref()),
            (TEXTURE_SLOTS[4], self.emissive_map.as_deref()),
        ];
        slots
            .into_iter()
            .filter_map(|(slot, path)| path.map(|p| (slot, p)))
    }

    /// 声明了几张贴图。
    pub fn texture_count(&self) -> usize {
        self.textures().count()
    }

    /// 五个槽位的**裸路径**（按 [`TEXTURE_SLOTS`] 顺序，未声明的给 `None`）。
    ///
    /// 与 [`Self::textures`] 的区别：那个跳过未声明槽、给的是 `(&'static str, &str)`，
    /// 适合「校验存在性」；这个给固定 5 元组，适合「按槽位下标填渲染侧字段」。
    pub fn texture_paths(&self) -> [Option<&str>; 5] {
        [
            self.albedo.as_deref(),
            self.normal.as_deref(),
            self.metallic_roughness.as_deref(),
            self.occlusion.as_deref(),
            self.emissive_map.as_deref(),
        ]
    }

    /// 把一份场景材质解析成渲染层直吃的 [`crate::mesh::MaterialData`]。
    ///
    /// 贴图槽填的是 `(AssetId, 0)`：`AssetId` 由**解析后的绝对路径**散列得到，
    /// 与 `r3e_asset::import_texture` 给独立图片分配的 id 是同一个函数、同一串路径，
    /// 因此「文档层写路径」与「运行期导入像素」不需要共享任何映射表就能对上。
    ///
    /// `base_dir` 是场景文件所在目录（相对路径按它解析）。给 `None` 时相对路径原样
    /// 参与散列——这只在「路径本来就是绝对路径」时可靠，调用方应尽量传真实目录。
    pub fn to_material_data(&self, base_dir: Option<&Path>) -> crate::mesh::MaterialData {
        let ids = self.texture_asset_ids(base_dir);
        crate::mesh::MaterialData {
            name: self.name.clone(),
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: self.metallic,
            roughness: self.roughness,
            base_color_texture: slot_handle(ids[0]),
            normal_texture: slot_handle(ids[1]),
            roughness_texture: slot_handle(ids[2]),
            occlusion_texture: slot_handle(ids[3]),
            emissive_texture: slot_handle(ids[4]),
            uv_scale: self.uv_scale,
        }
    }

    /// 五张贴图各自解析到的 `AssetId`（未声明的给 `None`）。
    ///
    /// 注意：这里**只算 id、不碰磁盘**——文件是否存在由校验器负责
    /// （`material.texture_missing`），渲染链路拿到的是「该去哪找」而不是「找到了没有」。
    pub fn texture_asset_ids(&self, base_dir: Option<&Path>) -> [Option<AssetId>; 5] {
        let paths = self.texture_paths();
        let mut out = [None; 5];
        for (slot, path) in out.iter_mut().zip(paths.iter()) {
            if let Some(rel) = path {
                *slot = Some(AssetId::from_path(&resolve_texture_path(base_dir, rel)));
            }
        }
        out
    }
}

/// 把声明里的贴图路径解析成「参与 `AssetId` 散列的绝对路径字符串」。
///
/// 三条规则，缺一条就会出现「文档说 A、运行期找 B」：
///
/// 1. **绝对路径原样用**（只把反斜杠统一成正斜杠，与 [`sanitize_path`] 同口径）；
/// 2. 相对路径拼到 `base_dir` 上；
/// 3. 没有 `base_dir` 时原样返回——调用方自己有责任保证路径可解析。
///
/// 这里不做 `canonicalize`：它要碰磁盘、还会把不存在的路径变成错误，
/// 而散列必须对「文件尚未创建」的场景也稳定可算。
pub fn resolve_texture_path(base_dir: Option<&Path>, rel: &str) -> String {
    let unified = rel.trim().replace('\\', "/");
    let path = Path::new(&unified);
    match (path.is_absolute(), base_dir) {
        (true, _) | (_, None) => unified,
        (false, Some(dir)) => {
            let joined = dir.join(&unified);
            joined.to_string_lossy().replace('\\', "/")
        }
    }
}

fn slot_handle(id: Option<AssetId>) -> Option<(AssetId, u32)> {
    // 独立图片资产里恒有且仅有一条贴图记录，下标固定 0（见 `r3e_asset::texture_import`）。
    id.map(|asset| (asset, 0))
}

/// 贴图路径收敛：去首尾空白，空串归一成 `None`，超长按字符截断；反斜杠统一成正斜杠。
///
/// 统一斜杠是为了让「`tex\a.png` 与 `tex/a.png`」在哈希与存在性判定里是同一个路径——
/// Windows 上两种写法都合法，不归一就会出现「改一个斜杠哈希变了但文件没变」。
fn sanitize_path(path: Option<String>) -> Option<String> {
    let raw = path?;
    let trimmed = raw.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(&trimmed, TEXTURE_PATH_MAX_LEN))
}

/// uv 缩放收敛：非有限值 / 非正值回到 `1.0`（不缩放），超上限夹到 [`UV_SCALE_MAX`]。
fn sanitize_uv_scale(v: f32) -> f32 {
    if !v.is_finite() || v < UV_SCALE_MIN {
        return 1.0;
    }
    v.min(UV_SCALE_MAX)
}

/// 按 `char` 边界截断到 `max` 个字符（不切坏 UTF-8）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// 材质引用收敛：去首尾空白，空串归一成 `None`，超长按字符截断。
///
/// `materialRef` 是纯名字寻址（不是路径），所以不做斜杠归一。
pub fn normalize_material_ref(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(trimmed, SCENE_MATERIAL_NAME_MAX_LEN))
}

/// 场景层面的材质表操作：按 name 覆盖式写入（同名原地替换、顺序不变）。
///
/// 与 [`crate::terrain::upsert_patch`] 同一套语义：**上限只拦新增、不拦覆盖**。
/// 返回 `true` 表示写入生效（新建或覆盖），`false` 表示名字收敛后为空，或表已满且是新名字。
pub fn upsert_material(materials: &mut Vec<SceneMaterialDecl>, decl: SceneMaterialDecl) -> bool {
    let decl = decl.sanitized();
    if decl.name.is_empty() {
        return false;
    }
    if let Some(slot) = materials.iter_mut().find(|m| m.name == decl.name) {
        *slot = decl;
        return true;
    }
    if materials.len() >= SCENE_MATERIAL_MAX {
        return false;
    }
    materials.push(decl);
    true
}

/// 按 name 移除；返回是否真的删掉了。
pub fn remove_material(materials: &mut Vec<SceneMaterialDecl>, name: &str) -> bool {
    match materials.iter().position(|m| m.name == name) {
        Some(idx) => {
            materials.remove(idx);
            true
        }
        None => false,
    }
}

/// 按 name 查一份材质。
pub fn find_material<'a>(
    materials: &'a [SceneMaterialDecl],
    name: &str,
) -> Option<&'a SceneMaterialDecl> {
    materials.iter().find(|m| m.name == name)
}

/// 把加载来的材质表整体收敛：丢空名、同名后者覆盖前者、超上限截断。
pub fn normalize_all(materials: Vec<SceneMaterialDecl>) -> Vec<SceneMaterialDecl> {
    let mut out: Vec<SceneMaterialDecl> = Vec::new();
    for decl in materials {
        if decl.name.trim().is_empty() {
            continue;
        }
        let _ = upsert_material(&mut out, decl);
    }
    out
}

/// 场景侧的材质读写口：内联材质（`Node::material`）、场景材质表（`materials[]`）
/// 与节点引用（`materialRef`）三者的读写全部收在这里。
///
/// 为什么不在 `scene.rs` 里：那个文件已挂在体量门禁的债务白名单上（不得再长），
/// 而这些方法的主题恰好就是「材质」，与本文档同源，拆出来两边都更好读。
impl Scene {
    /// 读取物件当前生效的材质。
    ///
    /// 物件不存在返回 `None`；物件存在但没声明材质时返回缺省材质
    /// （与渲染侧「回退到网格资产材质 / 默认材质」的可见结果一致）。
    pub fn material_of(&self, name: &str) -> Option<SurfaceMaterial> {
        self.find(name)
            .map(|n| n.material.unwrap_or_default().sanitized())
    }

    /// 整体写入物件材质。写入前统一收敛（NaN / inf / 超界值不进场景）。
    ///
    /// 返回 `false` 表示物件不存在。
    pub fn set_material(&mut self, name: &str, material: SurfaceMaterial) -> bool {
        match self.find_mut(name) {
            Some(node) => {
                node.material = Some(material.sanitized());
                true
            }
            None => false,
        }
    }

    /// 只改 metallic / roughness，保留已有 emissive。
    pub fn set_material_params(&mut self, name: &str, metallic: f32, roughness: f32) -> bool {
        let mut material = self.material_of(name).unwrap_or_default();
        material.metallic = metallic;
        material.roughness = roughness;
        self.set_material(name, material)
    }

    /// 只改自发光（线性 RGB），保留 metallic / roughness。
    pub fn set_emissive(&mut self, name: &str, emissive: [f32; 3]) -> bool {
        let mut material = self.material_of(name).unwrap_or_default();
        material.emissive = emissive;
        self.set_material(name, material)
    }

    /// 撤销物件级材质覆盖，回退到网格资产 / 默认材质。返回 `false` 表示物件不存在。
    pub fn clear_material(&mut self, name: &str) -> bool {
        match self.find_mut(name) {
            Some(node) => {
                node.material = None;
                true
            }
            None => false,
        }
    }

    /// 设置物件的场景材质引用（M1-S4-A）。`None` 表示清除引用。
    ///
    /// 只做名字收敛与节点存在性判断，**不判材质是否在 `materials[]` 里**、也不判与内联
    /// 材质是否冲突：这两件事都是「文档层可见、可跨命令修正」的语义问题，交给校验器
    /// （`material.ref_missing` / `material.ref_conflict`）而不是写点。
    ///
    /// 返回值：`true` = 真的改动了；`false` = 物件不存在，或引用值未变。
    pub fn set_node_material_ref(&mut self, name: &str, material: Option<&str>) -> bool {
        let wanted = self::normalize_material_ref(material.map(|m| m.to_string()));
        match self.find_mut(name) {
            Some(node) if node.material_ref != wanted => {
                node.material_ref = wanted;
                true
            }
            _ => false,
        }
    }

    /// 清除物件的场景材质引用（回到「不引用」）。返回 `false` 表示物件不存在。
    pub fn clear_node_material_ref(&mut self, name: &str) -> bool {
        match self.find_mut(name) {
            Some(node) => {
                node.material_ref = None;
                true
            }
            None => false,
        }
    }

    /// 读物件的材质引用名字。物件不存在时返回 `None`（与「存在但没引用」同形，
    /// 需要区分时先用 [`Scene::node`] 判存在）。
    pub fn material_ref_of(&self, name: &str) -> Option<&str> {
        self.find(name)?.material_ref.as_deref()
    }

    /// 按名字取场景材质表的一行（M1-S4-A）。
    ///
    /// 表本身由 `upsert` 维护唯一性，但仍按「同名取第一条」实现：
    /// 手工编辑过的场景文件可能带重复行（校验器会报 `material.dup_name`），
    /// 这种情况下渲染不该 panic，取第一条即可。
    pub fn material_decl(&self, name: &str) -> Option<&SceneMaterialDecl> {
        self.materials.iter().find(|decl| decl.name == name)
    }

    /// 设置材质贴图的解析基准目录（M1-S4-A）。不进序列化，见字段注释。
    pub fn set_texture_base_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.texture_base_dir = dir;
    }

    /// 物件最终使用的场景材质（引用存在且表里能找到才返回）。
    ///
    /// 三种情况都返回 `None`：没引用、引用的名字在表里不存在（校验器会报
    /// `material.ref_missing`）、物件本身不存在。渲染层拿到 `None` 就走原路径。
    pub fn node_scene_material(&self, id: crate::NodeId) -> Option<&SceneMaterialDecl> {
        let node = self.get(id)?;
        self.material_decl(node.material_ref.as_deref()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(name: &str) -> SceneMaterialDecl {
        SceneMaterialDecl {
            name: name.to_string(),
            ..SceneMaterialDecl::default()
        }
    }

    #[test]
    fn 同名覆盖保持位置且只拦新增() {
        let mut list = Vec::new();
        assert!(upsert_material(&mut list, decl("brick")));
        assert!(upsert_material(&mut list, decl("wood")));
        let mut next = decl("brick");
        next.roughness = 0.2;
        assert!(upsert_material(&mut list, next));
        assert_eq!(list.len(), 2, "同名覆盖不该追加");
        assert_eq!(list[0].name, "brick", "覆盖应保持原位置");
        assert_eq!(list[0].roughness, 0.2);

        let mut full = Vec::new();
        for i in 0..SCENE_MATERIAL_MAX {
            assert!(upsert_material(&mut full, decl(&format!("m{i}"))));
        }
        assert!(
            !upsert_material(&mut full, decl("overflow")),
            "满载时新名字必须被拒"
        );
        let mut over = decl("m0");
        over.roughness = 0.9;
        assert!(upsert_material(&mut full, over), "满载不拦覆盖");
    }

    #[test]
    fn 空名被拒且收敛路径为空串归一成none() {
        let mut list = Vec::new();
        assert!(!upsert_material(&mut list, decl("   ")));
        assert!(list.is_empty());

        let mut d = decl("brick");
        d.albedo = Some("   ".to_string());
        d.normal = Some("tex\\brick_n.png".to_string());
        let d = d.sanitized();
        assert_eq!(d.albedo, None, "全空白路径应归一成 None");
        assert_eq!(
            d.normal.as_deref(),
            Some("tex/brick_n.png"),
            "反斜杠应统一成正斜杠"
        );
    }

    #[test]
    fn 标量走同一套收敛且uv缩放夹紧() {
        let mut d = decl("brick");
        d.metallic = f32::NAN;
        d.roughness = 5.0;
        d.emissive = [f32::INFINITY, -1.0, 0.5];
        d.uv_scale = [0.0, 100000.0];
        let d = d.sanitized();
        assert_eq!(d.metallic, 0.0, "NaN 回缺省");
        assert_eq!(d.roughness, 1.0, "超界夹到 1");
        assert_eq!(d.emissive[0], crate::material::MAX_EMISSIVE);
        assert_eq!(d.emissive[1], 0.0);
        assert_eq!(d.uv_scale[0], 1.0, "非正 uv 缩放回到不缩放");
        assert_eq!(d.uv_scale[1], UV_SCALE_MAX);
    }

    #[test]
    fn textures按槽顺序产出且跳过未声明() {
        let mut d = decl("brick");
        d.emissive_map = Some("tex/e.png".to_string());
        d.albedo = Some("tex/a.png".to_string());
        let slots: Vec<&str> = d.textures().map(|(slot, _)| slot).collect();
        assert_eq!(slots, vec!["albedo", "emissiveMap"]);
        assert_eq!(d.texture_count(), 2);
    }

    #[test]
    fn normalize_all丢空名与同名前项() {
        let mut a = decl("brick");
        a.roughness = 0.2;
        let mut b = decl("brick");
        b.roughness = 0.9;
        let out = normalize_all(vec![decl(""), a, b, decl("   ")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].roughness, 0.9, "同名后者覆盖前者");
    }
}
