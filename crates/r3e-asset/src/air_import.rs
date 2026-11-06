//! AIR / AIRB 读取层：把插件落盘的中间格式读回 `AssetEntry`。
//!
//! # 权威定义
//!
//! 格式不由引擎定义，而由 ai-ide 的 `asset3d` 插件定义：
//!
//! * `D:\ai_ide\plugins\asset3d\src\air.rs` —— AIR v1（`.air.json`）；
//! * `D:\ai_ide\plugins\asset3d\src\air_bin.rs` —— AIRB 容器 v1（`.air.bin`）。
//!
//! 本模块**只读**，字段逐条镜像那两份结构。`tests/fixtures/air/` 里放的是插件真实
//! 导出的样本，`air_json_real_fixture` / `airb_real_fixture` 两个用例把两边口径锁在
//! 一起：插件字段一旦改名或增删，测试立刻红，不会出现「插件悄悄改了、引擎还在读旧布局」。
//!
//! # 校验口径
//!
//! * magic（`AIRB`）、`format`（`air` / `airb`）、`air_version`、`airb_version`
//!   全部硬校验，不认识就报错，绝不「猜着读」；
//! * 必填字段缺失交给 `serde` 报 `missing field`，统一转成
//!   [`AirLoadError::MetaJson`]；**只有插件自己也标了 `default` 的诊断性字段**
//!   （`issues` / `mime_type` / 可选引用）才允许缺省；
//! * 几何不变量（数组长度一致、索引不越界、材质分区不越界、骨骼槽位合法）
//!   在解析完成后统一校验，失败即 [`AirLoadError::Invalid`]。
//!
//! # 与既有导入器的关系
//!
//! 输出同样是 `AssetEntry`，渲染 / 物理 / 脚本拿到的结构与 `.fbx` / `.glb` 完全一致
//! （顶点已是世界空间、右手系 +Y up、UV 左上原点、CCW），因此不需要任何格式分支。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use r3e_core::anim::{AnimClip, AnimKey, AnimTrack};
use r3e_core::mesh::TextureImage;
use r3e_core::skin::{SkinData, SkinJoint};
use r3e_core::{AssetId, Mat4, Quat, Vec3};
use serde::Deserialize;
use thiserror::Error;

use crate::importer::AssetImporter;
use crate::{
    decode_image_bytes, AssetEntry, AssetError, MaterialInfo, MaterialPart, MeshAsset, NodeAsset,
    TextureInfo,
};

// ---------------------------------------------------------------------------
// 格式常量（与插件 `air.rs` / `air_bin.rs` 逐字对应）
// ---------------------------------------------------------------------------

/// AIR JSON 的 `format` 标记。
pub const AIR_FORMAT_TAG: &str = "air";
/// AIR 结构版本（插件 `air::AIR_VERSION`）。
pub const AIR_VERSION: u32 = 1;
/// AIRB 的 `format` 标记（meta 内的双保险）。
pub const AIRB_FORMAT_TAG: &str = "airb";
/// AIRB 容器 magic（插件 `air_bin::MAGIC`）。
pub const AIRB_MAGIC: [u8; 4] = *b"AIRB";
/// AIRB 容器版本（插件 `air_bin::VERSION`）。
pub const AIRB_VERSION: u32 = 1;
/// AIRB 头部长度：magic(4) + version(4) + meta_len(4) + blob_len(8)。
pub const AIRB_HEADER_LEN: usize = 20;

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// AIR 读取失败的原因。每一个分支都对应一种「宁可报错也不猜」的场景。
#[derive(Debug, Error)]
pub enum AirLoadError {
    #[error("AIR 数据为空（0 字节）")]
    Empty,
    #[error("既不是 AIRB（magic 不符）也不是 AIR JSON（首个非空字符不是 `{{`）")]
    UnknownFormat,
    #[error("AIRB magic 不匹配：期望 \"AIRB\"，实际 {found:?}")]
    BadMagic { found: [u8; 4] },
    #[error("AIRB 被截断：需要 {needed} 字节，实际只有 {available} 字节")]
    Truncated { needed: usize, available: usize },
    #[error("{kind} 版本不支持：文件声明 v{found}，本加载器只支持 v{supported}")]
    UnsupportedVersion {
        kind: &'static str,
        found: u32,
        supported: u32,
    },
    #[error("AIR 头 JSON 无法解析（含字段缺失）：{0}")]
    MetaJson(String),
    #[error("AIRB meta 声明的 blob 长度 {meta} 与头部 {header} 不一致")]
    BlobLengthMismatch { header: u64, meta: u64 },
    #[error("AIRB span 数量与网格数量不一致：meta.meshes={spans}，air.meshes={meshes}")]
    SpanCountMismatch { spans: usize, meshes: usize },
    #[error("AIRB 几何 span 越界：mesh #{mesh} 的 {section} 需要读到 blob 第 {needed} 字节，blob 只有 {blob_len} 字节")]
    SpanOutOfBounds {
        mesh: usize,
        section: &'static str,
        needed: usize,
        blob_len: usize,
    },
    #[error("AIR 内容不合法：{0}")]
    Invalid(String),
}

