// 地形漫游：WASD 驱动圆柱走 200×200 高度场（平地 → 15° → 50° → 坡顶平台）
//
// 为什么脚本层自己解算运动学：r3e-app 的脚本宿主注入的是 NullCharacterHost，
// `character_*` 只会返回 backend = "none"；脚本能拿到的只有 `physics_raycast`。
// 所以这里沿用 stairs 实验场那套「向下射线贴地 + 台阶/坡度限值」的解法：
// 物理世界给地面高度，脚本判定能不能走。
//
// 与 stairs 实验场的关键差别：**地面不是代理盒，而是高度场本身**。
// C07 把 `terrain` 段铺成了 heightfield collider（`Physics::set_heightfield`），
// 射线打中的就是屏幕上看的那张双线性面 —— 高度场是曲面，射线命中点就是真实坡度，
// 不存在「代理盒比视觉面高半格」的对账问题。
//
// 后端限制：只有真物理后端（rapier）会把高度场建成 collider，射线才打得到地形；
// 内置后端（builtin）只做解析式查询，`physics_raycast` 打高度场会返回空。
// 所以本脚本必须配 `--physics-backend rapier` 跑（tools/drive_roam.py 已写死）。
//
// 键位：W/A/S/D 移动（W = -Z）、Shift 冲刺、按住 C 切高位俯视、
//       R 回出生点、T 直接传送到坡顶平台（对账用）。
//
// 注意：Rhai 函数访问不到顶层 const/let，限值在各函数内以字面量重述。

// 从 (x, z) 向 -Y 打射线取地面高度；没打中返回 -999。
fn ground_at(x, z, from_y) {
    // 起点抬 2.0 m、射线长 12 m：坡上大高差与陡坡上沿都要能打进。
    let hit = physics_raycast(x, from_y + 2.0, z, 0.0, -1.0, 0.0, 12.0);
    if hit.len() == 0 {
        return -999.0;
    }
    // 返回 [node, distance, point.x, point.y, point.z]，地面高度取 point.y
    hit[3]
}

// 16 块里落在哪一块（200m 域以 -100 起、四等分 50m 一块）。
fn block_of(x, z) {
    let bx = ((x + 100.0) / 50.0).floor();
    let bz = ((z + 100.0) / 50.0).floor();
    "块 " + bx.to_string() + "," + bz.to_string()
}

// 一次移动求解：返回 [blocked, new_x, new_y(脚底), new_z, slope_deg, note]
fn walk_step(px, pz, fy, ix, iz, dist) {
    let g = ground_at(px + ix * dist, pz + iz * dist, fy);
    if g < -500.0 {
        return [true, px, fy, pz, 0.0, "前方无地面"];
    }
    let dy = g - fy;
    // 前瞻 0.6 m：高度场是连续的，采样太密会把缓坡读成台阶
    let ahead_y = ground_at(px + ix * 0.6, pz + iz * 0.6, fy);
    let ndy = 0.0;
    if ahead_y > -500.0 { ndy = ahead_y - fy; }
    let slope = (ndy / 0.6).atan().to_degrees();
    // 高度场没有"台阶"，dy 只用来兜住陡坡一步跨太大导致的穿透
    if dy > 0.6 {
        return [true, px, fy, pz, slope, "落差超限 " + dy.to_string()];
    }
    if slope > 45.0 && ndy > 0.05 {
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
    camera(p[0] - 12.0, 14.0, p[2] + 16.0, p[0] + 10.0, 2.0, p[2] - 4.0);
    log("terrain-roam 就绪：WASD 移动 / Shift 冲刺 / R 回出生点 / T 上坡顶 · 物理后端 = " + physics_backend());
    text_add("hud_title", "r3e 地形漫游", 24.0, 20.0, 26.0, 1.0, 0.93, 0.75);
    text_add("hud_keys", "WASD 移动 · Shift 冲刺 · R 回起点 · T 上坡顶", 24.0, 56.0, 16.0, 0.78, 0.85, 0.95);
    text_add("hud_l1", "", 24.0, 88.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l2", "", 24.0, 112.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_l3", "", 24.0, 136.0, 17.0, 0.92, 0.95, 1.0);
    text_add("hud_rule", "地形即碰撞源：贴地高度来自 heightfield 射线，与渲染同一张采样面", 24.0, 168.0, 15.0, 0.62, 0.70, 0.80);
}

fn on_update(dt) {
    // 传送必须发生在**读位置之前**：R/T 的 `set_pos` 写的是新坐标，而本帧末尾的静止贴地
    // 校正会拿帧首缓存的旧坐标再写一次 —— 谁先读 `get_pos` 决定了传送活不活得下来。
    let teleported = false;
    if key_down("r") {
        set_pos("player", -90.0, 0.9, 20.0);
        teleported = true;
    }
    if key_down("t") {
        set_pos("player", 50.0, 29.193 + 0.9, 20.0);
        teleported = true;
    }

    let p = get_pos("player");
    let fy = p[1] - 0.9;

    // 传送当帧立刻按新坐标贴一次地：硬编码的落点高度只在射线打不中时兜底
    // （首帧物理未装配，`ground_at` 返回 -999）。
    if teleported {
        let tg = ground_at(p[0], p[2], fy);
        if tg > -500.0 {
            set_pos("player", p[0], tg + 0.9, p[2]);
        }
    }

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
    // 传送当帧不做位移求解：`walk_step` 的前瞻起点是帧首的 p，刚传送过就作废了
    if (ix != 0.0 || iz != 0.0) && teleported == false {
        let r = walk_step(p[0], p[2], fy, ix, iz, sp * dt);
        blocked = r[0];
        slope = r[4];
        note = r[5];
        if blocked == false {
            set_pos("player", r[1], r[2] + 0.9, r[3]);
        }
    }

    // 第一帧物理还没装配完，射线全落空（-999），此时只记录不写位，避免把角色按进地下
    let g_now = ground_at(p[0], p[2], fy);
    if g_now > -500.0 && blocked == false && ix == 0.0 && iz == 0.0 && teleported == false {
        // 静止时也做一次贴地校正：坡面上按住不动不会缓慢下沉
        // （传送帧跳过：落点高度已在上面的传送分支里按新坐标贴过，再贴一次没有意义）
        set_pos("player", p[0], g_now + 0.9, p[2]);
    }

    if key_down("c") {
        camera(0.0, 120.0, 60.0, 0.0, 20.0, -10.0);
    } else {
        let back = p[0] - 12.0;
        camera(back, fy + 9.0, p[2] + 14.0, p[0] + 6.0, fy + 1.0, p[2] - 2.0);
    }

    let flag = "通行";
    if blocked { flag = "阻挡 <- " + note; }
    text_set("hud_l1", "位置 x=" + p[0].to_string() + " 脚底 y=" + fy.to_string() + " · " + block_of(p[0], p[2]));
    text_set("hud_l2", "前方坡度 " + slope.abs().to_string() + " 度 · 距终点 " + (50.0 - p[0]).to_string() + " m");
    text_set("hud_l3", "状态: " + flag);
}
