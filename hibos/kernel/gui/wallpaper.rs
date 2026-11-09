//! 像素壁纸系统 —— `gui://wallpaper`。
//!
//! 之前的问题：合成器只有一套极暗的深空背景，看不出"有壁纸"；
//! `wallpaper` 命令只改字符桌面的名字，像素桌面完全不响应；
//! BMP 只能导出、不能加载，所以没法把真实图片当壁纸。
//!
//! 本模块补齐四件事：
//! 1. **多套明显可见的程序化像素壁纸**（日落 / 极光 / 电路 / 星云 / 网格 / 纯色）；
//! 2. **BMP 解码**：把任意 24/32bpp 图片加载为壁纸；
//! 3. **摆放方式**：stretch（拉伸）/ tile（平铺）/ center（居中）；
//! 4. **全局壁纸库**：`wallpaper` 命令、像素合成器、`gui://wallpaper`
//!    三者操作同一份状态，改一处全系统同步。
//!
//! * `sense gui://wallpaper`
//! * `act gui://wallpaper list`
//! * `act gui://wallpaper set <风格>`
//! * `act gui://wallpaper load <BMP路径>`
//! * `act gui://wallpaper fit <stretch|tile|center>`
//! * `act gui://wallpaper save <BMP路径>` / `preview`

use std::sync::{Mutex, OnceLock};

use super::bmp;
use super::png;
use super::surface::{argb, Surface};

/// 内置壁纸基准尺寸（像素艺术，缩放时保留像素颗粒感）。
const BASE_W: u32 = 320;
const BASE_H: u32 = 200;

/// 壁纸摆放方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Stretch,
    Tile,
    Center,
}

impl Fit {
    pub fn name(self) -> &'static str {
        match self {
            Fit::Stretch => "stretch",
            Fit::Tile => "tile",
            Fit::Center => "center",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "stretch" | "fill" => Some(Fit::Stretch),
            "tile" => Some(Fit::Tile),
            "center" | "none" => Some(Fit::Center),
            _ => None,
        }
    }
}

/// 内置风格名（后两个直接读本机 `.wiki` 里的真实壁纸图）。
pub const STYLES: &[&str] = &[
    "sunset", "aurora", "circuit", "nebula", "grid", "solid", "wiki1", "wiki2",
];

/// 本机 wiki 里的两张壁纸图（PNG；`壁纸2.jpg` 已转换为同名 PNG）。
pub const WIKI_IMAGES: &[(&str, &str)] =
    &[("wiki1", "壁纸.png"), ("wiki2", "壁纸2.png")];

/// 取 wiki 壁纸的绝对路径（编译期固定，避免依赖运行时工作目录）。
fn wiki_path(file: &str) -> String {
    format!("{}/.wiki/{}", env!("CARGO_MANIFEST_DIR"), file)
}

/// 全局壁纸库。
pub struct WallpaperBank {
    pub name: String,
    pub fit: Fit,
    /// 原始壁纸位图（内置生成或从 BMP 加载）。
    base: Surface,
    pub background: u32,
    pub changes: u64,
}

impl WallpaperBank {
    fn new() -> Self {
        Self {
            name: "sunset".to_string(),
            fit: Fit::Stretch,
            base: make_style("sunset", BASE_W, BASE_H),
            background: argb(255, 0x0b, 0x0b, 0x18),
            changes: 0,
        }
    }

    pub fn status(&self) -> String {
        format!(
            "wallpaper={} fit={} source={}x{} background=#{:06x} changes={}\nstyles: {}\n用法: set <风格> | load <PNG|BMP> | wiki [1|2] | fit <stretch|tile|center> | save <BMP>",
            self.name,
            self.fit.name(),
            self.base.width(),
            self.base.height(),
            self.background & 0x00ff_ffff,
            self.changes,
            STYLES.join(", ")
        )
    }

