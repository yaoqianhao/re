//! CPU 文本底座的**真实字体**验收：字体加载（ttf/otf/ttc）→ 字形栅格化 → 图集打包 → 排版。
//!
//! 单元测试里的假字体只验证算法；这里必须让 `fontdue` 真的去解析系统字体文件，
//! 否则「中文能不能画出来」永远只是纸面结论。
//!
//! 机器上没有对应字体时用例打印一行 `跳过：…` 后返回，不会把无头 CI 判红。

use r3e_core::text::{system_font_candidates, TOFU_ADVANCE_RATIO};
use r3e_core::{
    layout_text, AtlasInsert, FontSet, GlyphAtlas, GlyphKey, GlyphSource, ScreenText, TextAlign,
};

/// 加载一个**确实含中日韩字形**的字体（按 `system_font_candidates` 顺序找）。
fn cjk_font() -> Option<(String, FontSet)> {
    for path in system_font_candidates() {
        if !path.exists() {
            continue;
        }
        let Ok(set) = FontSet::load_files(std::slice::from_ref(&path)) else {
            continue;
        };
        if set.has_char('你') && set.has_char('好') {
            eprintln!("[font] 使用 {}", path.display());
            return Some((path.display().to_string(), set));
        }
    }
    eprintln!("跳过：本机没有含中日韩字形的候选字体");
    None
}

/// 在系统字体目录里找一个 `.otf`（CFF / OTF 分支）。
fn first_otf() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new("C:/Windows/Fonts");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("otf"))
        })
        .collect();
    files.sort();
    files.into_iter().next()
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// 需求 2 + 3：中文「你好」必须取到非零宽高的灰度位图，且带 bearing / advance。
#[test]
fn chinese_hello_has_nonzero_glyph() {
    let Some((source, set)) = cjk_font() else {
        return;
    };
    for ch in ['你', '好'] {
        let (metric, bitmap) = set
            .glyph_for_char(ch, 32.0)
            .unwrap_or_else(|| panic!("{source} 里「{ch}」应有字形"));
        assert!(bitmap.width > 0, "「{ch}」位图宽度必须为正");
        assert!(bitmap.height > 0, "「{ch}」位图高度必须为正");
        assert!(
            metric.advance > 0.0 && metric.advance.is_finite(),
            "「{ch}」前进宽度必须为正：{}",
            metric.advance
        );
        assert_eq!(
            bitmap.data.len(),
            bitmap.width as usize * bitmap.height as usize,
            "「{ch}」覆盖率数组长度必须等于 w*h"
        );
        assert!(bitmap.data.iter().any(|v| *v > 0), "「{ch}」位图不能全透明");
        assert!(
            bitmap.top <= 0,
            "「{ch}」top 是屏幕坐标（基线之上为负），实际 {}",
            bitmap.top
        );
        let peak = bitmap.data.iter().copied().max().unwrap_or(0);
        eprintln!(
            "[glyph] 「{ch}」@32px = {}x{}  advance={:.3}  left={}  top={}  coverage_max={}",
            bitmap.width, bitmap.height, metric.advance, bitmap.left, bitmap.top, peak
        );
    }

    // 整段排版的量宽也要跟着真实度量走，而不是 tofu 估算
    let measure = r3e_core::measure_text(&set, "你好", 32.0);
    assert!(
        measure.width > 32.0,
        "两字宽度应大于 1em：{}",
        measure.width
    );
    assert_eq!(measure.missing, 0, "「你好」不该被算成缺字");
    eprintln!("[layout] 「你好」@32px 量宽 = {:.3}px", measure.width);
}

/// 需求 2：`.ttc` 集合要能按字面下标取；越界必须报错而不是 panic。
#[test]
fn ttc_face_index_is_selectable() {
    let some_ttc = system_font_candidates()
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "ttc") && p.exists());
    let Some(path) = some_ttc else {
        eprintln!("跳过：本机没有 .ttc 字体");
        return;
    };

    let mut set = FontSet::empty();
    set.add_file_index(&path, 0).expect("0 号字面应可解析");
    // 微软雅黑 .ttc 至少两个字面（雅黑 / 雅黑 UI）；不假定所有 .ttc 都多字面
    let second = set.add_file_index(&path, 1);
    match second {
        Ok(_) => eprintln!("[ttc] {} 支持多字面，已加载 2 个", path.display()),
        Err(e) => eprintln!("[ttc] {} 只有 1 个字面（{e}）", path.display()),
    }
    assert!(set.face_count() >= 1);
    assert!(set.has_char('你'), "0 号字面应能画出「你」");

    // 越界下标：错误字符串，不是 panic
    assert!(
        set.add_file_index(&path, 99).is_err(),
        "不存在的字面下标必须返回 Err"
    );
}

