// 扩展三十三探针：骨骼冻结 + 整只模型刚体旋转。
//
// 用法（配合 37 关节的 tirger 样本）：
//   r3e assets/scene_anim.json scripts/joint_rigid_spin_demo.rhai --run-seconds 3
//
// 期望：关节矩阵「脏段 0 / 写 0 / 跳过 37 / 省 100%」——整体刚体运动不再产生伪脏。
// 网格节点 diaojingbaihu 与骨骼根 tirger 必须同步转，才是「实例整体变换变了、骨骼姿势没变」。
fn on_start() {
    print(`[probe] 动画实例: ${anims()}`);
    for inst in anims() {
        // 冻结动画：时间不再推进，骨骼局部变换逐帧位级相同
        set_anim_speed(inst, 0.0);
    }
}

fn on_update(dt) {
    rotate("tirger", 0.0, 2.0, 0.0);
    rotate("diaojingbaihu", 0.0, 2.0, 0.0);
}
