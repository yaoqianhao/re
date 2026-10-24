// 阶段 C 示例：最小动画状态机 + cross-fade。
//
// 三个状态：idle_loop（默认）/ run_loop（按住鼠标左键）/ atk（点击左键，单次播放）。
// 切换一律走 `set_anim_clip_fade(实例, 片段, 时长)`，过渡期间旧动作淡出、新动作淡入；
// 攻击播完（非循环自动暂停）后按当前输入回到 idle 或 run。
//
// 启动：r3e assets/scene_anim.json scripts/anim_state.rhai --show-bones
//
// 注意：Rhai 函数看不到脚本顶层变量，状态机不存全局变量，而是「每帧从场景里读当前
// 片段 + 播放状态 + 输入」重新判定，因此热重载脚本不会残留半截状态。

// 在片段名列表里按关键字找片段（片段名如 idle_loop / run_loop / atk）。
fn pick(clips, key) {
    for c in clips {
        if c.contains(key) {
            return c;
        }
    }
    ""
}

fn on_start() {
    print(`动画实例数: ${anim_count()}，可切换片段: ${anim_clips("tirger")}`);
    print("操作：按住左键跑，点击左键攻击，松开回待机");
}

fn on_update(dt) {
    let inst = anims()[0];
    let clips = anim_clips("tirger");
    if clips.len == 0 {
        return;
    }
    let idle = pick(clips, "idle");
    let run = pick(clips, "run");
    let atk = pick(clips, "atk");

    let cur = anim_clip(inst);
    let attacking = cur.contains("atk");
    let moving = mouse_down("left");

    // 1) 攻击优先级最高：单次播放，0.12s 极短过渡（手感要紧）
    if mouse_pressed("left") && atk != "" {
        set_anim_looping(inst, false);
        if set_anim_clip_fade(inst, atk, 0.12) {
            print(`→ atk（fade 0.12s）`);
        }
        return;
    }

    if attacking {
        // 非循环片段播完会自动暂停，据此判定「攻击结束」
        if !anim_playing(inst) {
            let next = if moving { run } else { idle };
            set_anim_looping(inst, true);
            set_anim_clip_fade(inst, next, 0.2);
            print(`atk → ${next}（fade 0.2s）`);
        }
        return;
    }

    // 2) 移动状态机：按住跑、松开待机，0.25s 过渡
    let want = if moving { run } else { idle };
    if want != "" && cur != want {
        set_anim_looping(inst, true);
        if set_anim_clip_fade(inst, want, 0.25) {
            print(`${cur} → ${want}（fade 0.25s）`);
        }
    }
}

// 过渡进度可随时查询：anim_fade(实例) 返回新动作权重，anim_fade_from(实例) 返回旧动作名，
// 例如做 HUD 或调试输出时可以展示 `idle_loop→run_loop 60%`。
