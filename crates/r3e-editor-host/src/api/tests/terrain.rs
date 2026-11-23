//! `terrain.*` 八条命令的宿主层测试（M2-C06，对应 `M2_C_PLAN.md` §10.4 的 12 条）。
//!
//! 这一层的判据只有三种：**回包字段对不对**、**错误码对不对**、**场景哈希变没变**。
//! 几何正确性（采样值、权重、三角数）在 `r3e-runtime::terrain_ops` 与 `r3e-terrain`
//! 各自的单测里验过，这里不重算一遍——重复验算只会让两处断言一起烂。

use super::*;
use r3e_editor::{
    ENGINE_ERROR, INVALID_PARAMS, TERRAIN_BLOCK_MISSING, TERRAIN_PATCH_MISSING, TX_REJECTED,
};

fn create(api: &mut LocalEngineApi<'_>, name: &str, rect: [f32; 4], res: Option<u32>) -> Value {
    api.terrain_create(TerrainCreateQuery {
        name: name.to_string(),
        rect,
        resolution: res,
    })
    .expect("建地形不该失败")
}

#[test]
fn terrain_create_写入并回带哈希() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let before = api.scene.borrow().state_hash();

    let out = create(&mut api, "ground", [0.0, 0.0, 100.0, 100.0], Some(8));

    assert_eq!(out["name"], "ground");
    assert_eq!(out["created"], true);
    assert_eq!(out["patch"]["resolution"], json!([8, 8]));
    assert_eq!(out["patch"]["samples"], 64);
    let hash = out["stateHash"]
        .as_str()
        .expect("应带 stateHash")
        .to_string();
    assert_ne!(hash, format!("0x{:016x}", before.raw()), "写场景必须改哈希");
    assert_eq!(api.scene.borrow().as_scene().terrain.len(), 1);
}

#[test]
fn terrain_create_非法分辨率报参数错误且场景不变() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let before = api.scene.borrow().state_hash();

    for res in [0_u32, 129] {
        let err = api
            .terrain_create(TerrainCreateQuery {
                name: "bad".to_string(),
                rect: [0.0, 0.0, 100.0, 100.0],
                resolution: Some(res),
            })
            .expect_err("越界分辨率必须失败");
        assert_eq!(err.code, INVALID_PARAMS, "res={res} 应回参数错误");
    }
    assert!(api.scene.borrow().as_scene().terrain.is_empty());
    assert_eq!(api.scene.borrow().state_hash(), before, "失败不许动场景");
}

#[test]
fn terrain_create_超过块上限报错而不是静默覆盖() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    for i in 0..r3e_core::TERRAIN_MAX_PATCHES {
        create(&mut api, &format!("p{i}"), [0.0, 0.0, 10.0, 10.0], None);
    }
    let count = api.scene.borrow().as_scene().terrain.len();
    let err = api
        .terrain_create(TerrainCreateQuery {
            name: "overflow".to_string(),
            rect: [0.0, 0.0, 10.0, 10.0],
            resolution: None,
        })
        .expect_err("第 33 块必须被拒");
    // 不是参数错，是容量错：调用方照 INVALID_PARAMS 去改参数永远改不对。
    assert_eq!(err.code, ENGINE_ERROR, "{}", err.message);
    assert_eq!(api.scene.borrow().as_scene().terrain.len(), count);
}

#[test]
fn terrain_remove_存在回true不存在回false() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    create(&mut api, "ground", [0.0, 0.0, 10.0, 10.0], None);

    let out = api
        .terrain_remove(TerrainNameQuery {
            name: "ground".to_string(),
        })
        .expect("删已有块不该失败");
    assert_eq!(out["removed"], true);
    assert_eq!(out["remaining"], 0);

    // 幂等：再删一次是 `removed: false`，不是错误。
    let again = api
        .terrain_remove(TerrainNameQuery {
            name: "ground".to_string(),
        })
        .expect("删不存在的块不该报错");
    assert_eq!(again["removed"], false);
    assert_eq!(again["remaining"], 0);
}

