# r3e 物理系统

> 状态：**已接入两个后端**。内置最简后端（重力 / 地面碰撞 / 弹性 / 摩擦 / 休眠，零依赖）+
> Rapier3D 后端（物件互撞、堆叠、旋转、碰撞事件，由 `--features rapier` 隔离）。
> 默认**关闭**、默认后端仍是 `builtin`；后端可运行时切换，`PhysicsBackend` 契约对上层零变化。

---

## 1. 一分钟上手

```bash
# 专用物理演示场景（场景文件里已 enabled=true）
cargo run -- assets/scene_physics.json scripts/physics_demo.rhai

# Rapier 演示：堆叠方块 + 碰撞事件（必须先带 feature 编译，否则 rapier 不可选）
cargo run --features rapier -- assets/scene_rapier.json scripts/rapier_demo.rhai

# 后端运行时切换演示：3s 降 builtin、6s 升回 rapier，观察堆叠行为与重绑日志
cargo run --features rapier -- assets/scene_rapier.json scripts/backend_switch_demo.rhai

# 查询层演示：射线自上而下打、球体圈选附近的刚体、球/盒扫掠（两后端结果应一致）
cargo run -- assets/scene.json scripts/query_demo.rhai --physics on
cargo run --features rapier -- assets/scene.json scripts/query_demo.rhai --physics on --physics-backend rapier

# 用命令行临时打开物理（覆盖场景文件设置）
cargo run -- --physics on

# 打开物理并换重力 / 地面高度
cargo run -- assets/scene_physics.json scripts/physics_demo.rhai --physics on --gravity 0,-3.0,0 --ground 0.5

# 只看不模拟（null 后端空转，脚本仍可 attach 但不产生位移）
cargo run -- --physics on --physics-backend null

# 带 rapier 构建时用命令行指定后端
cargo run --features rapier -- assets/scene_physics.json scripts/physics_demo.rhai --physics on --physics-backend rapier
```

窗口标题会实时显示物理状态：`物理 rapier / 6 刚体` 或 `物理关`。

---

## 2. 三层开关与优先级

物理配置按 **命令行 > 场景文件 > 内置默认** 合并，后两者缺席时由前一层决定：

| 层级 | 入口 | 写法 | 特点 |
| --- | --- | --- | --- |
| 命令行 | `--physics` / `--physics-backend` / `--gravity` / `--ground` / `--ccd` | `--physics on` | 最高优先级，且**粘住**热重载（`Physics.overrides`） |
| 场景文件 | `scene.json` 的 `physics` 段 | `"physics": { "enabled": true }` | 随场景走，热重载时重新读取 |
| 脚本 | `physics_set_enabled()` / `physics_set_backend()` 等 | `physics_set_enabled(true)` | 运行时任意时刻生效，热重载后回到场景值 |
| 内置默认 | 代码常量 | `enabled=false`、`gravity=(0,-9.81,0)`、`ground=0`、`ccd=true`、后端 `null` | 缺省即「与物理未接入时的行为完全一致」 |

几条关键语义：

- **开启但没指定后端** → 自动升级到 `builtin`。不会出现「开关开了、后端还是 null、什么都不动」的困惑。
- **关闭时**：`step` 直接返回、`attach_body` 返回 `false`、`body_count()` 恒为 `0`，物件完全由脚本驱动。
- **命令行粘性**：`--physics on` 之后即使脚本热重载（重放场景 JSON，其中写的是 `enabled:false`），物理仍保持开启。
- **脚本改动是易失的**：脚本里 `physics_set_enabled(false)` 后热重载，物理状态回到场景文件声明的值。

### 场景文件写法

```json
{
  "physics": {
    "enabled": true,
    "backend": "builtin",
    "gravity": [0, -9.81, 0],
    "ground": 0.0,
    "ccd": true
  },
  "objects": [
    { "name": "ground", "mesh": "plane", "scale": [30, 1, 30] },
    {
      "name": "ball", "mesh": "sphere",
      "position": [-3, 6, 0], "scale": [1, 1, 1],
      "physics": { "body": "dynamic", "mass": 1.0, "restitution": 0.85, "friction": 0.3 }
    },
    {
      "name": "pillar", "mesh": "cylinder",
      "position": [3, 2, -3], "scale": [1.2, 4, 1.2],
      "physics": { "body": "static", "collider": "cylinder" }
    }
  ]
}
```

`physics` 段全部字段可省，`physics` 键本身也可省（等于没声明）。

---

## 3. 物件级物理字段

