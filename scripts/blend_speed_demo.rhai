// 扩展五十二示例：速率随样本插值（慢步→疾行的步频跟着参数连续变化）。
//
// 一层里挂 idle_loop / run_loop 两路片段、由一维速度空间分配份额时，份额决定的不只是姿态，
// 还应决定「这一层该以多快的节奏推进」：参数刚离开 idle 端点时步频只该比原速快一点点，
// 走到 run 顶点才满速——否则玩家轻推摇杆就会看到步频从慢步直接跳到疾行。
//
// 这个示例把 idle_loop / run_loop 挂在同一层，用一维速度空间驱动权重与速率：
//   阶段一 v=0.0   全 idle_loop     —— 权重全在未变速那一路，倍率 1.0（与五十一之前逐位一致）
//   阶段二 v=0.5   各半            —— 倍率插到 1.35（idle 1.0 / run 1.7）
//   阶段三 v=1.0   全 run_loop      —— 倍率 1.7，层时间以 1.7 倍速推进
//   阶段四 v=1.0   再抬 run 到 2.4  —— 倍率跟着样本改，参数不动步频也变
//   阶段五 v=0.5   回到半程          —— 倍率 (1.0 + 2.4) / 2，证明倍率始终是样本速率的加权结果
// 启动：r3e assets/scene_anim.json scripts/blend_speed_demo.rhai --run-seconds 6 --show-bones
//
// 日志每帧的 `blend=` 行在**有变速样本**时尾部追加 `spd=`，可直接看出当前插出来的倍率；
// 对比相邻阶段的 `run=` 片段时间差即可确认「参数连续 → 步频连续」。

fn pick(clips, key) {
    for c in clips {
        if c.contains(key) {
            return c;
        }
    }
    ""
}

fn on_start() {
    let inst = anims()[0];
    let clips = anim_clips("tirger");
    print(`速率随样本插值示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle_loop");
    let run = pick(clips, "run_loop");
    if idle == "" || run == "" {
        print("缺少 idle_loop / run_loop 片段，示例跳过");
        return;
    }

    // 一层两路：idle_loop 全额、run_loop 由空间接管
    add_anim_layer(inst, "move", idle, 1.0);
    add_anim_layer_clip(inst, "move", run, 0.0);
    print("层信息：" + anim_layer_info(inst, "move"));

    // 一维速度空间：idle_loop 在 0、run_loop 在 1
    set_anim_layer_blend_1d(inst, "move", "speed");
    add_anim_layer_blend_sample(inst, "move", idle, 0.0, 0.0);
    add_anim_layer_blend_sample(inst, "move", run, 1.0, 0.0);
    print("1D 空间已挂：" + anim_layer_blend(inst, "move"));

    // run_loop 的样本速率抬到 1.7：只有离开 idle 端点才会生效
    let ok = set_anim_layer_blend_sample_speed(inst, "move", run, 1.7);
    print("设 run 速率 1.7 → " + ok);
    print("当前插出倍率 spd=" + anim_layer_blend_speed(inst, "move"));
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let et = time();
    if et < 1.0 {
        // 全 idle_loop：倍率必须回到 1.0（权重全落在未变速那一路）
        set_anim_layer_blend_value(inst, "move", 0.0, 0.0);
    } else if et < 2.0 {
        // 各半：倍率 (1.0 + 1.7) / 2
        set_anim_layer_blend_value(inst, "move", 0.5, 0.0);
    } else if et < 3.0 {
        // 全 run_loop：倍率 1.7
        set_anim_layer_blend_value(inst, "move", 1.0, 0.0);
    } else if et < 4.0 {
        // 参数不动，只改样本速率：步频应立刻变，但姿态不变
        let run = pick(anim_clips("tirger"), "run_loop");
        set_anim_layer_blend_sample_speed(inst, "move", run, 2.4);
        set_anim_layer_blend_value(inst, "move", 1.0, 0.0);
    } else {
        // 退回半程：倍率回到 (1.0 + 2.4) / 2
        set_anim_layer_blend_value(inst, "move", 0.5, 0.0);
    }

    if et < 5.5 {
        print("et=" + et + " spd=" + anim_layer_blend_speed(inst, "move") + " " + anim_layer_blend(inst, "move"));
    }
}
