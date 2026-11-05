//! 场景问题的定位层（P0-2）。
//!
//! 校验结果原来的形态是 `{kind, detail}`：`detail` 是写给人看的句子
//! （"顶点 #7 |normal| = 0.5"），模型要照着它去改文件，得先自己解析文本、
//! 再猜这个顶点属于哪个节点的哪份网格。这一层把定位拆成**结构化字段**：
//!
//! - `node`：`{id, name, objectIndex}`，`objectIndex` 是场景文件 `objects` 里的下标；
//! - `field`：该问题归属的节点字段（网格问题一律 `mesh`，材质问题 `material.*`）；
//! - `path`：可直接拼给人看的场景文件路径，如 `$.objects[2].mesh`；
//! - `resource`：这个网格从哪来（内置程序化网格 / 外部资产第 N 个网格），
//!   外部资产额外给出 `assetPath` 与资产声明下标 `assetIndex`；
//! - `anchors`：网格内部下标（`vertex` / `index` / `part`），三个都可能为空；
//! - `mesh` / `material`：出问题处附近的上下文（网格形状与计数、节点材质），
//!   让模型不必再发一次 `scene_inspect` 就能决定改成什么值。
//!
//! 这里只做「按已知事实填表」，不猜：拿不到的上下文宁可写 `null`。

use r3e_core::air::{AirIssue, AirIssueKind};
use r3e_core::mesh::MeshData;
use r3e_core::{AssetId, MeshRef, Node, Scene};
use serde_json::{json, Value};

/// 一条违规 → 带定位的 JSON 条目（保留原 `kind` / `detail` 字段，向后兼容）。
pub fn issue_json(
    scene: &Scene,
    node: &Node,
    object_index: usize,
    issue: &AirIssue,
    mesh: Option<&MeshData>,
) -> Value {
    let field = field_of(&issue.kind);
    let mut entry = json!({
        "kind": issue.kind.as_str(),
        "detail": issue.detail,
        "location": {
            "node": node_json(node, object_index),
            "field": field,
            "path": object_path(object_index, field),
            "resource": resource_json(scene, &node.mesh),
            "anchors": {
                "vertex": issue.anchor.vertex,
                "index": issue.anchor.index,
                "part": issue.anchor.part,
            },
            "mesh": mesh_json(&node.mesh, mesh),
            "material": material_json(node),
        },
    });
    // 外部资产网格没有 AIR 结论（插件不带资产管线），别让它看起来像"通过了"。
    if node.mesh.is_asset() {
        entry["location"]["resource"]["external"] = json!(true);
        entry["location"]["hint"] =
            json!("外部资产网格需资产管线（r3e-asset）才能校验，本插件跳过");
    }
    entry
}

/// 「这个节点的网格没校验」也是一个「问题」：单独给一条带定位的条目，
/// 而不是只在 `warnings` 里潦草提一句。
pub fn skipped_issue_json(scene: &Scene, node: &Node, object_index: usize) -> Value {
    json!({
        "kind": "mesh-not-validated",
        "detail": "外部资产网格，插件无资产管线，未做 AIR 校验",
        "location": {
            "node": node_json(node, object_index),
            "field": "mesh",
            "path": object_path(object_index, "mesh"),
            "resource": resource_json(scene, &node.mesh),
            "anchors": { "vertex": Value::Null, "index": Value::Null, "part": Value::Null },
            "mesh": mesh_json(&node.mesh, None),
            "material": material_json(node),
            "hint": "改用场景校验之外的资产检查（--dump-asset）确认网格是否合规",
        },
    })
}

/// 问题归属的节点字段。网格内部的数据结构性问题一律归到 `mesh`——
/// 顶点/索引不在场景文件里，能改的只有"用哪个网格"。
fn field_of(kind: &AirIssueKind) -> &'static str {
    match kind {
        AirIssueKind::EmptyMesh
        | AirIssueKind::IndexNotTriangleList
        | AirIssueKind::IndexOutOfRange
        | AirIssueKind::NonFinitePosition
        | AirIssueKind::NonFiniteNormal
        | AirIssueKind::NormalNotUnit
        | AirIssueKind::NonFiniteUv
        | AirIssueKind::SkinWeightNegative
        | AirIssueKind::SkinWeightSum
        | AirIssueKind::SkinJointOutOfRange
        | AirIssueKind::DegenerateAabb
        | AirIssueKind::PartsCoverage => "mesh",
    }
}

fn object_path(object_index: usize, field: &str) -> String {
    format!("$.objects[{object_index}].{field}")
}

fn node_json(node: &Node, object_index: usize) -> Value {
    json!({
        "id": node.id,
        "name": node.name,
        "objectIndex": object_index,
        "visible": node.visible,
    })
}

fn resource_json(scene: &Scene, mesh_ref: &MeshRef) -> Value {
    match mesh_ref {
        MeshRef::Builtin(kind) => json!({
            "kind": "builtin",
            "value": kind.as_str(),
            "external": false,
            "assetPath": Value::Null,
            "assetIndex": Value::Null,
            "meshIndex": Value::Null,
        }),
        MeshRef::Asset { asset, mesh } => {
            // 资产声明按「路径散列」反查：`MeshRef` 里只有散列值，
            // 场景文件里写的是路径，模型要改的是路径那一行。
            let found = scene
                .assets
                .iter()
                .enumerate()
                .find(|(_, decl)| AssetId::from_path(&decl.path) == *asset);
            json!({
                "kind": "asset",
                "value": mesh_ref.as_string(),
                "external": true,
                "assetId": asset.to_string(),
                "assetPath": found.as_ref().map(|(_, d)| json!(d.path)),
                "assetIndex": found.map(|(i, _)| json!(i)),
                "meshIndex": *mesh,
            })
        }
    }
}

fn mesh_json(mesh: &MeshRef, data: Option<&MeshData>) -> Value {
    match data {
        Some(data) => json!({
            "shape": mesh.as_string(),
            "vertices": data.vertices.len(),
            "triangles": data.indices.len() / 3,
            "indices": data.indices.len(),
        }),
        None => json!({
            "shape": mesh.as_string(),
            "vertices": Value::Null,
            "triangles": Value::Null,
            "indices": Value::Null,
        }),
    }
}

fn material_json(node: &Node) -> Value {
    match node.material {
        Some(material) => json!({
            "declared": true,
            "metallic": round(material.metallic),
            "roughness": round(material.roughness),
            "emissive": [
                round(material.emissive[0]),
                round(material.emissive[1]),
                round(material.emissive[2]),
            ],
        }),
        None => json!({ "declared": false }),
    }
}

/// 与 `tools.rs` 同一套取整口径（3 位小数），避免同一份数据两处格式不一致。
fn round(value: f32) -> f64 {
    (f64::from(value) * 1000.0).round() / 1000.0
}
