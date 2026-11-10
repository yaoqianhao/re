// 扩展三十五取证脚本：制造「多段脏区」，看暂存环把 N 次 write_buffer 收成 1 次。
// 40 个 cube 按记录序排列，每帧只动下标相差 10 的 4 个（间隔 10 > MERGE_GAP 8），
// 于是 plan_upload 会切出 4 段独立脏区。
// 用 frame() 而不是 time() 驱动，保证两次运行的动画相位完全一致，截图可逐位比对。
// 用法：r3e.exe assets/scene.json scripts\staging_demo.rhai --run-seconds 12

fn on_start() {
    let n = 40;
    for i in 0..n {
        let name = "slot_" + i.to_string();
        create_node(name, "cube", (i % 10) * 1.2 - 5.4, 0.6, -((i / 10) * 1.2) - 3.0);
        set_scale_uniform(name, 0.3);
        set_color(name, 0.6, 0.7, 0.35);
    }
    log("取证场景：生成 " + n + " 个 cube");
}

fn on_update(dt) {
    let f = frame() % 20;
    // 只动 4 个相距 10 的实例 → 4 段脏区
    for k in 0..4 {
        let name = "slot_" + (k * 10).to_string();
        if !exists(name) { continue; }
        let y = 0.6 + ((f + k * 5) % 20) * 0.02;
        set_pos(name, (k * 10 % 10) * 1.2 - 5.4, y, -((k * 10 / 10) * 1.2) - 3.0);
    }
}
