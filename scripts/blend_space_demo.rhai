// 扩展四十五示例：1D / 2D 混合空间驱动动画层权重。
//
// 混合空间把「参数 → 各片段权重」这段算术从脚本搬进引擎：脚本只管往参数轴上扔一个数
// （跑得多快、看向哪个方向），权重由样本表的几何插值算出来，脚本不再自己写 if/else。
//
// 这个示例挂两层：
//   locomotion —— 1D（speed 轴）：idle@0 / run@1，参数由当前片段播放时间推进；
//   aim        —— 2D（aim.x, aim.y）：idle 与 atk 落在对角两点，参数固定 (-0.5, 0.5)。
// 启动：r3e assets/scene_anim.json scripts/blend_space_demo.rhai --run-seconds 3 --show-bones
//
// 日志里每帧的 `blend=` 行就是探针入口：维度、样本数、被拒次数、当前参数、算出来的权重。

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
    print(`混合空间示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    let atk = pick(clips, "atk");
    if idle == "" || run == "" {
        print("缺少 idle / run 片段，示例跳过");
        return;
    }

    // ① 一维：speed 轴 0=idle、1=run
    add_anim_layer(inst, "locomotion", idle, 1.0);
    add_anim_layer_clip(inst, "locomotion", run, 0.0);
    set_anim_layer_blend_1d(inst, "locomotion", "speed");
    add_anim_layer_blend_sample(inst, "locomotion", idle, 0.0, 0.0);
    add_anim_layer_blend_sample(inst, "locomotion", run, 1.0, 0.0);
    print(`1D 空间已挂：${anim_layer_blend(inst, "locomotion")}`);

    // ② 二维：aim 平面，idle / run / atk 三个样本撑起一个三角
    if atk != "" {
        add_anim_layer(inst, "aim", idle, 0.5);
        add_anim_layer_clip(inst, "aim", run, 0.0);
        add_anim_layer_clip(inst, "aim", atk, 0.0);
        set_anim_layer_blend_2d(inst, "aim", "aim.x", "aim.y");
        add_anim_layer_blend_sample(inst, "aim", idle, -1.0, -1.0);
        add_anim_layer_blend_sample(inst, "aim", run, 1.0, -1.0);
        add_anim_layer_blend_sample(inst, "aim", atk, -1.0, 1.0);
        // 故意加一个和 idle 同位置的样本：引擎应当拒绝（返回 false），drop 计数 +1
        let dup = add_anim_layer_blend_sample(inst, "aim", atk, -1.0, -1.0);
        print("重复样本被拒: " + (!dup));
        set_anim_layer_blend_value(inst, "aim", -0.5, 0.5);
        print("2D 空间已挂：" + anim_layer_blend(inst, "aim"));
    }
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let t = anim_time(inst);

    // 一维参数跟着片段播放时间走：0 → 1 单调推过整条轴，越过 1 之后由外插钳制在 run。
    if set_anim_layer_blend_value(inst, "locomotion", t, 0.0) {
        // 前 1.5 秒逐帧打点，够看清 weight 从 idle 一路交到 run
        if time() < 1.5 {
            let line = anim_layer_blend(inst, "locomotion");
            print("t=" + t + " 1D " + line);
        }
    }

    // 二维参数：沿对角线扫过去，验证三角内权重连续、出框后钳制到最近边
    set_anim_layer_blend_value(inst, "aim", t * 0.6 - 1.0, 1.0 - t * 0.4);
    if time() < 1.5 {
        let line2 = anim_layer_blend(inst, "aim");
        print("t=" + t + " 2D " + line2);
    }
}
