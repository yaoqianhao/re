// 扩展四十九示例：3D 混合空间（四面体剖分）驱动动画层权重。
//
// 四十五把「八向移动 × 三档速度」压成两层 2D 叠：方向一层、速度一层，层与层之间靠
// 层权重再混一次。三维空间把 24 个样本摆进同一张图（x = 左右、y = 前后、z = 速度档），
// 参数落在哪个四面体里就按四个顶点插值，样本点上权重精确为 1 —— 不会再出现
// 「上层权重 0.5 把下层两档混成半速」这种叠加误差。
//
// 这个示例挂一层：
//   move —— 3D（move.x, move.y, speed）：idle 在原点、run 沿 +x、atk 沿 +y、
//           run 再占 +z 顶点，四个不共面样本撑起一个四面体。
// 参数由片段播放时间推进：先在 x 轴上扫（t 0→1），再钻进四面体内部（0.3,0.3,0.3），
// 最后踩到顶点（0,0,1）确认权重塌成单路。
// 启动：r3e assets/scene_anim.json scripts/blend_space_3d_demo.rhai --run-seconds 3 --show-bones
//
// 日志里每帧的 `blend=` 行就是探针入口：维度、样本数、四面体数、被拒次数、参数、权重。

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
    print(`三维混合空间示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    let atk = pick(clips, "atk");
    if idle == "" || run == "" || atk == "" {
        print("缺少 idle / run / atk 片段，示例跳过");
        return;
    }

    add_anim_layer(inst, "move", idle, 1.0);
    add_anim_layer_clip(inst, "move", run, 0.0);
    add_anim_layer_clip(inst, "move", atk, 0.0);
    set_anim_layer_blend_3d(inst, "move", "move.x", "move.y", "speed");
    // 四个不共面样本 → tet=1
    add_anim_layer_blend_sample_3d(inst, "move", idle, 0.0, 0.0, 0.0);
    add_anim_layer_blend_sample_3d(inst, "move", run, 1.0, 0.0, 0.0);
    add_anim_layer_blend_sample_3d(inst, "move", atk, 0.0, 1.0, 0.0);
    add_anim_layer_blend_sample_3d(inst, "move", run, 0.0, 0.0, 1.0);
    // 故意加一个同位置的样本：引擎应当拒绝，drop 计数 +1
    let dup = add_anim_layer_blend_sample_3d(inst, "move", idle, 0.0, 0.0, 0.0);
    print("重复样本被拒: " + (!dup));
    print("3D 空间已挂：" + anim_layer_blend(inst, "move"));
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let t = anim_time(inst);
    // 阶段按墙钟时间切（片段播放时间只用来在阶段一里推进参数）
    let et = time();

    // 阶段一（et < 1.5）：沿 x 轴扫 idle → run
    // 阶段二（1.5 ≤ et < 2.5）：钻进四面体内部 (0.3, 0.3, 0.3)，几路各分一份
    // 阶段三（et ≥ 2.5）：踩 +z 顶点，权重塌成 run=1，与顶点样本逐位一致
    if et < 1.5 {
        set_anim_layer_blend_value_3d(inst, "move", t, 0.0, 0.0);
    } else if et < 2.5 {
        set_anim_layer_blend_value_3d(inst, "move", 0.3, 0.3, 0.3);
    } else {
        set_anim_layer_blend_value_3d(inst, "move", 0.0, 0.0, 1.0);
    }

    if et < 3.2 {
        print("et=" + et + " t=" + t + " 3D " + anim_layer_blend(inst, "move"));
    }
}