| 字段 | 类型 | 缺省 | 说明 |
| --- | --- | --- | --- |
| `body` | string | `dynamic` | `static` / `dynamic` / `kinematic`（`fixed` 是 static 别名） |
| `collider` | string | 由 mesh 推导 | `box` / `sphere` / `capsule` / `cylinder` |
| `mass` | f32 | `1.0` | ≤1e-4 视为缺省 |
| `restitution` | f32 | `0.2` | 0 = 完全不弹，1 = 完全弹性 |
| `friction` | f32 | `0.6` | 落地时的切向速度衰减系数 |
| `half_extents` | [f32;3] | `scale * 0.5` | 显式覆盖碰撞体半尺寸 |
| `ccd` | bool | 跟场景级 `physics.ccd`（再缺省为 `true`） | 逐物件开关连续碰撞检测；后端不申报 `ccd` 时该字段无效（见 7.6） |

**只有声明了 `physics` 的物件才会被自动绑定成刚体**。没声明的（纯装饰）永远由脚本驱动。

---

## 4. 关闭物理开关的边界

| 情况 | 生效层 | 优先级 |
| --- | --- | --- |
| `--physics on` + 场景 `enabled:false` | 命令行 | 命令行赢 |
| 场景 `enabled:true` + `--physics off` | 命令行 | 命令行赢 |
| 场景 `enabled:true` + 脚本 `physics_set_enabled(false)` | 脚本 | 脚本赢（运行时） |
| 场景 `enabled:true` + 脚本关闭 + 热重载 | 场景文件 | 场景文件赢（脚本改动被丢弃） |

---

## 5. 内置后端的物理规则

`BuiltinBackend` 是纯 Rust 自实现（零外部依赖）。每帧先按 1/60 秒切子步（总时长 clamp 到 0.1s、最多 6 步），
每个子步对动态刚体做一次半隐式欧拉积分：

```text
v += g * h
p += v * h
if p.y <= ground + support:            // 只有下穿地面才求解
    p.y = ground + support
    if v.y < 0:
        if -v.y > 0.6 and restitution > 0.01:
            v.y = -v.y * restitution    // 够快才反弹，避免落地抖动
        else:
            v.y = 0
    v.x *= clamp(1 - friction*8*h)      // 切向摩擦，指数衰减，与帧率无关
    v.z *= clamp(1 - friction*8*h)
    if |v|² < 0.0025 and 持续 > 0.35s:
        sleeping = true; v = 0
else:
    v *= (1 - 0.02*h)                   // 空气阻尼，防长时间自由落体数值过大
```

| 概念 | 取值 / 规则 |
| --- | --- |
| 支撑半高 `support` | 球用 `half.x`（半径），盒/柱/胶囊用 `half.y` |
| 地面 | 无限大水平平面，高度由 `ground` 决定，默认 `y=0` |
| 反弹阈值 | 法向速度 > **0.6 m/s** 才反弹，否则直接归零（消除落地抖动） |
| 摩擦 | 切向速度乘以 `1 - friction*8*h`，`friction` 钳制到 `[0, 2]` |
| 休眠 | 速度模长 < **0.05 m/s** 持续 **0.35s** → 休眠并清零速度 |
| 唤醒 | `apply_impulse`，或脚本 `set_pos` 造成位置突变（`pull_from_scene` 检测） |
| 静态 / 运动学刚体 | `inv_mass = 0`，不参与积分；`static` 完全不动，`kinematic` 由脚本挪动后同步 |
| 质量 | 冲量按 `inv_mass = 1/mass` 折算成速度增量 |
| 节点回收 | 脚本 `remove_node` 后，`sync_to_scene` 找不到节点即自动回收刚体，无句柄泄漏 |

### 刻意不做的能力（保证 ~400 行可读）

- **物件之间不碰撞**：只与地面平面相交，球不会撞球、盒子不会压柱子。
- **无旋转动力学**：物体平移、不翻滚、不受力矩。
- **无关节、无堆叠求解、无 CCD**：不做金字塔堆叠，也不做连续碰撞检测（高速物体会单帧跨过地面）。

需要这些能力时走第 8 节的切换路径，而不是继续往内置后端加。

---

## 6. 脚本 API

### 6.1 开关与环境

