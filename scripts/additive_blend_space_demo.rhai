// 扩展五十示例：加法层与混合空间的组合语义（份额叠的是增量，不是姿态）。
//
// 加法层叠的是「片段当前姿态 − 该片段首帧」这份增量。一层里挂多路片段、由混合空间
// 按参数分配份额时，份额的正确含义是「各路增量各占几成」：
//     0.5 × 左倾增量 + 0.5 × 右倾增量 = 抵消
// 而扩展四十九之前这条路径沿用的是覆盖层的「姿态加权平均」——先把各路姿态平均，
// 再减去平均参考姿态。位置项因为线性恰好等价，旋转（四元数混合）与缩放（比值混合）
// 会被平均钝化：增量恒为零的那一路，会把别人的增量稀释成混轴姿态。
//
// 这个示例把 idle / run / atk 三路挂在同一个加法层，用 2D 空间 (turn, amp) 驱动权重：
//   阶段一 (0,0)  权重塌到 idle 单路 —— 单路口零算术，与四十九之前逐位一致
//   阶段二 (1,0)  idle / run 各半   —— 两份增量按半份相加
//   阶段三 (1,1)  三路混合          —— 份额相加仍为 1，增量线性复合
//   阶段四 (0,2)  塌到 atk 顶点     —— 权重单路，验证不会残留别路份额
// 启动：r3e assets/scene_anim.json scripts/additive_blend_space_demo.rhai --run-seconds 4 --show-bones
//
// 日志每帧的 `blend=` 行给出维度 / 样本数 / 参数 / 三路份额，配合 `--show-bones`
// 的骨骼输出即可对比「加法语义」与「姿态平均」在旋转上的差别。

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
    print(`加法层 + 混合空间示例：实例 ${inst}，片段 ${clips}`);
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    let atk = pick(clips, "atk");
    if idle == "" || run == "" || atk == "" {
        print("缺少 idle / run / atk 片段，示例跳过");
        return;
    }

    // 同一层挂三路，直接声明成加法层
    add_anim_layer_additive(inst, "add", idle, 1.0);
    add_anim_layer_clip(inst, "add", run, 0.0);
    add_anim_layer_clip(inst, "add", atk, 0.0);
    print("加法层信息：" + anim_layer_info(inst, "add"));

    // 2D 空间 (turn, amp)：三个样本铺成直角三角形
    set_anim_layer_blend_2d(inst, "add", "move.x", "amp");
    add_anim_layer_blend_sample(inst, "add", idle, 0.0, 0.0);
    add_anim_layer_blend_sample(inst, "add", run, 2.0, 0.0);
    add_anim_layer_blend_sample(inst, "add", atk, 0.0, 2.0);
    print("2D 空间已挂：" + anim_layer_blend(inst, "add"));
}

fn on_update(dt) {
    let inst = anims()[0];
    if inst == "" {
        return;
    }
    let et = time();
    if et < 1.0 {
        // 单路口：权重应塌成 idle = 1，其余归零
        set_anim_layer_blend_value(inst, "add", 0.0, 0.0);
    } else if et < 2.0 {
        // idle / run 各半
        set_anim_layer_blend_value(inst, "add", 1.0, 0.0);
    } else if et < 3.0 {
        // 三路混合
        set_anim_layer_blend_value(inst, "add", 1.0, 1.0);
    } else {
        // 塌到 atk 顶点
        set_anim_layer_blend_value(inst, "add", 0.0, 2.0);
    }

    if et < 3.5 {
        print("et=" + et + " " + anim_layer_blend(inst, "add"));
    }
}
