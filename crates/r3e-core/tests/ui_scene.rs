//! UI 元素在场景层的链路：场景文件 → 场景（载入即收敛）→ 保存 → 重新载入。
//!
//! 与 `postfx_scene.rs` / `environment_scene.rs` 同构，关心三件事：
//! 1. 不写 `ui` 段时场景保持空、保存也不凭空写出 `ui` 段——老场景不该被塞进 UI 层；
//! 2. 场景文件里的非法数值 / 非法枚举在载入那一刻就收敛，不留 NaN、不 panic；
//! 3. 写进去的 UI 元素能原样保存回 JSON，再载入逐字段一致（编辑器改一处不该丢其余字段）。
//!
//! 另外把「载入后的元素」直接喂给命中测试与指针状态机，确认场景数据与交互判定
//! 用的是同一套矩形（`app` / 脚本 / 编辑器都只读 `scene.ui`）。

use r3e_core::ui::{hit_test_key, resolve_rect, UiInteraction, UiKind, UiValue};
use r3e_core::Scene;

fn scene_json_with_ui(ui: &str) -> String {
    format!(
        r#"{{
  "camera": {{ "position": [0.0, 1.0, 5.0], "target": [0.0, 0.0, 0.0] }},
  "objects": [{{ "name": "ball", "mesh": "cube" }}],
  "ui": {ui}
}}"#
    )
}

/// 没有 `ui` 段：场景为空，保存时也不该写出 `ui` 字段。
#[test]
fn missing_ui_section_stays_empty() {
    let scene = Scene::new();
    assert!(scene.ui.is_empty());
    assert!(scene.to_scene_file().ui.is_empty());
}

/// 只写部分字段时其余回落到缺省值（`#[serde(default)]` 生效），而不是被整段归零。
#[test]
fn partial_ui_element_uses_field_defaults() {
    let scene = Scene::from_json_str(&scene_json_with_ui(
        r#"[{ "key": "panel", "size": [{ "value": 320.0, "unit": "px" }, { "value": 120.0, "unit": "px" }] }]"#,
    ))
    .expect("载入场景");
    assert_eq!(scene.ui.len(), 1);
    let el = &scene.ui[0];
    assert_eq!(el.key, "panel");
    assert_eq!(el.kind, UiKind::Rect, "缺 kind 回落矩形");
    assert_eq!(el.size[0], UiValue::px(320.0));
    assert_eq!(el.layer, 0);
    assert!(el.visible, "缺 visible 默认可见");
}

/// 单位、类型、锚点这些「名字」必须能被 JSON 反序列化认可，写回时也保持同名。
#[test]
fn ui_element_round_trips_through_json() {
    let json = scene_json_with_ui(
        r#"[
    {
      "key": "btn_start",
      "kind": "image",
      "anchor": "bottom_center",
      "offset": [
        { "value": -160.0, "unit": "px" },
        { "value": -72.0, "unit": "px" }
      ],
      "size": [
        { "value": 280.0, "unit": "px" },
        { "value": 50.0, "unit": "percent" }
      ],
      "color": [0.2, 0.4, 0.9, 0.85],
      "border_width": 4.0,
      "image": "ui/btn.png",
      "uv": [0.0, 0.0, 0.5, 1.0],
      "slice_margin": [
        { "value": 8.0, "unit": "px" },
        { "value": 8.0, "unit": "px" },
        { "value": 8.0, "unit": "px" },
        { "value": 8.0, "unit": "px" }
      ],
      "layer": 6,
      "visible": false
    }
  ]"#,
    );
    let scene = Scene::from_json_str(&json).expect("载入场景");
    let el = &scene.ui[0];
    assert_eq!(el.kind, UiKind::Image);
    assert_eq!(el.anchor.name(), "bottom_center");
    assert_eq!(el.offset[0], UiValue::px(-160.0));
    assert_eq!(el.size[1], UiValue::pct(50.0));
    assert_eq!(el.uv, [0.0, 0.0, 0.5, 1.0]);
    assert_eq!(el.slice_margin[2], UiValue::px(8.0));
    assert_eq!(el.layer, 6);
    assert!(!el.visible);

    // 保存 → 重新载入：逐字段一致（这条挡住「编辑器存一次丢一半字段」）。
    let saved = serde_json::to_string(&scene.to_scene_file()).expect("序列化");
    let reloaded = Scene::from_json_str(&saved).expect("重新载入");
    assert_eq!(reloaded.ui.len(), 1);
    assert_eq!(reloaded.ui[0], scene.ui[0]);
}