| 函数 | 返回 | 说明 |
| --- | --- | --- |
| `physics_enabled()` | bool | 当前开关状态 |
| `physics_set_enabled(on)` | bool | 返回**操作后的**开关状态 |
| `physics_backend()` | string | `null` / `builtin` / `rapier` |
| `physics_set_backend(name)` | bool | 运行时切后端，会**清空已有刚体**；`rapier` 未编译时返回 false |
| `physics_capabilities()` | string | 当前后端能力位，逗号分隔（见 7.3） |
| `physics_has_capability(name)` | bool | 单个能力位查询，脚本据此分支而非硬编码后端名 |
| `physics_set_ccd(on)` | bool | 设置**缺省** CCD 并批量应用到现存刚体；返回后端是否支持（`builtin` 返回 false 并打印「未支持」） |
| `physics_ccd_default()` | bool | 当前缺省 CCD 开关（新建刚体继承该值） |
| `physics_take_events()` | array | 取走本帧碰撞事件（每条形如 `started:2-3`），无 events 能力时恒为空数组 |
| `physics_raycast(ox,oy,oz, dx,dy,dz, max_dist)` | array | 射线查询，命中返回 `[node, 距离, x, y, z]`，未命中返回空数组（见 7.4） |
| `physics_overlap(x,y,z, radius)` | array | 球体重叠查询，返回命中节点 id 数组（升序），无命中为空（见 7.4） |
| `physics_raycast_full(ox,oy,oz, dx,dy,dz, max_dist)` | map | 射线查询**具名版**：`{hit, node, handle, distance, point, normal, inside}`，按名取值，IDE 可补全（见 7.4） |
| `physics_shape_cast_sphere(ox,oy,oz, dx,dy,dz, radius, max_dist)` | map | 球扫掠：球心从起点沿方向推进，返回首次接触信息（同上字段，见 7.4） |
| `physics_shape_cast_box(ox,oy,oz, dx,dy,dz, hx,hy,hz, max_dist)` | map | 盒扫掠（半尺寸 `hx,hy,hz`），字段同上（见 7.4） |
| `physics_gravity()` | f64 | 取 `y` 分量 |
| `physics_set_gravity(x,y,z)` | void | 设置重力向量 |
| `physics_ground()` | f64 | 地面平面高度 |
| `physics_set_ground(y)` | void | 设置地面平面高度 |

### 6.2 刚体

| 函数 | 返回 | 说明 |
| --- | --- | --- |
| `attach_body(name, kind, mass)` | bool | 为节点创建刚体；物理关闭时返回 false；重复调用会先回收旧刚体 |
| `detach_body(name)` | bool | 销毁该节点的刚体并清空 `Node::body` |
| `body_impulse(name, x, y, z)` | bool | 施加瞬时冲量（会唤醒休眠体） |
| `body_set_ccd(name, on)` | bool | 逐刚体开关 CCD；无该刚体或后端不支持时返回 false（见 7.6） |
| `body_ccd(name)` | bool | 读回该刚体当前 CCD 状态；无该刚体或后端不支持时返回 false |
| `body_count()` | i64 | 当前刚体数；物理关闭时恒为 0 |

`attach_body` 的碰撞形状优先取节点 `physics.collider`，没有则按 `mesh` 推导（球 → 球、柱 → 柱、其余 → 盒），
半尺寸取 `scale * 0.5`。因此脚本 `create_node` 出来的物件可以直接 attach，无需额外声明。

### 6.3 示例

```rhai
fn on_start() {
    // 场景里已 enabled=true，这里只确认环境
    log("后端=" + physics_backend() + " 重力=" + physics_gravity() + " 地面=" + physics_ground());
}

fn on_update(dt) {
    // 每 2 秒从空中丢一个球
    let slot = (time() / 2.0).floor();
    let name = "drop_" + slot.to_string();
    if !exists(name) {
        create_node(name, "sphere", rand(-6.0, 6.0), 11.0, rand(-4.0, 4.0));
        set_scale_uniform(name, rand(0.6, 1.2));
        if attach_body(name, "dynamic", 1.0) {
            body_impulse(name, rand(-3.0, 3.0), 0.0, rand(-3.0, 3.0));
        }
    }
}
```

---

## 7. Rapier 后端

### 7.1 编译与启用

Rapier 走 **optional feature**，默认构建完全不含它（`cargo build` 不拉 `rapier3d` / `parry3d` / `nalgebra`）：

```toml
# crates/r3e-physics/Cargo.toml
rapier = ["dep:rapier3d"]

# crates/r3e-app/Cargo.toml —— 透传，方便直接 cargo run --features rapier
rapier = ["r3e-physics/rapier"]
```

| 构建方式 | `builtin` | `rapier` | 说明 |
| --- | --- | --- | --- |
| `cargo run` | 可用 | **不可用**（`set_kind(Rapier)` 报错，`set_backend("rapier")` 返回 false） | 默认构建，二进制不带物理引擎 |
| `cargo run --features rapier` | 可用 | 可用 | 两后端同在一个二进制里，可运行时互切 |

「未编译就明确报错、不静默降级」是刻意行为：`PhysicsKind::available_names()` 会按当前构建返回 `builtin / null` 或 `builtin / null / rapier`，
命令行帮助、未知后端提示、脚本报错都用它，避免用户以为切了后端其实没切。

### 7.2 与内置后端的行为差异

