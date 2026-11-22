// 无人值守巡检夹具（验收用，不是给玩家玩的）：把圆柱传送到指定赛道起点，
// 恒定向 -Z 前进，每帧走同一个 walk_step 求解，用 HUD + stdout 记录高度与阻挡原因。
// 与 walk.rhai 共用同一套「射线贴地 + 台阶 0.35m / 坡角 45 度」判定，只是把键盘输入
// 换成常量输入，用来在无人工操作时取得可复现的对照证据。

fn ground_at(x, z, from_y) {
    let hit = physics_raycast(x, from_y + 1.4, z, 0.0, -1.0, 0.0, 8.0);
    if hit.len() == 0 {
        return -999.0;
    }
    // 返回数组顺序是 [node, distance, point.x, point.y, point.z]，地面高度取 point.y
    hit[3]
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
    camera(-16.0, 8.0, 10.0, -16.0, 1.0, -4.0);
    text_add("hud_t", "A: 楼梯 级高 0.10m（可通行）", 24.0, 20.0, 22.0, 1.0, 0.93, 0.75);
    text_add("hud_l1", "", 24.0, 56.0, 16.0, 0.92, 0.95, 1.0);
    text_add("hud_l2", "", 24.0, 80.0, 16.0, 0.92, 0.95, 1.0);
    log("auto 起点 x=-16.0 z=6.0");
}

fn on_update(dt) {
    let p = get_pos("player");
    let fy = p[1] - 0.9;
    let r = walk_step(p[0], p[2], fy, 0.0, -1.0, 4.0 * dt);
    if r[0] == false {
        set_pos("player", r[1], r[2] + 0.9, r[3]);
    }
    camera(p[0], p[1] + 3.5, p[2] + 7.0, p[0], p[1] - 0.5, p[2] - 3.0);
    let st = "通行";
    if r[0] { st = "阻挡<-" + r[5]; }
    text_set("hud_l1", "z = " + p[2].to_string() + "   脚底 = " + fy.to_string());
    text_set("hud_l2", "状态: " + st);
    log("z=" + p[2].to_string() + " y=" + fy.to_string() + " " + st);
}
