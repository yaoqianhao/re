// 示例：蜂群 —— 运行时批量生成 / 销毁物件，演示纯脚本驱动的动态场景。
// 用法：cargo run --release -- assets/scene.json scripts/example_swarm.rhai

// 注意：脚本顶层语句会被每帧重复执行，因此常量放在函数里。
fn on_start() {
    let count = 60;
    log("蜂群示例：生成 " + count + " 个物件");
    for i in 0..count {
        let name = "bee_" + i.to_string();
        create_node(name, "cube", rand(-6.0, 6.0), rand(0.3, 3.0), rand(-6.0, 6.0));
        set_scale_uniform(name, 0.18);
        set_color(name, rand(0.5, 1.0), rand(0.3, 0.9), 0.35);
    }
}

fn on_update(dt) {
    let t = time();
    let count = 60;
    for i in 0..count {
        let name = "bee_" + i.to_string();
        if !exists(name) { continue; }
        let phase = i * 0.7;
        let r = 2.5 + (i % 7) * 0.55;
        let x = (t * 0.6 + phase).cos() * r;
        let z = (t * 0.8 + phase).sin() * r;
        let y = 0.6 + (t * 2.0 + phase).sin() * 0.5;
        set_pos(name, x, y, z);
        rotate(name, 0.0, 120.0 * dt, 45.0 * dt);
    }
}
