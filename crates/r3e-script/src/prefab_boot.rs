//! 声明式 prefab 实例的「开机展开」——补上 M2-E 留下的那个空接线。
//!
//! # 为什么挂在脚本宿主，而不是 `App::new`
//!
//! [`Scene::instantiate_declared`] 的前提是**模板表已经灌好**，而模板走的唯一入口是脚本
//! 里的 `prefab_define`（`app` 侧没有任何模板来源：场景文件只落实例记录，模板体不进
//! 文件）。也就是说，模板就位的那一刻恰好是脚本 `on_start` 跑完的那一刻。
//!
//! 挂在脚本生命周期尾部，一条改动同时覆盖三条真实路径，且不需要在 `app` 里存「我到底
//! 展开过没有」这类状态：
//!
//! * 启动：`App::new` → `ScriptHost::load_file` → `ScriptHost::load_str`；
//! * F5 / 编辑器改 scene.json：`App` → `ScriptHost::rebuild_scene`；
//! * 脚本热重载：`App` → `ScriptHost::reload_if_changed` → `load_file` → `load_str`。
//!
//! # 为什么单独一个文件
//!
//! `lib.rs` 近 2000 行、被 `xtask/size_debt.json` 盯着只许变小。两条调用点都写成一行
//! 收尾调用，`lib.rs` 净增 0 行；逻辑落在这里，后续再改也不用动债文件。
//!
//! [`Scene::instantiate_declared`]: r3e_core::Scene::instantiate_declared

use super::*;

impl ScriptHost {
    /// 把场景 `instances` 里**还没有对应节点**的实例展开成节点，返回本次展开的条数。
    ///
    /// 幂等：已经有节点的实例直接跳过（判据是 `Scene::find(name)`，与
    /// `instantiate_declared` 内部过滤口径一致），所以每跑一次脚本、每重建一次场景都可以
    /// 无脑调，不会造出重复节点。
    ///
    /// # 失败策略
    ///
    /// 失败**不冒泡**，只打日志并返回本次已展开条数（可能是 `0`）。理由是不对等：
    ///
    /// * 模板没登记这种错误，`prefab_define` 自己已经报过一遍，这里再让整段脚本载入
    ///   失败，会把「模型没出来」升级成「引擎起不来」；
    /// * 展开失败时实例记录**仍在** `instances` 表里（`instantiate_declared` 只读表、
    ///   失败不动表），场景快照照样把它写回磁盘，`prefab.orphan` 校验也照样能看见它们，
    ///   不会出现「实例悄悄消失」这种最难查的形态。
    ///
    /// 注意 `instantiate_declared` 是「遇错即停」：前面成功的实例已经成节点、后面没轮到
    /// 的仍留在表里，两边都能被 `prefab.info` / `prefab.orphan` 观察到，不存在中间黑箱。
    pub fn expand_declared_prefabs(&self) -> usize {
        // 一次 write 里完成「展开 + 算世界矩阵」：节点是算出来的结果，`spawn` 只写局部
        // 变换，不补这一步的话同帧查询读到的世界位置还是零（`prefab_spawn` 走的是同一
        // 条纪律，见 `api/prefab.rs`）。
        let outcome = self.doc.borrow_mut().write(|scene| {
            let ids = scene.instantiate_declared();
            scene.update_world_transforms();
            ids
        });
        match outcome {
            Ok(ids) => {
                if !ids.is_empty() {
                    println!("[script] 声明式 prefab 实例展开 {} 条", ids.len());
                }
                ids.len()
            }
            Err(error) => {
                println!("[script] 声明式 prefab 实例展开失败: {error}");
                0
            }
        }
    }

    /// 脚本一轮跑完的收尾：展开声明式实例 + 复位错误槽。
    ///
    /// 复位错误槽是 `load_str` 的原有行为（上一轮 `on_start` 的错误不带给下一轮），
    /// 顺序不能反：先展开（可能打印新错误）再清槽，否则展开失败会被立刻抹掉。
    pub(crate) fn finish_script_run(&mut self) -> Result<(), String> {
        self.expand_declared_prefabs();
        self.last_error = None;
        Ok(())
    }

    /// 场景重建后的收尾：只展开，不动错误槽。
    ///
    /// 与 `finish_script_run` 的差别只在错误槽：`rebuild_scene` 从不复位
    /// `last_error`（脚本重放失败的错误要留给宿主显示），保持原语义。
    pub(crate) fn finish_rebuild(&mut self) -> Result<(), String> {
        self.expand_declared_prefabs();
        Ok(())
    }
}
