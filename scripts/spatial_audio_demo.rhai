// 3D 空间化音频演示（§12 第十项）
//
// 用法：
//   cargo run -- assets/scene_spatial_audio.json scripts/spatial_audio_demo.rhai
//   cargo run -- assets/scene_spatial_audio.json scripts/spatial_audio_demo.rhai --run-seconds 8
//
// 这个场景里放了两个声源，形状都是听得出来的：
//   horn（橙球，绕半径 6 的圆高速巡游）→ 绑定节点，位置/速度全由 app 每帧从
//        节点世界变换灌进来，inverse 距离模型。朝相机冲来时 rate > 1（音调升高），
//        掉头远离时 rate < 1；
//   bell（蓝柱，静止在 z = -6）  → 手动坐标 + linear 距离模型，越走近越响。
//
// 听者不用脚本管：`app` 每帧把**活动相机**的 position/forward 灌进覆盖层，
// 所以用 WASD/QE 飞、或鼠标转头，听到的远近关系都跟着画面走。想验证这一点，
// 就飞近红色方块（z = -9，离 bell 更近）再飞回绿色方块（z = 3，离 horn 的轨道更近）。
//
// 判读方法（每 0.5 秒打一行）：
//   gain 是**叠加在平面音量之上的**距离衰减系数（1.0 = 没衰减，越小越远）；
//   rate 是多普勒下发的播放倍速（1.0 = 与听者相对静止）；
//   两者都只影响已 `audio_spatial_attach*` 的 key，其余声音逐位不变。

const RADIUS = 6.0;
const SPEED = 1.6; // rad/s → 线速度约 9.6 m/s，多普勒听得出来
const REPORT_INTERVAL = 0.5;

let angle = 0.0;
let report_acc = 0.0;

fn on_start() {
    log("音频后端 = " + audio_backend()
        + " / 可用 = " + audio_available().to_string()
        + " / 空间化已启用 = " + audio_spatial_enabled().to_string());

    // 先把声音起起来：这两行与历史版本完全一样，都是平面声。
    audio_play_ex("horn", "assets/audio/blip.wav", 0.9, true);
    audio_play_ex("bell", "assets/audio/ambience.wav", 0.4, true);

    // 再叠加空间化覆盖层：注册顺序无所谓，下一次 tick 就会结算。
    // horn 走 inverse（min/d，远处退得快），多普勒强度 1.0；
    audio_spatial_attach_ex("horn", 6.0, 30.0, "inverse", 1.0);
    audio_spatial_attach_node("horn", "horn", 6.0, 30.0);
    // bell 静止，坐标直接给；linear 模型便于听出「距离线性变响」。
    audio_spatial_attach_ex("bell", 3.0, 30.0, "linear", 0.0);
    audio_spatial_set_position("bell", 0.0, 1.6, -6.0);

    log("空间化声源 = " + audio_spatial_count().to_string()
        + " / keys = " + audio_spatial_keys().to_string());
    log("判读：gain 为距离衰减系数（1.0 无衰减），rate 为多普勒播放倍速（1.0 无相对运动）");
}

fn on_update(dt) {
    angle += dt * SPEED;

    // 只动节点，不动声源：位置/速度由 app 从节点世界矩阵推导，
    // 这样脚本不需要（也不应该）去维护一份和渲染不同步的坐标。
    set_pos("horn", RADIUS * cos(angle), 1.2, RADIUS * sin(angle));

    report_acc += dt;
    if report_acc >= REPORT_INTERVAL {
        report_acc = 0.0;
        // Rhai 的语句在换行处就结束，长串拼接不能像 Rust 那样折行（会报
        // 「表达式过于复杂」），所以这里用 += 一句一段地攒。
        let hp = audio_spatial_position("horn");
        let lp = audio_spatial_listener();
        let line = "listener=[";
        line += lp[0].to_string();
        line += ", " + lp[2].to_string() + "] horn=[";
        line += hp[0].to_string() + ", " + hp[2].to_string() + "]";
        line += " | horn gain=" + audio_spatial_gain("horn").to_string();
        line += " rate=" + audio_spatial_rate("horn").to_string();
        line += " | bell dist=" + audio_spatial_distance("bell").to_string();
        line += " gain=" + audio_spatial_gain("bell").to_string();
        log(line);
        log("  " + audio_spatial_status());
    }
}
