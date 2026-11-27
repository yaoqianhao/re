//! Prefab 族静态校验的契约（M2-E05）。code：`prefab.override_leak`。
//!
//! 与 `tests/terrain_issues.rs` 同一立场：每条负例从「合法 prefab 场景」出发只破坏
//! 一个约束，断言必须出现指定 code；正向用例确认不误报。
//!
//! `assert_error` 的首个字符串实参恒为 code，`xtask invariants` 按此扫源码，
//! 换写法会让能力清单漂移。
//!
//! 三条触发面里，②③ 是同一件事的逐字段与汇总（计划 §6.4 说明过），所以负例只按
//! 「抄回模板值」写一条；①（引用了不存在的模板）**绕过实例化入口直写 `scene.instances`**
//! ——`spawn_instance` 当场就拒，走公开入口造不出这种数据，而手改 `.r3e.json` 能造出来。

use r3e_core::prefab::{FieldOverride, ObjectOverride, PrefabInstance, PrefabTemplate};
use r3e_core::{ObjectDesc, Scene};
use r3e_doc::{Document, Severity};

/// 最小模板：ID + 原型名 + 一个网格 + 两个标签。
fn template() -> PrefabTemplate {
    PrefabTemplate::new(
        "prop.crate",
        ObjectDesc {
            name: "crate".to_string(),
            mesh: Some("crate.glb".to_string()),
            tags: Some(vec!["prop".to_string(), "breakable".to_string()]),
            // 显式给 `visible`：正向用例要证明「`Clear` 一个模板里真有值的字段」是
            // 真实差异（模板 `visible` 缺省为 `None` 时，`Clear` 反而是冗余覆盖）。
            visible: Some(true),
            ..ObjectDesc::default()
        },
    )
}

fn scene_with_template() -> Scene {
    let mut scene = Scene::new();
    scene.define_prefab(template()).expect("登记模板");
    scene
}

fn codes(doc: &Document) -> Vec<&'static str> {
    doc.validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.code)
        .collect()
}

fn assert_error(doc: &Document, code: &str) {
    let found = codes(doc);
    assert!(found.contains(&code), "期望 {code}，实得 {found:?}");
}

/// 泄漏用例报出的那条 issue 的正文（挑第一条同 code 的）。
fn leak_message(doc: &Document) -> String {
    doc.validate()
        .into_iter()
        .find(|i| i.code == "prefab.override_leak")
        .map(|i| i.msg)
        .expect("应报 prefab.override_leak")
}

// ---------- 负向 ----------

/// 触发面②：覆盖里的字段与模板同值 = 把模板值抄回实例。
#[test]
fn redundant_override_copied_from_template_is_error() {
    let mut scene = scene_with_template();
    // 抄回模板值（`mesh` 与模板一模一样）——合法但会让实例不再跟随模板。
    // 真差异（默认 scale 为 `None`）：这一项必须留在覆盖里，且不该被一起点名。
    let overrides = ObjectOverride {
        mesh: Some(FieldOverride::set("crate.glb".to_string())),
        scale: Some(FieldOverride::set([2.0, 2.0, 2.0])),
        ..ObjectOverride::default()
    };
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate").with_override(overrides))
        .expect("实例化应成功：冗余覆盖在当前语义下是合法写入");

    let doc = Document::new(scene);
    assert_error(&doc, "prefab.override_leak");
    let message = leak_message(&doc);
    assert!(
        message.contains("mesh"),
        "报错应点名冗余字段，实际：{message}"
    );
    assert!(
        !message.contains("scale"),
        "真正不同的字段不该被点名，实际：{message}"
    );
}

/// 触发面①：实例引用了没登记的模板（直写实例表 —— 手改场景文件的形态）。
#[test]
fn instance_pointing_to_unknown_template_is_error() {
    let mut scene = scene_with_template();
    scene
        .instances
        .push(PrefabInstance::new("ghost_1", "prop.missing"));
    let doc = Document::new(scene);
    assert_error(&doc, "prefab.override_leak");
    let message = leak_message(&doc);
    assert!(
        message.contains("prop.missing"),
        "报错应点名缺失的模板 ID，实际：{message}"
    );
}

// ---------- 正向 ----------

/// 只存真实差异、模板全都在表里：不报 prefab 族问题。
#[test]
fn minimal_override_is_silent() {
    let mut scene = scene_with_template();
    let overrides = ObjectOverride {
        scale: Some(FieldOverride::set([2.0, 2.0, 2.0])),
        visible: Some(FieldOverride::clear()),
        ..ObjectOverride::default()
    };
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate").with_override(overrides))
        .expect("实例化应成功");

    let doc = Document::new(scene);
    assert!(
        !codes(&doc).contains(&"prefab.override_leak"),
        "最小差异覆盖不该判泄漏，实得 {:?}",
        codes(&doc)
    );
}

/// 判据③：仓库真实样例（`assets/prefab/prop/crate.json`）正跑防假阳性。
///
/// `r3e-doc` 依赖表里没有 `serde_json`（E03 卡已定「不改 Cargo 依赖」），所以这里
/// 把样例的 `proto` **逐字段镜像**成 [`ObjectDesc`]（与 `tests/prefab_diff.rs`、
/// E03 `prefab_command.rs` 的 `full_proto` 同源）；真实文件的解析与实例化路径由
/// E04 的 `r3e-editor-host/tests/prefab_api.rs`（直接读该文件）与
/// `r3e-core/tests/prefab.rs` 的样例 schema 正跑覆盖。
///
/// 覆盖取「真实差异」：样例 `visible = true`，实例改成 `false`。
#[test]
fn real_sample_prefab_scene_is_silent() {
    let mut scene = Scene::new();
    scene
        .define_prefab(PrefabTemplate::new(
            "prop.crate",
            ObjectDesc {
                name: "crate".to_string(),
                mesh: Some("props/crate.glb".to_string()),
                position: Some([0.0, 0.4, 0.0]),
                rotation: Some([0.0, 0.0, 0.0]),
                scale: Some([1.0, 1.0, 1.0]),
                color: Some([0.72, 0.55, 0.34, 1.0]),
                parent: None,
                visible: Some(true),
                physics: Some(r3e_core::PhysicsDesc {
                    body: "dynamic".to_string(),
                    mass: 12.0,
                    restitution: Some(0.15),
                    friction: Some(0.6),
                    collider: Some("box".to_string()),
                    half_extents: Some([0.4, 0.4, 0.4]),
                    ccd: Some(false),
                }),
                material: Some(r3e_core::SurfaceMaterial {
                    metallic: 0.0,
                    roughness: 0.62,
                    emissive: [0.0, 0.0, 0.0],
                }),
                material_ref: None,
                tags: Some(vec![
                    "prop".to_string(),
                    "crate".to_string(),
                    "breakable".to_string(),
                ]),
                interact_radius: Some(1.2),
            },
        ))
        .expect("样例模板 ID 形状合法");
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate").with_override(
            ObjectOverride {
                visible: Some(FieldOverride::set(false)),
                ..ObjectOverride::default()
            },
        ))
        .expect("模板已登记、实例名未占用");

    let doc = Document::new(scene);
    assert!(
        !codes(&doc).contains(&"prefab.override_leak"),
        "真实样例的正跑不该被误判为泄漏，实得 {:?}",
        codes(&doc)
    );
}
