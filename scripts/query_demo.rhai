// 物理查询层演示（Rhai）：raycast / overlap
//
// 跑法：cargo run --features rapier -- assets/scene.json scripts/query_demo.rhai
//   内置后端也能跑（解析法查 AABB）；想对比精确窄相就换 rapier。
//
// 演示三件事：
// 1. physics_raycast 打一条从上方垂直向下的射线，拿回「最近的」物体与命中点；
// 2. physics_overlap 用球体圈一片区域，拿回范围内所有节点；
// 3. 脚本只按能力位（raycast / overlap）分支，两种后端下代码完全一致。

fn on_start() {
    log("后端 = " + physics_backend() + " / 能力 = " + physics_capabilities());

    // 查询对象全用静态刚体：演示的是「查得到」，不受各后端模拟能力差异影响
    create_node("ground_box", "box", 0.0, 1.0, 0.0);
    set_scale("ground_box", 4.0, 0.5, 4.0);
    attach_body("ground_box", "static", 1.0);

    create_node("center_cube", "cube", 0.0, 2.0, 0.0);
    set_scale_uniform("center_cube", 0.8);
    attach_body("center_cube", "static", 1.0);

    // 只有申报了查询能力的后端才值得查，否则直接跳过
    log("能射线查询 = " + physics_has_capability("raycast"));
    log("能重叠查询 = " + physics_has_capability("overlap"));
}

fn on_update(dt) {
    let t = time();
    // 每 60 帧（约 1s）投一次射线：从 y=8 朝下打，最远 20 米
    if physics_has_capability("raycast") && frame() % 60 == 0 {
        let hit = physics_raycast(0.0, 8.0, 0.0, 0.0, -1.0, 0.0, 20.0);
        if hit.len() == 0 {
            log("[" + t.to_string() + "s] 射线未命中");
        } else {
            let head = "[" + t.to_string() + "s] 命中节点 " + hit[0].to_string();
            head += " / 距离 " + hit[1].to_string();
            head += " / 命中点 (" + hit[2].to_string();
            head += ", " + hit[3].to_string();
            head += ", " + hit[4].to_string() + ")";
            log(head);
        }
    }

    // 每 120 帧（约 2s）圈一次：球心固定在场景中间，半径 3
    if physics_has_capability("overlap") && frame() % 120 == 0 {
        let inside = physics_overlap(0.0, 2.0, 0.0, 3.0);
        log("[" + t.to_string() + "s] 半径 3 内共 " + inside.len().to_string() + " 个物体");
    }
}
