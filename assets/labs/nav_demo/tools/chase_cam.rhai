// nav_demo 追踪镜头（窗口实时预览版）
//
// 用法（宿主第二个位置参数就是脚本路径）：
//   target\x86_64-pc-windows-gnu\debug\r3e.exe assets\labs\nav_demo\nav_demo.r3e.json assets\labs\nav_demo\tools\chase_cam.rhai
//
// 语义：每帧读取 agent 节点位置，把相机贴到「agent 后上方 +Z」并看向 agent，
// 得到第三人称追踪镜头——画面里 agent 位置固定、世界在动。
// 只读节点位置、只写相机，不碰任何场景几何，可与 nav.* RPC 同时使用。
//
// 说明：Rhai 的函数访问不到顶层 let/const，做跨帧平滑需要额外状态载体，
// 这里保持「无状态硬跟随」——宿主每帧都重设相机，与 agent 位置严格一致，
// 不存在漂移。需要平滑（含朝向自适应）的离线出片版本见 nav_demo_chase.py。
//
// 偏移量在函数内以字面量重述（同上原因）。

fn on_update(dt) {
    // agent 是 nav_demo.r3e.json 里的圆柱体，get_pos 返回 [x, y, z]
    let p = get_pos("agent");

    // 相机偏移：正 Z 方向 22m（场景原俯视也是从 +Z 侧看）、高 13m
    let back = 22.0;
    let height = 13.0;

    // 看向 agent 脚底往上 1.2m（比节点原点抬一点，构图更稳）
    camera(p[0], p[1] + height, p[2] + back, p[0], p[1] + 1.2, p[2]);

    // 追踪视角视野略宽于俯视图的 46 度
    fov(50.0);
}
