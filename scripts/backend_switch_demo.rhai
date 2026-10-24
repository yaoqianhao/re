// 后端运行时切换演示（Rhai）
//
// 跑法：cargo run --features rapier -- assets/scene_rapier.json scripts/backend_switch_demo.rhai
//
// 演示三件事：
// 1. 同一份脚本、同一个场景，运行中在 rapier 与 builtin 之间来回切；
// 2. 切换会清空刚体并 bump generation，app 下一代自动重绑场景刚体（日志「物理已绑定 N 个场景刚体」会再出现）；
// 3. 脚本只问能力（physics_has_capability），不问后端名，因此两种后端下都能安全跑。

fn on_start() {
    log("起始后端 = " + physics_backend() + " / 能力 = " + physics_capabilities());
}

fn on_update(dt) {
    let t = time();

    // 3s：降到 builtin（只剩「刚体 vs 地面」，堆叠会塌穿）
    if t > 3.0 && t < 3.2 && physics_backend() != "builtin" {
        let ok = physics_set_backend("builtin");
        log("[3s] 切 builtin -> " + ok + " / 刚体数 " + body_count() + " / 能力 " + physics_capabilities());
    }

    // 6s：升回 rapier（堆叠恢复、碰撞事件恢复）
    if t > 6.0 && t < 6.2 && physics_backend() != "rapier" {
        let ok = physics_set_backend("rapier");
        log("[6s] 切 rapier -> " + ok + " / 刚体数 " + body_count() + " / 能力 " + physics_capabilities());
    }

    // 持续丢方块，观察两种后端下的堆叠差异
    let slot = (t / 1.0).floor();
    let name = "cube_" + slot.to_string();
    if !exists(name) {
        create_node(name, "cube", rand(-1.2, 1.2), 7.0, rand(-1.2, 1.2));
        set_scale_uniform(name, 0.9);
        set_color(name, rand(0.5, 1.0), rand(0.5, 1.0), rand(0.5, 1.0));
        attach_body(name, "dynamic", 1.0);
    }
    let old = "cube_" + (slot - 10).to_string();
    if exists(old) {
        remove_node(old);
    }

    // 只有申报了 events 能力的后端才有事件
    if physics_has_capability("events") && (t - (t / 2.0).floor() * 2.0) < dt {
        let events = physics_take_events();
        log("[" + t.to_string() + "s] 碰撞事件 " + events.len().to_string() + " 条");
    }
}
