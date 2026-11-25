//! 契约：宿主侧 `impl EngineApi for LocalEngineApi` 的 62 个方法 ↔ 命令表 RPC 条目。
//!
//! 前三处契约管的是「脚本注册 ↔ 表」和「dispatch ↔ 表」；这里管第三边：宿主到底把
//! 哪些 RPC 方法实出来了。少实现一个方法，线上就是 `unsupported`；多实现一个没人认的
//! 方法，说明 `dispatch` 里少了分支。
//!
//! 方法清单不手抄，直接 `include_str!` 被测源码再扫实现块——手抄的清单会在下一次加
//! 方法时悄悄变旧，扫源码不会。
//!
//! 扫**两个**块：引擎 API 实现块，以及导航子 trait 实现块（M2-D04 起 `nav.*` 十条在
//! 协议层拆成了 `NavApi`，宿主照同样分层单独成块）。命令表不关心「哪个 trait」，
//! 只关心「宿主到底实现了哪些 rpc」，所以两侧合并成一份清单再对表。

use std::collections::BTreeSet;

use r3e_core::cmd_spec;

const API_MOD_SRC: &str = include_str!("../src/api/mod.rs");

/// 扫实现块里的方法名（只认 4 空格缩进的直接成员，嵌套函数不算）。
///
/// 块尾判据是「下一行以 `impl ` 开头」——两个块相邻时也不会互相吞掉。
fn block_methods(marker: &str) -> Vec<&'static str> {
    let start = API_MOD_SRC
        .find(marker)
        .unwrap_or_else(|| panic!("`crates/r3e-editor-host/src/api/mod.rs` 里找不到 `{marker}`"));
    let body = &API_MOD_SRC[start..];
    // 从第 1 个字节之后找下一个 `impl`，避免匹配到起始行自己。
    let end = body[1..]
        .find("\nimpl ")
        .map(|idx| idx + 1)
        .unwrap_or(body.len());
    body[..end]
        .lines()
        .filter_map(|line| {
            if !line.starts_with("    fn ") {
                return None;
            }
            let rest = line.trim_start().strip_prefix("fn ")?;
            let name = rest.split('(').next()?.trim();
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
        })
        .collect()
}

/// 宿主侧全部实现方法 = 引擎 API + 导航子 trait。
fn host_methods() -> Vec<&'static str> {
    let mut methods = block_methods("impl EngineApi");
    methods.extend(block_methods("impl NavApi"));
    methods
}

/// 把宿主方法名折成 `dispatch` 认的点分名。
///
/// 规则是**照抄** `dispatch` 的既有约定，不是新发明：前三段命名空间（`scene` / `light` /
/// `ui` / `script` / `audio.spatial`）保留点，尾段去掉下划线（`dispatch` 里 `canonical()`
/// 先删下划线，所以 `scene_set_transform` 对应的是 `scene.settransform`）。
fn rpc_of(method: &str) -> Option<String> {
    let flat = |s: &str| s.replace('_', "");
    match method {
        "info" => Some("engine.info".to_string()),
        "engine_stats" => Some("engine.stats".to_string()),
        "capture_frame" => Some("capture.frame".to_string()),
        // 存档两条是三段名（`dispatch` 里就是 `"scene.archive.save"`），不适用「尾段合并」。
        "scene_archive_save" => Some("scene.archive.save".to_string()),
        "scene_archive_load" => Some("scene.archive.load".to_string()),
        _ => {
            for prefix in [
                "audio_spatial_",
                "physics_",
                "mesh_",
                "scene_",
                "light_",
                "ui_",
                "script_",
                "tx_",
                "query_",
                // `material_*` 折成 `scene.material.*`：材质表是场景的一部分，
                // 归在 `scene` 命名空间下，而不是自立一个顶级 `material.` 域。
                "material_",
                "nav_",
                // `character_*` 折成 `character.*`（M2-B01）。
                "character_",
                // `terrain_*` 折成 `terrain.*`（M2-C06）。
                "terrain_",
            ] {
                if let Some(tail) = method.strip_prefix(prefix) {
                    let ns = match prefix {
                        "audio_spatial_" => "audio.spatial",
                        "physics_" => "physics",
                        "mesh_" => "mesh",
                        "scene_" => "scene",
                        "light_" => "light",
                        "ui_" => "ui",
                        "script_" => "script",
                        "tx_" => "tx",
                        "query_" => "query",
                        "material_" => "scene.material",
                        "nav_" => "nav",
                        "character_" => "character",
                        "terrain_" => "terrain",
                        _ => unreachable!(),
                    };
                    return Some(format!("{ns}.{}", flat(tail)));
                }
            }
            None
        }
    }
}

#[test]
fn 宿主实现的每个方法都能在表里找到rpc() {
    let declared: BTreeSet<&str> = cmd_spec::rpc_names().collect();
    let methods = host_methods();
    assert_eq!(
        methods.len(),
        83,
        "宿主实现方法数应为 83（M1-S1-D 起 `scene.snapshot` / `mesh.stats` / `scene.validate` / `physics.simulate` 也在内；M2-B01 加 `character.*` 六条；M2-C06 加 `terrain.*` 八条；M2-D04 加 `nav.*` 七条），实际 {}",
        methods.len()
    );

    let mut problems = Vec::new();
    for method in methods {
        match rpc_of(method) {
            Some(rpc) if declared.contains(rpc.as_str()) => {}
            Some(rpc) => problems.push(format!("`{method}` → `{rpc}` 不在命令表里")),
            None => problems.push(format!("`{method}` 折不出点分名（宿主动词不在约定前缀内）")),
        }
    }
    assert!(problems.is_empty(), "宿主实现与命令表对不上：{problems:#?}");
}

#[test]
fn 表里的每个rpc宿主都实现() {
    let mapped: BTreeSet<String> = host_methods().iter().filter_map(|m| rpc_of(m)).collect();
    let declared: BTreeSet<&str> = cmd_spec::rpc_names().collect();

    let missing: Vec<&&str> = declared
        .iter()
        .filter(|rpc| !mapped.contains(**rpc))
        .collect();
    assert!(
        missing.is_empty(),
        "命令表里有、宿主没实现（线上会回 unsupported）：{missing:?}"
    );
}

#[test]
fn 宿主方法与rpc映射是双射() {
    let mapped: Vec<String> = host_methods().iter().filter_map(|m| rpc_of(m)).collect();
    let unique: BTreeSet<&String> = mapped.iter().collect();
    assert_eq!(
        mapped.len(),
        unique.len(),
        "两个宿主方法折到了同一个 rpc 名：{mapped:?}"
    );
}