| 行为 | `builtin` | `rapier` |
| --- | --- | --- |
| 刚体 vs 地面 | 有（无限平面） | 有（`halfspace`，随 `ground` 平移） |
| 刚体 vs 刚体 | **无** | 有，可堆叠、可互相推挤 |
| 旋转 / 翻滚 / 力矩 | 无 | 有，`sync_to_scene` 写回 `transform.rotation` |
| 碰撞事件 | 无 | 有（`started` / `stopped`） |
| CCD（高速防穿透） | 无（能力位 `false`） | 有，**默认开**；运动学 / 动态刚体有效，固定体由 rapier 自动扫掠（见 7.6） |
| 关节 | 无 | 底层支持，门面暂不暴露 |
| 子步策略 | 1/60 切分，最多 6 步 | 同左，`MAX_SUBSTEPS = 6` 保持一致节奏 |
| 静态 `plane` | 装饰用，碰撞由 `ground` 平面负责 | 同左（不做网格碰撞） |

对齐过的语义（换后端不会让脚本行为跳变）：

- `Static` 完全不动：脚本挪静态物件不会带动碰撞，需重建刚体。
- `Kinematic` 每帧从节点推位姿给物理引擎，由它带着动态体走。
- `Dynamic` 由物理积分；脚本 `set_pos` 位移超过 `1e-4` 视为**瞬移**，位置以节点为准并清零线速度/角速度。
- 节点被 `remove_node` → `sync_to_scene` 回收刚体（`records` 里查不到节点即销毁），无句柄泄漏。
- 地面本身不是场景节点，事件里以 `none` 表示（例如 `started:none-9` 是 9 号节点砸地）。

### 7.3 能力位

`PhysicsBackend::capabilities()` 用 7 个布尔位申报真实能力，**这是脚本做兼容分支的唯一正确姿势**：

| 位 / 脚本名 | builtin | rapier |
| --- | --- | --- |
| `body_body`（`body_body_contacts`） | false | true |
| `rotation`（`rotational_dynamics`） | false | true |
| `joints` | false | **false** |
| `ccd` | false | **true** |
| `events`（`collision_events`） | false | true |
| `raycast` | true | true |
| `overlap` | true | true |
| `shape_cast` | true | true |

`raycast` / `overlap` / `shape_cast` 是查询层能力：builtin 用解析法查 AABB（`shape_cast` 靠
Minkowski 展开 + slab），rapier 走 parry 的 `cast_ray_and_get_normal` / `intersection_test` /
`cast_shapes`，两侧都真能查出结果，因此都如实报 `true`。

申报口径：**能力位只报门面真能用到的能力**（`BackendCaps::EXPOSED` = `CONTACT_CCD`）。
Rapier 底层确实还有 `impulse_joints` / `multibody_joints`，但 `PhysicsBackend` 契约里没有建关节的方法，
脚本更没有任何入口——这种情况下报 `true` 就是虚报，脚本会拿着 `physics_has_capability("joints") == true`
去调用并不存在的接口。所以 `joints` 一律为 `false`；CCD 则因为门面已有
`set_ccd` / `ccd_of` / `set_ccd_default` 与脚本三件套（`physics_set_ccd` / `body_set_ccd` / `body_ccd`），
已经点亮为 `true`。`BackendCaps::FULL`（含 joints）仍只是给未来预留的档位，
`tests/backend_contract.rs::no_backend_overclaims_capabilities` 会拦住任何越界申报，
并反向校验「报了 `ccd` 就必须真能设进去」。

脚本侧：`physics_capabilities()` 拿整串（全 false 时为 `none`），`physics_has_capability("events")` 查单个。
**不要在脚本里判断 `physics_backend() == "rapier"`** —— 那会把「后端名」和「能力」绑死，将来接入第三个后端就又要改脚本。

### 7.4 查询层：raycast / overlap / shape-cast

查询是**后端无关**的：`PhysicsBackend` 上三个带默认实现的可选方法，门面 `Physics::raycast` /
`Physics::overlap_sphere` / `Physics::shape_cast`（另有 `shape_cast_sphere` / `shape_cast_box` 便捷版）
再做一次能力位与开关过滤（物理关闭时直接返回空，不会拿旧数据糊弄）。

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `PhysicsBackend::raycast` | `Ray`, `max_distance` | `Option<RayHit>`（`handle` / `node` / `distance` / `point` / `normal`） |
| `PhysicsBackend::overlap_sphere` | `center`, `radius` | `Vec<OverlapHit>`（按 `node` 升序） |
| `PhysicsBackend::shape_cast` | `origin`, `dir`, `CastShape`, `max_distance` | `Option<ShapeCastHit>`（同上 + `started_inside`） |

`CastShape` 是查询形状（不是刚体形状）：`CastShape::Sphere { radius }` / `CastShape::Box { half }`，
未来加胶囊/圆柱只扩这个枚举，不动 trait。

两个后端的实现差异：

