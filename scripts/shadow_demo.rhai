// 阶段 D 示例：方向光阴影贴图（shadow mapping）+ 光源绕场景扫动。
//
// 演示点：
//   1) 场景 `light.shadow` 段开启后，地面能同时看到方块 / 球 / 柱子的投影；
//   2) 光源方向每帧变化，影子跟着光走 —— 证明阴影贴图是逐帧重渲的，
//      不是启动时烤一次的静态贴图；
//   3) set_shadow_strength 在 ~0.29 ~ 0.95 之间呼吸，验强度衰减链路；
//   4) 右键切换 set_shadow(false)，可肉眼对比「整帧跳过阴影通道」的效果。
//
// 启动：r3e assets/scene_shadow.json scripts/shadow_demo.rhai
//      r3e assets/scene_shadow.json scripts/shadow_demo.rhai --run-seconds 8   // 无人值守自检

fn on_start() {
    print("阶段 D 阴影演示：光源绕场景扫动，影子逐帧重渲");
    print("右键 = 开关阴影（对比有无影子的差别）");
}

fn on_update(dt) {
    // 太阳绕 Y 轴旋转：约 24 秒一圈，四个方向的长影子都能看到
    let t = time();
    let ang = t * 0.26;
    let dx = -0.95;
    let dz = -0.65;
    let c = ang.cos();
    let s = ang.sin();
    set_light(dx * c - dz * s, -0.62, dx * s + dz * c);

    // 强度呼吸 0.29 ~ 0.95：确认 uniform 里的 strength 真在生效
    set_shadow_strength(0.62 + 0.33 * (t * 0.9).cos());

    if mouse_pressed("right") {
        let cur = shadow_enabled();
        set_shadow(!cur);
        print(`阴影切换 -> ${!cur}`);
    }
}