/// 非法数值在载入时被收敛：颜色夹到 `[0,1]`、uv 夹到整张、非有限值折零。
#[test]
fn scene_load_sanitizes_ui_values() {
    let scene = Scene::from_json_str(&scene_json_with_ui(
        r#"[{
      "key": "bad",
      "color": [1.8, -0.5, 0.25, 2.0],
      "uv": [1.4, -0.3, 0.9, 0.6],
      "border_width": -8.0,
      "size": [
        { "value": 120.0, "unit": "px" },
        { "value": 80.0, "unit": "px" }
      ]
    }]"#,
    ))
    .expect("载入场景");
    let el = &scene.ui[0];
    assert_eq!(el.color, [1.0, 0.0, 0.25, 1.0]);
    assert_eq!(el.border_width, 0.0, "负线宽夹到 0（= 不画）");
    // uv 两端被夹到合法区间并保证 u0 <= u1。
    assert!(
        el.uv[0] <= el.uv[2] && el.uv[0] >= 0.0 && el.uv[2] <= 1.0,
        "{:?}",
        el.uv
    );
    assert!(
        el.uv[1] <= el.uv[3] && el.uv[1] >= 0.0 && el.uv[3] <= 1.0,
        "{:?}",
        el.uv
    );
}

/// 未知枚举名不是 panic 也不是静默变成第一项，而是如实报错（编辑器能提示用户改场景）。
#[test]
fn unknown_ui_kind_is_an_error() {
    let err = Scene::from_json_str(&scene_json_with_ui(
        r#"[{ "key": "x", "kind": "圆角矩形" }]"#,
    ))
    .expect_err("未知 kind 应报错");
    assert!(!err.is_empty(), "错误信息不该为空");
}

/// 载入后的元素直接参与命中：按 layer 取最上层，且百分比尺寸按屏幕换算。
#[test]
fn scene_ui_elements_drive_hit_test() {
    let scene = Scene::from_json_str(&scene_json_with_ui(
        r#"[
    {
      "key": "bg",
      "layer": 0,
      "size": [{ "value": 100.0, "unit": "percent" }, { "value": 100.0, "unit": "percent" }]
    },
    {
      "key": "btn",
      "layer": 5,
      "offset": [{ "value": 40.0, "unit": "px" }, { "value": 40.0, "unit": "px" }],
      "size": [{ "value": 200.0, "unit": "px" }, { "value": 60.0, "unit": "px" }]
    }
  ]"#,
    ))
    .expect("载入场景");

    let screen = (1280.0, 720.0);
    // 全屏背景的百分比尺寸按屏幕换算成 1280x720。
    let bg_rect = resolve_rect(&scene.ui[0], screen).expect("矩形可解");
    assert_eq!((bg_rect.width, bg_rect.height), (1280.0, 720.0));
    // 按钮压在背景上：命中取 layer 大的那个。
    assert_eq!(hit_test_key(&scene.ui, screen, 100.0, 60.0), Some("btn"));
    // 按钮之外仍是背景。
    assert_eq!(hit_test_key(&scene.ui, screen, 600.0, 600.0), Some("bg"));
    // 不可见元素点不到。
    let mut hidden = scene.ui.clone();
    hidden[1].visible = false;
    assert_eq!(hit_test_key(&hidden, screen, 100.0, 60.0), Some("bg"));
}

/// 载入后的元素驱动指针状态机：按下 → 抬起 = 一次 clicked，移动 = hovered / unhovered。
#[test]
fn scene_ui_elements_drive_interaction_events() {
    let scene = Scene::from_json_str(&scene_json_with_ui(
        r#"[{
      "key": "btn",
      "offset": [{ "value": 40.0, "unit": "px" }, { "value": 40.0, "unit": "px" }],
      "size": [{ "value": 200.0, "unit": "px" }, { "value": 60.0, "unit": "px" }]
    }]"#,
    ))
    .expect("载入场景");
    let screen = (1280.0, 720.0);
    let mut it = UiInteraction::new();

    it.update(&scene.ui, screen, Some((100.0, 60.0)), false, false, false);
    assert_eq!(it.hover(), Some("btn"));
    assert!(it.pointer_over_ui());

    it.update(&scene.ui, screen, Some((100.0, 60.0)), true, true, false);
    assert_eq!(it.active(), Some("btn"));

    it.update(&scene.ui, screen, Some((100.0, 60.0)), true, false, true);
    // 抬起时光标还在元素上：hover 保持（hover 表示「指针在元素上」，与按键无关），
    // 但按下捕获已经释放。
    assert_eq!(it.hover(), Some("btn"));
    assert!(it.pointer_over_ui());
    assert_eq!(it.active(), None, "抬起后不该还挂着按下捕获");
    assert!(
        it.events()
            .iter()
            .any(|ev| ev.kind.name() == "clicked" && ev.key == "btn"),
        "同元素内按下并抬起应产出 clicked: {:?}",
        it.events()
            .iter()
            .map(|e| (e.kind.name(), e.key.clone()))
            .collect::<Vec<_>>()
    );

    // 移出元素：unhovered，且不再算压在 UI 上（app 据此把相机拖动还回去）。
    it.update(&scene.ui, screen, Some((900.0, 600.0)), true, false, false);
    assert_eq!(it.hover(), None);
    assert!(!it.pointer_over_ui());
    assert!(
        it.events()
            .iter()
            .any(|ev| ev.kind.name() == "unhovered" && ev.key == "btn"),
        "移出应产出 unhovered"
    );
}