| | builtin | rapier |
| --- | --- | --- |
| 射线 | 刚体世界 AABB 做 slab 测试，取最近 | parry `cast_ray_and_get_normal` 逐碰撞体窄相，取最近 |
| 覆盖形状 | 精确（builtin 不旋转，AABB 即包围体；球按半径立方体近似） | 精确（球 / 盒 / 胶囊 / 圆柱真实窄相） |
| 球重叠 | 球心到 AABB 最近距离 ≤ 半径 | parry `intersection_test`（Ball vs 碰撞体） |
| 扫掠 | Minkowski 展开：把目标 AABB 各轴外扩查询形状半尺寸后复用 slab，取 `t_enter` | parry `cast_shapes`，`vel = dir × max_distance`、`max_toi = 1`，`time_of_impact` 再折回世界距离 |
| 地面 | 不参与（地面不是刚体） | 不参与（半空间不在 `records` 里） |

约定：

- `Ray::new(origin, dir)` **自动归一化方向**，零向量退化为 `+Y`，脚本不用自己除长度；
- `max_distance` 用世界单位，`<= 0` 视为「查不到」；
- 射线起点在物体内部时 `distance = 0`、法线取 `-dir`（与常见引擎口径一致）；
- **扫掠起点已经重叠**时同样 `distance = 0`、`started_inside = true`、法线 `-dir`。
  Rapier 的穿透法线是「最小平移方向」，与推进方向无关（球心正对盒心时甚至可能朝上），
  门面统一改写成 `-dir`，保证两后端只认一条规则；
- 扫掠的 `distance` / `max_distance` 都是**沿推进方向的世界距离**（不是时间比例），
  脚本拿它和实际米数比较即可；方向由 `dir` 自动归一化，零向量直接返回未命中；
- builtin 的球扫掠用**立方膨胀**近似：沿盒面推进与 Rapier 完全一致，只在对角线方向略保守
  （提前一点点命中），属内置后端的已知口径；
- 查不到返回 `None` / 空数组，不返回哨兵节点；
- 暂不支持过滤（layers / mask / 排除自身），需要在脚本里拿 `node` 二筛。
- **位姿一致性**：`attach_body` 会先 `update_world_transforms()` 再取 `world_position()`。脚本里
  「`create_node` → `attach_body`」连写时节点世界矩阵还没刷新，少了这一步刚体会停在原点
  （踩坑：builtin 查询按节点世界位姿算 AABB，rapier 按刚体位姿，一旦刚体落错位置，两后端命中结果会不一致）。

脚本示例见 `scripts/query_demo.rhai`：

```rhai
// 具名 Map：IDE 能补全字段，未命中时字段照样齐备（hit=false / node=-1），不用先判长度
let hit = physics_raycast_full(0.0, 8.0, 0.0, 0.0, -1.0, 0.0, 20.0);
if hit.hit {
    log("命中节点 " + hit.node + " 距离 " + hit.distance + " 法线 y=" + hit.normal[1]);
}
let inside = physics_overlap(0.0, 2.0, 0.0, 3.0);

// 扫掠：半径 0.4 的球从 y=6 往下推 20 米
if physics_has_capability("shape_cast") {
    let cast = physics_shape_cast_sphere(0.0, 6.0, 0.0, 0.0, -1.0, 0.0, 0.4, 20.0);
    log("推进 " + cast.distance + " 米后接触节点 " + cast.node);
}
```

旧的 `physics_raycast`（返回 `[node, 距离, x, y, z]` 数组）保留不动，老脚本不受影响；
新脚本建议用 `_full` 具名版——按名取值不会被下标顺序绑架。

### 7.5 运行时切换

`physics_set_backend("builtin")` / `("rapier")` / `("null")`，语义与 `set_kind` 一致：

1. 清空**全部**刚体并 bump `generation`；
2. `app` 在下一代重新把「场景声明了 physics 的物件」绑成刚体；
3. 脚本此前 `attach_body` 的手工刚体不会自动恢复，需要脚本在 `generation` 变化后自行重绑。

切换是「换引擎」而非「改参数」，飞行中的物件会回到场景初始位姿重新下落 —— 这是预期行为，文档与日志都如实写明。

### 7.6 CCD（连续碰撞检测）

**要解决的问题**：离散检测只在每帧末尾判重叠。`dt=0.05`、初速 300 m/s 的子弹一帧走 15 m，
一面 4 cm 厚的墙会被整帧跨过去——先穿透、再「已经出去了」，永远测不到接触。

**四层入口，缺一不可**：

| 层 | 入口 | 语义 |
| --- | --- | --- |
| 契约 | `PhysicsBackend::set_ccd` / `ccd_of` / `set_ccd_default` / `ccd_default` | 带默认实现（`false` / `None` / 不做事），新后端不实现也能编译，能力位如实报 `false` |
| 门面 | `Physics::set_ccd` / `ccd_of` / `set_ccd_default` / `set_ccd_all` | `set_ccd_all` 批量刷已存在的刚体并返回改动数量 |
| 场景 | 全局 `physics.ccd` + 逐物件 `physics.ccd` | 逐物件优先；两级都省 = 开（默认防穿透，而不是默认放穿透） |
| 脚本 / CLI | `physics_set_ccd(on)` / `physics_ccd_default()` / `body_set_ccd(name, on)` / `body_ccd(name)`、`--ccd on\|off` | 运行时改缺省值会同时刷现存刚体，避免「改了开关但老刚体没变」的半生效 |

