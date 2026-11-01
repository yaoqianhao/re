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

    // 4) 九宫格（9-slice）图片：把 32x32 的小图拉到 220x120 也不会糊 —— 四角保持原尺寸
    //    （8px 不拉伸），四边单向拉伸、中心双向拉伸。参数顺序是 左 / 上 / 右 / 下，
    //    单位同样支持 "24px" 与 "10%"（百分比按元素自身宽 / 高换算）。
    //    这里只是演示切分效果，真实项目应换成专门画的九宫格美术（圆角边框等）。
    //    ui_set_slice("key") 不带参数 = 关闭九宫格。
    ui_image("slice_frame", "topleft", "24px", "176px", "220px", "120px", "assets/ui/icon.png", 0.85, 0.74, 0.42, 0.95);
    ui_set_slice("slice_frame", "8px", "8px", "8px", "8px");
    ui_set_layer("slice_frame", 2);

    log("UI 节点数 = " + ui_count());

    // ---- 音频（M1 非空间化音频：常规音效 + 背景音乐，没有 3D 衰减 / 多普勒）----
    // 第一个参数是自己起的声音名字（key），不是文件名：
    // 同名 key 再次 play = 停掉旧实例、从头重播，不会叠加成两层；想同时放两份就起两个 key。
    // 仓库自带两个素材（合成正弦波形，非第三方版权素材）：
    //   assets/audio/blip.wav      0.18s 短音效，880Hz 指数衰减
    //   assets/audio/ambience.wav  2.0s  可无缝循环，220Hz+330Hz 叠加
    audio_set_master_volume(0.8);
    let amb_ok = audio_play_ex("ambience", "assets/audio/ambience.wav", 0.35, true);
    log("音频后端 = " + audio_backend()
        + " / 设备可用 = " + audio_available().to_string()
        + " / 环境音启动 = " + amb_ok.to_string()
        + " / 在播实例 = " + audio_count().to_string());
    text_add("audio_hud", "", 24.0, 120.0, 16.0, 0.55, 0.78, 0.62);
    text_set_wrap("audio_hud", 900.0);

    // ---- PBR 材质（直接光照 Cook-Torrance）----
    // 两个旋钮都是 0..1：
    //   metallic   0 = 绝缘体（塑料 / 石头），1 = 纯金属（漫反射归零，只剩染色高光）
    //   roughness  0 = 镜面，1 = 完全漫反射；内部下限夹到 0.045 防 GGX 除零
    // 写入前由 core 的 SurfaceMaterial::sanitized 收敛（NaN / ±inf / 超界都夹回），
    // 脚本写脏值也不会污染场景。三件套 = GGX 法线分布 + Smith 遮蔽 + Schlick 菲涅尔。
    set_material("spinner", 1.0, 0.18);    // 抛光金属：高光锐利、没有漫反射
    set_material("orbiter", 0.0, 0.85);    // 粗糙绝缘体：哑光塑料
    set_material("sky_cube", 0.0, 0.35);   // 半光滑塑料：有明确高光带

    // 自发光只加不减：不乘 n·l、不吃阴影、不参与光照计算。
    // 数值可以大于 1（线性 HDR，上限 16），用来做灯管 / 宝石这类开口。
    create_node("lamp", "cube", 0.0, 3.4, -2.8);
    set_scale_uniform("lamp", 0.22);
    set_emissive("lamp", 0.25, 1.4, 2.2);

    // 读回的是「收敛之后真正生效」的材质快照
    let mat = material_of("spinner");
    log("spinner 材质 metallic=" + mat.metallic.to_string()
        + " roughness=" + mat.roughness.to_string());

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

    // ---- 音频演示（按键触发）----
    // J 敲一下音效：连按只是重启同一个 key 的实例，不会叠成十几层
    if key_pressed("j") {
        audio_play("blip", "assets/audio/blip.wav");
    }
    // K 暂停 / 从暂停处继续环境音；没在播就重新拉起来
    if key_pressed("k") {
        if audio_is_paused("ambience") {
            audio_resume("ambience");
        } else if !audio_pause("ambience") {
            audio_play_ex("ambience", "assets/audio/ambience.wav", 0.35, true);
        }
    }
    // L 停止 / 重新开始环境音
    if key_pressed("l") {
        if audio_is_playing("ambience") {
            audio_stop("ambience");
        } else {
            audio_play_ex("ambience", "assets/audio/ambience.wav", 0.35, true);
        }
    }
    // M 总音量在 0.8 / 0.2 之间切换（乘到每条声音的音量上）
    if key_pressed("m") {
        let v = audio_master_volume();
        if v > 0.5 {
            audio_set_master_volume(0.2);
        } else {
            audio_set_master_volume(0.8);
        }
    }
    text_set("audio_hud", audio_status() + " | J 音效 / K 暂停续 / L 停起 / M 总音量");

    // 5) 相机极缓慢环绕（不干扰手动操作时可注释掉）
    // orbit(6.0 * dt * 60.0, 0.0);
}
