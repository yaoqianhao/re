// 楼梯 / 斜坡走廊巡检：键盘控制圆柱体（脚本层运动学，不依赖 character_* 控制器）
//
// 为什么不用 character_step：r3e-app 的运行时脚本宿主注入的是 NullCharacterHost
// （dep 规则里 r3e-runtime 也不允许依赖 r3e-character），脚本里调 character_* 只会
// 拿到 backend = "none"。本脚本因此自带一套「向下射线贴地 + 台阶/坡度限值」的
// 运动学解算：物理世界负责给出地面高度（physics_raycast），脚本负责判定能不能走。
//
// 口径（与场景一致）：圆柱高 1.8 m，节点原点在几何中心；可跨级高 <= 0.35 m，
// 可攀坡角 <= 45 度；前瞻 0.55 m 采样地面高差反推坡度。
//
// 注意：Rhai 的函数访问不到脚本顶层的 const / let（实测报 Variable not found），
// 因此下面的限值在各函数内以字面量重述，改参数时三处一起改。
//
// 键位：W/A/S/D 移动（W = -Z）、Shift 冲刺、按住 C 看全景。

// 从 (x, z) 向 -Y 打射线取地面高度；没打中返回 -999（用 < -500 判「没有地面」）。
fn ground_at(x, z, from_y) {
    // 起点抬 1.4 m、射线长 8 m：保证站在 0.5 m 台阶下也能打到上方台面。
    let hit = physics_raycast(x, from_y + 1.4, z, 0.0, -1.0, 0.0, 8.0);
    if hit.len() == 0 {
        return -999.0;
    }
    // 返回数组顺序是 [node, distance, point.x, point.y, point.z]，地面高度取 point.y
    hit[3]
}

fn lane_name(x) {
    let idx = ((x + 16.0) / 4.0).round();
    if idx <= 0 { return "楼梯 12 级 x 0.10m"; }
    if idx == 1 { return "楼梯 10 级 x 0.18m"; }
    if idx == 2 { return "楼梯 8 级 x 0.25m"; }
    if idx == 3 { return "楼梯 6 级 x 0.32m"; }
    if idx == 4 { return "楼梯 4 级 x 0.50m (超限)"; }
    if idx == 5 { return "斜坡 12 度"; }
    if idx == 6 { return "斜坡 25 度"; }
    if idx == 7 { return "斜坡 40 度"; }
    if idx == 8 { return "斜坡 60 度 (超限)"; }
    "赛道之外"
}

// 一次移动求解：返回 [blocked, new_x, new_y(脚底), new_z, slope_deg, note]
fn walk_step(px, pz, fy, ix, iz, dist) {
    let g = ground_at(px + ix * dist, pz + iz * dist, fy);
    if g < -500.0 {
        return [true, px, fy, pz, 0.0, "前方无地面"];
    }
    let dy = g - fy;
    let ahead_y = ground_at(px + ix * 0.55, pz + iz * 0.55, fy);
    let ndy = 0.0;
    if ahead_y > -500.0 { ndy = ahead_y - fy; }
    let slope = (ndy / 0.55).atan().to_degrees();
    if dy > 0.35 {
        return [true, px, fy, pz, slope, "台阶超限 " + dy.to_string()];
    }
    if slope > 45.0 && ndy > 0.05 {
        return [true, px, fy, pz, slope, "坡度超限 " + slope.to_string()];
    }
    let note = "平地";
    if ndy > 0.02 { note = "上坡 / 上台阶"; }
    if ndy < -0.02 { note = "下坡 / 下台阶"; }
    [false, px + ix * dist, g, pz + iz * dist, slope, note]
}

fn on_start() {
    let p = get_pos("player");
    let g = ground_at(p[0], p[2], 0.0);
    if g > -500.0 {
        set_pos("player", p[0], g + 0.9, p[2]);
    }
    camera(0.0, 20.0, 32.0, 0.0, 2.0, -6.0);
    log("stairs-lab 就绪：WASD 移动 / Shift 冲刺 / 按住 C 看全景；物理后端 = " + physics_backend());

    text_add("hud_title", "r3e 楼梯 / 斜坡巡检", 24.0, 20.0, 26.0, 1.0, 0.93, 0.75);
    text_add("hud_keys", "WASD 移动 · Shift 冲刺 · 按住 C 看全景", 24.0, 56.0, 16.0, 0.78, 0.85, 0.95);
    text_add("hud_l1", "", 24.0, 88.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l2", "", 24.0, 112.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l3", "", 24.0, 136.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_rule", "限值：可跨级高 <= 0.35m · 可攀坡角 <= 45 度", 24.0, 168.0, 15.0, 0.62, 0.70, 0.80);
}

fn on_update(dt) {
    let p = get_pos("player");
    let fy = p[1] - 0.9;

    let ix = 0.0;
    let iz = 0.0;
    if key_down("a") { ix -= 1.0; }
    if key_down("d") { ix += 1.0; }
    if key_down("w") { iz -= 1.0; }
    if key_down("s") { iz += 1.0; }
    if ix != 0.0 && iz != 0.0 {
        ix = ix * 0.70710678;
        iz = iz * 0.70710678;
    }

    let sp = 4.2;
    if key_down("shift") { sp = 8.0; }

    let blocked = false;
    let note = "-";
    let slope = 0.0;
    if ix != 0.0 || iz != 0.0 {
        let r = walk_step(p[0], p[2], fy, ix, iz, sp * dt);
        blocked = r[0];
        slope = r[4];
        note = r[5];
        if blocked == false {
            set_pos("player", r[1], r[2] + 0.9, r[3]);
        }
    }

    if key_down("c") {
        camera(0.0, 20.0, 32.0, 0.0, 2.0, -6.0);
    } else {
        camera(p[0], fy + 5.0, p[2] + 8.0, p[0], fy + 1.0, p[2] - 2.0);
    }

    let flag = "通行";
    if blocked { flag = "阻挡 <- " + note; }
    text_set("hud_l1", "当前赛道: " + lane_name(p[0]));
    text_set("hud_l2", "脚底高度 " + fy.to_string() + " m · 前方坡度 " + slope.abs().to_string() + " 度");
    text_set("hud_l3", "状态: " + flag);
}
