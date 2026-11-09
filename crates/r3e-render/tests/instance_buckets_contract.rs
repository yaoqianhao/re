//! 主通道「单次遍历 + 分桶」的静态契约（扩展二十七，不需要 GPU）。
//!
//! 扩展二十七把 `prepare_instances` 从「外层遍历批次键、内层遍历全场景」改成
//! 单次遍历后分桶摊平。这个改动对外不可见、却极其容易被后续重构改回去（改回去
//! 不会报错，只是每帧白扫一遍 `键数 × 节点数`），所以在这里钉住四件事：
//!
//! 1. `prepare_instances` 里遍历场景只允许**一次**（多出来就说明又回到逐键扫描）；
//! 2. 实例确实经 `buckets` 收拢（分桶存在，剔除在前、入桶在后）；
//! 3. 键顺序仍是「不透明段 + 半透明段」，半透明段仍在全部批次之后；
//! 4. 剔除计数（视锥 / 缺包围盒）与半透明组间排序没有被顺手删掉。

const SOURCE: &str = include_str!("../src/renderer.rs");

/// 截出 `prepare_instances` 的函数体：从签名行到下一个同级 `fn` 之前。
fn prepare_instances_body() -> &'static str {
    let start = SOURCE
        .find("fn prepare_instances")
        .expect("renderer.rs 里必须还有 prepare_instances");
    let rest = &SOURCE[start..];
    let end = rest[1..]
        .find("\n    /// ")
        .or_else(|| rest[1..].find("\n    fn "))
        .or_else(|| rest[1..].find("\n    pub fn "))
        .expect("prepare_instances 之后应该还有别的成员函数");
    &rest[..end + 1]
}

/// 场景迭代只允许一次：两次就是退回了逐键扫描。
#[test]
fn scene_is_iterated_once() {
    let body = prepare_instances_body();
    let count = body.matches("scene.iter()").count();
    assert_eq!(
        count, 1,
        "prepare_instances 里 scene.iter() 出现 {count} 次；分桶重构要求只遍历一遍场景"
    );
}

/// 实例先按批次键入桶，再摊平——分桶这一步不能丢。
#[test]
fn instances_are_collected_into_buckets() {
    let body = prepare_instances_body();
    assert!(
        body.contains("buckets.entry(key).or_default().push(StagedInstance"),
        "实例必须经 buckets 收拢（分桶后统一摊平）"
    );
    // 入桶必须在剔除之后：剔除分支 continue 掉的实例不该留下痕迹
    let cull = body.find("culled += 1").expect("视锥剔除计数丢失");
    let push = body.find("buckets.entry(key)").expect("分桶丢失");
    assert!(
        cull < push,
        "剔除判定必须排在入桶之前，否则被剔除的实例仍会占实例槽位"
    );
}

/// 批次排列：不透明键在前、半透明键在后，组内顺序仍是场景迭代序。
#[test]
fn opaque_batches_still_come_before_transparent() {
    let body = prepare_instances_body();
    let opaque = body
        .find("let opaque_key_count = opaque_keys.len();")
        .expect("不透明段起点丢失");
    let extend = body
        .find("keys.extend(transparent_keys);")
        .expect("半透明键拼接丢失");
    assert!(
        opaque < extend,
        "必须先记下不透明段长度，再把半透明键接在后面"
    );
    assert!(
        body.contains("if !buckets.contains_key(&key)"),
        "键只在首次出现时登记，否则批次顺序会被重复推入打乱"
    );
}

/// 剔除统计与半透明组间排序是既有行为，重构不得顺手删。
#[test]
fn cull_stats_and_transparent_order_survive() {
    let body = prepare_instances_body();
    assert!(body.contains("culled += 1"), "视锥剔除计数丢失");
    assert!(body.contains("cull_unknown += 1"), "缺包围盒计数丢失");
    assert!(
        body.contains("pending_transparent.sort_by(|a, b| b.0.total_cmp(&a.0));"),
        "半透明组间排序（远组先画）丢失"
    );
    assert!(
        body.contains("transparency::back_to_front(&depths)"),
        "半透明组内由远及近排序丢失"
    );
}
