// 地形漫游（自动巡检版）：无需键盘，按运行时刻自动跑完「平地 → 15° → 撞 50° 被挡 → 坡顶平台」。
//
// 为什么要有自动版：真窗口键盘演示（roam.rhai）依赖 OS 注入按键，窗口抢不到前台时
// 脚本一动不动，证据就没法复用。自动版把「谁按键」这一层去掉，只留「地形能不能走」，
// 于是同一条命令在任何机器上都跑出同一条轨迹 —— C07 的肉眼可见样例走这条。
// 键盘版仍然保留给人手动玩。
//
// 与 roam.rhai 的差别只有驱动来源：`time()` 给阶段，其余（贴地射线、台阶/坡度限值、
// 地形即碰撞源）完全同源同参数，两版读数可直接对账。
//
// 阶段（秒）：0–8 冲刺向 +X（坡度上限 35°，挡在 35° 陡坎处）
//           8–24 步行（坡度上限 55°，能走上实测峰值 50° 的坡）直到坡顶 x=50
//           24 做一次保底校验（没走到就传送并记日志）· 25 起拉高位俯视，看整张 200×200 地形
//
// 每帧 log 一行坐标与状态：引擎日志对相同行去重，所以留下的就是**轨迹拐点**。
//
// 坡度门限是分段的：冲刺 35°、步行 55°。同一个 50° 坡，冲刺上不去、走路能上，
// 才证明限值真的在生效，而不是被一个宽松常数一把抹平。

fn ground_at(x, z, from_y) {
    // 起点抬 2.0 m、射线 12 m：50° 坡沿的高差与平台边缘都要打得到
    let hit = physics_raycast(x, from_y + 2.0, z, 0.0, -1.0, 0.0, 12.0);
    if hit.len() == 0 {
        return -999.0;
    }
    hit[3]
}

fn block_of(x, z) {
    let bx = ((x + 100.0) / 50.0).floor();
    let bz = ((z + 100.0) / 50.0).floor();
    "块 " + bx.to_string() + "," + bz.to_string()
}

// 一次移动求解：返回 [blocked, new_x, new_y(脚底), new_z, slope_deg, note]
// `max_slope` 是这一段的坡度上限（度）：冲刺给宽的、步行给窄的，
// 「同一个坡，冲刺上不去、走路能上」才是坡度门限真的在起作用。
fn walk_step(px, pz, fy, ix, iz, dist, max_slope) {
    let g = ground_at(px + ix * dist, pz + iz * dist, fy);
    if g < -500.0 {
        return [true, px, fy, pz, 0.0, "前方无地面"];
    }
    let dy = g - fy;
    let ahead_y = ground_at(px + ix * 0.6, pz + iz * 0.6, fy);
    let ndy = 0.0;
    if ahead_y > -500.0 { ndy = ahead_y - fy; }
    let slope = (ndy / 0.6).atan().to_degrees();
    if dy > 0.6 {
        return [true, px, fy, pz, slope, "落差超限 " + dy.to_string()];
    }
    if slope > max_slope && ndy > 0.05 {
        return [true, px, fy, pz, slope, "坡度超限 " + slope.to_string()];
    }
    let note = "平地";
    if ndy > 0.02 { note = "上坡"; }
    if ndy < -0.02 { note = "下坡"; }
    [false, px + ix * dist, g, pz + iz * dist, slope, note]
}

fn on_start() {
    let p = get_pos("player");
    let g = ground_at(p[0], p[2], 0.0);
    if g > -500.0 {
        set_pos("player", p[0], g + 0.9, p[2]);
    }
    camera(p[0] - 10.0, 12.0, p[2] + 14.0, p[0] + 10.0, 2.0, p[2] - 2.0);
    log("auto-roam 起始：x=" + p[0].to_string() + " 后端 = " + physics_backend());
    text_add("hud_title", "r3e 地形漫游（自动巡检）", 24.0, 20.0, 26.0, 1.0, 0.93, 0.75);
    text_add("hud_keys", "自动：冲刺 8s（限 35°）→ 步行（限 55°）上坡 → 25s 俯视", 24.0, 56.0, 16.0, 0.78, 0.85, 0.95);
    text_add("hud_l1", "", 24.0, 88.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l2", "", 24.0, 112.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l3", "", 24.0, 136.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_rule", "地形即碰撞源：贴地高度来自 heightfield 射线，与渲染同一张采样面", 24.0, 168.0, 15.0, 0.62, 0.70, 0.80);
}

fn on_update(dt) {
    let t = time();

    // 坡顶保底：一次性事件，靠 time 窗口而不是「只做一次」的标记位（Rhai 函数间没有全局变量）
    // 走得上就如实报「无需传送」——传送只兜底，不掩盖坡度判定本身。
    if t >= 24.0 && t < 24.2 {
        let q = get_pos("player");
        if q[0] < 47.0 {
            set_pos("player", 50.0, 29.193 + 0.9, 20.0);
            log("auto-roam 24s 仍停在 x=" + q[0].to_string() + "，保底传送到坡顶");
        } else {
            log("auto-roam 24s 已步行抵达 x=" + q[0].to_string() + "，无需传送");
        }
    }

    let p = get_pos("player");
    let fy = p[1] - 0.9;

    let phase = "冲刺（限 35°）";
    let sp = 8.0;
    let max_slope = 35.0;
    if t >= 8.0 { phase = "步行（限 55°）"; sp = 6.5; max_slope = 55.0; }
    if t >= 24.0 { phase = "坡顶平台"; sp = 0.0; }

    let blocked = false;
    let note = "-";
    let slope = 0.0;
    if sp > 0.0 {
        let r = walk_step(p[0], p[2], fy, 1.0, 0.0, sp * dt, max_slope);
        blocked = r[0];
        slope = r[4];
        note = r[5];
        if blocked == false {
            set_pos("player", r[1], r[2] + 0.9, r[3]);
        }
    } else if t >= 25.0 {
        camera(0.0, 120.0, 60.0, 0.0, 20.0, -10.0);
    }

    // 静止贴地校正：站在坡面上不动时不会缓慢下沉
    let g_now = ground_at(p[0], p[2], fy);
    if g_now > -500.0 && blocked == false && sp == 0.0 {
        set_pos("player", p[0], g_now + 0.9, p[2]);
    }

    if sp > 0.0 {
        camera(p[0] - 12.0, fy + 9.0, p[2] + 14.0, p[0] + 6.0, fy + 1.0, p[2] - 2.0);
    }

    let flag = "通行";
    if blocked { flag = "阻挡 <- " + note; }
    text_set("hud_l1", "位置 x=" + p[0].to_string() + " 脚底 y=" + fy.to_string() + " · " + block_of(p[0], p[2]));
    text_set("hud_l2", "阶段 " + phase + " · 前方坡度 " + slope.abs().to_string() + " 度");
    text_set("hud_l3", "状态: " + flag);
    log("auto x=" + p[0].to_string() + " y=" + fy.to_string() + " z=" + p[2].to_string() + " " + flag + " | " + phase);
}