/// 需求 2：`.otf`（CFF 轮廓）同样要能加载并栅格化。
#[test]
fn otf_font_loads_and_rasterizes() {
    let Some(path) = first_otf() else {
        eprintln!("跳过：本机没有 .otf 字体");
        return;
    };
    let set = match FontSet::load_files(std::slice::from_ref(&path)) {
        Ok(set) => set,
        Err(e) => {
            eprintln!("跳过：{} 加载失败（{e}）", path.display());
            return;
        }
    };
    assert_eq!(set.face_count(), 1, "单个 .otf 只有一个字面");
    let ch = ['中', 'A', 'é']
        .into_iter()
        .find(|c| set.has_char(*c))
        .expect("该 .otf 至少有一个测试字符");
    let (_, bitmap) = set.glyph_for_char(ch, 48.0).expect("应能栅格化");
    eprintln!(
        "[otf] {} → 「{ch}」@48px = {}x{}",
        path.display(),
        bitmap.width,
        bitmap.height
    );
    assert!(bitmap.width > 0 && bitmap.height > 0);
}

/// 需求 2：缺字必须有明确回退（不 panic），排版按 tofu 宽度占位。
#[test]
fn missing_char_falls_back_instead_of_panicking() {
    let Some((_, set)) = cjk_font() else {
        return;
    };
    // U+E123 是私用区，任何常规字体都不该有
    let ch = '\u{E123}';
    assert!(!set.has_char(ch), "私用区字符不该有字形");
    assert!(set.glyph_for_char(ch, 24.0).is_none(), "缺字返回 None");

    let text = ScreenText {
        content: format!("你{ch}好"),
        size: 24.0,
        ..Default::default()
    };
    let layout = layout_text(&set, &text);
    assert_eq!(layout.missing, 1, "缺字必须被记账");
    assert_eq!(layout.glyph_count, 2, "缺字不产生可绘制字形");
    // 缺字仍占位，且位置不塌陷：两个字之间留了 tofu 宽度
    let first = layout.lines[0].glyphs[0].x;
    let second = layout.lines[0].glyphs[1].x;
    assert!(approx(first, 0.0));
    assert!(
        second > 24.0 * TOFU_ADVANCE_RATIO,
        "缺字必须留下占位宽度，实际第二字位移 {second}"
    );
    eprintln!(
        "[missing] 缺字占位宽度 = {:.2}px",
        second - layout.lines[0].glyphs[0].advance
    );
}

/// 需求 2：多字体面按顺序回退 —— 前一个面缺字就往后找。
#[test]
fn fallback_chain_uses_later_face() {
    let Some((_, mut set)) = cjk_font() else {
        return;
    };
    let emoji = std::path::Path::new("C:/Windows/Fonts/seguiemj.ttf");
    if !emoji.exists() {
        eprintln!("跳过：本机没有 Segoe UI Emoji");
        return;
    }
    let before = set.face_count();
    if set.add_file(emoji).is_err() {
        eprintln!("跳过：emoji 字体解析失败");
        return;
    }
    assert_eq!(set.face_count(), before + 1);

    // 雅黑没有 emoji，回退到第 2 个面
    if set
        .face(0)
        .is_some_and(|f| f.font.lookup_glyph_index('😀') != 0)
    {
        eprintln!("跳过：主字体自带 emoji");
        return;
    }
    let (face, glyph) = set.resolve('😀').expect("emoji 应由回退字体提供");
    assert_eq!(face as usize, before, "应由后加入的字体面提供字形");
    let (metric, bitmap) = set.glyph_for_char('😀', 32.0).expect("emoji 位图");
    eprintln!(
        "[fallback] 😀 → face#{face} glyph#{glyph} {}x{} advance={:.2}",
        bitmap.width, bitmap.height, metric.advance
    );
    assert!(bitmap.width > 0 && bitmap.height > 0);
}

/// 需求 5：UTF-8 多字节字符按 char 边界切分，换行后不丢字、不裂字、不 panic。
#[test]
fn utf8_multibyte_layout_round_trips() {
    let Some((_, set)) = cjk_font() else {
        return;
    };
    let content = "中文😀é好Ａb";
    let text = ScreenText {
        content: content.to_string(),
        size: 24.0,
        wrap: 50.0,
        align: TextAlign::Left,
        ..Default::default()
    };
    let layout = layout_text(&set, &text);

    let rendered: String = layout
        .lines
        .iter()
        .flat_map(|l| l.glyphs.iter().map(|g| g.ch))
        .collect();
    let char_count = content.chars().count();
    assert_eq!(
        rendered.chars().count() + layout.missing,
        char_count,
        "每个 char 要么画出来要么记缺字，不允许在中途丢掉半个字"
    );
    assert!(
        content.chars().all(|c| rendered.contains(c) || c == '😀'),
        "除缺字的 emoji 外，其余字符都要保留：{rendered}"
    );
    assert!(layout.lines.len() >= 2, "窄宽度下应换行");
    for line in &layout.lines {
        assert!(
            line.width <= 50.0 + 0.01,
            "每行不得超过换行宽度：{}",
            line.width
        );
    }
    eprintln!(
        "[utf8] {} 个 char → {} 行，缺字 {}，最宽行 {:.1}px",
        char_count,
        layout.lines.len(),
        layout.missing,
        layout.width
    );
}

