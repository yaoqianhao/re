// 扩展四十七示例：相位同步组（每路错半步 + 上下半身对齐）。
//
// 四十六解决了「同一层内长短片段不同拍」，但步态编排还需要两件本事：
// ① 错步偏移：同层内让某一路整体提前/后移半个周期（跑步比走路的脚掌早半步落地）；
// ② 跨层对齐：上半身跟下半身共用一个相位，避免「腿在迈、手还停在上一拍」。
//
// 本示例把两者串起来，日志里三个阶段就是证据：
//   t < 1.0  偏移 0、无源层 —— 与四十六逐位一致（探针里没有 off= / src= 段）
//   t ≥ 1.0  给 run 那一路加 0.5 错步 —— 探针出现 off=run=0.500，drift 仍为 0
//   t ≥ 2.0  lower 跟随 upper 的相位 —— 探针出现 src=upper，sync_phase 收敛到 upper 的相位
//
// 启动：r3e assets/scene_anim.json scripts/phase_group_demo.rhai --run-seconds 3.2
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
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    if idle == "" || run == "" {
        print("缺少 idle / run 片段，示例跳过");
        return;
    }
    print(`相位同步组示例：实例 ${inst}，片段 ${clips}`);

    // 下半身：走 + 跑同一层（典型会错拍的组合），锁相位后再谈错步。
    add_anim_layer(inst, "lower", idle, 0.5);
    add_anim_layer_clip(inst, "lower", run, 0.5);
    set_anim_layer_sync(inst, "lower", true);

    // 上半身：单路、半速，作为下半身的相位源。
    add_anim_layer(inst, "upper", run, 0.5);
    set_anim_layer_speed(inst, "upper", 0.5);
    set_anim_layer_sync(inst, "upper", true);
    print("层已建：" + anim_layer_info(inst, "lower"));
    dump(inst, "lower", run, 0.0);
}

// 一行探针：偏移 / 源层 / 本层相位 / 实际采样相位 / drift。
// 先拆成局部变量再拼，避免 rhai 对超长单表达式报复杂度超限。
fn dump(inst, layer, clip, t) {
    let off = anim_layer_clip_phase_offset(inst, layer, clip);
    let src = anim_layer_sync_source(inst, layer);
    let phase = anim_layer_phase(inst, layer);
    let sp = anim_layer_sync_phase(inst, layer);
    let probe = anim_layer_sync_probe(inst, layer);
    let head = "PHASE t=" + t + " off=" + off + " src=" + src;
    print(head + " phase=" + phase + " sync_phase=" + sp + " " + probe);
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let t = anim_time(inst);
    let run = pick(anim_clips("tirger"), "run");
    if run == "" {
        return;
    }

    // 阶段二：给跑那一路错半步。是否已经设过直接问引擎，脚本不自己记状态。
    if t >= 1.0 && anim_layer_clip_phase_offset(inst, "lower", run) == 0.0 {
        let ok = set_anim_layer_clip_phase_offset(inst, "lower", run, 0.5);
        print("=== 加错步半步 ok=" + ok + " off=" + anim_layer_clip_phase_offset(inst, "lower", run));
    }

    // 阶段三：下半身跟随上半身相位。
    if t >= 2.0 && anim_layer_sync_source(inst, "lower") == "" {
        let ok = set_anim_layer_sync_source(inst, "lower", "upper");
        let src = anim_layer_sync_source(inst, "lower");
        let up = anim_layer_phase(inst, "upper");
        print("=== 跨层对齐 ok=" + ok + " src=" + src + " upper_phase=" + up);
        print("层已改：" + anim_layer_info(inst, "lower"));
    }

    dump(inst, "lower", run, t);
}
