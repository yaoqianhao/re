// P3 节点动画脚本示例：列出实例与可切换片段、按时间轮播动作、查询状态。
//
// 动画实例名形如 `<资产名>/<片段名>`（如 `tirger/idle_loop`），资产名取 FBX 文件名（去扩展名）。
// 同一骨架的其它动作不要重新 spawn，用 `set_anim_clip(实例名, "片段名")` 原地切换。
// 先跑一次 `r3e --dump-anim <fbx>` 可看到该资产内所有片段名。
//
// 启动：r3e assets/scene_anim.json scripts/anim_demo.rhai --show-bones

// 注意：Rhai 的函数看不到脚本顶层变量，状态一律靠「每帧查询场景 + 重算」表达，
// 这样热重载脚本也不会残留上一份脚本的变量。
fn on_start() {
    print(`动画实例数: ${anim_count()}`);
    for name in anims() {
        print(`  ${name} -> ${anim_info(name)}`);
    }
    print(`tirger 可切换片段: ${anim_clips("tirger")}`);
}

fn on_update(dt) {
    let inst = anims()[0];
    let clips = anim_clips("tirger");
    if clips.len == 0 {
        return;
    }
    // 每 3 秒换一个动作，检验切换片段不残留旧姿态
    let idx = (time() / 3.0).to_int() % clips.len;
    let want = clips[idx];
    if anim_clip(inst) != want && set_anim_clip(inst, want) {
        print(`切到 tirger/${want}：${anim_info(inst)}`);
    }
}
// hot-reload probe 104256
