// Rapier 后端演示脚本（Rhai）
//
// 跑法（必须先带 feature 编译，否则 rapier 不可选）：
//   cargo run --features rapier -- assets/scene_rapier.json scripts/rapier_demo.rhai
//
// 本脚本刻意不写任何「后端是 rapier 就怎样」的分支：只按能力位问「你能做什么」，
// 于是同一份脚本在 builtin / rapier / 将来任何后端上都能跑，只是可做的事多少不同。

fn on_start() {
    log("物理后端 = " + physics_backend()
        + " / 能力 = " + physics_capabilities()
        + " / 重力 = " + physics_gravity()
        + " / 刚体数 = " + body_count());
}

fn on_update(dt) {
    let t = time();
    let slot = (t / 1.5).floor();

    // 每 1.5 秒往平台上方丢一个方块：堆叠只有真有互撞的后端才立得住
    let name = "cube_" + slot.to_string();
    if !exists(name) {
        create_node(name, "cube", rand(-1.6, 1.6), 7.5, rand(-1.6, 1.6));
        set_scale_uniform(name, rand(0.7, 1.1));
        set_color(name, rand(0.45, 1.0), rand(0.45, 1.0), rand(0.45, 1.0));
        if attach_body(name, "dynamic", 1.0) {
            body_impulse(name, rand(-0.4, 0.4), 0.0, rand(-0.4, 0.4));
        }
    }

    // 只留最近 12 个，旧方块回收（节点移除后后端自动清刚体）
    let old = "cube_" + (slot - 12).to_string();
    if exists(old) {
        remove_node(old);
    }

    // 碰撞事件：只有申报了 events 能力的后端才有货；其它后端返回空数组
    if physics_has_capability("events") && (t - (t / 3.0).floor() * 3.0) < dt {
        let events = physics_take_events();
        let line = "最近碰撞事件 " + events.len().to_string() + " 条";
        if events.len() > 0 {
            line += "，例如 " + events[0];
        }
        log("[" + t.to_string() + "s] " + line);
    }

    // 运行时切换示例（需要时取消注释）：
    // if physics_has_capability("body_body") {
    //     physics_set_backend("builtin");   // 降级：会清空刚体，物件只与地面相交
    //     physics_set_backend("rapier");    // 升回真实物理
    // }
}
