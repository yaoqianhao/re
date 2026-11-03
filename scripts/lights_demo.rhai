// 局部光源演示（扩展十五）：点光绕圈 + 聚光俯照，观察衰减、光锥与开关
//
// 用法：
//   cargo run -- scripts/lights_demo.rhai
//   cargo run -- assets/scene.json scripts/lights_demo.rhai --run-seconds 5
//
// 演示内容：
// * `torch`：点光，绕原点水平绕圈，暖白色，半径随呼吸微调；
// * `beam` ：聚光，悬在原点上方俯照，冷白色，内外锥固定；
// * 每 0.5 秒打印一次原点的照度——数值来自 `local_light_irradiance`，
//   与 GPU 里的衰减/锥形判定是同一套公式，可用来对照窗口里的实际亮度。
//
// 约定（与其它 demo 一致）：脚本只管「每帧把灯挪到哪」，命中与收敛在
// `r3e_core::light`；写错 key 时 `light_set_*` 返回 false，不会悄悄新建一盏灯。

const RADIUS = 1.8;
const HEIGHT = 1.2;
const SPEED = 1.1;

let angle = 0.0;
let print_acc = 0.0;

fn on_start() {
    // 点光：位置 + 颜色 + 强度 + 半径
    light_add_point("torch", RADIUS, HEIGHT, 0.0, 1.0, 0.72, 0.34, 6.0, 6.0);
    // 聚光：位置 + 朝向 + 颜色 + 强度 + 半径 + 内角 + 外角（度）——共 13 个参数
    light_add_spot("beam", 0.0, 4.0, 0.0, 0.0, -1.0, 0.0, 0.55, 0.78, 1.0, 14.0, 8.0, 12.0, 26.0);

    log("局部光源演示就绪：暖色点光绕圈，冷色聚光俯照");
    log("灯数 = " + local_light_count() + "，总开关 = " + lights_enabled());
    log("灯位 = " + local_light_keys());
}

fn on_update(dt) {
    angle += dt * SPEED;

    // ---- 每帧挪灯：绕原点画圆，半径呼吸 ----
    light_set_pos("torch", RADIUS * cos(angle), HEIGHT, RADIUS * sin(angle));
    light_set_range("torch", 5.5 + 0.8 * sin(angle * 2.0));

    // ---- 周期性读数：原点照度 / 灯数 / 单盏字段 ----
    print_acc += dt;
    if print_acc >= 0.5 {
        print_acc = 0.0;
        // 一行里塞太多拼接会撞上 rhai 的「表达式复杂度」上限，先算好再分两行打印
        let irr = round(local_light_irradiance(0.0, 0.0, 0.0) * 1000.0) / 1000.0;
        let rng = round(local_light("torch").range * 100.0) / 100.0;
        log("原点照度 " + irr + "，torch 强度 " + local_light_intensity("torch"));
        log("torch 半径 " + rng + "，beam 类型 " + local_light_kind("beam"));
    }
}
