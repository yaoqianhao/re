// 键盘链路探针：完全走真实键盘输入（key_down），每帧打印玩家位置，供真窗口按键驱动时核对。
// 相机刻意拉高拉远，避免贴坡时视野只剩坡面、看不到赛道全貌。
fn ground_at(x, z, from_y) {
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
    "赛道之外（平地）"
}

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
    set_pos("player", -16.0, 0.9, 6.0);
    camera(-16.0, 14.0, 18.0, -8.0, 1.0, -6.0);
    log("kbd 起点 x=-16.0 z=6.0（等待真实键盘输入）");
    text_add("hud_title", "r3e 楼梯 / 斜坡键盘巡检", 24.0, 18.0, 24.0, 1.0, 0.93, 0.75);
    text_add("hud_keys", "W 前 / S 后 / A 左 / D 右 · Shift 冲刺", 24.0, 52.0, 16.0, 0.78, 0.85, 0.95);
    text_add("hud_l1", "", 24.0, 84.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l2", "", 24.0, 108.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l3", "", 24.0, 132.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_rule", "限值：可跨级高 <= 0.35m · 可攀坡角 <= 45 度", 24.0, 162.0, 15.0, 0.62, 0.70, 0.80);
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
    if ix != 0.0 && iz != 0.0 { ix = ix * 0.70710678; iz = iz * 0.70710678; }

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

    // 侧后方跟随，抬高 5.5m、退后 13m：坡顶也看得到脚下地形
    camera(p[0] + 3.0, fy + 5.5, p[2] + 13.0, p[0], fy + 1.2, p[2] - 5.0);

    let flag = "通行";
    if blocked { flag = "阻挡 <- " + note; }
    text_set("hud_l1", "当前赛道: " + lane_name(p[0]));
    text_set("hud_l2", "脚底 " + fy.to_string() + " m · 位置 x " + p[0].to_string() + " z " + p[2].to_string());
    text_set("hud_l3", "状态: " + flag + " · 前方坡度 " + slope.abs().to_string() + " 度");
    log(`kbd x=${p[0]} y=${fy} z=${p[2]} ${flag}`);
}
