// 扩展五十一示例：混合空间剖分缓存（脏标记 + 延迟提交）。
//
// 编辑器里拖样本是「一次改一个」的节奏，而扩展五十一之前每次 `add_sample` 都立刻整表
// 重剖（2D Bowyer–Watson 三角剖分 / 3D 四面体剖分）。装 12 个样本就付 12 次重建，
// 第 12 次的结果已经把前 11 次全废掉了——纯浪费。
//
// 现在的口径：
//   · `add_anim_layer_blend_sample*` 只置脏标记，不重建；
//   · 求值侧**不看缓存**：挂起时现场算一份临时拓扑，权重与提交后逐位相同；
//   · 提交点有两个：逐帧 advance（引擎自动），或脚本显式 `build_anim_layer_blend`；
//   · 报数 / 回显走 `triangle_count` / `tetra_count`，挂起时现场数一遍，
//     所以不会出现「刚加完样本 tri=0」这种会误导编辑器的中间态。
//
// 启动：r3e assets/scene_anim.json scripts/blend_cache_demo.rhai --run-seconds 4 --show-bones
//
// 日志里重点看三件事：
//   ① 装配期每加一个样本 stale 都翻成 true，但 `blend=` 行的 tri 数量已经是最终值；
//   ② 装配完成后 `build` 第一次返回 true、第二次返回 false（不重复付钱）；
//   ③ 再补样本 → stale 又 true → 下一帧 advance 后自动回 false。

fn pick(clips, key) {
    for c in clips {
        if c.contains(key) {
            return c;
        }
    }
    ""
}

// 顶层 let 在 r3e 的脚本引擎里每帧重新初始化，不能当跨帧标志用；
// 这里用回显里的 n= 判断样本表是否还是装配时的 4 个 —— 重复坐标会被拒并计 drop，
// 所以宁可先判再调，别拿拒绝路径当去重。
fn seed_once(inst) {
    if !anim_layer_blend(inst, "cache").contains(" n=4 ") {
        return false;
    }
    let run = pick(anim_clips("tirger"), "run");
    if run == "" {
        return false;
    }
    let ok = add_anim_layer_blend_sample(inst, "cache", run, 0.0, -1.0);
    if ok {
        print("补样本后 stale=" + anim_layer_blend_stale(inst, "cache"));
    }
    ok
}

fn on_start() {
    let inst = anims()[0];
    let clips = anim_clips("tirger");
    print(`剖分缓存示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    let atk = pick(clips, "atk");
    let atk2 = pick(clips, "atk2");
    if idle == "" || run == "" || atk == "" || atk2 == "" {
        print("缺少 idle / run / atk / atk2 片段，示例跳过");
        return;
    }

    add_anim_layer(inst, "cache", idle, 1.0);
    add_anim_layer_clip(inst, "cache", run, 0.0);
    add_anim_layer_clip(inst, "cache", atk, 0.0);
    add_anim_layer_clip(inst, "cache", atk2, 0.0);

    // 2D 空间：四角 × 三档速 = 12 个样本，模拟编辑器一次性铺格子
    set_anim_layer_blend_2d(inst, "cache", "move.x", "move.y");
    let rows = [
        [idle, -1.0, -1.0],
        [run, 1.0, -1.0],
        [atk, -1.0, 1.0],
        [atk2, 1.0, 1.0],
    ];
    let added = 0;
    for r in rows {
        if add_anim_layer_blend_sample(inst, "cache", r[0], r[1], r[2]) {
            added += 1;
        }
    }
    print(`装配完成：成功 ${added} 个样本，stale=` + anim_layer_blend_stale(inst, "cache"));
    // 挂起状态下回显：tri 已经是最终值（4 个样本 → 2 个三角形）
    print("挂起中  " + anim_layer_blend(inst, "cache"));

    // 显式提交：第一次真的重建，第二次是空操作
    let first = build_anim_layer_blend(inst, "cache");
    let second = build_anim_layer_blend(inst, "cache");
    print(`显式提交：第一次=${first} 第二次=${second} stale=` + anim_layer_blend_stale(inst, "cache"));
    print("提交后  " + anim_layer_blend(inst, "cache"));
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let et = time();
    if et < 1.0 {
        set_anim_layer_blend_value(inst, "cache", 0.0, 0.0);
    } else if et < 2.0 {
        // 补一个样本：又挂起，靠 advance 自动提交（本帧之后应回 false）
        if et < 1.05 {
            seed_once(inst);
        }
        set_anim_layer_blend_value(inst, "cache", 0.5, -0.5);
    } else if et < 3.0 {
        set_anim_layer_blend_value(inst, "cache", 1.0, 0.0);
    } else {
        set_anim_layer_blend_value(inst, "cache", -1.0, 1.0);
    }

    if et < 3.5 {
        print("et=" + et + " stale=" + anim_layer_blend_stale(inst, "cache") + " " + anim_layer_blend(inst, "cache"));
    }
}
