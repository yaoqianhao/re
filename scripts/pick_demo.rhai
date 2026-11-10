// 3D 屏幕射线拾取演示（扩展三十四）：屏幕坐标 → 物件
//
// 用法：
//   cargo run -- scripts/pick_demo.rhai
//   cargo run -- scripts/pick_demo.rhai --run-seconds 3
//
// 三条新 API（都不要求物件挂刚体，只要求它有包围盒）：
//   pick_at(x, y)         屏幕像素 → 最近命中（Map）
//   pick_under_mouse()    同上，坐标取当前光标（省两次 mouse_x/y）
//   pick_all_at(x, y)     同一射线上所有命中，按距离升序（Array<Map>）
//
// 返回字段：hit / node / name / distance / point[x,y,z]。
// 未命中时 hit=false 且其余字段是空模板（node=-1、name=""、point=[]），
// 所以 `if r.hit { ... }` 永远不会因为缺键报错。
//
// 与物理 raycast 的区别：
//   raycast 走物理后端，只有刚体才可能命中；pick_* 走渲染几何，装饰物也能点。
//   二者字段刻意不共用，避免"点中空气却被当作命中刚体"。

const HI = 0.95;   // 悬停高亮色
let hovered = "";
let picks = 0;
let probed = false;

fn on_start() {
    // 三件摆件：左 cube / 中 sphere / 右 cylinder，都在相机默认视角里
    create_node("pick_a", "cube", -1.8, 0.6, 0.0);
    create_node("pick_b", "sphere", 0.0, 0.9, 0.0);
    create_node("pick_c", "cylinder", 1.8, 0.6, 0.0);
    set_scale_uniform("pick_a", 0.9);
    set_scale_uniform("pick_b", 0.9);
    set_scale_uniform("pick_c", 0.9);
    set_color("pick_a", 0.30, 0.55, 0.90);
    set_color("pick_b", 0.35, 0.85, 0.55);
    set_color("pick_c", 0.90, 0.60, 0.30);

    log("拾取演示就绪：光标压到物件上会变亮，左键点击把它弹起来，P 键打印穿透列表");
}

fn on_update(dt) {
    // ---- 0) 开局自检（只需一次，且必须等画布有尺寸）----
    // 注意：`on_start` 阶段窗口还没上报尺寸，`screen_size()` 是 (0, 0)，
    // 此时 `pick_at` 的射线退化、必然点空。所以自检放在首个"画布已就绪"的帧。
    if !probed && screen_width() > 0.0 {
        probed = true;
        let s = screen_size();
        let c = pick_at(s[0] * 0.5, s[1] * 0.5);
        let head = "画布 " + s[0] + "x" + s[1];
        if c.hit {
            let tail = "，中心自检命中 " + c.name + "（距离 " + c.distance + "）";
            log(head + tail);
        } else {
            log(head + "，中心自检未命中（物件没压在视线正中间）");
        }
    }

    // ---- 1) 悬停：每帧按当前光标重算，离开自然回落 ----
    let h = pick_under_mouse();
    let now = if h.hit { h.name } else { "" };
    if now != hovered {
        // 旧目标还原
        if hovered == "pick_a" { set_color("pick_a", 0.30, 0.55, 0.90); }
        if hovered == "pick_b" { set_color("pick_b", 0.35, 0.85, 0.55); }
        if hovered == "pick_c" { set_color("pick_c", 0.90, 0.60, 0.30); }
        // 新目标高亮
        if now != "" {
            set_color(now, HI, HI, HI);
            log("悬停 " + now + "（距离 " + h.distance + "）");
        }
        hovered = now;
    }

    // ---- 2) 点击：左键按下瞬间对被点中物件做一次位移 ----
    //     （用 mouse_pressed 而不是 mouse_down：按住不放不该每帧弹一次）
    if mouse_pressed("left") && h.hit {
        picks += 1;
        move_by(h.name, 0.0, 0.35, 0.0);
        let p = h.point;
        let at = "，命中点 (" + p[0] + ", " + p[1] + ", " + p[2] + ")";
        log("第 " + picks + " 次点中 " + h.name + at);
    }

    // ---- 3) 穿透：一条射线上所有命中，按距离升序 ----
    if key_pressed("p") {
        let all = pick_all_at(mouse_x(), mouse_y());
        let names = [];
        for it in all {
            names.push(it.name + "@" + it.distance);
        }
        log("光标下 " + all.len() + " 个物件：" + names.to_string());
    }
}

fn on_stop() {
    remove_nodes("pick_");
    log(format("拾取演示结束：共点中 {} 次", picks));
}