**生效范围（rapier 0.36 的真实行为，别想当然）**：

| 刚体类型 | `enable_ccd(true)` 的效果 |
| --- | --- |
| `dynamic` | 有效：按 TOI 截断本步位移 |
| `kinematic` | 有效：快速移动的运动学体不再穿过动态体 |
| `static` / 固定体 | **无效也不必要**：rapier 对固定碰撞体本来就做自动扫掠（`ccd_solver` 非空时） |

所以 `ccd_of()` 对固定体一律返回 `false` —— 它报的是「这一位真的被设上了吗」，不是「这个物体会不会穿」。
门面对非动态体调用 `set_ccd` 会返回 `false`，不假装成功。

**穿透对照测试**（`crates/r3e-physics/tests/ccd_penetration.rs`，无窗口、纯数值）：

- 靶体用**运动学**薄板（半厚 0.02 m），不用静态墙——静态墙本来就有自动扫掠，观察不到差异；
- 子弹是 0.05 m 半径动态球，`dt=0.05`、初速 300 m/s，单步位移 5 m；
- 地面沉到 `y=-100`，避免无限地面蹭掉子弹动量（这是写这个用例时踩到的坑）；
- 断言：关 CCD 时 `x > 1.0`（穿过去了），开 CCD 时 `x < 0.0`（停在板前）；另有一例校验
  `set_ccd_default` / `set_ccd_all` / `set_ccd` / `ccd_of` 的读回一致性，以及无效句柄如实返回失败。

**肉眼可见的对照**：`cargo run --release --features rapier -- assets/scene_ccd.json scripts/ccd_demo.rhai`
—— 一面 4 cm 厚运动学薄墙 + 两颗只差 `ccd` 一位的球（120 m/s，即每帧 2 m）。蓝色 `bullet_ccd` 停在
墙前 `x ≈ -0.22` 且不再前进；黄色 `bullet_no_ccd` 头也不回地飞向 +x。脚本会把两组 x 每 0.5 秒打一次日志，
并给出「被墙挡住 / 穿墙」的结论行。墙用 `kinematic` 是因为静态墙本来就自动扫掠，开关两边都挡得住。

**代价**：CCD 会让每个高速动态体做一次扫掠，密集子弹场景下开销可见；因此才保留「逐物件关掉」这一档
（大量慢速道具可以省下来），而不是简单地全局硬开。默认开是为了修掉「高速物件穿地」这种一看就是缺陷的表现。

---

## 8. 与帧循环 / 热重载的配合

```
dt → scripts.tick → camera input → sync_physics_bindings → physics.step → update_world_transforms
```

- **绑定时机**：`Physics` 维护一个 `generation`（配置世代号）。开关切换、后端重建、`clear_bodies` 都会自增；
  `app` 只在世代变化时把「场景声明了 physics 的物件」重绑一次。因此脚本手动 `attach_body` 的刚体不会被覆盖。
- **脚本每帧改位置会与物理打架**：`pull_from_scene` 会识别「脚本挪了节点」并唤醒刚体，随后物理把它拽回去。
  正确做法是物理开启时不要对同一物件 `set_pos`（`scripts/main.rhai` 里就是 `if !physics_enabled() { ... }` 包起来）。
- **热重载**：清空全部刚体 → 重放场景 JSON → 重新应用场景 physics 配置 → 重跑脚本；
  命令行的 `--physics` 等覆盖项保留（`overrides` 粘性）。

---

## 9. 再接入第三个后端（或替换 Rapier）

契约没有变，改动面固定在三处 —— Rapier 就是照这个模板加进来的：

1. 新建 `crates/r3e-physics/src/xxx_backend.rs`，实现 `PhysicsBackend` 的必需方法：
   `name` / `create_body` / `destroy_body` / `apply_impulse` / `set_gravity` / `step` / `sync_to_scene` / `body_count`；
   若该引擎能出事件，再实现 `drain_events` 并在 `capabilities().collision_events` 如实置位。
   **能力位只报门面真能用到的能力**：契约里没有对应方法、脚本没有入口的位（目前是 `joints` / `ccd`）
   一律置 `false`，否则脚本会拿到用不了的承诺。
2. `PhysicsKind` 增加分支 + `Physics::rebuild_backend` 指向新实例 + 在 `available_names` 里按 feature 暴露。
3. `Cargo.toml` 加 optional 依赖与 feature，`r3e-app` 透传同名 feature。

