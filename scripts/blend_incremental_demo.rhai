// 扩展五十三示例：增量剖分（细粒度脏标记）。
//
// 扩展五十一解决了「一次拖拽付 N 次重建」，但付费单位还是整表：加一个样本与清空重铺
// 付一样的钱。样本上千（动作捕捉按帧铺格子）时，整表重剖的 O(n²) 就成了编辑器卡顿的来源。
//
// 现在的口径：
//   · 续算现场（超级三角形骨架 + 已插入的样本前缀 + 包围盒）随剖分一起缓存；
//   · 新样本点落在**旧包围盒内**时，直接把点插进已有剖分（2D 扫当前三角形表挑出外接圆
//     含该点的、消内边重建；3D 同理换四面体与外接球），只在包围盒 / 维度 / 样本前缀
//     对不上时才回退整表重剖；
//   · 账本 `anim_layer_blend_build_stats` 回显 `inc=增量续算次数 full=整表重剖次数`，
//     编辑器与探针靠它确认「补点真的走了续算」。
//
// 启动：r3e assets/scene_anim.json scripts/blend_incremental_demo.rhai --run-seconds 4 --show-bones
//
// 日志里重点看三件事：
//   ① 装配四角后第一次提交是 `inc=0 full=1`（骨架还没铺完的场景，只能整表建场）；
//   ② 之后每次往盒内补点，`et=` 行的 inc 加一、full 不动——同一份现场一直续用；
//   ③ 权重与「一次性全加再重剖」逐位相同（同一行的 `tri=` 与 `w=` 不会因为走了
//      增量路径而变形），即接线方式对姿态零影响。

fn pick(clips, key) {
    for c in clips {
        if c.contains(key) {
            return c;
        }
    }
    ""
}

// 顶层 let 在 r3e 脚本引擎里每帧重新初始化，不能当跨帧标志用；
// 这里拿账本回显当守卫：只有当前账本还等于 before 时才补点，避免同一帧重复加。
fn step_once(inst, layer, before, clip, x, y, tag) {
    if anim_layer_blend_build_stats(inst, layer) != before {
        return;
    }
    if add_anim_layer_blend_sample(inst, layer, clip, x, y) {
        build_anim_layer_blend(inst, layer);
        print(
            `补点 ${tag}(${clip} @ ${x},${y}) → ` + anim_layer_blend_build_stats(inst, layer)
        );
    }
}

fn on_start() {
    let inst = anims()[0];
    let clips = anim_clips("tirger");
    print(`增量剖分示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    let atk = pick(clips, "atk");
    let atk2 = pick(clips, "atk2");
    if idle == "" || run == "" || atk == "" || atk2 == "" {
        print("缺少 idle / run / atk / atk2 片段，示例跳过");
        return;
    }

    add_anim_layer(inst, "inc", idle, 1.0);
    add_anim_layer_clip(inst, "inc", run, 0.0);
    add_anim_layer_clip(inst, "inc", atk, 0.0);
    add_anim_layer_clip(inst, "inc", atk2, 0.0);

    // 先把四角打满：盒内补点能否续算，全看这一步的包围盒定得够不够大
    set_anim_layer_blend_2d(inst, "inc", "move.x", "move.y");
    let corners = [
        [idle, -1.0, -1.0],
        [run, 1.0, -1.0],
        [atk, -1.0, 1.0],
        [atk2, 1.0, 1.0],
    ];
    let added = 0;
    for r in corners {
        if add_anim_layer_blend_sample(inst, "inc", r[0], r[1], r[2]) {
            added += 1;
        }
    }
    build_anim_layer_blend(inst, "inc");
    print(`装配完成：${added} 个角点，账本 ` + anim_layer_blend_build_stats(inst, "inc"));
    print("提交后  " + anim_layer_blend(inst, "inc"));
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let et = time();

    // 盒内补点：每次都该是 inc 加一、full 不动
    if et >= 1.0 {
        step_once(
            inst, "inc", "inc=0 full=1", pick(anim_clips("tirger"), "run"), 0.0, -1.0, "棱中点",
        );
    }
    if et >= 2.0 {
        step_once(
            inst, "inc", "inc=1 full=1", pick(anim_clips("tirger"), "atk"), 0.0, 1.0, "棱中点",
        );
    }
    if et >= 3.0 {
        step_once(
            inst, "inc", "inc=2 full=1", pick(anim_clips("tirger"), "idle"), 0.0, 0.0, "体心",
        );
    }

    if et < 1.0 {
        set_anim_layer_blend_value(inst, "inc", 0.0, 0.0);
    } else if et < 2.0 {
        set_anim_layer_blend_value(inst, "inc", 0.5, -1.0);
    } else if et < 3.0 {
        set_anim_layer_blend_value(inst, "inc", 0.0, 0.0);
    } else {
        set_anim_layer_blend_value(inst, "inc", -0.5, 0.25);
    }

    if et < 3.5 {
        print(
            "et=" + et + " " + anim_layer_blend_build_stats(inst, "inc") + " " + anim_layer_blend(
                inst,
                "inc",
            )
        );
    }
}
