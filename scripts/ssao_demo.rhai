// 扩展十七示例：屏幕空间环境光遮蔽（SSAO）。
//
// 演示点：
//   1) 场景 `postfx.ssao` 段开启后，柱子与墙的夹角、台阶接缝、球体贴地的那一圈
//      会明显压暗——这是深度里算出来的接触阴影，不是贴图也不是阴影贴图的接缝；
//   2) 强度在 0.5 ~ 2.4 之间呼吸：强度上去时遮蔽更深，但 AO 下限有 MIN_AO_FACTOR
//      兜着，不会糊成死黑；
//   3) 数字键 1/2/3 切半径 0.35 / 0.85 / 1.6：半径小只在缝隙里出现，半径大整面墙
//      都会发灰——说明核确实是按视空间半径伸缩的；
//   4) 数字键 4/5 切样本 4 / 16：样本少时遮蔽图上会有采样噪点，散焦 4×4 只能压一部分；
//   5) 右键开关遮蔽，肉眼对比「有/无遮蔽」，并看探针日志里 pass 数在 2 与 0 之间跳。
//
// 启动：r3e assets/scene_ssao.json scripts/ssao_demo.rhai
//      r3e assets/scene_ssao.json scripts/ssao_demo.rhai --run-seconds 8   // 无人值守自检

fn on_start() {
    print("扩展十七 遮蔽演示：墙角的接触阴影由深度重建而来");
    print(`右键 = 开关遮蔽；1/2/3 = 半径；4/5 = 样本（当前半径 ${ssao_radius()}，样本 ${ssao_samples()}）`);
}

fn on_update(dt) {
    // 强度呼吸 0.5 ~ 2.4：确认 uniform 里的 intensity 每帧都在生效
    let t = time();
    set_ssao_intensity(1.45 + 0.95 * (t * 0.8).sin());

    if key_pressed("1") { set_ssao_radius(0.35); print("半径 -> 0.35（只有缝隙里发暗）"); }
    if key_pressed("2") { set_ssao_radius(0.85); print("半径 -> 0.85（默认）"); }
    if key_pressed("3") { set_ssao_radius(1.6); print("半径 -> 1.6（整面墙发灰）"); }

    if key_pressed("4") { set_ssao_samples(4); print("样本 -> 4（噪点明显）"); }
    if key_pressed("5") { set_ssao_samples(16); print("样本 -> 16（散焦后干净）"); }

    if mouse_pressed("right") {
        let cur = ssao_enabled();
        set_ssao_enabled(!cur);
        print(`遮蔽切换 -> ${!cur}`);
    }
}
