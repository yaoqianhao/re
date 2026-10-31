// r3e 演示脚本（Rhai）
// 保存本文件即可热重载：引擎会重放场景并重新执行本脚本。

fn on_start() {
    log("脚本已启动，场景物件数 = " + count());
    // 物理开启时才真正创建刚体（关闭状态下 attach_body 返回 false）。
    // 开启后 orbiter 改由物理驱动，脚本不再每帧强设位置。
    if physics_enabled() {
        attach_body("orbiter", "dynamic", 1.5);
        body_impulse("orbiter", 2.2, 0.0, 0.0);
    }
    // ---- 2D 屏幕空间文本（M1 文字渲染）----
    // 屏幕坐标单位是像素，原点在左上角；(x, y) 是文本框第一行行顶的锚点。
    text_add("hud_title", "r3e 引擎 · 屏幕空间文字", 24.0, 20.0, 30.0, 1.0, 0.92, 0.72);
    text_add("hud_sub", "中文 / English / 数字 0123456789 都能画", 24.0, 60.0, 18.0, 0.78, 0.85, 0.95);
    text_set_wrap("hud_sub", 520.0);        // 超过 520px 自动折行
    text_add("hud_status", "", 24.0, 96.0, 16.0, 0.62, 0.70, 0.80);
    // 右对齐版本：x 是右边界
    text_add("hud_right", "对齐：右\n第二行", 1240.0, 20.0, 16.0, 0.55, 0.62, 0.72);
    text_set_align("hud_right", "right");
    // 居中版本：x 是水平中心
    text_add("hud_center", "居中对齐", 640.0, 20.0, 16.0, 0.55, 0.62, 0.72);
    text_set_align("hud_center", "center");
    log("文本节点数 = " + text_count());

    // ---- 2D 屏幕空间 UI（M1 最小 UI）----
    // 尺寸 / 偏移写成带单位字符串："24px" 是像素，"50%" 是屏幕宽高的百分比。
    // 锚点：topleft / topcenter / topright / midleft / center / midright /
    //       bottomleft / bottomcenter / bottomright
    // 所有元素用 key 定位，同名重复提交只覆盖，不会堆积。

    // 1) 左上角血条：背景 + 当前值两层矩形（都锚在左上角，靠偏移错开）
    ui_rect("hp_bg", "topleft", "24px", "132px", "260px", "22px", 0.04, 0.05, 0.07, 0.78);
    ui_rect("hp_fill", "topleft", "27px", "135px", "180px", "16px", 0.88, 0.20, 0.24, 0.95);
    text_add("hp_text", "HP 100 / 100", 30.0, 135.0, 14.0, 1.0, 0.96, 0.96);
    ui_set_layer("hp_bg", 0);
    ui_set_layer("hp_fill", 1);

    // 2) 底部对话框底框：半透明矩形 + 边框（宽度按屏幕百分比，换分辨率不错位）
    ui_rect("dlg_bg", "bottomcenter", "0px", "-56px", "72%", "132px", 0.03, 0.04, 0.06, 0.62);
    ui_border("dlg_frame", "bottomcenter", "0px", "-56px", "72%", "132px", 2.0, 0.85, 0.74, 0.42, 0.9);
    ui_set_layer("dlg_bg", 0);
    ui_set_layer("dlg_frame", 2);
    // 文本仍是绝对像素坐标（text_* 不支持百分比）；x 给屏幕水平中心、配合居中对齐。
    text_add("dlg_text", "（对话框）这里的台词由 text_* 画在 UI 之上。", 640.0, 556.0, 18.0, 0.94, 0.95, 0.98);
    text_set_align("dlg_text", "center");

    // 3) 右上角小图标：从 png 加载（找不到文件时渲染层回退成纯色块，不会崩）
    ui_image("hud_icon", "topright", "-40px", "24px", "48px", "48px", "assets/ui/icon.png", 1.0, 1.0, 1.0, 1.0);
    ui_border("hud_icon_ring", "topright", "-42px", "22px", "52px", "52px", 1.0, 0.85, 0.74, 0.42, 0.8);
    ui_set_layer("hud_icon", 3);

    log("UI 节点数 = " + ui_count());

    text_add("audio_hud", "", 24.0, 120.0, 16.0, 0.55, 0.78, 0.62);
    text_set_wrap("audio_hud", 900.0);
    log("物理后端 = " + physics_backend()
        + " / 开关 = " + physics_enabled()
        + " / 刚体数 = " + body_count());
}

fn on_update(dt) {
    let t = time();

    // 1) 立方体自转 + 上下浮动
    rotate("spinner", 0.0, 70.0 * dt, 0.0);
    set_pos("spinner", 0.0, 1.2 + (t * 1.6).sin() * 0.35, 0.0);

    // 2) 球体绕立方体公转，远近交替时缩放
    //    物理开启时改为物理驱动（重力 + 落地反弹），避免脚本每帧抢位置
    if !physics_enabled() {
        let radius = 3.0;
        let x = (t * 0.9).cos() * radius;
        let z = (t * 0.9).sin() * radius;
        set_pos("orbiter", x, 1.5 + (t * 2.4).sin() * 0.55, z);
        set_scale_uniform("orbiter", 0.8 + (t * 3.0).sin() * 0.18);
    }

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

    // ---- UI HUD 每帧刷新（同名覆盖不新增节点；百分比尺寸在 resize 后自动重算）----
    let hp = 0.35 + 0.6 * ((t * 0.6).sin() * 0.5 + 0.5);      // 0.35 ~ 0.95
    let fill_w = (180.0 * hp).round();
    ui_set_size("hp_fill", fill_w.to_string() + "px", "16px");
    ui_set_alpha("hp_fill", 0.55 + hp * 0.4);
    text_set("hp_text", "HP " + (hp * 100.0).round().to_string() + " / 100");
    // 对话框台词跟着鼠标/时间走，验证文本与 UI 的层次（文本默认在 UI 之上）
    text_set("dlg_text", "（对话框）帧号 " + frame().to_string()
        + " · 文本默认绘制在 UI 之上，可用 set_ui_above_text(true) 反转");

    // ---- 文本 HUD 每帧刷新（同名 text_set 只改内容，不会新增节点）----
    text_set("hud_status",
        "帧率 " + ((fps() * 10.0).round() / 10.0).to_string()
        + " / 帧号 " + frame().to_string()
        + " / 时间 " + (t * 1.0).to_string()
        + " / 物件 " + count().to_string()
        + " / 文本 " + text_count().to_string());

    // 5) 相机极缓慢环绕（不干扰手动操作时可注释掉）
    // orbit(6.0 * dt * 60.0, 0.0);
}