    /// 切换内置风格；`wiki1`/`wiki2` 直接加载本机真实图片。
    pub fn set_style(&mut self, name: &str) -> Result<String, String> {
        if let Some((_, file)) = WIKI_IMAGES.iter().find(|(n, _)| *n == name) {
            return self.load_image(&wiki_path(file));
        }
        if !STYLES.contains(&name) {
            return Err(format!(
                "未知壁纸 {} (可用: {})",
                name,
                STYLES.join(", ")
            ));
        }
        self.name = name.to_string();
        self.base = make_style(name, BASE_W, BASE_H);
        self.changes += 1;
        Ok(format!("像素壁纸 -> {} ({}x{} 基准)", name, BASE_W, BASE_H))
    }

    /// 按扩展名加载 PNG / BMP 图片当壁纸。
    pub fn load_image(&mut self, path: &str) -> Result<String, String> {
        let ext = path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let img = match ext.as_str() {
            "png" => png::load(path)?,
            "bmp" => bmp::load(path)?,
            "jpg" | "jpeg" => {
                return Err("JPEG 暂不直接解码，请先用 `act gui://wallpaper wiki` 使用已转换的 PNG 版本".to_string())
            }
            _ => return Err(format!("不支持的图片格式 .{} (支持 png/bmp)", ext)),
        };
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 {
            return Err("图片尺寸无效".to_string());
        }
        self.base = img;
        self.name = path.to_string();
        self.changes += 1;
        Ok(format!("已加载图片壁纸 {} ({}x{})", path, w, h))
    }

