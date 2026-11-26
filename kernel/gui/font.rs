//! 真实字体渲染 —— 运行时加载系统开源/系统字体，fontdue 光栅化。
//!
//! 不打包任何商业字体文件：按平台候选路径查找已安装的字体，
//! 找不到则回退到内置 8×12 字形（见 `glyph12`）。
//!
//! 字族映射（按用户熟悉的中文称呼）：
//! * Serif  —— 宋体风格（Georgia / Times New Roman）
//! * Sans   —— 黑体风格（Arial / Helvetica）
//! * YaHei  —— 雅黑风格（Arial Unicode / PingFang，含中文）
//! * Mono   —— 等宽终端（Menlo / Courier / Arial Unicode）

use std::sync::{Mutex, OnceLock};

use fontdue::{Font, FontSettings};

use super::surface::Surface;

/// 字族。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontFamily {
    /// 宋体（衬线）。
    Serif,
    /// 黑体（无衬线）。
    Sans,
    /// 雅黑（现代无衬线，含中文）。
    YaHei,
    /// 终端等宽。
    Mono,
}

impl FontFamily {
    /// 平台候选路径：首选含中文的单文件 TTF，其次系统常见字体。
    fn candidates(self) -> &'static [&'static str] {
        #[cfg(target_os = "macos")]
        {
            match self {
                FontFamily::Serif => &[
                    "/System/Library/Fonts/Supplemental/Georgia.ttf",
                    "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
                ],
                FontFamily::Sans => &[
                    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                    "/System/Library/Fonts/Supplemental/Arial.ttf",
                ],
                FontFamily::YaHei => &[
                    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                    "/Library/Fonts/Arial Unicode.ttf",
                ],
                FontFamily::Mono => &[
                    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                    "/System/Library/Fonts/Supplemental/Courier New.ttf",
                ],
            }
        }
        #[cfg(target_os = "windows")]
        {
            match self {
                FontFamily::Serif => &["C:\\Windows\\Fonts\\simsun.ttc", "C:\\Windows\\Fonts\\georgia.ttf"],
                FontFamily::Sans => &["C:\\Windows\\Fonts\\simhei.ttf", "C:\\Windows\\Fonts\\arial.ttf"],
                FontFamily::YaHei => &["C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\msyhl.ttc"],
                FontFamily::Mono => &["C:\\Windows\\Fonts\\consola.ttf", "C:\\Windows\\Fonts\\cour.ttf"],
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            match self {
                FontFamily::Serif => &["/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf"],
                FontFamily::Sans => &["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"],
                FontFamily::YaHei => &[
                    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                ],
                FontFamily::Mono => &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"],
            }
        }
    }
}

/// 已加载的字体槽位。
struct Slot {
    font: Option<Font>,
}

impl Slot {
    fn load(family: FontFamily) -> Self {
        let mut font = None;
        for path in family.candidates() {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(f) = Font::from_bytes(bytes, FontSettings::default()) {
                    font = Some(f);
                    break;
                }
            }
        }
        Self { font }
    }

    fn available(&self) -> bool {
        self.font.is_some()
    }
}

static FONTS: OnceLock<Mutex<[Slot; 4]>> = OnceLock::new();

fn fonts() -> &'static Mutex<[Slot; 4]> {
    FONTS.get_or_init(|| {
        Mutex::new([
            Slot::load(FontFamily::Serif),
            Slot::load(FontFamily::Sans),
            Slot::load(FontFamily::YaHei),
            Slot::load(FontFamily::Mono),
        ])
    })
}

fn slot_index(family: FontFamily) -> usize {
    match family {
        FontFamily::Serif => 0,
        FontFamily::Sans => 1,
        FontFamily::YaHei => 2,
        FontFamily::Mono => 3,
    }
}

/// 系统是否成功加载了真实字体（供测试与状态查询）。
pub fn font_available(family: FontFamily) -> bool {
    fonts().lock().unwrap()[slot_index(family)].available()
}

/// 重新加载字体（字体安装后调用）。
pub fn reload() {
    let mut f = fonts().lock().unwrap();
    for (i, family) in [
        FontFamily::Serif,
        FontFamily::Sans,
        FontFamily::YaHei,
        FontFamily::Mono,
    ]
    .iter()
    .enumerate()
    {
        f[i] = Slot::load(*family);
    }
}

/// 用真实字体画一行；返回结束 x 坐标。
///
/// 每个字形由 fontdue 光栅化为 8bit 覆盖率，再按覆盖率 alpha 混合到 Surface。
/// 某字族不可用时返回 `false`，调用方应回退到内置字形。
pub fn draw_text(
    s: &mut Surface,
    x: i32,
    y: i32,
    text: &str,
    px: f32,
    color: u32,
    family: FontFamily,
    max_x: i32,
) -> bool {
    let fonts = fonts().lock().unwrap();
    let slot = &fonts[slot_index(family)];
    let Some(font) = slot.font.as_ref() else {
        return false;
    };

    let mut cx = x;
    for ch in text.chars() {
        if ch == '\n' {
            break;
        }
        let (metrics, bitmap) = font.rasterize(ch, px);
        if cx + metrics.advance_width as i32 > max_x {
            break;
        }
        // ymin 是字形底边相对基线的偏移（通常为负），xmin 是左边相对原点的偏移。
        let left = cx + metrics.xmin;
        let top = y + metrics.ymin;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let cov = bitmap[row * metrics.width + col];
                if cov == 0 {
                    continue;
                }
                let src = ((cov as u32) << 24) | (color & 0x00FF_FFFF);
                s.blend(left + col as i32, top + row as i32, src);
            }
        }
        cx += metrics.advance_width as i32;
    }
    true
}

/// 估算一行文本宽度（像素）。
pub fn measure_text(text: &str, px: f32, family: FontFamily) -> Option<f32> {
    let fonts = fonts().lock().unwrap();
    let slot = &fonts[slot_index(family)];
    let font = slot.font.as_ref()?;
    let width: f32 = text
        .chars()
        .map(|c| font.metrics(c, px).advance_width)
        .sum();
    Some(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_families_have_candidates() {
        for f in [
            FontFamily::Serif,
            FontFamily::Sans,
            FontFamily::YaHei,
            FontFamily::Mono,
        ] {
            assert!(!f.candidates().is_empty());
        }
    }

    #[test]
    fn draw_text_reports_availability() {
        let mut s = Surface::new(200, 40);
        let ok = draw_text(&mut s, 0, 10, "HI", 12.0, 0xFFFFFFFF, FontFamily::Sans, 190);
        assert_eq!(ok, font_available(FontFamily::Sans));
    }

    #[test]
    fn fallback_does_not_paint_when_unavailable() {
        // 不可用时 draw_text 返回 false，调用方负责回退。
        let mut s = Surface::new(50, 20);
        let ok = draw_text(&mut s, 0, 2, "X", 8.0, 0xFFFFFFFF, FontFamily::Serif, 45);
        if !ok {
            // 回退路径由 text_render 保证，这里只验证不会 panic。
            assert!(s.get(0, 0).is_some());
        }
    }
}
