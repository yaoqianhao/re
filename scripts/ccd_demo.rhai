// CCD 穿透对照演示（Rhai）
//
// 跑法（必须先带 feature 编译，否则 rapier 不可选）：
//   cargo run --features rapier -- assets/scene_ccd.json scripts/ccd_demo.rhai
//
// 场景里两颗球完全一样，只有 ccd 一位不同，同时以 120 m/s 射向同一面 4 cm 厚的运动学薄墙：
//   bullet_ccd（蓝）    → 停在墙前（回弹一点）
//   bullet_no_ccd（黄） → 整帧跨过墙，头也不回
// 60 FPS 下每帧走 2 m，墙厚 0.04 m，离散检测必然漏判 —— 这就是 CCD 存在的理由。
// 墙刻意用 kinematic 而非 static：rapier 对固定碰撞体本来就做自动扫掠，观测不到差异。
//
// 想现场看对比：运行时调
//   physics_set_ccd(false)                 → 全体关掉（现存刚体一起刷）
//   body_set_ccd("bullet_no_ccd", true)    → 只救这一颗

let launched = false;
let reported = false;

fn on_start() {
    log("后端 = " + physics_backend()
        + " / 能力 = " + physics_capabilities()
        + " / CCD 默认 = " + physics_ccd_default().to_string());
    if !physics_has_capability("ccd") {
        log("当前后端不支持 CCD，跳过穿透对照");
    }
}

fn on_update(dt) {
    let t = time();

    // 刚体由 app 层在渲染器就绪后统一绑定，on_start 时 body_count() 还是 0，
    // 所以发射动作放到第一帧「刚体已就位」时做。
    if !launched && body_count() > 0 {
        launched = true;
        log("刚体已绑定 " + body_count().to_string() + " 个；"
            + "bullet_ccd ccd=" + body_ccd("bullet_ccd").to_string()
            + " / bullet_no_ccd ccd=" + body_ccd("bullet_no_ccd").to_string());
        // 质量 1.0，冲量数值即速度（m/s）
        body_impulse("bullet_ccd", 120.0, 0.0, 0.0);
        body_impulse("bullet_no_ccd", 120.0, 0.0, 0.0);
    }

    // 每 0.5 秒报一次 x：> 0 表示已经跑到墙的另一侧
    if launched && (t - (t / 0.5).floor() * 0.5) < dt {
        let a = get_pos("bullet_ccd");
        let b = get_pos("bullet_no_ccd");
        log("[" + t.to_string() + "s] ccd球 x=" + a[0].to_string()
            + " / 无ccd球 x=" + b[0].to_string());
    }

    // 结论只报一次，且用「是否越过墙体中面」判，不看绝对值（回弹会让它飘很远）
    if launched && !reported && t > 0.6 {
        reported = true;
        let a = get_pos("bullet_ccd");
        let b = get_pos("bullet_no_ccd");
        log("结论：ccd球 " + (if a[0] > 0.5 { "穿墙" } else { "被墙挡住" })
            + " / 无ccd球 " + (if b[0] > 0.5 { "穿墙" } else { "被墙挡住" }));
    }
}
