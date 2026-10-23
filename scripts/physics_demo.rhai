// 物理演示脚本（Rhai）
// 跑法：cargo run -- assets/scene_physics.json scripts/physics_demo.rhai
//       开关也可用命令行覆盖：--physics off / --gravity 0,-3.0,0 / --ground 0.5
//
// 场景里声明了 physics 的物件会在物理开启时自动绑定，本脚本只负责丢新球。

fn on_start() {
    log("物理后端 = " + physics_backend()
        + " / 开关 = " + physics_enabled()
        + " / 重力 = " + physics_gravity()
        + " / 地面 = " + physics_ground()
        + " / 刚体数 = " + body_count());
}

fn on_update(dt) {
    let t = time();

    // 每 2 秒从空中丢一个球：建节点 -> 立刻绑刚体 -> 给一点横向初速度
    let slot = (t / 2.0).floor();
    let name = "drop_" + slot.to_string();
    if !exists(name) {
        create_node(name, "sphere", rand(-6.0, 6.0), 11.0, rand(-4.0, 4.0));
        set_scale_uniform(name, rand(0.6, 1.2));
        set_color(name, rand(0.35, 1.0), rand(0.35, 1.0), rand(0.35, 1.0));
        if attach_body(name, "dynamic", 1.0) {
            body_impulse(name, rand(-3.0, 3.0), 0.0, rand(-3.0, 3.0));
        }
    }

    // 旧球回收，避免无限堆积（移除节点后后端会自动清理刚体）
    let old = "drop_" + (slot - 8).to_string();
    if exists(old) {
        remove_node(old);
    }

    // 运行时开关示例（需要时取消注释）：
    // if physics_enabled() { physics_set_enabled(false) } else { physics_set_enabled(true) }
    // 运行时切换后端示例：physics_set_backend("null") / physics_set_backend("builtin")
}