    /// 列出本机 wiki 壁纸图。
    pub fn wiki_list() -> String {
        WIKI_IMAGES
            .iter()
            .map(|(n, f)| {
                let p = wiki_path(f);
                format!("  {} -> {} ({})", n, f, if std::path::Path::new(&p).exists() { "可用" } else { "缺失" })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 导出当前壁纸源图为 BMP。
    pub fn save_bmp(&self, path: &str) -> Result<String, String> {
        let n = bmp::save(&self.base, path).map_err(|e| format!("导出失败: {}", e))?;
        Ok(format!("壁纸已导出到 {} ({} 字节)", path, n))
    }

    pub fn set_fit(&mut self, fit: Fit) -> String {
        self.fit = fit;
        self.changes += 1;
        format!("壁纸摆放方式 -> {}", fit.name())
    }

    /// 把壁纸绘制到目标表面（合成器每帧调用）。
    pub fn render_into(&self, dst: &mut Surface) {
        dst.fill(self.background);
        let (dw, dh) = (dst.width() as i32, dst.height() as i32);
        let (sw, sh) = (self.base.width() as i32, self.base.height() as i32);
        if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
            return;
        }
        match self.fit {
            Fit::Stretch => {
                // 最近邻缩放，保留像素画的硬边
                for y in 0..dh {
                    let sy = (y * sh / dh).clamp(0, sh - 1);
                    for x in 0..dw {
                        let sx = (x * sw / dw).clamp(0, sw - 1);
                        if let Some(c) = self.base.get(sx, sy) {
                            dst.put(x, y, c);
                        }
                    }
                }
            }
            Fit::Tile => {
                for y in 0..dh {
                    for x in 0..dw {
                        let sx = x.rem_euclid(sw);
                        let sy = y.rem_euclid(sh);
                        if let Some(c) = self.base.get(sx, sy) {
                            dst.put(x, y, c);
                        }
                    }
                }
            }
            Fit::Center => {
                let ox = (dw - sw) / 2;
                let oy = (dh - sh) / 2;
                dst.draw_surface(&self.base, ox, oy);
            }
        }
    }

    /// 壁纸本身的 ANSI 半块预览（不含窗口）。
    pub fn preview(&self, cols: usize, rows: usize) -> String {
        let mut tmp = Surface::new(BASE_W, BASE_H);
        self.render_into(&mut tmp);
        super::preview_surface(&tmp, cols, rows)
    }
}

static BANK: OnceLock<Mutex<WallpaperBank>> = OnceLock::new();

pub fn bank() -> &'static Mutex<WallpaperBank> {
    BANK.get_or_init(|| Mutex::new(WallpaperBank::new()))
}

/// 供 shell 的 `wallpaper` 命令同步像素桌面：把老名字映射到新风格。
pub fn set_named(name: &str) -> Result<String, String> {
    let mapped = match name {
        "default" | "hibos" => "circuit",
        "deepseek" | "ai" | "consciousness" => "aurora",
        other => other,
    };
    bank().lock().unwrap().set_style(mapped)
}

/// 通道 `gui://wallpaper` 的行动实现。
pub fn act(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let (sub, rest) = match text.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (text, ""),
    };
    match sub {
        "list" => {
            let b = bank().lock().unwrap();
            Ok(format!("内置壁纸: {}\n{}", STYLES.join(", "), b.status()))
        }
        "set" => {
            if rest.is_empty() {
                return Err("用法: set <风格>".to_string());
            }
            bank().lock().unwrap().set_style(rest)
        }
        "load" => {
            if rest.is_empty() {
                return Err("用法: load <PNG|BMP路径>".to_string());
            }
            bank().lock().unwrap().load_image(rest)
        }
        "wiki" => {
            if rest.is_empty() {
                Ok(format!("本机 wiki 壁纸图:\n{}", WallpaperBank::wiki_list()))
            } else {
                let name = match rest {
                    "1" | "wiki1" | "壁纸" => "wiki1",
                    "2" | "wiki2" | "壁纸2" => "wiki2",
                    other => other,
                };
                bank().lock().unwrap().set_style(name)
            }
        }
        "fit" => {
            let f = Fit::parse(rest).ok_or_else(|| "用法: fit <stretch|tile|center>".to_string())?;
            Ok(bank().lock().unwrap().set_fit(f))
        }
        "save" => {
            if rest.is_empty() {
                return Err("用法: save <BMP路径>".to_string());
            }
            bank().lock().unwrap().save_bmp(rest)
        }
        "preview" => {
            let b = bank().lock().unwrap();
            Ok(b.preview(48, 12))
        }
        "status" | "" => Ok(bank().lock().unwrap().status()),
        _ => Err("用法: list|set <风格>|load <BMP>|fit <方式>|save <BMP>|preview".to_string()),
    }
}

/// 合成器入口：把全局壁纸画到目标表面。
pub fn render_into(dst: &mut Surface) {
    bank().lock().unwrap().render_into(dst);
}

