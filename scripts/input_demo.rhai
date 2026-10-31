// 输入演示脚本（Rhai）：键盘 WASD + 鼠标左键 / 右键 + 手柄按键与摇杆
//
// 用法：cargo run --bin r3e -- --script scripts/input_demo.rhai
// （保存本文件也会触发热重载，引擎会重放场景后重跑脚本）
//
// 约定：Rhai 里 spawn / despawn 是保留字，生成物件请用 create_node / remove_node。

fn on_start() {
    create_node("probe", "cube", 0.0, 0.6, 0.0);
    set_scale_uniform("probe", 0.5);
    set_color("probe", 0.95, 0.85, 0.30);
    log("输入演示就绪：WASD 移动探针 / 左键生成方块 / 右键清空 / 手柄 A 换色 / 左摇杆移动 / 右摇杆旋转");
}

fn on_update(dt) {
    // ---------- 键盘 ----------
    // key_down：持续按住，适合移动；key_pressed：本帧按下边沿，适合开关类交互
    let step = 5.0 * dt;
    let kx = 0.0;
    let kz = 0.0;
    if key_down("a") { kx -= step; }
    if key_down("d") { kx += step; }
    if key_down("w") { kz -= step; }
    if key_down("s") { kz += step; }
    if kx != 0.0 || kz != 0.0 {
        move_by("probe", kx, 0.0, kz);
    }

    // 空格：按下瞬间换色（按住不放不会连续触发，系统自动重复已被引擎过滤）
    if key_pressed("space") {
        set_color("probe", rand(0.3, 1.0), rand(0.3, 1.0), rand(0.3, 1.0));
    }

    // R：清空左键生成的方块；按住列表可直接用于 HUD
    if key_pressed("r") {
        remove_nodes("click_");
    }
    if key_held_count() > 3 {
        log("同时按住的键: " + keys_held().to_string());
    }

    // ---------- 手柄 ----------
    if gamepad_connected() {
        // 左摇杆：模拟量移动（死区已在引擎侧处理，0.0 表示未推杆）
        let lx = pad_axis("left_x");
        let ly = pad_axis("left_y");
        if lx != 0.0 || ly != 0.0 {
            move_by("probe", lx * 4.0 * dt, 0.0, -ly * 4.0 * dt);
        }

        // 右摇杆：旋转
        let rx = pad_axis("right_x");
        let ry = pad_axis("right_y");
        if rx != 0.0 || ry != 0.0 {
            rotate("probe", 0.0, rx * 120.0 * dt, 0.0);
            rotate("probe", ry * 90.0 * dt, 0.0, 0.0);
        }

        // 扳机：RT 抬高 / LT 下沉（引擎侧同时用扳机做相机推拉）
        let lift = pad_axis("rt") - pad_axis("lt");
        if lift > 0.05 || lift < -0.05 {
            move_by("probe", 0.0, lift * 3.0 * dt, 0.0);
        }

        // A 键（按下瞬间）：换颜色；B 键：清空左键生成的方块
        if pad_pressed("a") {
            set_color("probe", rand(0.3, 1.0), rand(0.3, 1.0), rand(0.3, 1.0));
        }
        if pad_pressed("b") {
            remove_nodes("click_");
        }
    }

    // ---------- 鼠标 ----------
    // 左键：在光标对应的地面位置生成一个方块
    if mouse_pressed("left") {
        let wx = (mouse_x() / 1280.0 - 0.5) * 14.0;
        let wz = (mouse_y() / 720.0 - 0.5) * 14.0;
        let name = "click_" + frame().to_string();
        if create_node(name, "cube", wx, 0.35, wz) {
            set_scale_uniform(name, 0.35);
            set_color(name, rand(0.4, 1.0), rand(0.5, 1.0), rand(0.6, 1.0));
        }
    }

    // 右键：清掉所有左键生成的方块
    if mouse_pressed("right") {
        remove_nodes("click_");
    }

    // 滚轮：跟随缩放探针（鼠标滚轮缩放相机仍在引擎侧生效）
    let s = scroll();
    if s != 0.0 {
        rotate("probe", 0.0, s * 8.0, 0.0);
    }
}