场景 JSON、脚本 API、`app` 层、`core::Scene` 都**不需要改**。

**唯一红线**：外部引擎的类型只能出现在那一个后端文件里。门面、`app`、脚本层只允许出现 `glam::Vec3` / `Quat`
和 `PhysicsBackend` 契约类型；高级能力通过 `BackendCaps` 位申报，不往 trait 里塞后端专属方法。
守不住这条，第二次切换就要动上层代码，可插拔就名存实亡。

回归口径：`crates/r3e-physics/tests/backend_contract.rs` 是一组**后端无关**的契约测试，
对当前构建里所有 `available_names()` 逐个跑（落体、销毁回收、切换清空、能力矩阵自洽），
Rapier 专属的堆叠 + 碰撞事件用例单独用 `#[cfg(feature = "rapier")]` 关起来。加新后端时把名字塞进列表即可复用。

---

## 10. 已实测行为

| 场景 | 观测 |
| --- | --- |
| `assets/scene_physics.json`，默认配置 | 场景 `enabled:true` → 日志「物理: 启用 / 后端 builtin」，绑定 5 个场景刚体 |
| 同场景 `--physics off` | 日志「物理: 关闭」，不绑定任何刚体，脚本 `attach_body` 返回 false |
| `assets/scene.json`（无 physics 段）`--physics on` | 后端自动升级为 `builtin`，绑定 3 个刚体（地面 + 两柱），脚本挂上 `orbiter` 后共 4 个 |
| 下落积分 | 球从 `y=6` 落到 `y=0.5` 后反弹至 `y≈4.34`（restitution 0.85），逐次衰减 |
| 静止与休眠 | 盒子从 `y=12` 落到 `y=0.6999…`（half=0.7）后数值恒定不再抖动 |
| 运行时开关 | `physics_set_enabled(false/true)`、`physics_set_backend("null"/"builtin")` 均生效 |
| 默认构建 `physics_set_backend("rapier")` | 返回 false 并打印「未编译进本构建」，不静默降级 |
| `cargo run --features rapier -- assets/scene_rapier.json scripts/rapier_demo.rhai` | 日志「物理: 启用 / 后端 rapier / 能力 [body_body,rotation,ccd,events,raycast,overlap,shape_cast] / CCD 默认 on」，绑定 6 个场景刚体；脚本打印碰撞事件（如 `started:none-9` 砸地、`stopped:10-11` 方块分离），方块在平台上堆住不倒 |
| `scripts/backend_switch_demo.rhai`（同场景，运行中切换） | 3s `physics_set_backend("builtin")` → true、刚体数归 0、能力变 `none`，引擎打印「物理后端 -> builtin」并重新绑定 6 个场景刚体；6s 切回 rapier 同样成功，碰撞事件恢复 |
| `--physics on --physics-backend rapier --run-seconds 2`（实测日志） | 「能力 [body_body,rotation,ccd,events,raycast,overlap,shape_cast] / **CCD 默认 on**」；加 `--ccd off` 后同一行变「CCD 默认 off」，能力位仍是 `ccd`（能力 = 有没有，不是开没开） |
| `cargo test -p r3e-physics --features rapier --test ccd_penetration` | 3 passed：关 CCD 子弹穿板 `x > 1.0`、开 CCD 停在板前 `x < 0.0`、`set_ccd_default`/`set_ccd_all`/`ccd_of` 读回一致且非法句柄返回 false |
| `assets/scene_ccd.json scripts/ccd_demo.rhai`（实测） | 日志「bullet_ccd ccd=true / bullet_no_ccd ccd=false」；0.5s 时 ccd 球 `x ≈ -0.218`（墙前）、无 ccd 球 `x ≈ 45.4`；2s 时 ccd 球稳在 `-0.2199…`、无 ccd 球到 `225.8` —— 结论行「ccd球 被墙挡住 / 无ccd球 穿墙」 |
| `assets/scene.json scripts/query_demo.rhai --physics on` | 两后端一致：`命中节点 8 / 距离 5.6 / 命中点 (0, 2.4, 0)`（自上而下打静态方块顶面），球体重叠 `半径 3 内共 3 个物体`（方块 + 自建地板盒 + 场景地面） |
| 同场景 `--physics off` 跑 query_demo | `physics_has_capability("raycast")` = false，脚本直接跳过查询 |
| `assets/scene.json scripts/query_demo.rhai --physics on --run-seconds 4`（后端 builtin） | 日志 `能力 = [raycast,overlap,shape_cast]`；`physics_raycast_full` 命中节点 8 / 距离 5.6 / 命中点 (0, 2.4, 0) / 法线 (0, 1, 0)；球扫掠（r=0.4 自 y=6 下推）距离 **3.1999998**、接触点 (0, 2.8, 0)、法线 (0, 1, 0)；盒扫掠（half=0.5）距离 **3.0999999**、接触点 (0, 2.9, 0) |
| 同场景换 `--physics-backend rapier` | 同脚本同参数：射线距离 5.6、法线 (0, 1, 0) 一致；球扫掠距离 **3.2000022**、盒扫掠 **3.0999999**、节点同为 8——两后端在轴对齐面上仅差浮点末位，验证 Minkowski 解析法与 parry `cast_shapes` 同解 |