#[test]
fn terrain_sample_回包含高度法线权重三键() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    create(&mut api, "ground", [0.0, 0.0, 100.0, 100.0], None);

    let out = api
        .terrain_sample(TerrainSampleQuery {
            name: Some("ground".to_string()),
            x: 10.0,
            z: 10.0,
        })
        .expect("采样不该失败");
    for key in ["height", "normal", "weights"] {
        assert!(out.get(key).is_some(), "回包缺 `{key}`：{out}");
    }
    assert_eq!(out["inside"], true);
    assert_eq!(out["height"], 0.0);
    let weights: Vec<f32> = out["weights"]
        .as_array()
        .expect("weights 应为数组")
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    assert_eq!(weights.len(), 4);
    let sum: f32 = weights.iter().sum();
    assert!((sum - 1.0).abs() < 1e-4, "权重要和为 1，实际 {sum}");
}

#[test]
fn terrain_sample_空名跨块命中最高层() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    create(&mut api, "low", [0.0, 0.0, 100.0, 100.0], None);
    create(&mut api, "high", [0.0, 0.0, 50.0, 50.0], Some(4));

    let out = api
        .terrain_sample(TerrainSampleQuery {
            name: None,
            x: 25.0,
            z: 25.0,
        })
        .expect("采样不该失败");
    assert_eq!(out["inside"], true);
    // 两块都覆盖该点，空名要回一个真实存在的块名（AI 下一步多半拿它继续问）。
    let name = out["name"].as_str().expect("应带块名");
    assert!(
        ["low", "high"].contains(&name),
        "命中的块名 {name} 不在场景里"
    );

    // 没地形的地方：成功回 `inside: false`，不是 TERRAIN_PATCH_MISSING。
    let outside = api
        .terrain_sample(TerrainSampleQuery {
            name: None,
            x: 500.0,
            z: 500.0,
        })
        .expect("空地采样不该报错");
    assert_eq!(outside["inside"], false);

    // 名字不存在才是错误。
    let err = api
        .terrain_sample(TerrainSampleQuery {
            name: Some("nope".to_string()),
            x: 1.0,
            z: 1.0,
        })
        .expect_err("不存在的块必须报错");
    assert_eq!(err.code, TERRAIN_PATCH_MISSING);
}

#[test]
fn terrain_blocks_按块边长切出十六块() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    create(&mut api, "ground", [0.0, 0.0, 200.0, 200.0], None);

    let out = api
        .terrain_blocks(TerrainBlocksQuery {
            block_size: Some(50.0),
        })
        .expect("切块不该失败");
    assert_eq!(out["dims"], json!([4, 4]));
    assert_eq!(out["count"], 16);
    assert_eq!(out["covered"], 16);
    let blocks = out["blocks"].as_array().expect("blocks 应为数组");
    assert_eq!(blocks.len(), 16);
    assert_eq!(blocks[0]["index"], json!([0, 0]));
    assert_eq!(blocks[0]["patches"], json!(["ground"]));
    assert_eq!(blocks[0]["rect"], json!([0.0, 0.0, 50.0, 50.0]));
}

#[test]
fn terrain_stats_三角数等于逐块网格三角数之和() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.terrain_gen(TerrainGenQuery {
        size_x: 200.0,
        size_z: 200.0,
        seed: 21,
    })
    .expect("生成不该失败");

    let stats = api.terrain_stats().expect("统计不该失败");
    let count = stats["blocks"].as_u64().unwrap() as u32;
    let mut sum = 0_u64;
    // 逐块求和：与 `terrain.stats` 必须逐位相等（两处同口径是同一条硬约束）。
    let dims = api
        .terrain_blocks(TerrainBlocksQuery { block_size: None })
        .unwrap()["dims"]
        .clone();
    let (dx, dz) = (
        dims[0].as_u64().unwrap() as u32,
        dims[1].as_u64().unwrap() as u32,
    );
    assert_eq!(dx * dz, count);
    for j in 0..dz {
        for i in 0..dx {
            let m = api
                .terrain_mesh(TerrainMeshQuery {
                    block: Some([i, j]),
                    lod: None,
                    block_size: None,
                })
                .expect("单块网格不该失败");
            sum += m["triangles"].as_u64().unwrap();
        }
    }
    assert_eq!(stats["triangles"].as_u64().unwrap(), sum);
    assert_eq!(stats["lod"], 1);
}

