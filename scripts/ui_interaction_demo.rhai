// UI 指针交互演示（扩展十四）：命中测试 + hover / click 事件到脚本的闭环
//
// 用法：
//   cargo run -- scripts/ui_interaction_demo.rhai
//   cargo run -- assets/scene.json scripts/ui_interaction_demo.rhai --run-seconds 3
//
// 交互约定：
// * 两个按钮都在屏幕底部居中排开，左侧「开始」、右侧「重置」；
// * 指针悬停时按钮变亮（ui_hovered 读的是 app 每帧推进的 hover 状态）；
// * 点「开始」把 spinner 染成青色并发一条 log，点「重置」还原；
// * 指针压在 UI 上时相机不吃这次拖动——这条在 app 层判定，脚本无需关心。
//
// 事件语义（core::ui::UiInteraction）：
//   hovered / unhovered 进出行、pressed 按下、clicked 同元素内按下并抬起、
//   cancelled 按下后没在同一元素上抬起、released 有按下捕获时抬起总会收到。

const BTN_W = "280px";
const BTN_H = "72px";
const BTN_Y = "-72px";

let clicks = 0;

fn paint(key, over, r, g, b) {
    if over {
        ui_set_color(key, r + 0.12, g + 0.12, b + 0.12, 1.0);
    } else {
        ui_set_color(key, r, g, b, 1.0);
    }
}

fn on_start() {
    // 底部左侧：开始
    ui_rect("btn_start", "bottom_center", "-160px", BTN_Y, BTN_W, BTN_H, 0.18, 0.42, 0.85, 1.0);
    ui_border("btn_start_frame", "bottom_center", "-160px", BTN_Y, BTN_W, BTN_H, 3.0, 0.75, 0.88, 1.0, 1.0);
    // 底部右侧：重置
    ui_rect("btn_reset", "bottom_center", "160px", BTN_Y, BTN_W, BTN_H, 0.78, 0.32, 0.26, 1.0);
    ui_border("btn_reset_frame", "bottom_center", "160px", BTN_Y, BTN_W, BTN_H, 3.0, 1.0, 0.85, 0.80, 1.0);
    // 左上角状态面板
    ui_rect("panel", "top_left", "24px", "24px", "420px", "132px", 0.05, 0.06, 0.09, 0.70);

    log("UI 交互演示就绪：悬停按钮会变亮，点「开始」给 spinner 上色，点「重置」还原");
    log("UI 元素数 = " + ui_count() + "，容量上限 = " + ui_capacity());
}

fn on_update(dt) {
    // ---- 悬停高亮：每帧按当前 hover 重算，指针离开自然回落 ----
    paint("btn_start", ui_hovered("btn_start"), 0.18, 0.42, 0.85);
    paint("btn_reset", ui_hovered("btn_reset"), 0.78, 0.32, 0.26);

    // ---- 点击：同一元素上按下并抬起 ----
    if ui_clicked("btn_start") {
        clicks += 1;
        set_color("spinner", 0.20, 0.85, 0.80);
        log(format("点击「开始」第 {} 次：spinner → 青色", clicks));
    }
    if ui_clicked("btn_reset") {
        set_color("spinner", 0.85, 0.85, 0.88);
        log("点击「重置」：spinner 还原");
    }

    // ---- 事件级读取：需要区分 pressed / cancelled 时直接遍历事件数组 ----
    for ev in ui_events() {
        if ev.kind == "pressed" {
            log(format("按下 {}", ev.key));
        }
        if ev.kind == "cancelled" {
            log(format("按下后在 {} 之外抬起，视为取消", ev.key));
        }
    }

    // ---- 命中查询：把按钮中心反查一次，确认脚本侧与渲染侧用的是同一套矩形 ----
    // （只在第一次点击后打印，避免刷屏）
    if clicks == 1 {
        let hit = ui_hit(160.0, 480.0);
        if hit != "" {
            log(format("坐标 (160,480) 命中最上层元素：{}", hit));
        }
    }
}