测试：`cargo test --workspace` 全绿；`cargo test -p r3e-physics --features rapier` 额外跑 Rapier 契约用例
（落体、销毁回收、后端切换清空、能力矩阵，`every_backend_raycast_and_overlap_find_nearest_body`
——两后端都命中最近的盒子、背向打空、`max_distance` 截断、球重叠与关闭后失效，
`every_backend_shape_cast_reports_nearest_surface`——球/盒扫掠命中近表面（距离 4.5、法线 -Z）、
起始重叠报 `distance=0 + started_inside + 法线 -dir`、背向打空、`max_distance` 截断、零半径短路、
关闭后失效，以及 `rapier_stacks_bodies_and_reports_collision_events`——球落平台触发真实互撞事件）。

---

## 11. 已知限制

按后端分列，都属于「该后端的定位」，不是待修 bug：

**`builtin`（刻意保持 ~400 行可读）**

- 物件互不碰撞，所以「一堆球掉进箱子」只会全部穿到地面。
- 无旋转，盒子落地不会侧翻，斜坡上不会滚动。
- 高速度（>~50 m/s）可能单帧穿过地面（无 CCD，能力位如实 `false`）；需要防穿透请切 `rapier` 后端。
- 扫掠查询只有球 / 盒两种形状，且球按立方 Minkowski 膨胀近似：沿盒面对齐方向与 Rapier 一致，
  斜向略保守（提前一点命中）。

**`rapier`**

- 关节（joint）底层支持但门面未暴露，脚本暂不能建约束，能力位 `joints=false`。
- CCD 已支持且默认开，但只对动态 / 运动学刚体生效（固定体靠 rapier 自动扫掠）；高速物件的扫掠有开销，
  密集弹幕这类场景建议用 `physics_set_ccd(false)` 或逐物件 `body_set_ccd` 关掉。
- 静态 `plane` 只是装饰，物理地面永远是 `ground` 定义的无限平面，没有地形 / 网格碰撞。
- 初始重叠时 parry 报的是「最小平移方向」，门面统一改写成 `-dir`（契约口径，见 7.4），
  想拿真实分离向量得用 overlap + 自己算。

**通用**

- 物理与脚本共用 `RefCell<Scene>`，脚本里不能长期持有 `borrow()` 期间再 `attach_body`（现有 API 已规避）。
- 切换后端会清空全部刚体并让物件回到场景初始位姿，运行中「换引擎」不是无损操作。

---

## 12. 碰撞体形状：只由几何决定（S4 已落地）

**变化**：早期按 `MeshRef::Builtin(Sphere/Cylinder/...)` 特判形状，外部资产一律退化成长方体。
统一 AIR 之后，物理层**不再认识 `MeshRef` 来源**，只吃 `MeshData`：

```rust
BodyDesc::infer_with(&mesh_data, scale)      // 由几何判定形状
BodyDesc::from_node_with(node, Some(&mesh))  // 场景构建时的默认路径
```

`infer_shape(&MeshData)` 的判定顺序：

| 判定 | 条件 | 结果 |
| --- | --- | --- |
| 球 | AABB 三轴半尺寸两两相对差 ≤ 5%，且全部顶点到中心距离与半径偏差 ≤ 2% 半径 | `Sphere { radius }` |
| 圆柱 | XZ 两轴半尺寸相对差 ≤ 5%，且「侧壁顶点」（径向距 ≈ 半径，容差 2%）占顶点数一半以上、同时没有顶点落在 5%–95% 半径的中间带 | `Cylinder { radius, half_height }`（轴向 Y） |
| 其它 | 兜底 | `Box { half_extents }`（= AABB 半尺寸） |

要点：

- 判定只用**顶点分布**，不看名字、不看来源、不看 `MeshRef`；
- 半尺寸一律乘物件 `scale`，所以「同一网格放大 3 倍」得到的是 3 倍体积的碰撞体；
- 判定失败不报错，兜底长方体永远可用（导入面永远不因物理而失败）；
- 网格为空时退化为单位长方体，只告警。

验收：`cargo test -p r3e-physics --features rapier`（16 单测 + 9 契约 + 3 穿透 = 28 passed），其中
`shape_inference_reads_geometry_not_source`、`infer_reads_mesh_shape` 覆盖球/圆柱/盒与平移不变性。
