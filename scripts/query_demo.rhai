// 物理查询层演示（Rhai）：raycast / overlap / shape-cast
//
// 跑法：cargo run --features rapier -- assets/scene.json scripts/query_demo.rhai --physics on --run-seconds 4
//   内置后端也能跑（解析法查 AABB）；想对比精确窄相就把 --physics-backend 换成 rapier。
//
// 演示四件事：
// 1. physics_raycast_full 用**具名 Map** 打一条垂直向下的射线（IDE 能补全 hit/node/distance...）；
// 2. physics_overlap 用球体圈一片区域，拿回范围内所有节点；
// 3. physics_shape_cast_sphere / _box 做扫掠：回答「这个球/盒往前推多远会撞到谁」；
// 4. 脚本只按能力位（raycast / overlap / shape_cast）分支，两种后端下代码完全一致。

// 把查询结果 Map 拼成一行日志；未命中时字段依旧齐备（hit=false、node=-1），不用先判长度。
fn fmt_hit(tag, r) {
    if !r.hit {
        return "[" + tag + "] 未命中";
    }
    let s = "[" + tag + "] 节点 " + r.node.to_string();
    s += " / 距离 " + r.distance.to_string();
    s += " / 形状中心 (" + r.point[0].to_string() + ", " + r.point[1].to_string();
    s += ", " + r.point[2].to_string() + ")";
    s += " / 法线 (" + r.normal[0].to_string() + ", " + r.normal[1].to_string();
    s += ", " + r.normal[2].to_string() + ")";
    if r.inside { s += " / 起点已重叠"; }
    s
}

fn on_start() {
    log("后端 = " + physics_backend() + " / 能力 = " + physics_capabilities());
    log("能射线 = " + physics_has_capability("raycast")
        + " / 能重叠 = " + physics_has_capability("overlap")
        + " / 能扫掠 = " + physics_has_capability("shape_cast"));

    // 查询对象全用静态刚体：演示的是「查得到」，不受各后端模拟能力差异影响
    create_node("ground_box", "box", 0.0, 1.0, 0.0);
    set_scale("ground_box", 4.0, 0.5, 4.0);
    attach_body("ground_box", "static", 1.0);

    create_node("center_cube", "cube", 0.0, 2.0, 0.0);
    set_scale_uniform("center_cube", 0.8);
    attach_body("center_cube", "static", 1.0);
}

fn on_update(dt) {
    let t = time();

    // 每 60 帧（约 1s）投一次射线：从 y=8 朝下打，最远 20 米（具名 Map 版）
    if physics_has_capability("raycast") && frame() % 60 == 0 {
        let hit = physics_raycast_full(0.0, 8.0, 0.0, 0.0, -1.0, 0.0, 20.0);
        log("[" + t.to_string() + "s] " + fmt_hit("raycast", hit));
    }

    // 每 120 帧（约 2s）圈一次：球心固定在场景中间，半径 3
    if physics_has_capability("overlap") && frame() % 120 == 0 {
        let inside = physics_overlap(0.0, 2.0, 0.0, 3.0);
        log("[" + t.to_string() + "s] 半径 3 内共 " + inside.len().to_string() + " 个物体");
    }

    // 每 120 帧扫掠一次：把半径 0.4 的球从 y=6 往下推 20 米
    // 解析解：顶面 y=2.4 → 球心推 3.2 处接触（6 - 2.4 - 0.4）
    if physics_has_capability("shape_cast") && frame() % 120 == 30 {
        let r = physics_shape_cast_sphere(0.0, 6.0, 0.0, 0.0, -1.0, 0.0, 0.4, 20.0);
        log("[" + t.to_string() + "s] " + fmt_hit("cast-sphere", r));
    }

    // 半尺寸 0.5 的盒从 y=6 往下推：顶面 y=2.4 → 盒心推 3.1 处接触
    if physics_has_capability("shape_cast") && frame() % 120 == 60 {
        let r = physics_shape_cast_box(0.0, 6.0, 0.0, 0.0, -1.0, 0.0, 0.5, 0.5, 0.5, 20.0);
        log("[" + t.to_string() + "s] " + fmt_hit("cast-box", r));
    }
}