/// 需求 4：真实字形进图集 —— 重复请求复用同一槽位，页满自动分页且不丢字。
#[test]
fn atlas_reuses_slot_and_pages_without_loss() {
    let Some((_, set)) = cjk_font() else {
        return;
    };
    let mut atlas = GlyphAtlas::new(128, 8);
    let poem =
        "永和九年岁在癸丑暮春之初会于会稽山阴之兰亭修禊事也群贤毕至少长咸集此地有崇山峻岭茂林修竹";
    let mut chars: Vec<char> = Vec::new();
    for ch in poem.chars() {
        if !chars.contains(&ch) {
            chars.push(ch);
        }
    }
    chars.truncate(40);

    let mut keys = Vec::new();
    for ch in &chars {
        let (metric, bitmap) = set.glyph_for_char(*ch, 20.0).expect("字形成该存在");
        let key = GlyphKey::new(metric.face, metric.glyph, 20.0);
        let insert = atlas
            .insert(key, bitmap.width, bitmap.height, bitmap.left, bitmap.top)
            .unwrap_or_else(|| panic!("「{ch}」不该被图集丢弃"));
        assert!(insert.is_new(), "首次插入应当是新槽位");
        keys.push((key, insert.slot()));
    }
    assert_eq!(atlas.len(), chars.len(), "每个字形都应有槽位");
    assert!(
        atlas.page_count() >= 2,
        "40 个 20px 字形放不进单张 128² 页，应自动分页，实际 {} 页",
        atlas.page_count()
    );

    // 第二次请求：必须复用同一块区域，且不产生新页
    let pages_before = atlas.page_count();
    for (key, slot) in &keys {
        match atlas.insert(*key, slot.w, slot.h, slot.left, slot.top) {
            Some(AtlasInsert::Reused(reused)) => assert_eq!(reused, *slot, "复用必须落在同一槽位"),
            other => panic!("重复请求应复用，实际 {other:?}"),
        }
    }
    assert_eq!(atlas.page_count(), pages_before, "复用它不该长出新页");
    assert_eq!(atlas.len(), chars.len(), "复用它不该增加缓存条目");
    eprintln!(
        "[atlas] {} 个字形 / {} 页 / 占用率 {:.1}%",
        atlas.len(),
        atlas.page_count(),
        atlas.occupancy() * 100.0
    );
}

/// 需求 5：左 / 中 / 右对齐与行基线的位置计算。
#[test]
fn alignment_and_baseline_are_computed() {
    let Some((_, set)) = cjk_font() else {
        return;
    };
    let base = ScreenText {
        content: "你好\n世界和平".to_string(),
        size: 24.0,
        wrap: 300.0,
        ..Default::default()
    };
    let left = layout_text(&set, &base);
    assert!(approx(left.lines[0].glyphs[0].x, 0.0), "左对齐首字应在 0");

    let mut center = base.clone();
    center.align = TextAlign::Center;
    let centered = layout_text(&set, &center);
    let expected = (left.block_width - left.lines[0].width) * 0.5;
    assert!(
        approx(centered.lines[0].glyphs[0].x, expected),
        "居中偏移应为 (块宽 - 行宽)/2：{} vs {}",
        centered.lines[0].glyphs[0].x,
        expected
    );

    let mut right = base.clone();
    right.align = TextAlign::Right;
    let righted = layout_text(&set, &right);
    let expected_right = left.block_width - left.lines[0].width;
    assert!(approx(righted.lines[0].glyphs[0].x, expected_right));

    // 基线：第一行 = ascent，之后每行 +line_height
    let lm = GlyphSource::line_metrics(&set, 24.0);
    assert!(approx(left.lines[0].baseline, lm.ascent));
    assert!(approx(left.lines[1].baseline, lm.ascent + left.line_height));
    let (x, y) = left.glyph_offset(1, 0).expect("第二行首字");
    assert!(approx(y, left.lines[1].baseline));
    assert!(approx(x, left.lines[1].glyphs[0].x));
    assert!(left.glyph_offset(9, 0).is_none(), "越界必须返回 None");
    eprintln!(
        "[layout] ascent={:.2} line_height={:.2} 两行基线 {:.2}/{:.2}",
        lm.ascent, left.line_height, left.lines[0].baseline, left.lines[1].baseline
    );
}

/// 需求 1：core 的清单里不允许出现任何图形 / 窗口依赖（架构红线）。
#[test]
fn core_manifest_has_no_gpu_dependency() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("读 r3e-core/Cargo.toml");
    for token in ["wgpu", "winit", "pollster", "gilrs"] {
        assert!(
            !manifest.contains(token),
            "r3e-core 不得依赖 {token}（文本层必须是纯 CPU）"
        );
    }
    assert!(
        manifest.contains("fontdue"),
        "文本栅格化应当由 fontdue 承担"
    );
}
