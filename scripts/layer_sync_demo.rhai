// 扩展四十六示例：层内相位同步（走跑不再错拍）。
//
// 一整层只共享一个「层时间」，片段各自有自己的时长。不锁相位时，短片段先播完就挂在
// 末帧上，长片段还在走 —— 常见表现就是跑步动作停住、脚步却还在滑。锁上相位后，每路
// 片段的采样时间改成 `相位 × 本片段时长`，同一相位永远采到同一段步态（脚掌同时落地），
// 步频差异照旧保留。
//
// 启动：r3e assets/scene_anim.json scripts/layer_sync_demo.rhai --run-seconds 2.4 --show-bones
//
// 日志里每帧的 `SYNC` 行就是证据：`drift` 是各路片段相位与层相位的最大偏差。
// 前 1 秒策略为 off，drift 一路上涨；到 1 秒时切到 phase，drift 立刻归零。

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
    print(`相位同步示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    if idle == "" || run == "" {
        print("缺少 idle / run 片段，示例跳过");
        return;
    }

    // 两层同权重的走跑混合：一个长片段配一个短片段，正是会错拍的典型组合。
    add_anim_layer(inst, "locomotion", idle, 0.5);
    add_anim_layer_clip(inst, "locomotion", run, 0.5);
    print("层已建：" + anim_layer_info(inst, "locomotion"));
    print("SYNC off  t=0 " + anim_layer_sync_probe(inst, "locomotion"));
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let t = anim_time(inst);

    // 1 秒处切到相位锁：同一层的相位不变，只是每路片段的采样时间换了口径。
    // 是否已切换直接问引擎（anim_layer_sync），脚本不必自己记状态。
    if t >= 1.0 && !anim_layer_sync(inst, "locomotion") {
        let ok = set_anim_layer_sync(inst, "locomotion", true);
        print("=== 切到相位锁 ok=" + ok + " now=" + anim_layer_sync(inst, "locomotion"));
        print("层已改：" + anim_layer_info(inst, "locomotion"));
    }

    let tag = if anim_layer_sync(inst, "locomotion") { "phase" } else { "off  " };
    print("SYNC " + tag + " t=" + t + " " + anim_layer_sync_probe(inst, "locomotion"));
}
