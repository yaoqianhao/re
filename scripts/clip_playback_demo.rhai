// 扩展四十八示例：片段自带播放速率与循环标志。
//
// 四十六 / 四十七 解决的是「同一层内长短片段不同拍」与「跨层对齐」，默认前提是
// 每路片段都按层时间轴走、到末帧即停。但真实编排里常常要的是：
// ① 同一层里某一路跑得更快（群演里一个「跑」压在一个「走」上，各自节拍不同）；
// ② 某一路播一次就停住（一次性受击 / 落地缓冲），别跟着层无限循环。
// 这两种诉求以前只能整层调 `set_anim_layer_speed`，一调就把同层其他路一起带走。
//
// 本示例三个阶段，日志里就是证据（场景里的层时间在 2.1 s 处回绕，所以三个阶段都排在一个循环内）：
//   t < 0.5  不设播放参数 —— 探针与四十七逐位一致（没有 pb= 段，两路时间都等于层时间）
//   t ≥ 0.5  给 run 那一路设 1.5 倍速 + 不循环 —— 探针出现 pb=run=1.50:once，这路时间被折到末帧
//   t ≥ 1.5  清除播放参数 —— pb= 段消失，这路时间逐位回到层时间
//
// 启动：r3e assets/scene_anim.json scripts/clip_playback_demo.rhai --run-seconds 2.0
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
    print(`片段播放示例：实例 ${inst}，片段 ${clips}`);

    // 一层两路：走 + 跑。**不锁相位**——片段自带速率只在 `sync = off` 下参与采样，
    // 相位锁层的每路时间由相位决定，要变速请调层 speed（这条边界本示例也想让人撞一次）。
    add_anim_layer(inst, "body", idle, 0.5);
    add_anim_layer_clip(inst, "body", run, 0.5);
    print("层已建：" + anim_layer_info(inst, "body"));
    dump(inst, "body", idle, run, 0.0);
}

// 一行探针：两路各自采到的时间 + 自带速率/循环回显 + 引擎侧探针行。
// 先拆局部变量再拼，避免 rhai 对超长单表达式报复杂度超限。
fn dump(inst, layer, idle, run, t) {
    let ti = anim_layer_clip_time(inst, layer, idle);
    let tr = anim_layer_clip_time(inst, layer, run);
    let sp = anim_layer_clip_speed(inst, layer, run);
    let lp = anim_layer_clip_looping(inst, layer, run);
    let probe = anim_layer_sync_probe(inst, layer);
    let head = "CLIP t=" + t + " idle_t=" + ti + " run_t=" + tr;
    print(head + " run_speed=" + sp + " run_looping=" + lp + " " + probe);
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let t = anim_time(inst);
    let clips = anim_clips("tirger");
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    if idle == "" || run == "" {
        return;
    }

    // 阶段二：给跑那一路单独提速并改成「只播一次」。是否已经设过直接问引擎，脚本不自己记状态。
    if t >= 0.5 && t < 1.5 && anim_layer_clip_speed(inst, "body", run) < 0.0 {
        let ok = set_anim_layer_clip_playback(inst, "body", run, 1.5, false);
        let sp = anim_layer_clip_speed(inst, "body", run);
        print("=== 该路提速 1.5x 且不循环 ok=" + ok + " speed=" + sp);
        print("层已改：" + anim_layer_info(inst, "body"));
    }

    // 阶段三：清除播放参数，回到跟着层时间轴走的老口径。
    if t >= 1.5 && t < 2.1 && anim_layer_clip_speed(inst, "body", run) >= 0.0 {
        let ok = clear_anim_layer_clip_playback(inst, "body", run);
        let sp = anim_layer_clip_speed(inst, "body", run);
        print("=== 清除播放参数 ok=" + ok + " speed=" + sp + "（回到 -1 = 跟层走）");
        print("层已改：" + anim_layer_info(inst, "body"));
    }

    dump(inst, "body", idle, run, t);
}
