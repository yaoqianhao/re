//! 主通道「单次遍历 + 分桶」的静态契约（扩展二十七，不需要 GPU）。
//!
//! 扩展二十七把 `prepare_instances` 从「外层遍历批次键、内层遍历全场景」改成
//! 单次遍历后分桶摊平。这个改动对外不可见、却极其容易被后续重构改回去（改回去
//! 不会报错，只是每帧白扫一遍 `键数 × 节点数`），所以在这里钉住五件事：
//!
//! 1. `prepare_instances` 里遍历场景只允许**一次**（多出来就说明又回到逐键扫描）；
//! 2. 实例经共享的 `staged_all` 收拢、桶里只存下标，剔除在前、入桶在后；
//! 3. 键登记按**段列表**判重，不能拿桶是否已存在当判据；
//! 4. 桶键带「段」标志：同一 (网格, 材质) 组合可能同时落在不透明段与半透明段，
//!    两段必须各自成桶；
//! 5. 剔除计数（视锥 / 缺包围盒）与半透明顺序没有被顺手删掉。
//!
//! 第 3、4 条是拿像素回归换来的教训：把「键是否首次出现」判在 `buckets` 上时，
//! 同一个网格的玻璃物件（alpha < 1）与不透明物件共用一键，半透明段的键登记不上，
//! 摊平阶段整桶 `remove` 不到 —— 玻璃面板凭空消失，实例数悄悄少几个，而画面
//! 只在面板那块区域变暗，肉眼看几乎发现不了。`visual-gpu` 严格模式就是被它弄红的。

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

/// 实例先收进共享容器、再按批次键把下标分进桶，最后统一摊平。
#[test]
fn instances_are_collected_into_buckets() {
    let body = prepare_instances_body();
    assert!(
        body.contains("staged_all.push(StagedInstance"),
        "实例必须收进共享的 staged_all（分桶后统一摊平）"
    );
    assert!(
        body.contains(".entry((key.0, key.1, false))"),
        "不透明段的桶丢失"
    );
    // 入桶必须在剔除之后：被视锥剔除的实例不该留下痕迹
    let cull = body.find("culled += 1").expect("视锥剔除计数丢失");
    let push = body
        .find(".entry((key.0, key.1, false))")
        .expect("分桶丢失");
    assert!(
        cull < push,
        "剔除判定必须排在入桶之前，否则被剔除的实例仍会占实例槽位"
    );
}

/// 键登记必须按段列表判重，不能看桶里有没有这个键。
///
/// 同一 (网格, 材质) 键会同时出现在不透明段与半透明段（物件颜色 alpha 不同），
/// 桶是跨段共用的，用桶判重就会把后一段的键吞掉。
#[test]
fn keys_are_registered_per_segment() {
    let body = prepare_instances_body();
    assert!(
        body.contains("if !bucket_keys.contains(&key)"),
        "键必须按所属段列表（opaque_keys / transparent_keys）判重后登记"
    );
    assert!(
        !body.contains("if !buckets.contains_key(&key)"),
        "禁止用桶是否已有该键来判断键是否需要登记：同一网格跨两段时会把半透明键吞掉"
    );
}

/// 桶键必须带段标志：两段各自成桶，摊平也按段取桶。
#[test]
fn bucket_key_carries_segment_flag() {
    let body = prepare_instances_body();
    assert!(
        body.contains(".entry((key.0, key.1, true))"),
        "半透明段的桶丢失：玻璃 / 水面整层会凭空消失"
    );
    assert!(
        body.contains("buckets.remove(&(key, surface, segment_transparent))"),
        "摊平必须按 (键, 段) 取桶，否则两段互相顶掉"
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
