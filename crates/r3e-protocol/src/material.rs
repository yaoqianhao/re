//! `scene.material.*` 的参数承载（M1-S4-D）。
//!
//! **与 `patch.rs` 的 `*Patch` 不是一套语义**，别混用：
//!
//! * `*Patch` 是「只给要改的字段，其余保持原值」——打补丁；
//! * [`MaterialUpsert`] 是「这一行材质从此长这样」——**整行声明**。
//!
//! 为什么材质表走整行声明而不是补丁：材质表按 `name` 是唯一主键，一行就是一份完整
//! 材质。若用「只给要改的字段」的补丁语义，`albedo: null` 到底是「清掉贴图」还是
//! 「别动贴图」就说不清（JSON 里 `null` 同时承担这两个意思），只能再引入一个
//! `Option<Option<String>>` 的三态，读起来更糟。整行声明把这件事一次说清：
//! 没给的字段取 [`MaterialUpsert::default`] 的缺省值，想保住现有贴图就把
//! `scene.material.list` 读到的值原样回传。
//!
//! 收敛（NaN / 越界 / 名字超长 / 路径去空白）仍然只在宿主侧做——协议层只搬值。

/// `scene.material.upsert` 的入参：一行材质表的完整声明。
///
/// 所有字段 `None` = 取缺省（三标量走 `SurfaceMaterial::default`、贴图不声明、
/// uv 不缩放）。`name` 必须给，收敛后为空由宿主判参数错误。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialUpsert {
    /// 唯一主键（`materialRef` 按它寻址）。
    pub name: String,
    /// 金属度 0..1；`None` = 缺省。
    pub metallic: Option<f32>,
    /// 粗糙度；`None` = 缺省。
    pub roughness: Option<f32>,
    /// 自发光（线性 RGB）；`None` = 缺省。
    pub emissive: Option<[f32; 3]>,
    /// 反照率贴图路径；`None` = 不声明这张贴图。
    pub albedo: Option<String>,
    /// 法线贴图路径。
    pub normal: Option<String>,
    /// 金属度 / 粗糙度打包贴图路径。
    pub metallic_roughness: Option<String>,
    /// 环境光遮蔽贴图路径。
    pub occlusion: Option<String>,
    /// 自发光贴图路径。
    pub emissive_map: Option<String>,
    /// uv 缩放；`None` = 不缩放。
    pub uv_scale: Option<[f32; 2]>,
}
