//! 角色不变量（M2-B04）：`character.config.invalid` / `character.ground.penetration`。
//!
//! 立场与 `validate.rs` 一致：负向用例从「合法场景」出发**只破坏一个约束**，
//! 断言必须出现指定 code 的 `Error`；正向用例守住「别把正常角色判成错的」。
//!
//! 首个字符串实参固定为 code（`xtask invariants` 按此约定扫源码，作为 B 源）。

use r3e_core::{CharacterSpec, ObjectDesc};
use r3e_doc::{Document, Severity};

fn doc_with_character(name: &str, pos: [f32; 3], spec: CharacterSpec) -> Document {
    let mut doc = Document::default();
    doc.write(|s| {
        let id = s
            .spawn(ObjectDesc {
                name: name.to_string(),
                position: Some(pos),
                ..Default::default()
            })
            .expect("角色节点应当建成功");
        s.get_mut(id).expect("刚建的节点").character = Some(spec);
        // 直接改场景字段不经过命令层，世界矩阵要自己刷（命令路径由 apply 负责）
        s.update_world_transforms();
    });
    doc
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

/// 一个负向用例：标签（只用于报错文案）+ 把合法 spec 破坏成非法的那一步。
type Case = (&'static str, fn(&mut CharacterSpec));

fn assert_quiet(doc: &Document) {
    let found = codes(doc);
    assert!(found.is_empty(), "合法角色不该报任何 Error：{found:?}");
}

// ---------- character.config.invalid ----------

#[test]
fn default_spec_is_quiet() {
    let doc = doc_with_character("hero", [0.0, 0.0, 0.0], CharacterSpec::new());
    assert_quiet(&doc);
}

#[test]
fn invalid_spec_fields_are_error() {
    // 五个字段各破坏一次；任何一项越界都必须报出来（不做夹取，不静默生效）
    // clippy 的 type_complexity：把「标签 + 变异闭包」抽成别名，别让数组类型撑爆一行
    let cases: [Case; 5] = [
        ("半径 0", |s| s.radius = 0.0),
        ("身高 0.05", |s| s.height = 0.05),
        ("坡度 120 度", |s| s.slope_limit_deg = 120.0),
        ("台阶 3 米", |s| s.step_height = 3.0),
        ("吸附 5 米", |s| s.snap_distance = 5.0),
    ];
    for (label, mutate) in cases {
        let mut spec = CharacterSpec::new();
        mutate(&mut spec);
        let doc = doc_with_character("hero", [0.0, 0.0, 0.0], spec);
        let found = codes(&doc);
        assert!(
            found.contains(&"character.config.invalid"),
            "{label} 应当报 character.config.invalid，实得 {found:?}"
        );
        // 配置非法时几何不可信，不该再连带报穿地
        assert!(
            !found.contains(&"character.ground.penetration"),
            "{label} 不该连带报穿地：{found:?}"
        );
    }
    // 另配一条单点断言，让「三源一致」的自查能按字面找到失败断言
    let mut bad = CharacterSpec::new();
    bad.radius = -1.0;
    let bad_doc = doc_with_character("hero", [0.0, 0.0, 0.0], bad);
    assert_error(&bad_doc, "character.config.invalid");
}

// ---------- character.ground.penetration ----------

#[test]
fn buried_character_is_error() {
    let doc = doc_with_character("hero", [0.0, -0.5, 0.0], CharacterSpec::new());
    assert_error(&doc, "character.ground.penetration");
}

#[test]
fn snap_distance_absorbs_shallow_sink() {
    let doc = doc_with_character("hero", [0.0, -0.08, 0.0], CharacterSpec::new());
    assert_quiet(&doc);
}

// ---------- 登记表自检 ----------

#[test]
fn issue_code_table_lists_character_family() {
    use r3e_doc::{issue_code_by_code, Category};

    for code in ["character.config.invalid", "character.ground.penetration"] {
        let ic = issue_code_by_code(code).unwrap_or_else(|| panic!("{code} 未登记"));
        assert_eq!(ic.layer, "character", "层名要与 validate/character.rs 对齐");
        assert_eq!(ic.severity, Severity::Error);
    }
    // 前缀与族一一对应的契约：不扩枚举的话 character.* 会被静默归成 bounds 族
    assert_eq!(
        Category::from_code("character.config.invalid"),
        Category::Character
    );
    // 未登记前缀仍走兜底，别顺手把兜底改掉
    assert_eq!(Category::from_code("no.such.code"), Category::Bounds);
}

#[test]
fn character_family_codes_are_asserted_here() {
    // 三源一致的 B 源自查：两条码各需一条 `assert_error(..., "<code>")` 失败断言
    // （改码文本时这里先红，别让登记表与测试断言走散）
    let src = include_str!("character_issues.rs");
    for code in ["character.config.invalid", "character.ground.penetration"] {
        let quoted = format!("\"{code}\"");
        let asserted = src
            .lines()
            .any(|line| line.contains("assert_error(") && line.contains(&quoted));
        assert!(asserted, "缺 {code} 的 assert_error 失败断言");
    }
}
