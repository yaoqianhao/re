//! M1-S4-F 宿主层：`scene.verify`（三层体检的协议面出口）。
//!
//! 这一组测试盯的是**回包语义**而不是某条 issue 的算法（算法在 `verify::` 里已有
//! 各自用例）。三条底线：
//!
//! * `ok` 与 `passed` 分工不许糊：物理 / 导航没跑成时 `ok` 可以是 `true`，
//!   `passed` 必须是 `false`——「没检查」永远不等于「通过」；
//! * `unchecked` 必须点明「哪一层、为什么」，光一个布尔值救不了排查；
//! * 只读：调用前后场景 `state_hash` 不变（体检不许改场景）。

use super::*;
use r3e_editor::{VerifyTarget, INVALID_PARAMS};

/// 体检一个默认 fixture（3 个裸 cube、物理关闭、无导航槽位）。
fn verify(api: &mut LocalEngineApi<'_>, query: VerifyQuery) -> Value {
    api.scene_verify(query).expect("体检本身不该失败")
}

/// 三层里有两层没跑成：`ok` 可能为真，`passed` 必须为假，且原因要逐层写清。
#[test]
fn verify_没接物理与导航时绝不算通过() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let report = verify(&mut api, VerifyQuery::default());

    let unchecked = report["unchecked"].as_array().expect("unchecked 应为数组");
    let layers: Vec<&str> = unchecked
        .iter()
        .map(|u| u["layer"].as_str().expect("unchecked.layer 应为字符串"))
        .collect();
    assert!(
        layers.contains(&"physics"),
        "物理关闭必须记 unchecked：{layers:?}"
    );
    assert!(
        layers.contains(&"nav"),
        "没接导航槽位必须记 unchecked：{layers:?}"
    );
    for u in unchecked {
        let reason = u["reason"].as_str().expect("unchecked.reason 应为字符串");
        assert!(reason.len() > 8, "原因要能读懂，不能只说「没查」：{reason}");
    }

    assert_eq!(
        report["passed"],
        json!(false),
        "有两层没查，passed 不许为真：{report}"
    );
    assert_eq!(
        report["layers"],
        json!(["static"]),
        "只跑了静态层：{report}"
    );
}

/// 静态层的结论要带「层」与「路径」：前端按 `layer` 分组、按 `path` 定位节点。
#[test]
fn verify_静态层结论带层与节点路径() {
    let (scene, physics, mut scripts) = fixture();
    // fixture 的 3 个 cube 全在原点且都是单位立方体 —— 必然互相重叠。
    let mut api = api(&scene, &physics, &mut scripts);
    let report = verify(&mut api, VerifyQuery::default());

    let issues = report["issues"].as_array().expect("issues 应为数组");
    assert!(!issues.is_empty(), "全在原点不该零结论：{report}");
    for issue in issues {
        assert!(issue["layer"].is_string(), "每条结论都要带登记层：{issue}");
        assert!(
            issue["path"]
                .as_str()
                .is_some_and(|p| p.starts_with("nodes[")),
            "结论要能定位到节点：{issue}"
        );
        assert!(
            issue["code"].is_string() && issue["severity"].is_string(),
            "code / severity 必填：{issue}"
        );
    }
    assert_eq!(
        report["counts"]["error"],
        json!(0),
        "重叠只是 suspect（Warn），不该记 Error"
    );
}

/// `limit` 只截明细，不动计数：调用方要知道「还剩多少没看见」。
#[test]
fn verify_limit只截明细不动计数() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let full = verify(&mut api, VerifyQuery::default());
    let total = full["issues"].as_array().expect("issues 应为数组").len();
    assert!(total >= 2, "至少两条重叠才验得出截断，实际 {total}");

    let cut = verify(
        &mut api,
        VerifyQuery {
            targets: Vec::new(),
            limit: Some(1),
        },
    );
    assert_eq!(cut["issues"].as_array().expect("issues 应为数组").len(), 1);
    assert_eq!(cut["truncated"], json!(true), "截断了要标出来：{cut}");
    assert_eq!(
        cut["counts"]["byLayer"]["static"],
        json!(total),
        "计数不受截断影响"
    );
    assert_eq!(cut["counts"]["error"], full["counts"]["error"]);
}

/// 坐标非有限是**调用方的错**（协议层只管形状）：回参数错误并点名哪一项。
#[test]
fn verify_非有限坐标回参数错误() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let failure = api
        .scene_verify(VerifyQuery {
            targets: vec![VerifyTarget {
                name: "chest".to_string(),
                at: [0.0, f32::NAN, 0.0],
            }],
            limit: None,
        })
        .expect_err("NaN 坐标不该进入几何计算");
    assert_eq!(failure.code, INVALID_PARAMS, "错误码不对：{failure:?}");
    assert!(
        failure.message.contains("targets[0].at"),
        "消息要点名哪一项：{}",
        failure.message
    );
}

/// 体检是只读的：调用前后场景哈希必须一致（否则「查一次」会被当成「改了一次」）。
#[test]
fn verify_不改场景哈希() {
    let (scene, physics, mut scripts) = fixture();
    let before = scene.borrow().state_hash();
    let mut api = api(&scene, &physics, &mut scripts);
    let report = verify(
        &mut api,
        VerifyQuery {
            targets: vec![VerifyTarget {
                name: "chest".to_string(),
                at: [0.0, 1.0, 0.0],
            }],
            limit: None,
        },
    );
    let hash = report["stateHash"].as_str().expect("回包要带当前场景哈希");
    assert!(
        hash.starts_with("0x") && hash.len() == 18,
        "哈希形如 0x + 16 位十六进制：{hash}"
    );
    assert_eq!(before, scene.borrow().state_hash(), "体检不许改场景");
}

/// 物理开着但世界没装配：不许拿「世界里什么都没有」冒充「没有悬空」。
#[test]
fn verify_物理世界与场景不同步时记unchecked() {
    let (scene, physics, mut scripts) = fixture();
    // 带 `physics` 段的节点才会进物理世界：世界不装它，就与场景对不上。
    scene.borrow_mut().write(|scene| {
        scene
            .spawn(ObjectDesc {
                name: "crate".to_string(),
                mesh: Some("cube".to_string()),
                // 只写 `body`，其余字段走 serde 缺省——只关心「这个节点会进物理世界」。
                physics: Some(
                    serde_json::from_value::<r3e_core::PhysicsDesc>(json!({ "body": "static" }))
                        .expect("PhysicsDesc 应有 serde 缺省"),
                ),
                ..Default::default()
            })
            .expect("测试场景 spawn 不应失败");
    });
    physics.borrow_mut().set_enabled(true);
    let mut api = api(&scene, &physics, &mut scripts);
    let report = verify(&mut api, VerifyQuery::default());

    let reasons: Vec<&str> = report["unchecked"]
        .as_array()
        .expect("unchecked 应为数组")
        .iter()
        .filter(|u| u["layer"] == json!("physics"))
        .map(|u| u["reason"].as_str().expect("reason 应为字符串"))
        .collect();
    assert!(
        !reasons.is_empty(),
        "物理开着却没装世界，必须记 physics unchecked：{report}"
    );
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("不同步") || r.contains("shape_cast")),
        "原因要指向「世界/后端」而不是泛泛一句没查：{reasons:?}"
    );
    assert_eq!(
        report["passed"],
        json!(false),
        "物理层不可信，不能说 passed"
    );
}
