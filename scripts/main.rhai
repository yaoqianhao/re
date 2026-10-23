// r3e 演示脚本（Rhai）
// 保存本文件即可热重载：引擎会重放场景并重新执行本脚本。

fn on_start() {
    log("脚本已启动，场景物件数 = " + count());
    // 物理后端当前是预留的空实现，这里提前挂上刚体，换后端后立刻参与模拟
    attach_body("orbiter", "dynamic", 1.5);
    log("物理后端 = " + physics_backend() + "，刚体数 = " + body_count());
}

fn on_update(dt) {
    let t = time();

    // 1) 立方体自转 + 上下浮动
    rotate("spinner", 0.0, 70.0 * dt, 0.0);
    set_pos("spinner", 0.0, 1.2 + (t * 1.6).sin() * 0.35, 0.0);

    // 2) 球体绕立方体公转，远近交替时缩放
    let radius = 3.0;
    let x = (t * 0.9).cos() * radius;
    let z = (t * 0.9).sin() * radius;
    set_pos("orbiter", x, 1.5 + (t * 2.4).sin() * 0.55, z);
    set_scale_uniform("orbiter", 0.8 + (t * 3.0).sin() * 0.18);

    // 3) 顶部落下的方块随时间变色
    let hue = ((t * 0.7).sin() + 1.0) * 0.5;
    set_color("sky_cube", 0.30 + hue * 0.55, 0.85 - hue * 0.35, 0.55 + hue * 0.3);

    // 4) 每 6 秒生成一个临时物件，存活 4 秒后消失
    let slot = (t / 6.0).floor();
    let name = "temp_" + slot.to_string();
    if !exists(name) {
        create_node(name, "cube", rand(-6.0, 6.0), 0.35, rand(-6.0, 6.0));
        set_scale_uniform(name, 0.35);
        set_color(name, rand(0.4, 1.0), rand(0.4, 1.0), rand(0.4, 1.0));
    }
    let age = t - slot * 6.0;
    if age > 4.0 {
        remove_node(name);
    }

    // 5) 相机极缓慢环绕（不干扰手动操作时可注释掉）
    // orbit(6.0 * dt * 60.0, 0.0);
}
