// 局部光源阴影演示（扩展十八 C）：点光立方阴影 + 聚光单面阴影
//
// 用法：
//   cargo run -- assets/scene_local_shadow.json scripts/local_shadow_demo.rhai
//   cargo run -- assets/scene_local_shadow.json scripts/local_shadow_demo.rhai --run-seconds 5
//
// 这个场景刻意把方向光压到 0.35 并**关掉方向光阴影**：画面里所有影子只能来自两盏
// 局部光源，因此「地上有 lamp / beam 的影子」就是 C 项真的跑起来的直接证据；
// 反过来若只有亮度没有影子，说明深度 pass 没提交（看起止探针的「占用层数」）。
//
// 演示内容：
// * `lamp`：点光，绕原点水平绕圈（每帧移动光源 → 六个面的矩阵每帧都在重算）；
// * `beam`：聚光，悬在原点上方俯照，锥内外的影子由单面透视矩阵决定；
// * 每 0.5 秒打印一次「投影意愿 / 本帧实际投影盏数」，两者不一致就是被
//   MAX_LOCAL_SHADOW_CASTERS（4 盏）截断了。
//
// 约定（与其它 demo 一致）：脚本只管「每帧把灯挪到哪 + 开关投影」，
// 选面与矩阵拟合都在 `r3e_core::light`，渲染层不做业务判断。

const RADIUS = 2.4;
const HEIGHT = 3.4;
const SPEED = 0.7;

let angle = 0.0;
let print_acc = 0.0;

fn on_start() {
    // 场景里两盏灯已带 cast_shadow:true，这里再显式打开一次，顺便验证写接口
    light_set_cast_shadow("lamp", true);
    light_set_cast_shadow("beam", true);
    log("局部阴影演示就绪：影子只可能来自 lamp（点光 6 面）与 beam（聚光 1 面）");
    log("灯数 = " + local_light_count() + "，投影意愿 lamp = " + local_light_cast_shadow("lamp"));
    log("本帧实际投影盏数 = " + local_shadow_caster_count());
}

fn on_update(dt) {
    angle += dt * SPEED;

    // 点光绕圈：位置一变，六个面的光源相机与矩阵全部跟着重算
    light_set_pos("lamp", RADIUS * cos(angle), HEIGHT, RADIUS * sin(angle));

    // 聚光轻微摆头：观察单面阴影在锥边缘的稳定性（不应出现硬接缝）
    light_set_dir("beam", 0.25 * sin(angle * 1.3), -1.0, 0.25 * cos(angle * 1.3));

    print_acc += dt;
    if print_acc >= 0.5 {
        print_acc = 0.0;
        let casters = local_shadow_caster_count();
        let wish = local_light_count();
        log("本帧投影 " + casters + " 盏（意愿最多 " + wish + " 盏）");
        log("lamp 半径 " + round(local_light("lamp").range * 100.0) / 100.0);
    }
}