impl AirLoadError {
    /// 包成 [`AssetError::Load`]，让调用方只面对一种错误类型。
    pub fn into_asset_error(self, path: &Path) -> AssetError {
        AssetError::Load {
            path: path.display().to_string(),
            message: self.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// 结构镜像（只反序列化；字段缺省策略与插件严格一致）
// ---------------------------------------------------------------------------

/// 一次导入的完整产出（AIR v1）。
#[derive(Debug, Clone, Deserialize)]
pub struct AirDoc {
    /// 固定 `"air"`。
    pub format: String,
    pub air_version: u32,
    pub generator: String,
    pub source: AirSource,
    pub units: AirUnits,
    #[serde(default)]
    pub nodes: Vec<AirNode>,
    #[serde(default)]
    pub meshes: Vec<AirMesh>,
    #[serde(default)]
    pub materials: Vec<AirMaterial>,
    #[serde(default)]
    pub textures: Vec<AirTexture>,
    #[serde(default)]
    pub skins: Vec<AirSkin>,
    #[serde(default)]
    pub animations: Vec<AirAnimation>,
    pub stats: AirStats,
    #[serde(default)]
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirSource {
    pub path: String,
    pub container: String,
    pub bytes: u64,
    #[serde(default)]
    pub native_unit_meters: Option<f32>,
    #[serde(default)]
    pub native_up_axis: Option<String>,
    #[serde(default)]
    pub creator: Option<String>,
}

/// 归一化后的坐标与单位约定（随数据自带，消费方不必猜）。
#[derive(Debug, Clone, Deserialize)]
pub struct AirUnits {
    pub meters_per_unit: f32,
    pub up_axis: String,
    pub front_axis: String,
    pub right_axis: String,
    pub uv_origin: String,
    pub winding: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirNode {
    pub index: u32,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub parent: Option<u32>,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    #[serde(default)]
    pub mesh: Option<u32>,
    #[serde(default)]
    pub skin: Option<u32>,
}

/// 一个网格：顶点属性平铺存放，`material_parts` 划分三角形区段。
#[derive(Debug, Clone, Deserialize)]
pub struct AirMesh {
    pub name: String,
    /// 挂载它的节点路径（空表示场景根）。
    pub node: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// 三角形索引，CCW。
    pub indices: Vec<u32>,
    #[serde(default)]
    pub material_parts: Vec<AirMaterialPart>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    #[serde(default)]
    pub joints: Vec<[u16; 4]>,
    #[serde(default)]
    pub weights: Vec<[f32; 4]>,
    #[serde(default)]
    pub skin: Option<u32>,
    pub source_vertex_count: u32,
    pub source_face_count: u32,
    pub source_triangle_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirMaterialPart {
    #[serde(default)]
    pub material: Option<u32>,
    pub material_name: String,
    pub triangle_offset: u32,
    pub triangle_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirMaterial {
    pub index: u32,
    pub name: String,
    /// 线性空间 RGBA。
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    #[serde(default)]
    pub base_color_texture: Option<u32>,
    /// 法线贴图（扩展二十一；线性数据）。
    #[serde(default)]
    pub normal_texture: Option<u32>,
    /// 粗糙度贴图（扩展二十一；线性数据，取 G 通道）。
    #[serde(default)]
    pub roughness_texture: Option<u32>,
    /// AO 贴图（扩展二十一；线性数据，取 R 通道）。
    #[serde(default)]
    pub occlusion_texture: Option<u32>,
    #[serde(default)]
    pub double_sided: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirTexture {
    pub index: u32,
    pub name: String,
    pub slot: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// 原始编码后的字节数（不是像素量）。
    pub bytes: u64,
    pub embedded: bool,
    /// 随 AIR 一起携带的贴图（base64）。
    #[serde(default)]
    pub data_base64: Option<String>,
    /// 外部贴图的原始路径。
    #[serde(default)]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirSkin {
    pub index: u32,
    pub name: String,
    /// 关节 = 场景节点下标。
    pub joints: Vec<u32>,
    /// 逆绑定矩阵（列主序 4x4），world → 骨骼绑定空间。
    pub inverse_bind_matrices: Vec<[f32; 16]>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirAnimation {
    pub name: String,
    pub duration: f32,
    pub tracks: Vec<AirTrack>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirTrack {
    pub node: u32,
    /// `translation` / `rotation` / `scale`。
    pub property: String,
    pub times: Vec<f32>,
    pub values: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirStats {
    pub node_count: u32,
    pub mesh_count: u32,
    pub vertex_count: u64,
    pub triangle_count: u64,
    pub material_count: u32,
    pub texture_count: u32,
    pub skin_count: u32,
    pub joint_count: u32,
    pub animation_count: u32,
    pub geometry_bytes: u64,
}

/// AIRB 的 JSON 头（`air.meshes[i]` 的几何数组已被清空，只留 span）。
#[derive(Debug, Clone, Deserialize)]
pub struct AirbMeta {
    /// 固定 `"airb"`。
    pub format: String,
    pub airb_version: u32,
    pub blob_bytes: u64,
    pub meshes: Vec<MeshSpans>,
    pub air: AirDoc,
}

/// blob 内一段数组的位置。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Span {
    /// 相对 blob 起点的字节偏移。
    pub offset: u32,
    /// 元素个数（不是字节数）。
    pub count: u32,
}

/// 一个网格在 blob 内的六段数组位置。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct MeshSpans {
    pub positions: Span,
    pub normals: Span,
    pub uvs: Span,
    pub indices: Span,
    pub joints: Span,
    pub weights: Span,
}

/// 每个元素的字节数（与插件 `air_bin.rs` 的 `push_*` 一一对应）。
const ELEM_POSITION: usize = 12;
const ELEM_NORMAL: usize = 12;
const ELEM_UV: usize = 8;
const ELEM_INDEX: usize = 4;
const ELEM_JOINT: usize = 8;
const ELEM_WEIGHT: usize = 16;

// ---------------------------------------------------------------------------
// 解析入口
// ---------------------------------------------------------------------------

/// 数据是否以 AIRB magic 开头。
pub fn looks_like_airb(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == AIRB_MAGIC
}

/// 数据是否像一个 JSON 对象（跳过 UTF-8 BOM 与空白）。
pub fn looks_like_air_json(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    if rest.starts_with(&[0xEF, 0xBB, 0xBF]) {
        rest = &rest[3..];
    }
    rest.iter()
        .find(|b| !b.is_ascii_whitespace())
        .map(|b| *b == b'{')
        .unwrap_or(false)
}

/// 按内容（而不是扩展名）判定格式并解析：先看 magic，再看 JSON 首字符。
///
/// 之所以不看扩展名：`.air.json` 的扩展名是 `json`、`.air.bin` 是 `bin`，
/// 靠名字分派既不可靠也容易被改名绕过。
pub fn parse_bytes(bytes: &[u8]) -> Result<AirDoc, AirLoadError> {
    if bytes.is_empty() {
        return Err(AirLoadError::Empty);
    }
    if looks_like_airb(bytes) {
        parse_airb(bytes)
    } else if looks_like_air_json(bytes) {
        parse_json_doc(bytes)
    } else {
        Err(AirLoadError::UnknownFormat)
    }
}

/// 解析 `.air.json` 文本。
pub fn parse_json_doc(bytes: &[u8]) -> Result<AirDoc, AirLoadError> {
    if bytes.is_empty() {
        return Err(AirLoadError::Empty);
    }
    let doc: AirDoc =
        serde_json::from_slice(bytes).map_err(|err| AirLoadError::MetaJson(format!("{err}")))?;
    check_header(&doc)?;
    validate_doc(&doc)?;
    Ok(doc)
}

/// 解析 `.air.bin`（AIRB v1）：
/// magic + version + meta_len + blob_len 头，随后是 meta JSON 与平铺几何块。
pub fn parse_airb(bytes: &[u8]) -> Result<AirDoc, AirLoadError> {
    if bytes.is_empty() {
        return Err(AirLoadError::Empty);
    }
    if bytes.len() < AIRB_HEADER_LEN {
        return Err(AirLoadError::Truncated {
            needed: AIRB_HEADER_LEN,
            available: bytes.len(),
        });
    }
    if bytes[..4] != AIRB_MAGIC {
        let mut found = [0u8; 4];
        found.copy_from_slice(&bytes[..4]);
        return Err(AirLoadError::BadMagic { found });
    }

    let version = u32::from_le_bytes(read4(bytes, 4));
    if version != AIRB_VERSION {
        return Err(AirLoadError::UnsupportedVersion {
            kind: "AIRB 容器",
            found: version,
            supported: AIRB_VERSION,
        });
    }
    let meta_len = u32::from_le_bytes(read4(bytes, 8)) as usize;
    let blob_len = u64::from_le_bytes(read8(bytes, 12));

    let meta_end = AIRB_HEADER_LEN
        .checked_add(meta_len)
        .ok_or(AirLoadError::Truncated {
            needed: usize::MAX,
            available: bytes.len(),
        })?;
    let blob_start = meta_end;
    let needed = blob_start
        .checked_add(usize::try_from(blob_len).unwrap_or(usize::MAX))
        .ok_or(AirLoadError::Truncated {
            needed: usize::MAX,
            available: bytes.len(),
        })?;
    if needed > bytes.len() {
        return Err(AirLoadError::Truncated {
            needed,
            available: bytes.len(),
        });
    }

    let mut meta: AirbMeta = serde_json::from_slice(&bytes[AIRB_HEADER_LEN..meta_end])
        .map_err(|err| AirLoadError::MetaJson(format!("{err}")))?;

    if meta.format != AIRB_FORMAT_TAG {
        return Err(AirLoadError::Invalid(format!(
            "AIRB meta 的 format 应为 \"airb\"，实际 \"{}\"",
            meta.format
        )));
    }
    if meta.airb_version != AIRB_VERSION {
        return Err(AirLoadError::UnsupportedVersion {
            kind: "AIRB meta",
            found: meta.airb_version,
            supported: AIRB_VERSION,
        });
    }
    if meta.blob_bytes != blob_len {
        return Err(AirLoadError::BlobLengthMismatch {
            header: blob_len,
            meta: meta.blob_bytes,
        });
    }
    if meta.meshes.len() != meta.air.meshes.len() {
        return Err(AirLoadError::SpanCountMismatch {
            spans: meta.meshes.len(),
            meshes: meta.air.meshes.len(),
        });
    }

    let blob = &bytes[blob_start..needed];
    for (mesh_index, spans) in meta.meshes.iter().enumerate() {
        let mesh = &mut meta.air.meshes[mesh_index];
        fill_f32x3(
            blob,
            mesh_index,
            "positions",
            spans.positions,
            ELEM_POSITION,
            &mut mesh.positions,
        )?;
        fill_f32x3(
            blob,
            mesh_index,
            "normals",
            spans.normals,
            ELEM_NORMAL,
            &mut mesh.normals,
        )?;
        fill_f32x2(blob, mesh_index, "uvs", spans.uvs, &mut mesh.uvs)?;
        fill_u32(
            blob,
            mesh_index,
            "indices",
            spans.indices,
            &mut mesh.indices,
        )?;
        fill_u16x4(blob, mesh_index, "joints", spans.joints, &mut mesh.joints)?;
        fill_f32x4(
            blob,
            mesh_index,
            "weights",
            spans.weights,
            &mut mesh.weights,
        )?;
    }

    check_header(&meta.air)?;
    validate_doc(&meta.air)?;
    Ok(meta.air)
}

fn read4(bytes: &[u8], at: usize) -> [u8; 4] {
    let mut out = [0u8; 4];
    out.copy_from_slice(&bytes[at..at + 4]);
    out
}

fn read8(bytes: &[u8], at: usize) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes[at..at + 8]);
    out
}

/// 取 blob 内一段连续字节；越界时报出「本段需要读到第几字节」，便于定位截断点。
fn section<'a>(
    blob: &'a [u8],
    mesh: usize,
    name: &'static str,
    span: Span,
    elem_size: usize,
) -> Result<&'a [u8], AirLoadError> {
    if span.count == 0 {
        return Ok(&[]);
    }
    let start = span.offset as usize;
    let len =
        (span.count as usize)
            .checked_mul(elem_size)
            .ok_or(AirLoadError::SpanOutOfBounds {
                mesh,
                section: name,
                needed: usize::MAX,
                blob_len: blob.len(),
            })?;
    let end = start
        .checked_add(len)
        .ok_or(AirLoadError::SpanOutOfBounds {
            mesh,
            section: name,
            needed: usize::MAX,
            blob_len: blob.len(),
        })?;
    if end > blob.len() {
        return Err(AirLoadError::SpanOutOfBounds {
            mesh,
            section: name,
            needed: end,
            blob_len: blob.len(),
        });
    }
    Ok(&blob[start..end])
}

fn fill_f32x3(
    blob: &[u8],
    mesh: usize,
    name: &'static str,
    span: Span,
    elem_size: usize,
    out: &mut Vec<[f32; 3]>,
) -> Result<(), AirLoadError> {
    debug_assert_eq!(elem_size, ELEM_POSITION, "vec3 的磁盘宽度必须是 12 字节");
    let bytes = section(blob, mesh, name, span, elem_size)?;
    out.clear();
    out.reserve(bytes.len() / ELEM_POSITION);
    for chunk in bytes.chunks_exact(ELEM_POSITION) {
        out.push([
            f32::from_le_bytes(read4(chunk, 0)),
            f32::from_le_bytes(read4(chunk, 4)),
            f32::from_le_bytes(read4(chunk, 8)),
        ]);
    }
    Ok(())
}

fn fill_f32x2(
    blob: &[u8],
    mesh: usize,
    name: &'static str,
    span: Span,
    out: &mut Vec<[f32; 2]>,
) -> Result<(), AirLoadError> {
    let bytes = section(blob, mesh, name, span, ELEM_UV)?;
    out.clear();
    out.reserve(bytes.len() / ELEM_UV);
    for chunk in bytes.chunks_exact(ELEM_UV) {
        out.push([
            f32::from_le_bytes(read4(chunk, 0)),
            f32::from_le_bytes(read4(chunk, 4)),
        ]);
    }
    Ok(())
}

fn fill_f32x4(
    blob: &[u8],
    mesh: usize,
    name: &'static str,
    span: Span,
    out: &mut Vec<[f32; 4]>,
) -> Result<(), AirLoadError> {
    let bytes = section(blob, mesh, name, span, ELEM_WEIGHT)?;
    out.clear();
    out.reserve(bytes.len() / ELEM_WEIGHT);
    for chunk in bytes.chunks_exact(ELEM_WEIGHT) {
        out.push([
            f32::from_le_bytes(read4(chunk, 0)),
            f32::from_le_bytes(read4(chunk, 4)),
            f32::from_le_bytes(read4(chunk, 8)),
            f32::from_le_bytes(read4(chunk, 12)),
        ]);
    }
    Ok(())
}

fn fill_u32(
    blob: &[u8],
    mesh: usize,
    name: &'static str,
    span: Span,
    out: &mut Vec<u32>,
) -> Result<(), AirLoadError> {
    let bytes = section(blob, mesh, name, span, ELEM_INDEX)?;
    out.clear();
    out.reserve(bytes.len() / ELEM_INDEX);
    for chunk in bytes.chunks_exact(ELEM_INDEX) {
        out.push(u32::from_le_bytes(read4(chunk, 0)));
    }
    Ok(())
}

fn fill_u16x4(
    blob: &[u8],
    mesh: usize,
    name: &'static str,
    span: Span,
    out: &mut Vec<[u16; 4]>,
) -> Result<(), AirLoadError> {
    let bytes = section(blob, mesh, name, span, ELEM_JOINT)?;
    out.clear();
    out.reserve(bytes.len() / ELEM_JOINT);
    for chunk in bytes.chunks_exact(ELEM_JOINT) {
        let mut slot = [0u16; 4];
        for (k, item) in slot.iter_mut().enumerate() {
            *item = u16::from_le_bytes([chunk[k * 2], chunk[k * 2 + 1]]);
        }
        out.push(slot);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

/// 头部与版本校验：`format` / `air_version` 不符一律拒绝。
pub fn check_header(doc: &AirDoc) -> Result<(), AirLoadError> {
    if doc.format != AIR_FORMAT_TAG {
        return Err(AirLoadError::Invalid(format!(
            "AIR 的 format 应为 \"air\"，实际 \"{}\"",
            doc.format
        )));
    }
    if doc.air_version != AIR_VERSION {
        return Err(AirLoadError::UnsupportedVersion {
            kind: "AIR",
            found: doc.air_version,
            supported: AIR_VERSION,
        });
    }
    Ok(())
}

/// 几何与引用一致性校验：宁可拒绝，也不把坏数据塞给渲染链。
pub fn validate_doc(doc: &AirDoc) -> Result<(), AirLoadError> {
    for (mi, mesh) in doc.meshes.iter().enumerate() {
        let vcount = mesh.positions.len();
        if !mesh.normals.is_empty() && mesh.normals.len() != vcount {
            return Err(AirLoadError::Invalid(format!(
                "mesh #{mi} `{}` 的 normals 有 {} 个，positions 有 {vcount} 个（必须等长）",
                mesh.name,
                mesh.normals.len()
            )));
        }
        if !mesh.uvs.is_empty() && mesh.uvs.len() != vcount {
            return Err(AirLoadError::Invalid(format!(
                "mesh #{mi} `{}` 的 uvs 有 {} 个，positions 有 {vcount} 个（必须为空或等长）",
                mesh.name,
                mesh.uvs.len()
            )));
        }
        if mesh.indices.len() % 3 != 0 {
            return Err(AirLoadError::Invalid(format!(
                "mesh #{mi} `{}` 的索引数 {} 不是 3 的倍数（文件可能被截断）",
                mesh.name,
                mesh.indices.len()
            )));
        }
        if let Some(max) = mesh.indices.iter().copied().max() {
            if max as usize >= vcount {
                return Err(AirLoadError::Invalid(format!(
                    "mesh #{mi} `{}` 索引越界：最大索引 {max} ≥ 顶点数 {vcount}",
                    mesh.name
                )));
            }
        }
        if !mesh.joints.is_empty() && mesh.joints.len() != vcount {
            return Err(AirLoadError::Invalid(format!(
                "mesh #{mi} `{}` 的 joints 有 {} 个，positions 有 {vcount} 个（必须等长）",
                mesh.name,
                mesh.joints.len()
            )));
        }
        if !mesh.weights.is_empty() && mesh.weights.len() != mesh.joints.len() {
            return Err(AirLoadError::Invalid(format!(
                "mesh #{mi} `{}` 的 weights 有 {} 个，joints 有 {} 个（必须等长）",
                mesh.name,
                mesh.weights.len(),
                mesh.joints.len()
            )));
        }

        let triangles = (mesh.indices.len() / 3) as u32;
        for (pi, part) in mesh.material_parts.iter().enumerate() {
            if let Some(material) = part.material {
                if material as usize >= doc.materials.len() {
                    return Err(AirLoadError::Invalid(format!(
                        "mesh #{mi} `{}` 的分区 #{pi} 引用材质 {material}，但只有 {} 个材质",
                        mesh.name,
                        doc.materials.len()
                    )));
                }
            }
            let end = part.triangle_offset.saturating_add(part.triangle_count);
            if end > triangles {
                return Err(AirLoadError::Invalid(format!(
                    "mesh #{mi} `{}` 的分区 #{pi} 覆盖三角形 {}..{end}，超过总数 {triangles}",
                    mesh.name, part.triangle_offset
                )));
            }
        }

        if let Some(skin) = mesh.skin {
            let Some(bind) = doc.skins.get(skin as usize) else {
                return Err(AirLoadError::Invalid(format!(
                    "mesh #{mi} `{}` 引用骨骼表 {skin}，但只有 {} 个骨骼表",
                    mesh.name,
                    doc.skins.len()
                )));
            };
            let slots = bind.joints.len();
            if !mesh.joints.is_empty() {
                for (vi, slot) in mesh.joints.iter().enumerate() {
                    for joint in slot {
                        if *joint as usize >= slots {
                            return Err(AirLoadError::Invalid(format!(
                                "mesh #{mi} `{}` 顶点 {vi} 的关节槽位 {joint} 超过骨骼表 {skin} 的关节数 {slots}",
                                mesh.name
                            )));
                        }
                    }
                }
            }
        }
    }

    for (ti, texture) in doc.textures.iter().enumerate() {
        if texture.index != ti as u32 {
            return Err(AirLoadError::Invalid(format!(
                "贴图表第 {ti} 项的 index 是 {}（必须等于自身下标）",
                texture.index
            )));
        }
    }

    for (mi, material) in doc.materials.iter().enumerate() {
        if let Some(texture) = material.base_color_texture {
            if texture as usize >= doc.textures.len() {
                return Err(AirLoadError::Invalid(format!(
                    "材质 #{mi} `{}` 引用贴图 {texture}，但只有 {} 张贴图",
                    material.name,
                    doc.textures.len()
                )));
            }
        }
    }

    for (si, skin) in doc.skins.iter().enumerate() {
        if skin.joints.len() != skin.inverse_bind_matrices.len() {
            return Err(AirLoadError::Invalid(format!(
                "骨骼表 #{si} `{}` 的关节数 {} 与逆绑定矩阵数 {} 不一致",
                skin.name,
                skin.joints.len(),
                skin.inverse_bind_matrices.len()
            )));
        }
        for joint in &skin.joints {
            if *joint as usize >= doc.nodes.len() {
                return Err(AirLoadError::Invalid(format!(
                    "骨骼表 #{si} `{}` 引用节点 {joint}，但只有 {} 个节点",
                    skin.name,
                    doc.nodes.len()
                )));
            }
        }
    }

    for (ai, anim) in doc.animations.iter().enumerate() {
        if !anim.duration.is_finite() || anim.duration < 0.0 {
            return Err(AirLoadError::Invalid(format!(
                "动画 #{ai} `{}` 的时长 {} 非法",
                anim.name, anim.duration
            )));
        }
        for (ti, track) in anim.tracks.iter().enumerate() {
            if track.node as usize >= doc.nodes.len() {
                return Err(AirLoadError::Invalid(format!(
                    "动画 #{ai} `{}` 的轨道 #{ti} 引用节点 {}，但只有 {} 个节点",
                    anim.name,
                    track.node,
                    doc.nodes.len()
                )));
            }
            if track.times.len() != track.values.len() {
                return Err(AirLoadError::Invalid(format!(
                    "动画 #{ai} `{}` 的轨道 #{ti} 时间点数 {} 与值数 {} 不一致",
                    anim.name,
                    track.times.len(),
                    track.values.len()
                )));
            }
            if !matches!(
                track.property.as_str(),
                "translation" | "rotation" | "scale"
            ) {
                return Err(AirLoadError::Invalid(format!(
                    "动画 #{ai} `{}` 的轨道 #{ti} 属性 \"{}\" 不受支持（只认 translation / rotation / scale）",
                    anim.name, track.property
                )));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// 转换：AIR → AssetEntry
// ---------------------------------------------------------------------------

/// 把已解析的 AIR 文档转成引擎的 [`AssetEntry`]。
///
/// `base_dir` 用于外部贴图（`data_base64` 缺失且 `source_path` 是相对路径）的回查找，
/// 传 `None` 表示不碰文件系统。
pub fn to_asset_entry(
    doc: &AirDoc,
    id: AssetId,
    source_path: &str,
    base_dir: Option<&Path>,
) -> AssetEntry {
    let textures = convert_textures(doc, base_dir);
    let materials = convert_materials(doc, &textures);
    let meshes = convert_meshes(doc);
    let nodes = convert_nodes(doc);
    let skins = convert_skins(doc, &nodes);
    let animations = convert_animations(doc, &nodes);

    if !doc.issues.is_empty() {
        for issue in &doc.issues {
            log::warn!("AIR `{source_path}` 导入期问题：{issue}");
        }
    }

    AssetEntry {
        id,
        source_path: source_path.to_string(),
        meshes,
        materials,
        textures,
        nodes,
        animations,
        skins,
    }
}

fn convert_textures(doc: &AirDoc, base_dir: Option<&Path>) -> Vec<TextureInfo> {
    let mut out = Vec::with_capacity(doc.textures.len());
    for texture in &doc.textures {
        // 1) 随 AIR 内嵌的 base64 优先；2) 否则按 source_path 去源文件旁找。
        let embedded_bytes = texture
            .data_base64
            .as_deref()
            .and_then(|encoded| decode_base64(encoded).ok());
        let mut image: Option<Arc<TextureImage>> = embedded_bytes
            .as_deref()
            .and_then(|bytes| decode_image_bytes(bytes, Some(&texture.name)));
        if image.is_none() {
            if let (Some(dir), Some(relative)) = (base_dir, texture.source_path.as_deref()) {
                let candidate = resolve_relative(dir, relative);
                if candidate.is_file() {
                    match std::fs::read(&candidate) {
                        Ok(bytes) => {
                            image = decode_image_bytes(&bytes, Some(&texture.name));
                        }
                        Err(err) => {
                            log::debug!("AIR: 读外部贴图 `{}` 失败：{err}", candidate.display())
                        }
                    }
                }
            }
        }
        if image.is_none() {
            log::debug!(
                "AIR: 贴图 `{}` 没有可用像素（embedded={}, 携带数据={}），渲染将回退纯色",
                texture.name,
                texture.embedded,
                texture.data_base64.is_some()
            );
        }

        out.push(TextureInfo {
            index: texture.index,
            // AIR 没有 ufbx 意义上的元素 id，用贴图下标充当，跨材质去重语义不变。
            element_id: texture.index,
            slot: texture.slot.clone(),
            filename: texture.name.clone(),
            relative_filename: texture.source_path.clone().unwrap_or_default(),
            absolute_filename: texture
                .source_path
                .as_deref()
                .and_then(|relative| base_dir.map(|dir| resolve_relative(dir, relative)))
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            embedded: texture.embedded,
            content_bytes: embedded_bytes
                .as_ref()
                .map(|bytes| bytes.len())
                .unwrap_or(texture.bytes as usize),
            has_file: texture.embedded || texture.source_path.is_some(),
            uv_set: "UV0".to_string(),
            image,
        });
    }
    out
}

fn resolve_relative(dir: &Path, relative: &str) -> std::path::PathBuf {
    let normalized = relative.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    }
}

fn convert_materials(doc: &AirDoc, textures: &[TextureInfo]) -> Vec<MaterialInfo> {
    let mut out = Vec::with_capacity(doc.materials.len());
    for material in &doc.materials {
        let mut info = MaterialInfo {
            index: material.index,
            name: material.name.clone(),
            base_color: material.base_color,
            metallic: material.metallic,
            roughness: material.roughness,
            textures: Vec::new(),
            base_color_texture: material.base_color_texture,
            normal_texture: material.normal_texture,
            roughness_texture: material.roughness_texture,
            occlusion_texture: material.occlusion_texture,
        };
        for slot in [
            material.base_color_texture,
            material.normal_texture,
            material.roughness_texture,
            material.occlusion_texture,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(texture) = textures.get(slot as usize) {
                info.textures.push(texture.clone());
            }
        }
        out.push(info);
    }
    out
}

fn convert_meshes(doc: &AirDoc) -> Vec<MeshAsset> {
    let mut out = Vec::with_capacity(doc.meshes.len());
    for (mi, mesh) in doc.meshes.iter().enumerate() {
        // 节点局部矩阵只是元数据（顶点已是世界空间，与 FBX / glTF 一致）：
        // 优先用「反向引用」（node.mesh == mi），退化为按路径匹配，再退化单位阵。
        let owner = doc
            .nodes
            .iter()
            .find(|node| node.mesh == Some(mi as u32))
            .or_else(|| doc.nodes.iter().find(|node| node.path == mesh.node));
        let node_local_matrix = owner
            .map(node_trs)
            .unwrap_or(Mat4::IDENTITY)
            .to_cols_array_2d();

        out.push(MeshAsset {
            name: mesh.name.clone(),
            node_path: mesh.node.clone(),
            positions: mesh.positions.clone(),
            normals: mesh.normals.clone(),
            uvs: mesh.uvs.clone(),
            indices: mesh.indices.clone(),
            material_parts: mesh
                .material_parts
                .iter()
                .map(|part| MaterialPart {
                    material_index: part.material,
                    material_name: part.material_name.clone(),
                    triangle_offset: part.triangle_offset,
                    triangle_count: part.triangle_count,
                })
                .collect(),
            aabb_min: mesh.aabb_min,
            aabb_max: mesh.aabb_max,
            node_local_matrix,
            source_path: doc.source.path.clone(),
            joints: mesh.joints.clone(),
            weights: mesh.weights.clone(),
            skin: mesh.skin,
            source_vertex_count: mesh.source_vertex_count as usize,
            source_face_count: mesh.source_face_count as usize,
            source_triangle_count: mesh.source_triangle_count as usize,
        });
    }
    out
}

fn convert_nodes(doc: &AirDoc) -> Vec<NodeAsset> {
    doc.nodes
        .iter()
        .map(|node| NodeAsset {
            index: node.index,
            name: node.name.clone(),
            path: node.path.clone(),
            parent: node.parent,
            position: node.position,
            rotation: node.rotation,
            scale: node.scale,
            mesh: node.mesh,
        })
        .collect()
}

/// 节点 TRS 组合成 4x4（列主序）。
fn node_trs(node: &AirNode) -> Mat4 {
    let rotation = sanitized_quat(node.rotation);
    Mat4::from_scale_rotation_translation(
        Vec3::from(node.scale),
        rotation,
        Vec3::from(node.position),
    )
}

fn sanitized_quat(rotation: [f32; 4]) -> Quat {
    let quat = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);
    if quat.is_finite() && quat.length_squared() > 1e-12 {
        quat.normalize()
    } else {
        Quat::IDENTITY
    }
}

fn convert_skins(doc: &AirDoc, nodes: &[NodeAsset]) -> Vec<SkinData> {
    let mut out = Vec::with_capacity(doc.skins.len());
    for (si, skin) in doc.skins.iter().enumerate() {
        let mut joints = Vec::with_capacity(skin.joints.len());
        for (ji, node_index) in skin.joints.iter().enumerate() {
            // AIR 给的是 world → bind 的逆绑定矩阵，引擎要的是 bind 世界矩阵，
            // 两者互为逆；数值病态时退回单位阵并告警（引擎实际只用静止姿态，不影响求解）。
            // 插件里是列主序平铺的 16 个 f32，`from_cols_array` 正是这个布局。
            let inverse_bind = Mat4::from_cols_array(&skin.inverse_bind_matrices[ji]);
            let bind_world = inverse_bind.inverse();
            let bind_world = if bind_world.is_finite() {
                bind_world
            } else {
                log::warn!(
                    "AIR: 骨骼表 #{si} `{}` 的第 {ji} 个逆绑定矩阵不可逆，bind_world 退化为单位阵",
                    skin.name
                );
                Mat4::IDENTITY
            };
            joints.push(SkinJoint {
                node_index: *node_index,
                path: nodes
                    .get(*node_index as usize)
                    .map(|node| node.path.clone())
                    .unwrap_or_else(|| format!("node{node_index}")),
                bind_world,
            });
        }
        out.push(SkinData {
            name: skin.name.clone(),
            joints,
        });
    }
    out
}

fn convert_animations(doc: &AirDoc, nodes: &[NodeAsset]) -> Vec<AnimClip> {
    let mut out = Vec::with_capacity(doc.animations.len());
    for anim in &doc.animations {
        // node → (translation, rotation, scale) 三条轨道分组
        let mut grouped: BTreeMap<u32, AnimTracks> = BTreeMap::new();
        for track in &anim.tracks {
            let entry = grouped.entry(track.node).or_default();
            match track.property.as_str() {
                "translation" => entry.translation = Some(track),
                "rotation" => entry.rotation = Some(track),
                "scale" => entry.scale = Some(track),
                // validate_doc 已挡掉未知属性，这里只是防御。
                other => log::debug!("AIR: 动画轨道属性 `{other}` 已被校验阶段拒绝"),
            }
        }

        let mut tracks = Vec::with_capacity(grouped.len());
        for (node_index, group) in grouped {
            let bind = nodes.get(node_index as usize);
            let default_position = bind.map(|n| n.position).unwrap_or([0.0; 3]);
            let default_rotation = bind.map(|n| n.rotation).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let default_scale = bind.map(|n| n.scale).unwrap_or([1.0; 3]);

            // 时间轴取三条轨道的并集：任一属性缺关键帧时用绑定姿态补齐，避免抖动。
            let mut times: Vec<f32> = Vec::new();
            for track in [group.translation, group.rotation, group.scale]
                .into_iter()
                .flatten()
            {
                times.extend(track.times.iter().copied().filter(|t| t.is_finite()));
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            times.dedup_by(|a, b| (*a - *b).abs() <= 1e-6);

            let mut keys = Vec::with_capacity(times.len());
            for time in times {
                let position = group
                    .translation
                    .and_then(|track| sample_vec3(track, time))
                    .unwrap_or(default_position);
                let rotation = group
                    .rotation
                    .and_then(|track| sample_quat(track, time))
                    .map(|q| [q.x, q.y, q.z, q.w])
                    .unwrap_or(default_rotation);
                let scale = group
                    .scale
                    .and_then(|track| sample_vec3(track, time))
                    .unwrap_or(default_scale);
                keys.push(AnimKey {
                    time,
                    position: Vec3::from(position),
                    rotation: sanitized_quat(rotation),
                    scale: Vec3::from(scale),
                });
            }

            tracks.push(AnimTrack {
                node_path: bind.map(|n| n.path.clone()).unwrap_or_default(),
                node_index,
                keys,
            });
        }

        out.push(AnimClip {
            name: anim.name.clone(),
            duration: anim.duration,
            tracks,
        });
    }
    out
}

#[derive(Default, Clone, Copy)]
struct AnimTracks<'a> {
    translation: Option<&'a AirTrack>,
    rotation: Option<&'a AirTrack>,
    scale: Option<&'a AirTrack>,
}

/// 定位 `time` 落在哪两个关键帧之间，返回 `(左, 右, 插值权重)`。
fn bracket(times: &[f32], time: f32) -> Option<(usize, usize, f32)> {
    if times.is_empty() {
        return None;
    }
    let last = times.len() - 1;
    if time <= times[0] {
        return Some((0, 0, 0.0));
    }
    if time >= times[last] {
        return Some((last, last, 0.0));
    }
    let mut lo = 0usize;
    let mut hi = last;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if times[mid] <= time {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let span = times[hi] - times[lo];
    let weight = if span > 1e-6 {
        ((time - times[lo]) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some((lo, hi, weight))
}

fn sample_vec3(track: &AirTrack, time: f32) -> Option<[f32; 3]> {
    let (lo, hi, weight) = bracket(&track.times, time)?;
    let a = track.values.get(lo)?;
    let b = track.values.get(hi)?;
    Some([
        a[0] * (1.0 - weight) + b[0] * weight,
        a[1] * (1.0 - weight) + b[1] * weight,
        a[2] * (1.0 - weight) + b[2] * weight,
    ])
}

fn sample_quat(track: &AirTrack, time: f32) -> Option<Quat> {
    let (lo, hi, weight) = bracket(&track.times, time)?;
    let a = track.values.get(lo)?;
    let b = track.values.get(hi)?;
    let qa = Quat::from_xyzw(a[0], a[1], a[2], a[3]);
    let qb = Quat::from_xyzw(b[0], b[1], b[2], b[3]);
    if !qa.is_finite() || !qb.is_finite() {
        return None;
    }
    let qa = if qa.length_squared() > 1e-12 {
        qa.normalize()
    } else {
        Quat::IDENTITY
    };
    let qb = if qb.length_squared() > 1e-12 {
        qb.normalize()
    } else {
        Quat::IDENTITY
    };
    Some(qa.slerp(qb, weight))
}

// ---------------------------------------------------------------------------
// 贴图 base64
// ---------------------------------------------------------------------------

/// 标准 base64 解码（含 URL-safe 与缺省 padding 容错）。
///
/// 不引第三方 crate：AIR 的贴图块是标准 base64，这点代码足够，且避免给
/// `r3e-asset` 再加一个只为解码贴图的依赖。
pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.bytes() {
        let value = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b'\n' | b'\r' | b'\t' | b' ' => continue,
            other => return Err(format!("base64 出现非法字符 0x{other:02x}")),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 导入器 / 文件入口
// ---------------------------------------------------------------------------

/// 按内容读取一份 AIR 文件（`.air.json` 或 `.air.bin`，不看扩展名）。
pub fn load_air_file(path: &Path) -> Result<AssetEntry, AssetError> {
    let display = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|err| AssetError::Load {
        path: display.clone(),
        message: format!("读取失败：{err}"),
    })?;
    let doc = parse_bytes(&bytes).map_err(|err| err.into_asset_error(path))?;
    Ok(to_asset_entry(
        &doc,
        AssetId::from_path(&display),
        &display,
        path.parent(),
    ))
}

/// 从内存字节读一份 AIR（测试与插件回传场景用）。
pub fn load_air_bytes(
    bytes: &[u8],
    id: AssetId,
    source_path: &str,
    base_dir: Option<&Path>,
) -> Result<AssetEntry, AirLoadError> {
    let doc = parse_bytes(bytes)?;
    Ok(to_asset_entry(&doc, id, source_path, base_dir))
}

/// 便捷别名：`load_air_file` 的短名。
pub fn import_air(path: &Path) -> Result<AssetEntry, AssetError> {
    load_air_file(path)
}

/// AIR 导入器：让 `.air` / `.airb` 这类「已经改好名」的文件也能走
/// [`crate::ImporterRegistry`] 的统一分派。
///
/// 注意：插件默认导出的是 `.air.json` / `.air.bin`，扩展名分别是 `json` / `bin`，
/// 靠扩展名无法分派——那两个必须走 [`crate::AssetLibrary::load_air`]（按内容嗅探）。
#[derive(Debug, Default, Clone, Copy)]
pub struct AirImporter;

impl AssetImporter for AirImporter {
    fn name(&self) -> &'static str {
        "air"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["air", "airb"]
    }

    fn import(&self, path: &Path) -> Result<AssetEntry, AssetError> {
        load_air_file(path)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_JSON: &[u8] = include_bytes!("../tests/fixtures/air/cube.air.json");
    const CUBE_BIN: &[u8] = include_bytes!("../tests/fixtures/air/cube.air.bin");
    const QUAD_JSON: &[u8] = include_bytes!("../tests/fixtures/air/quad.air.json");
    const QUAD_BIN: &[u8] = include_bytes!("../tests/fixtures/air/quad.air.bin");

    /// 2x2 纯红 PNG（74 字节），用来验证内嵌贴图真能解出像素。
    const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=";

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/air")
            .join(name)
    }

    /// 最小合法 AIR 文档（单三角形、单材质、无骨骼）。
    fn minimal_doc() -> serde_json::Value {
        serde_json::json!({
            "format": "air",
            "air_version": 1,
            "generator": "tests",
            "source": { "path": "tri.glb", "container": "glb", "bytes": 10 },
            "units": {
                "meters_per_unit": 1.0,
                "up_axis": "+Y",
                "front_axis": "+Z",
                "right_axis": "+X",
                "uv_origin": "top-left",
                "winding": "ccw"
            },
            "nodes": [{
                "index": 0, "name": "Tri", "path": "Tri",
                "position": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
                "mesh": 0
            }],
            "meshes": [{
                "name": "Tri",
                "node": "Tri",
                "positions": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                "normals": [[0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
                "uvs": [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                "indices": [0u32, 1, 2],
                "material_parts": [{
                    "material": 0, "material_name": "Mat",
                    "triangle_offset": 0, "triangle_count": 1
                }],
                "aabb_min": [0.0, 0.0, 0.0],
                "aabb_max": [1.0, 1.0, 0.0],
                "source_vertex_count": 3,
                "source_face_count": 1,
                "source_triangle_count": 1
            }],
            "materials": [{
                "index": 0, "name": "Mat",
                "base_color": [1.0, 1.0, 1.0, 1.0],
                "metallic": 0.0, "roughness": 0.5
            }],
            "textures": [],
            "skins": [],
            "animations": [],
            "stats": {
                "node_count": 1, "mesh_count": 1,
                "vertex_count": 3, "triangle_count": 1,
                "material_count": 1, "texture_count": 0,
                "skin_count": 0, "joint_count": 0,
                "animation_count": 0, "geometry_bytes": 108
            },
            "issues": []
        })
    }

    fn doc_json_bytes(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec_pretty(value).expect("序列化测试文档")
    }

    /// 按 AIRB v1 布局打一个包；`blob` 由调用方按 6 段顺序拼好。
    fn build_airb(meta_json: &str, blob: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(AIRB_HEADER_LEN + meta_json.len() + blob.len());
        out.extend_from_slice(&AIRB_MAGIC);
        out.extend_from_slice(&AIRB_VERSION.to_le_bytes());
        out.extend_from_slice(&(meta_json.len() as u32).to_le_bytes());
        out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
        out.extend_from_slice(meta_json.as_bytes());
        out.extend_from_slice(blob);
        out
    }

    /// 把最小文档改造成 AIRB 的 meta（几何数组清空，只留 span）。
    fn minimal_meta(blob_len: u64) -> serde_json::Value {
        let mut doc = minimal_doc();
        let mesh = doc["meshes"][0].as_object_mut().unwrap();
        for key in ["positions", "normals", "uvs", "indices"] {
            mesh.insert(key.to_string(), serde_json::json!([]));
        }
        serde_json::json!({
            "format": "airb",
            "airb_version": 1,
            "blob_bytes": blob_len,
            "meshes": [{
                "positions": { "offset": 0, "count": 3 },
                "normals": { "offset": 36, "count": 3 },
                "uvs": { "offset": 72, "count": 3 },
                "indices": { "offset": 96, "count": 3 },
                "joints": { "offset": 108, "count": 0 },
                "weights": { "offset": 108, "count": 0 }
            }],
            "air": doc
        })
    }

    /// 按 minimal_meta 的 span 布局拼 blob：positions(36) + normals(36) + uvs(24) + indices(12)。
    fn minimal_blob() -> Vec<u8> {
        let mut blob = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for v in p {
                blob.extend_from_slice(&v.to_le_bytes());
            }
        }
        for _ in 0..3 {
            for v in [0.0f32, 0.0, 1.0] {
                blob.extend_from_slice(&v.to_le_bytes());
            }
        }
        for uv in [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0]] {
            for v in uv {
                blob.extend_from_slice(&v.to_le_bytes());
            }
        }
        for i in [0u32, 1, 2] {
            blob.extend_from_slice(&i.to_le_bytes());
        }
        blob
    }

    // ---- 正向：真实样本 ----

    #[test]
    fn air_json_real_fixture_parses_and_converts() {
        let doc = parse_json_doc(CUBE_JSON).expect("cube.air.json 必须能解析");
        assert_eq!(doc.format, AIR_FORMAT_TAG);
        assert_eq!(doc.air_version, AIR_VERSION);
        assert_eq!(doc.meshes.len(), 1);
        assert_eq!(doc.nodes.len(), 2);
        assert_eq!(doc.materials.len(), 1);

        let entry = to_asset_entry(&doc, AssetId::from_path("cube"), "cube.air.json", None);
        assert_eq!(entry.meshes.len(), 1);
        let mesh = &entry.meshes[0];
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.normals.len(), mesh.positions.len());
        assert_eq!(mesh.uvs.len(), mesh.positions.len());
        assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
        // 顶点已是世界空间：AABB 必须与文件中给出的一致。
        assert!((mesh.aabb_min[0] - doc.meshes[0].aabb_min[0]).abs() < 1e-6);
        assert!((mesh.aabb_max[1] - doc.meshes[0].aabb_max[1]).abs() < 1e-6);
        // 材质 / 贴图下标必须对齐（渲染靠 (asset, index) 取值）。
        assert_eq!(entry.materials.len(), 1);
        assert_eq!(entry.materials[0].base_color_texture, Some(0));
        assert_eq!(entry.textures.len(), 1);
        assert_eq!(entry.textures[0].index, 0);
        // 节点反向引用：mesh 0 挂在某个节点上，node_local_matrix 不应是 NaN。
        assert!(entry.meshes.iter().all(|m| m
            .node_local_matrix
            .iter()
            .flatten()
            .all(|v| v.is_finite())));
    }

    #[test]
    fn airb_real_fixture_matches_json_fixture() {
        let from_json = parse_json_doc(CUBE_JSON).expect("解析 JSON 样本");
        let from_bin = parse_airb(CUBE_BIN).expect("解析 AIRB 样本");
        assert_eq!(from_bin.meshes.len(), from_json.meshes.len());
        let a = &from_bin.meshes[0];
        let b = &from_json.meshes[0];
        assert_eq!(
            a.positions, b.positions,
            "AIRB 的 positions 必须与 JSON 完全一致"
        );
        assert_eq!(a.normals, b.normals, "AIRB 的 normals 必须与 JSON 完全一致");
        assert_eq!(a.uvs, b.uvs, "AIRB 的 uvs 必须与 JSON 完全一致");
        assert_eq!(a.indices, b.indices, "AIRB 的 indices 必须与 JSON 完全一致");
        assert_eq!(a.material_parts.len(), b.material_parts.len());
        assert_eq!(from_bin.nodes.len(), from_json.nodes.len());
        assert_eq!(from_bin.materials.len(), from_json.materials.len());
    }

    #[test]
    fn quad_fixtures_parse_in_both_formats() {
        let json = parse_json_doc(QUAD_JSON).expect("quad.air.json 必须能解析");
        let bin = parse_airb(QUAD_BIN).expect("quad.air.bin 必须能解析");
        assert_eq!(json.meshes[0].positions, bin.meshes[0].positions);
        assert!(json.meshes[0].indices.len() % 3 == 0);
    }

    #[test]
    fn parse_bytes_dispatches_by_content_not_extension() {
        assert!(matches!(
            parse_bytes(CUBE_BIN).map(|d| d.meshes.len()),
            Ok(1)
        ));
        assert!(matches!(
            parse_bytes(CUBE_JSON).map(|d| d.meshes.len()),
            Ok(1)
        ));
        assert!(looks_like_airb(CUBE_BIN));
        assert!(!looks_like_airb(CUBE_JSON));
        assert!(looks_like_air_json(CUBE_JSON));
        // 前置 BOM + 空白也要认。
        let mut with_bom = b"\xEF\xBB\xBF  ".to_vec();
        with_bom.extend_from_slice(CUBE_JSON);
        assert!(looks_like_air_json(&with_bom));
    }

    #[test]
    fn synthetic_airb_roundtrips_through_blob() {
        let blob = minimal_blob();
        let meta = minimal_meta(blob.len() as u64);
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        let doc = parse_airb(&bytes).expect("合成 AIRB 必须能解析");
        assert_eq!(doc.meshes[0].positions.len(), 3);
        assert_eq!(doc.meshes[0].normals[0], [0.0, 0.0, 1.0]);
        assert_eq!(doc.meshes[0].uvs[1], [1.0, 0.0]);
        assert_eq!(doc.meshes[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn embedded_texture_base64_decodes_to_pixels() {
        let mut doc = minimal_doc();
        doc["materials"][0]["base_color_texture"] = serde_json::json!(0);
        doc["textures"] = serde_json::json!([{
            "index": 0,
            "name": "checker.png",
            "slot": "baseColorTexture",
            "mime_type": "image/png",
            "width": 2,
            "height": 2,
            "bytes": 74,
            "embedded": true,
            "data_base64": TINY_PNG_BASE64
        }]);
        let parsed = parse_json_doc(&doc_json_bytes(&doc)).expect("带内嵌贴图的文档必须能解析");
        let entry = to_asset_entry(&parsed, AssetId::from_path("tex"), "tex.air.json", None);
        let image = entry.textures[0]
            .image
            .as_ref()
            .expect("内嵌 PNG 必须被解码成像素");
        assert_eq!((image.width, image.height), (2, 2));
        assert!(image.is_valid());
        assert_eq!(entry.textures[0].content_bytes, 74);
        assert_eq!(entry.materials[0].textures.len(), 1);
        assert_eq!(entry.materials[0].base_color_texture, Some(0));
    }

    #[test]
    fn base64_decoder_handles_padding_and_safe_alphabet() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello".to_vec());
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello".to_vec());
        assert_eq!(decode_base64("_-8=").unwrap(), vec![0xFF, 0xEF]);
        assert!(decode_base64("a$b").is_err());
    }

    // ---- 负例：AIRB ----

    #[test]
    fn airb_rejects_bad_magic() {
        let mut bytes = CUBE_BIN.to_vec();
        bytes[..4].copy_from_slice(b"XXXX");
        match parse_bytes(&bytes) {
            // 改坏 magic 后不再是 AIRB，也不是 JSON —— 必须明确报 UnknownFormat。
            Err(AirLoadError::UnknownFormat) => {}
            other => panic!("坏 magic 必须报错，实际 {other:?}"),
        }
        match parse_airb(&bytes) {
            Err(AirLoadError::BadMagic { found }) => assert_eq!(&found, b"XXXX"),
            other => panic!("坏 magic 必须报 BadMagic，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_truncated_header() {
        match parse_airb(&CUBE_BIN[..10]) {
            Err(AirLoadError::Truncated { needed, available }) => {
                assert_eq!(needed, AIRB_HEADER_LEN);
                assert_eq!(available, 10);
            }
            other => panic!("头部截断必须报 Truncated，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_truncated_blob() {
        let cut = CUBE_BIN.len() - 32;
        match parse_airb(&CUBE_BIN[..cut]) {
            Err(AirLoadError::Truncated { needed, available }) => {
                assert_eq!(needed, CUBE_BIN.len());
                assert_eq!(available, cut);
            }
            other => panic!("blob 截断必须报 Truncated，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_version_mismatch() {
        let mut bytes = CUBE_BIN.to_vec();
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        match parse_airb(&bytes) {
            Err(AirLoadError::UnsupportedVersion {
                kind,
                found,
                supported,
            }) => {
                assert_eq!(kind, "AIRB 容器");
                assert_eq!(found, 99);
                assert_eq!(supported, AIRB_VERSION);
            }
            other => panic!("版本不符必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_span_count_mismatch() {
        let blob = minimal_blob();
        let mut meta = minimal_meta(blob.len() as u64);
        meta["meshes"] = serde_json::json!([]);
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        match parse_airb(&bytes) {
            Err(AirLoadError::SpanCountMismatch { spans, meshes }) => {
                assert_eq!(spans, 0);
                assert_eq!(meshes, 1);
            }
            other => panic!("span 数量不一致必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_blob_length_mismatch() {
        let blob = minimal_blob();
        let mut meta = minimal_meta(blob.len() as u64 + 8);
        meta["blob_bytes"] = serde_json::json!(blob.len() as u64 + 8);
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        // 头部 blob_len 由 build_airb 按实际长度写，与 meta 声明不一致。
        match parse_airb(&bytes) {
            Err(AirLoadError::BlobLengthMismatch { header, meta }) => {
                assert_eq!(header, blob.len() as u64);
                assert_eq!(meta, blob.len() as u64 + 8);
            }
            other => panic!("blob 长度不一致必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_span_out_of_bounds() {
        let blob = minimal_blob();
        let mut meta = minimal_meta(blob.len() as u64);
        meta["meshes"][0]["positions"] = serde_json::json!({ "offset": 4096, "count": 3 });
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        match parse_airb(&bytes) {
            Err(AirLoadError::SpanOutOfBounds {
                mesh,
                section,
                needed,
                blob_len,
            }) => {
                assert_eq!(mesh, 0);
                assert_eq!(section, "positions");
                assert_eq!(needed, 4096 + 36);
                assert_eq!(blob_len, blob.len());
            }
            other => panic!("span 越界必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_wrong_meta_format_tag() {
        let blob = minimal_blob();
        let mut meta = minimal_meta(blob.len() as u64);
        meta["format"] = serde_json::json!("air");
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        match parse_airb(&bytes) {
            Err(AirLoadError::Invalid(message)) => {
                assert!(message.contains("airb"), "{message}");
            }
            other => panic!("format 标记不符必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_unsupported_air_version_in_meta() {
        let blob = minimal_blob();
        let mut meta = minimal_meta(blob.len() as u64);
        meta["air"]["air_version"] = serde_json::json!(2);
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        match parse_airb(&bytes) {
            Err(AirLoadError::UnsupportedVersion { kind, found, .. }) => {
                assert_eq!(kind, "AIR");
                assert_eq!(found, 2);
            }
            other => panic!("AIR 版本不符必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn airb_rejects_missing_required_meta_field() {
        let blob = minimal_blob();
        let mut meta = minimal_meta(blob.len() as u64);
        meta.as_object_mut().unwrap().remove("blob_bytes");
        let bytes = build_airb(&serde_json::to_string(&meta).unwrap(), &blob);
        match parse_airb(&bytes) {
            Err(AirLoadError::MetaJson(message)) => {
                assert!(message.contains("blob_bytes"), "{message}");
            }
            other => panic!("缺字段必须报 MetaJson，实际 {other:?}"),
        }
    }

    // ---- 负例：AIR JSON ----

    #[test]
    fn air_json_rejects_missing_field() {
        let mut doc = minimal_doc();
        doc.as_object_mut().unwrap().remove("units");
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::MetaJson(message)) => assert!(message.contains("units"), "{message}"),
            other => panic!("缺 units 必须报字段缺失，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_missing_mesh_field() {
        let mut doc = minimal_doc();
        doc["meshes"][0].as_object_mut().unwrap().remove("aabb_min");
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::MetaJson(message)) => {
                assert!(message.contains("aabb_min"), "{message}")
            }
            other => panic!("缺 aabb_min 必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_unsupported_version() {
        let mut doc = minimal_doc();
        doc["air_version"] = serde_json::json!(2);
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::UnsupportedVersion { kind, found, .. }) => {
                assert_eq!(kind, "AIR");
                assert_eq!(found, 2);
            }
            other => panic!("版本不符必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_wrong_format_tag() {
        let mut doc = minimal_doc();
        doc["format"] = serde_json::json!("gltf");
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::Invalid(message)) => assert!(message.contains("format"), "{message}"),
            other => panic!("format 不符必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_index_out_of_range() {
        let mut doc = minimal_doc();
        doc["meshes"][0]["indices"] = serde_json::json!([0, 1, 9]);
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::Invalid(message)) => assert!(message.contains("越界"), "{message}"),
            other => panic!("索引越界必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_bad_material_part_range() {
        let mut doc = minimal_doc();
        doc["meshes"][0]["material_parts"][0]["triangle_count"] = serde_json::json!(5);
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::Invalid(message)) => {
                assert!(message.contains("分区"), "{message}")
            }
            other => panic!("分区越界必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_unknown_animation_property() {
        let mut doc = minimal_doc();
        doc["animations"] = serde_json::json!([{
            "name": "Walk",
            "duration": 1.0,
            "tracks": [{
                "node": 0,
                "property": "morph",
                "times": [0.0],
                "values": [[0.0, 0.0, 0.0, 0.0]]
            }]
        }]);
        match parse_json_doc(&doc_json_bytes(&doc)) {
            Err(AirLoadError::Invalid(message)) => assert!(message.contains("morph"), "{message}"),
            other => panic!("未知动画属性必须报错，实际 {other:?}"),
        }
    }

    #[test]
    fn air_json_rejects_empty_and_garbage() {
        assert!(matches!(parse_bytes(&[]), Err(AirLoadError::Empty)));
        assert!(matches!(
            parse_bytes(b"hello world"),
            Err(AirLoadError::UnknownFormat)
        ));
        assert!(matches!(
            parse_bytes(b"{ not json"),
            Err(AirLoadError::MetaJson(_))
        ));
    }

    // ---- 转换：动画 / 骨骼 ----

    #[test]
    fn animations_become_engine_tracks_with_union_timeline() {
        let mut doc = minimal_doc();
        doc["animations"] = serde_json::json!([{
            "name": "Slide",
            "duration": 2.0,
            "tracks": [
                {
                    "node": 0,
                    "property": "translation",
                    "times": [0.0, 2.0],
                    "values": [[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]]
                },
                {
                    "node": 0,
                    "property": "rotation",
                    "times": [1.0, 2.0],
                    // 90° 绕 Y：w = cos45° = FRAC_1_SQRT_2（写成精确常量，避开 clippy 近似值告警）
                    "values": [[0.0, 0.0, 0.0, 1.0], [
                       0.0,
                       std::f64::consts::FRAC_1_SQRT_2,
                       0.0,
                       std::f64::consts::FRAC_1_SQRT_2
                   ]]
                }
            ]
        }]);
        let parsed = parse_json_doc(&doc_json_bytes(&doc)).expect("动画文档必须能解析");
        let entry = to_asset_entry(&parsed, AssetId::from_path("anim"), "anim.air.json", None);
        assert_eq!(entry.animations.len(), 1);
        let clip = &entry.animations[0];
        assert_eq!(clip.name, "Slide");
        assert!((clip.duration - 2.0).abs() < 1e-6);
        assert_eq!(clip.tracks.len(), 1, "同一节点的三条轨道必须合并成一条");
        let track = &clip.tracks[0];
        assert_eq!(track.node_index, 0);
        assert_eq!(track.node_path, "Tri");
        // 时间轴取并集：0 / 1 / 2
        assert_eq!(track.keys.len(), 3);
        assert!((track.keys[1].time - 1.0).abs() < 1e-6);
        // t=1 只有平移的关键帧端点插值（0→2 的中点 = 1）
        assert!((track.keys[1].position.x - 1.0).abs() < 1e-5);
        // t=1 还没有旋转关键帧的中段值，取首帧姿态
        assert!((track.keys[1].rotation.w - 1.0).abs() < 1e-5);
        // t=2 旋转到 90°（w ≈ 0.7071）
        assert!((track.keys[2].rotation.w - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
    }

    #[test]
    fn skins_convert_inverse_bind_to_bind_world() {
        let mut doc = minimal_doc();
        doc["skins"] = serde_json::json!([{
            "index": 0,
            "name": "Rig",
            "joints": [0],
            // 平移 -1 的逆（列主序）：它的逆应当是平移 +1
            "inverse_bind_matrices": [[
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                -1.0, 0.0, 0.0, 1.0
            ]]
        }]);
        doc["meshes"][0]["skin"] = serde_json::json!(0);
        doc["meshes"][0]["joints"] = serde_json::json!([[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        doc["meshes"][0]["weights"] = serde_json::json!([
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0]
        ]);

        let parsed = parse_json_doc(&doc_json_bytes(&doc)).expect("骨骼文档必须能解析");
        let entry = to_asset_entry(&parsed, AssetId::from_path("skin"), "skin.air.json", None);
        assert_eq!(entry.skins.len(), 1);
        assert_eq!(entry.skins[0].joint_count(), 1);
        assert_eq!(entry.skins[0].joints[0].path, "Tri");
        let bind = entry.skins[0].joints[0].bind_world;
        assert!((bind.w_axis.x - 1.0).abs() < 1e-6, "bind_world 应为平移 +1");
        assert!(entry.meshes[0].is_skinned());
    }

    // ---- 文件 / 资产库入口 ----

    #[test]
    fn load_air_file_accepts_both_containers() {
        let json = load_air_file(&fixture("cube.air.json")).expect("读 .air.json");
        let bin = load_air_file(&fixture("cube.air.bin")).expect("读 .air.bin");
        assert_eq!(json.vertex_total(), bin.vertex_total());
        assert_eq!(json.triangle_total(), bin.triangle_total());
        assert_eq!(
            json.source_path,
            fixture("cube.air.json").display().to_string()
        );
        assert_eq!(bin.meshes[0].positions, json.meshes[0].positions);
    }

    #[test]
    fn bad_file_reports_path_and_reason() {
        let err = load_air_file(Path::new("C:/definitely/not/here.air.json"))
            .expect_err("不存在的文件必须报错");
        match err {
            AssetError::Load { path, message } => {
                assert!(path.contains("not/here.air.json"));
                assert!(message.contains("读取失败"), "{message}");
            }
            other => panic!("意外的错误类型: {other:?}"),
        }
    }

    #[test]
    fn air_importer_exposes_extensions() {
        let importer = AirImporter;
        assert_eq!(importer.name(), "air");
        assert_eq!(importer.extensions(), &["air", "airb"]);
    }

    #[test]
    fn base64_and_spans_stay_aligned_with_plugin_layout() {
        // 逐元素宽度必须与插件 air_bin.rs 的 push_* 一致，否则读出来就是错位的。
        assert_eq!(ELEM_POSITION, 12);
        assert_eq!(ELEM_NORMAL, 12);
        assert_eq!(ELEM_UV, 8);
        assert_eq!(ELEM_INDEX, 4);
        assert_eq!(ELEM_JOINT, 8);
        assert_eq!(ELEM_WEIGHT, 16);
        assert_eq!(AIRB_HEADER_LEN, 20);
        assert_eq!(AIRB_MAGIC, *b"AIRB");
    }
}