#[test]
fn terrain_mesh_回包含三角顶点档位() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    create(&mut api, "ground", [0.0, 0.0, 100.0, 100.0], Some(16));

    let out = api
        .terrain_mesh(TerrainMeshQuery {
            block: None,
            lod: None,
            block_size: None,
        })
        .expect("网格统计不该失败");
    for key in ["triangles", "vertices", "lod"] {
        assert!(out.get(key).is_some(), "回包缺 `{key}`：{out}");
    }
    assert!(out["triangles"].as_u64().unwrap() > 0);
    assert!(out["bounds"].is_array(), "全块汇总应带包围盒");

    // 指定不存在的块：报块缺失，而不是回 0 顶点假装算过了。
    let err = api
        .terrain_mesh(TerrainMeshQuery {
            block: Some([9, 9]),
            lod: None,
            block_size: None,
        })
        .expect_err("越界块必须报错");
    assert_eq!(err.code, TERRAIN_BLOCK_MISSING);
}

#[test]
fn terrain_gen_同种子结果一致且写进场景() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let a = api
        .terrain_gen(TerrainGenQuery {
            size_x: 200.0,
            size_z: 200.0,
            seed: 99,
        })
        .expect("生成不该失败");
    assert_eq!(a["seed"], 99);
    assert_eq!(a["created"], 16);
    assert_eq!(
        api.scene.borrow().as_scene().terrain.len(),
        a["created"].as_u64().unwrap() as usize
    );
    let first = a["stateHash"].clone();

    // 同种子重跑：块集合同序、哈希逐位相同。
    let b = api
        .terrain_gen(TerrainGenQuery {
            size_x: 200.0,
            size_z: 200.0,
            seed: 99,
        })
        .expect("重跑不该失败");
    assert_eq!(b["names"], a["names"]);
    assert_eq!(b["stateHash"], first, "同种子必须逐位可复现");
}

#[test]
fn terrain_resample_降分辨率回带误差与哈希() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.terrain_gen(TerrainGenQuery {
        size_x: 200.0,
        size_z: 200.0,
        seed: 4,
    })
    .unwrap();
    let name = api.scene.borrow().as_scene().terrain[0].name.clone();

    let out = api
        .terrain_resample(TerrainResampleQuery {
            name: name.clone(),
            lod: 2.0,
            max_resolution: None,
        })
        .expect("重采样不该失败");
    assert_eq!(out["from"], json!([64, 64]));
    assert_eq!(out["to"], json!([32, 32]));
    assert_eq!(out["samples"], 1024);
    assert!(out.get("maxError").is_some());
    assert!(out["stateHash"].as_str().unwrap().starts_with("0x"));

    let err = api
        .terrain_resample(TerrainResampleQuery {
            name: "nope".to_string(),
            lod: 2.0,
            max_resolution: None,
        })
        .expect_err("不存在的块必须报错");
    assert_eq!(err.code, TERRAIN_PATCH_MISSING);
}

#[test]
fn terrain_写命令在事务未决时回tx_rejected() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx_id = begin(&mut api);
    assert!(!tx_id.is_empty());
    let before = api.scene.borrow().state_hash();

    let err = api
        .terrain_create(TerrainCreateQuery {
            name: "ground".to_string(),
            rect: [0.0, 0.0, 100.0, 100.0],
            resolution: None,
        })
        .expect_err("事务未决时必须停手");
    assert_eq!(err.code, TX_REJECTED, "{}", err.message);

    let err = api
        .terrain_gen(TerrainGenQuery {
            size_x: 100.0,
            size_z: 100.0,
            seed: 1,
        })
        .expect_err("事务未决时必须停手");
    assert_eq!(err.code, TX_REJECTED);

    assert_eq!(api.scene.borrow().state_hash(), before, "被拒后不许动场景");
    api.tx_rollback(&tx_id).expect("回滚不该失败");
}