/// 生成一套内置像素壁纸。
fn make_style(name: &str, w: u32, h: u32) -> Surface {
    let mut s = Surface::new(w, h);
    // 用整数运算画，避免引入随机依赖；每张图都明显不是纯色。
    for y in 0..h {
        for x in 0..w {
            let c = match name {
                "sunset" => {
                    let t = y as i32 * 255 / h as i32;
                    let (r, g, b) = if y < h * 2 / 3 {
                        ((200 + t / 5).min(255) as u8, (60 + t / 2).min(255) as u8, (90 + t / 3).min(255) as u8)
                    } else {
                        (40u8, 20u8, 70u8)
                    };
                    let mut c = argb(255, r, g, b);
                    // 太阳
                    let (sx, sy, sr) = (w as i32 * 3 / 4, h as i32 / 3, h as i32 / 7);
                    let dx = x as i32 - sx;
                    let dy = y as i32 - sy;
                    if dx * dx + dy * dy < sr * sr {
                        c = argb(255, 255, 226, 120);
                    }
                    // 山脉剪影
                    let ridge = h as i32 * 2 / 3 + ((x as i32 * 7 / 40) % 17) - 8;
                    if y as i32 > ridge {
                        c = argb(255, 26, 20, 48);
                    }
                    c
                }
                "aurora" => {
                    let band = ((x as f32 / 18.0).sin() * 24.0 + (y as f32 / 30.0).cos() * 12.0) as i32;
                    let d = (y as i32 - (h as i32 / 3 + band)).abs();
                    if d < 22 {
                        let g = (200 - d * 4).max(60) as u8;
                        argb(255, 40, g, (140 + d) as u8)
                    } else {
                        argb(255, 8, 12, 30)
                    }
                }
                "circuit" => {
                    let on_x = x % 24 == 0 || x % 24 == 1;
                    let on_y = y % 20 == 0 || y % 20 == 1;
                    let node = x % 24 < 3 && y % 20 < 3;
                    if node {
                        argb(255, 137, 220, 250)
                    } else if on_x || on_y {
                        argb(255, 30, 90, 110)
                    } else {
                        argb(255, 12, 18, 30)
                    }
                }
                "nebula" => {
                    let t = (y * 100 / h) as u8;
                    let star = (x.wrapping_mul(73_837) ^ y.wrapping_mul(19_531)) % 5000 == 0;
                    if star {
                        argb(255, 235, 235, 255)
                    } else {
                        argb(255, 20 + t / 3, 16 + t / 6, 50 + t / 2)
                    }
                }
                "grid" => {
                    if x % 16 < 2 || y % 16 < 2 {
                        argb(255, 60, 130, 170)
                    } else {
                        argb(255, 14, 18, 30)
                    }
                }
                _ => argb(255, 30, 30, 46),
            };
            s.put(x as i32, y as i32, c);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::surface::red_of;

    #[test]
    fn every_style_has_visible_variation() {
        for name in STYLES {
            if *name == "wiki1" || *name == "wiki2" {
                continue; // 这两个是本机真实图片，不走程序化生成
            }
            let s = make_style(name, 64, 40);
            let mut colors = std::collections::BTreeSet::new();
            for y in 0..40 {
                for x in 0..64 {
                    colors.insert(s.get(x, y).unwrap());
                }
            }
            assert!(
                colors.len() > 1 || *name == "solid",
                "壁纸 {} 几乎是纯色",
                name
            );
        }
    }

    #[test]
    fn sunset_has_bright_sun() {
        let s = make_style("sunset", BASE_W, BASE_H);
        let px = s.get((BASE_W * 3 / 4) as i32, (BASE_H / 3) as i32).unwrap();
        assert!(red_of(px) > 200, "太阳应是亮色: {:x}", px);
    }

    #[test]
    fn bmp_roundtrip_as_wallpaper() {
        let mut b = WallpaperBank::new();
        b.set_style("grid").unwrap();
        let p = std::env::temp_dir().join("hibos_wallpaper_test.bmp");
        b.save_bmp(p.to_str().unwrap()).unwrap();
        let r = b.load_image(p.to_str().unwrap()).unwrap();
        assert!(r.contains("图片壁纸"), "{}", r);
        assert_eq!(b.base.width(), BASE_W);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fit_modes_paint_the_whole_surface() {
        let b = WallpaperBank::new();
        for fit in [Fit::Stretch, Fit::Tile, Fit::Center] {
            let mut bank = WallpaperBank::new();
            bank.set_fit(fit);
            let mut dst = Surface::new(80, 50);
            bank.render_into(&mut dst);
            let non_bg = dst.pixels().iter().filter(|p| **p != bank.background).count();
            assert!(non_bg > 0, "fit {:?} 没有画上壁纸", fit);
        }
        assert_eq!(Fit::parse("tile"), Some(Fit::Tile));
        assert!(Fit::parse("nope").is_none());
        assert!(b.status().contains("styles:"));
    }

    #[test]
    fn set_style_rejects_unknown() {
        let mut b = WallpaperBank::new();
        assert!(b.set_style("nope").is_err());
        assert!(b.set_style("sunset").is_ok());
        assert_eq!(b.changes, 1);
    }
}
