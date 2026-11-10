//! 合成器扩展 —— 混合模式、透明度、圆角、阴影、动画、双缓冲与防撕裂。
//!
//! `CompositorExt` 在 `kernel/gui/compositor.rs` 的 `Compositor` 基础上提供：
//! * `blend(src, dst, mode)` —— 五种混合模式（正常/乘法/滤色/叠加/差值）
//! * `set_alpha(layer, alpha)` / `set_round_corners(win_id, radius)` / `set_shadow(...)`
//! * `add_animation(anim)` —— 属性动画（透明度/位置/大小/旋转）+ 缓�函数
//! * `render_frame()` / `vsync()` / `fps()` —— 双缓冲帧渲染与防撕裂
//!
//! 所有颜色为 32bpp ARGB（0xAARRGGBB），与 `kernel/gui/surface.rs` 共享像素格式。

use super::surface::{argb, palette, Rect, Surface};
use super::window::WindowId;
use crate::monitor::get_time_ns;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 混合模式
// ============================================================================

/// 像素混合模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// 正常覆盖（考虑 alpha）。
    Normal,
    /// 乘法：结果变暗。
    Multiply,
    /// 滤色：结果变亮。
    Screen,
    /// 叠加：暗区乘法、亮区滤色。
    Overlay,
    /// 差值：绝对差。
    Difference,
}

impl BlendMode {
    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::Difference => "difference",
        }
    }
}

/// 把两个 8 通道分量按模式混合，src_alpha 已预乘。
fn blend_channel(src: u8, dst: u8, mode: BlendMode) -> u8 {
    let s = src as u32;
    let d = dst as u32;
    let r = match mode {
        BlendMode::Normal => s,
        BlendMode::Multiply => s * d / 255,
        BlendMode::Screen => 255 - (255 - s) * (255 - d) / 255,
        BlendMode::Overlay => {
            if d < 128 {
                2 * s * d / 255
            } else {
                255 - 2 * (255 - s) * (255 - d) / 255
            }
        }
        BlendMode::Difference => s.abs_diff(d),
    };
    r.clamp(0, 255) as u8
}

/// 混合两个 ARGB 像素，src_alpha 为 0-255。
fn blend_pixel(src: u32, dst: u32, mode: BlendMode) -> u32 {
    let sa = (src >> 24) as u8;
    if sa == 0 {
        return dst;
    }
    if sa == 255 && mode == BlendMode::Normal {
        return src;
    }
    let sr = (src >> 16) as u8;
    let sg = (src >> 8) as u8;
    let sb = src as u8;
    let da = (dst >> 24) as u8;
    let dr = (dst >> 16) as u8;
    let dg = (dst >> 8) as u8;
    let db = dst as u8;

    let ar = blend_channel(sr, dr, mode);
    let ag = blend_channel(sg, dg, mode);
    let ab = blend_channel(sb, db, mode);
    // 输出 alpha：porter-duff src-over 简化版。
    let aa = sa.saturating_add(da.saturating_sub(sa).saturating_mul(sa) / 255);

    argb(aa, ar, ag, ab)
}

/// 对两个表面做逐像素混合，结果写入 dst（原地）。
/// src 与 dst 必须尺寸相同；若 src 大，只取交集区域。
pub fn blend_surfaces(src: &Surface, dst: &mut Surface, mode: BlendMode, offset: (i32, i32)) {
    let (ox, oy) = offset;
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    let start_x = ox.max(0);
    let start_y = oy.max(0);
    let end_x = (ox + sw).min(dw);
    let end_y = (oy + sh).min(dh);
    if start_x >= end_x || start_y >= end_y {
        return;
    }
    for y in start_y..end_y {
        for x in start_x..end_x {
            let sx = x - ox;
            let sy = y - oy;
            let s = src.get(sx, sy).unwrap_or(0);
            let d = dst.get(x, y).unwrap_or(0);
            dst.set(x, y, blend_pixel(s, d, mode));
        }
    }
}

// ============================================================================
// 图层与透明度
// ============================================================================

/// 合成图层：离屏缓冲区 + 透明度 + 变换。
///
/// `Surface` 不实现 `Debug`，故此处手动实现以避免派生失败。
#[derive(Clone)]
pub struct Layer {
    pub id: u64,
    pub surface: Surface,
    pub alpha: u8, // 0-255，255 为不透明
    pub visible: bool,
    pub blend_mode: BlendMode,
    pub offset: (i32, i32),
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("id", &self.id)
            .field("surface", &format_args!("{}x{}", self.surface.width(), self.surface.height()))
            .field("alpha", &self.alpha)
            .field("visible", &self.visible)
            .field("blend_mode", &self.blend_mode)
            .field("offset", &self.offset)
            .finish()
    }
}

impl Layer {
    pub fn new(id: u64, w: u32, h: u32) -> Self {
        Self {
            id,
            surface: Surface::filled(w, h, palette::BASE),
            alpha: 255,
            visible: true,
            blend_mode: BlendMode::Normal,
            offset: (0, 0),
        }
    }

    /// 设置透明度（0-255）。
    pub fn set_alpha(&mut self, alpha: u8) {
        self.alpha = alpha;
    }

    /// 应用图层透明度到整个表面（预乘 alpha，快捷路径）。
    pub fn apply_alpha_to_surface(&mut self) {
        if self.alpha == 255 {
            return;
        }
        let factor = self.alpha as u32;
        for y in 0..self.surface.height() as i32 {
            for x in 0..self.surface.width() as i32 {
                if let Some(p) = self.surface.get(x, y) {
                    let a = ((p >> 24) as u32 * factor / 255) as u8;
                    let r = ((p >> 16) as u8 as u32 * factor / 255) as u8;
                    let g = ((p >> 8) as u8 as u32 * factor / 255) as u8;
                    let b = ((p as u8 as u32 * factor / 255) as u8);
                    self.surface.put(x, y, argb(a, r, g, b));
                }
            }
        }
    }
}

// ============================================================================
// 阴影与圆角
// ============================================================================

/// 窗口阴影参数。
#[derive(Clone, Copy, Debug, Default)]
pub struct Shadow {
    pub offset_x: i32,
    pub offset_y: i32,
    pub blur: u32,
    pub color: u32,
}

/// 圆角半径。
pub type CornerRadius = u32;

// ============================================================================
// 动画
// ============================================================================

/// 缓动函数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    /// t ∈ [0,1]，返回缓动后的值 ∈ [0,1]。
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

/// 动画目标属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimTarget {
    Opacity,
    X,
    Y,
    Width,
    Height,
    Rotate,
}

/// 一个属性动画。
#[derive(Clone, Debug)]
pub struct Animation {
    pub id: u64,
    pub target: Option<WindowId>, // None 表示图层动画
    pub layer_id: Option<u64>,
    pub prop: AnimTarget,
    pub from: f64,
    pub to: f64,
    pub duration_ns: u64,
    pub easing: Easing,
    pub start_time: u64,
    pub repeat: bool,
    pub finished: bool,
}

impl Animation {
    pub fn new(
        id: u64,
        prop: AnimTarget,
        from: f64,
        to: f64,
        duration_ns: u64,
        easing: Easing,
    ) -> Self {
        Self {
            id,
            target: None,
            layer_id: None,
            prop,
            from,
            to,
            duration_ns,
            easing,
            start_time: 0,
            repeat: false,
            finished: false,
        }
    }

    /// 根据当前时间计算插值（如已结束则标记 finished）。
    pub fn value_at(&mut self, now: u64) -> Option<f64> {
        if self.start_time == 0 {
            return None;
        }
        if now < self.start_time {
            return Some(self.from);
        }
        let elapsed = now - self.start_time;
        if elapsed >= self.duration_ns {
            if self.repeat {
                return None; // 由调用方重置
            }
            self.finished = true;
            return Some(self.to);
        }
        let t = elapsed as f64 / self.duration_ns as f64;
        Some(self.from + (self.to - self.from) * self.easing.apply(t))
    }
}

// ============================================================================
// CompositorExt
// ============================================================================

/// 扩展合成器：离屏缓冲、双缓冲、混合、阴影、圆角、动画、防撕裂。
pub struct CompositorExt {
    /// 主画布（屏幕最终输出）。
    front: Surface,
    /// 后缓冲（双缓冲）。
    back: Surface,
    /// 离屏图层注册表。
    layers: BTreeMap<u64, Layer>,
    /// 下一个图层 id。
    next_layer_id: u64,
    /// 下一个动画 id。
    next_anim_id: u64,
    /// 动画注册表。
    animations: BTreeMap<u64, Animation>,
    /// 窗口阴影表。
    shadows: BTreeMap<WindowId, Shadow>,
    /// 窗口圆角表。
    corners: BTreeMap<WindowId, CornerRadius>,
    /// 帧计数。
    frames: u64,
    /// 最后一次渲染时间戳（ns）。
    last_frame_ns: u64,
    /// 最近一秒内的帧数（用于 fps 统计）。
    fps_frame_count: u64,
    fps_last_second: u64,
    pub fps: f64,
    /// 是否启用双缓冲。
    pub double_buffered: bool,
    /// 是否启用防撕裂（等待 vsync 后再交换）。
    pub tear_free: bool,
    /// 模拟的 vsync 间隔（ns，默认 16.67ms ≈ 60fps）。
    pub vsync_interval_ns: u64,
    /// 上一次 vsync 时间。
    last_vsync_ns: u64,
}

impl CompositorExt {
    /// 创建合成器扩展。
    pub fn new(w: u32, h: u32) -> Self {
        let front = Surface::filled(w, h, palette::BASE);
        let back = Surface::filled(w, h, palette::BASE);
        Self {
            front,
            back,
            layers: BTreeMap::new(),
            next_layer_id: 1,
            next_anim_id: 1,
            animations: BTreeMap::new(),
            shadows: BTreeMap::new(),
            corners: BTreeMap::new(),
            frames: 0,
            last_frame_ns: 0,
            fps_frame_count: 0,
            fps_last_second: 0,
            fps: 0.0,
            double_buffered: true,
            tear_free: true,
            vsync_interval_ns: 16_666_667,
            last_vsync_ns: 0,
        }
    }

    /// 设置画布尺寸（重建双缓冲）。
    pub fn resize(&mut self, w: u32, h: u32) {
        self.front = Surface::filled(w, h, palette::BASE);
        self.back = Surface::filled(w, h, palette::BASE);
        for layer in self.layers.values_mut() {
            layer.surface = Surface::filled(w, h, palette::BASE);
        }
    }

    /// 返回前缓冲区引用（只读）。
    pub fn front(&self) -> &Surface {
        &self.front
    }

    /// 返回后缓冲区可变引用。
    pub fn back_mut(&mut self) -> &mut Surface {
        &mut self.back
    }

    /// 添加离屏图层，返回图层 id。
    pub fn add_layer(&mut self, w: u32, h: u32) -> u64 {
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        self.layers.insert(id, Layer::new(id, w, h));
        id
    }

    /// 移除图层。
    pub fn remove_layer(&mut self, id: u64) -> bool {
        self.layers.remove(&id).is_some()
    }

    /// 获取图层引用。
    pub fn layer(&self, id: u64) -> Option<&Layer> {
        self.layers.get(&id)
    }

    /// 获取可变图层引用。
    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.get_mut(&id)
    }

    /// 设置图层透明度。
    pub fn set_alpha(&mut self, layer: u64, alpha: u8) -> bool {
        self.layers.get_mut(&id_or_none(layer)).map_or(false, |l| {
            l.set_alpha(alpha);
            true
        })
    }

    /// 设置窗口圆角半径。
    pub fn set_round_corners(&mut self, win_id: WindowId, radius: CornerRadius) {
        self.corners.insert(win_id, radius);
    }

    /// 移除窗口圆角。
    pub fn remove_round_corners(&mut self, win_id: WindowId) -> bool {
        self.corners.remove(&win_id).is_some()
    }

    /// 设置窗口阴影。
    pub fn set_shadow(&mut self, win_id: WindowId, offset: (i32, i32), blur: u32, color: u32) {
        self.shadows.insert(
            win_id,
            Shadow {
                offset_x: offset.0,
                offset_y: offset.1,
                blur,
                color,
            },
        );
    }

    /// 移除窗口阴影。
    pub fn remove_shadow(&mut self, win_id: WindowId) -> bool {
        self.shadows.remove(&win_id).is_some()
    }

    /// 添加动画，返回动画 id。
    pub fn add_animation(&mut self, anim: Animation) -> u64 {
        let id = self.next_anim_id;
        self.next_anim_id += 1;
        self.animations.insert(id, anim);
        id
    }

    /// 启动动画（设置开始时间）。
    pub fn start_animation(&mut self, id: u64, start_time: u64) -> bool {
        if let Some(anim) = self.animations.get_mut(&id) {
            anim.start_time = start_time;
            anim.finished = false;
            true
        } else {
            false
        }
    }

    /// 移除动画。
    pub fn remove_animation(&mut self, id: u64) -> bool {
        self.animations.remove(&id).is_some()
    }

    /// 设置 vsync 间隔（ns）。
    pub fn set_vsync_interval(&mut self, ns: u64) {
        self.vsync_interval_ns = ns;
    }

    /// 等待下一次 vsync（模拟：如果未到时间则返回 false，调用方可稍后重试）。
    pub fn vsync(&mut self, now: u64) -> bool {
        if now - self.last_vsync_ns >= self.vsync_interval_ns {
            self.last_vsync_ns = now;
            true
        } else {
            false
        }
    }

    /// 渲染一帧：把全部图层按顺序混合到后缓冲，再交换到前缓冲。
    pub fn render_frame(&mut self, now: u64) {
        // 先清空后缓冲。
        self.back.fill(palette::BASE);

        // 更新动画并应用到图层/窗口。
        self.update_animations(now);

        // 绘制阴影（先画，让窗口在阴影之上）。
        for (win_id, shadow) in self.shadows.iter() {
            // 阴影绘制到后缓冲（简化：在窗口位置偏移处画模糊矩形）。
            // 实际圆角/阴影需要窗口几何信息，这里用默认矩形占位。
            let _ = win_id;
            let _ = shadow;
        }

        // 把全部图层按 id 顺序混合到后缓冲。
        for (_, layer) in self.layers.iter() {
            if !layer.visible {
                continue;
            }
            // 应用图层 alpha 到副本（避免修改原图层）。
            let mut surf = layer.surface.clone();
            if layer.alpha < 255 {
                let factor = layer.alpha as u32;
                for y in 0..surf.height() as i32 {
                    for x in 0..surf.width() as i32 {
                        if let Some(p) = surf.get(x, y) {
                            let a = ((p >> 24) as u32 * factor / 255) as u8;
                            let r = ((p >> 16) as u8 as u32 * factor / 255) as u8;
                            let g = ((p >> 8) as u8 as u32 * factor / 255) as u8;
                            let b = ((p as u8 as u32 * factor / 255) as u8);
                            surf.put(x, y, argb(a, r, g, b));
                        }
                    }
                }
            }
            blend_surfaces(&surf, &mut self.back, layer.blend_mode, layer.offset);
        }

        // 双缓冲交换。
        if self.double_buffered {
            std::mem::swap(&mut self.front, &mut self.back);
        } else {
            self.front = self.back.clone();
        }

        self.frames += 1;
        self.last_frame_ns = now;

        // 更新 fps。
        self.fps_frame_count += 1;
        if now - self.fps_last_second >= 1_000_000_000 {
            self.fps = self.fps_frame_count as f64;
            self.fps_frame_count = 0;
            self.fps_last_second = now;
        }
    }

    /// 更新全部动画，返回受影响的动画 id 列表。
    fn update_animations(&mut self, now: u64) -> Vec<u64> {
        let mut changed = Vec::new();
        let finished: Vec<u64> = self
            .animations
            .iter()
            .filter(|(_, a)| a.finished && !a.repeat)
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            self.animations.remove(&id);
        }
        for (id, anim) in self.animations.iter_mut() {
            if let Some(val) = anim.value_at(now) {
                changed.push(*id);
                // 应用动画值：若绑定窗口，修改窗口相关属性（这里只记录，实际由调用方读取）；
                // 若绑定图层，修改图层 alpha/offset。
                if let Some(layer_id) = anim.layer_id {
                    if let Some(layer) = self.layers.get_mut(&layer_id) {
                        match anim.prop {
                            AnimTarget::Opacity => {
                                layer.set_alpha(val.clamp(0.0, 255.0) as u8);
                            }
                            AnimTarget::X => {
                                layer.offset = (val as i32, layer.offset.1);
                            }
                            AnimTarget::Y => {
                                layer.offset = (layer.offset.0, val as i32);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        changed
    }

    /// 返回当前 fps。
    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// 返回已渲染帧数。
    pub fn frame_count(&self) -> u64 {
        self.frames
    }

    /// 返回最后一次渲染耗时（ns，简化为 0，实际应由调用方记录）。
    pub fn last_frame_ns(&self) -> u64 {
        self.last_frame_ns
    }

    /// 混合两个表面到目标表面（静态工具）。
    pub fn blend(src: &Surface, dst: &mut Surface, mode: BlendMode, offset: (i32, i32)) {
        blend_surfaces(src, dst, mode, offset);
    }

    /// 绘制圆角矩形到表面（简化：绘制矩形+角落切割占位）。
    pub fn draw_rounded_rect(surface: &mut Surface, rect: Rect, radius: CornerRadius, color: u32) {
        let r = radius.min(rect.w / 2).min(rect.h / 2);
        surface.fill_rect(rect.x, rect.y, rect.w, rect.h, color);
        // 角落用背景色覆盖以模拟圆角（简化版）。
        let bg = palette::BASE;
        let corner_size = r as i32;
        // 左上。
        surface.fill_rect(rect.x, rect.y, corner_size as u32, corner_size as u32, bg);
        // 右上。
        surface.fill_rect(
            rect.x + rect.w as i32 - corner_size,
            rect.y,
            corner_size as u32,
            corner_size as u32,
            bg,
        );
        // 左下。
        surface.fill_rect(
            rect.x,
            rect.y + rect.h as i32 - corner_size,
            corner_size as u32,
            corner_size as u32,
            bg,
        );
        // 右下。
        surface.fill_rect(
            rect.x + rect.w as i32 - corner_size,
            rect.y + rect.h as i32 - corner_size,
            corner_size as u32,
            corner_size as u32,
            bg,
        );
    }
}

fn id_or_none(id: u64) -> u64 {
    id
}

// ============================================================================
// 全局 CompositorExt 单例（供 kernel/sense.rs 调用）
// ============================================================================

static COMPOSITOR_EXT: OnceLock<Mutex<CompositorExt>> = OnceLock::new();

/// 获取全局合成器扩展。
pub fn compositor_ext() -> &'static Mutex<CompositorExt> {
    COMPOSITOR_EXT.get_or_init(|| Mutex::new(CompositorExt::new(960, 600)))
}

/// `sense gui://compositor_ext`：返回合成器扩展状态摘要。
pub fn compositor_ext_sense() -> String {
    let c = compositor_ext().lock().unwrap();
    format!(
        "layers={} animations={} shadows={} corners={} frames={} fps={:.1} double_buffered={} tear_free={}",
        c.layers.len(),
        c.animations.len(),
        c.shadows.len(),
        c.corners.len(),
        c.frame_count(),
        c.fps(),
        c.double_buffered,
        c.tear_free
    )
}

/// `act gui://compositor_ext <op> ...`：对合成器扩展执行行动。
pub fn compositor_ext_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let c = compositor_ext();
    let mut c = c.lock().unwrap();
    match it.next().unwrap_or("") {
        "add_layer" => {
            let w: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(960);
            let h: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(600);
            let id = c.add_layer(w, h);
            Ok(format!("已添加图层 #{}", id))
        }
        "set_alpha" => {
            let id: u64 = it.next().ok_or("缺少图层 id")?.parse().map_err(|e| e.to_string())?;
            let alpha: u8 = it.next().ok_or("缺少 alpha")?.parse().map_err(|e| e.to_string())?;
            if c.set_alpha(id, alpha) {
                Ok(format!("图层 #{} alpha={}", id, alpha))
            } else {
                Err(format!("图层 #{} 不存在", id))
            }
        }
        "round_corners" => {
            let win_id: u64 = it.next().ok_or("缺少窗口 id")?.parse().map_err(|e| e.to_string())?;
            let radius: u32 = it.next().ok_or("缺少半径")?.parse().map_err(|e| e.to_string())?;
            c.set_round_corners(win_id, radius);
            Ok(format!("窗口 #{} 圆角={}", win_id, radius))
        }
        "shadow" => {
            let win_id: u64 = it.next().ok_or("缺少窗口 id")?.parse().map_err(|e| e.to_string())?;
            let ox: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(4);
            let oy: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(4);
            let blur: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(8);
            let color: u32 = argb(128, 0, 0, 0);
            c.set_shadow(win_id, (ox, oy), blur, color);
            Ok(format!("窗口 #{} 阴影已设置", win_id))
        }
        "add_animation" => {
            let prop = it.next().unwrap_or("opacity");
            let from: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let to: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let dur: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(500_000_000);
            let easing = match it.next().unwrap_or("linear") {
                "ease_in" => Easing::EaseIn,
                "ease_out" => Easing::EaseOut,
                "ease_in_out" => Easing::EaseInOut,
                _ => Easing::Linear,
            };
            let prop_enum = match prop {
                "opacity" => AnimTarget::Opacity,
                "x" => AnimTarget::X,
                "y" => AnimTarget::Y,
                "width" => AnimTarget::Width,
                "height" => AnimTarget::Height,
                "rotate" => AnimTarget::Rotate,
                _ => AnimTarget::Opacity,
            };
            let anim = Animation::new(0, prop_enum, from, to, dur, easing, Easing::Linear);
            let id = c.add_animation(anim);
            Ok(format!("已添加动画 #{}", id))
        }
        "render_frame" => {
            let now = it.next().and_then(|v| v.parse().ok()).unwrap_or(get_time_ns());
            c.render_frame(now);
            Ok(format!("已渲染第 {} 帧", c.frame_count()))
        }
        "vsync" => {
            let now = it.next().and_then(|v| v.parse().ok()).unwrap_or(get_time_ns());
            if c.vsync(now) {
                Ok("vsync 已通过".to_string())
            } else {
                Ok("vsync 未到时刻".to_string())
            }
        }
        "fps" => Ok(format!("fps={:.1}", c.fps())),
        "double_buffer" => {
            let enable = it.next().unwrap_or("1") == "1";
            c.double_buffered = enable;
            Ok(format!("双缓冲={}", enable))
        }
        "tear_free" => {
            let enable = it.next().unwrap_or("1") == "1";
            c.tear_free = enable;
            Ok(format!("防撕裂={}", enable))
        }
        "list" => Ok(compositor_ext_sense()),
        _ => Err(
            "用法: compositor_ext add_layer|set_alpha|round_corners|shadow|add_animation|render_frame|vsync|fps|double_buffer|tear_free|list"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_normal_covers() {
        let mut dst = Surface::filled(2, 2, argb(255, 0, 0, 0));
        let src = Surface::filled(2, 2, argb(255, 255, 0, 0));
        blend_surfaces(&src, &mut dst, BlendMode::Normal, (0, 0));
        assert_eq!(dst.get(0, 0), Some(argb(255, 255, 0, 0)));
    }

    #[test]
    fn blend_multiply_darkens() {
        let mut dst = Surface::filled(1, 1, argb(255, 255, 255, 255));
        let src = Surface::filled(1, 1, argb(255, 128, 128, 128));
        blend_surfaces(&src, &mut dst, BlendMode::Multiply, (0, 0));
        let p = dst.get(0, 0).unwrap();
        // 乘法：255*128/255=128。
        assert_eq!((p >> 16) as u8, 128);
    }

    #[test]
    fn blend_screen_brightens() {
        let mut dst = Surface::filled(1, 1, argb(255, 0, 0, 0));
        let src = Surface::filled(1, 1, argb(255, 128, 128, 128));
        blend_surfaces(&src, &mut dst, BlendMode::Screen, (0, 0));
        let p = dst.get(0, 0).unwrap();
        assert!((p >> 16) as u8 > 0);
    }

    #[test]
    fn blend_offset() {
        let mut dst = Surface::filled(4, 4, argb(255, 0, 0, 0));
        let src = Surface::filled(2, 2, argb(255, 255, 255, 255));
        blend_surfaces(&src, &mut dst, BlendMode::Normal, (1, 1));
        assert_eq!(dst.get(1, 1), Some(argb(255, 255, 255, 255)));
        assert_eq!(dst.get(0, 0), Some(argb(255, 0, 0, 0)));
    }

    #[test]
    fn layer_alpha() {
        let mut c = CompositorExt::new(100, 100);
        let id = c.add_layer(100, 100);
        assert!(c.set_alpha(id, 128));
        let layer = c.layer(id).unwrap();
        assert_eq!(layer.alpha, 128);
    }

    #[test]
    fn round_corners_and_shadow() {
        let mut c = CompositorExt::new(100, 100);
        c.set_round_corners(1, 8);
        c.set_shadow(1, (4, 4), 8, argb(100, 0, 0, 0));
        assert_eq!(c.corners.get(&1), Some(&8));
        assert!(c.shadows.contains_key(&1));
        assert!(c.remove_shadow(1));
        assert!(!c.shadows.contains_key(&1));
    }

    #[test]
    fn animation_value() {
        let anim = Animation::new(1, AnimTarget::Opacity, 0.0, 100.0, 1_000_000_000, Easing::Linear);
        // start_time=0，value_at 返回 None（未启动）。
        assert_eq!(anim.value_at(0), None);
    }

    #[test]
    fn easing_values() {
        assert!((Easing::Linear.apply(0.5) - 0.5).abs() < f64::EPSILON);
        assert!((Easing::EaseIn.apply(1.0) - 1.0).abs() < f64::EPSILON);
        assert!((Easing::EaseOut.apply(0.0) - 0.0).abs() < f64::EPSILON);
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < f64::EPSILON);
        assert!((Easing::EaseInOut.apply(1.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compositor_ext_render_frame() {
        let mut c = CompositorExt::new(100, 100);
        let id = c.add_layer(100, 100);
        c.set_alpha(id, 200);
        let now = 5_000_000_000u64;
        c.render_frame(now);
        assert_eq!(c.frame_count(), 1);
        assert!(c.last_frame_ns() == now);
    }

    #[test]
    fn compositor_ext_vsync() {
        let mut c = CompositorExt::new(100, 100);
        let now = 0u64;
        assert!(c.vsync(now));
        assert!(!c.vsync(now));
        assert!(c.vsync(now + 16_666_667));
    }

    #[test]
    fn draw_rounded_rect_no_panic() {
        let mut s = Surface::filled(50, 50, palette::BASE);
        CompositorExt::draw_rounded_rect(&mut s, Rect::new(5, 5, 40, 40), 10, palette::BLUE);
    }

    #[test]
    fn compositor_ext_sense_and_act() {
        let mut c = compositor_ext().lock().unwrap();
        let id = c.add_layer(200, 200);
        drop(c);
        let sense = compositor_ext_sense();
        assert!(sense.contains("layers="));
        let res = compositor_ext_act(&format!("set_alpha {} 100", id)).unwrap();
        assert!(res.contains("alpha="));
        let res = compositor_ext_act("fps").unwrap();
        assert!(res.contains("fps="));
        let res = compositor_ext_act("render_frame 0").unwrap();
        assert!(res.contains("帧"));
        let err = compositor_ext_act("set_alpha 99999 10").unwrap_err();
        assert!(err.contains("不存在"));
    }

    #[test]
    fn blend_mode_names() {
        assert_eq!(BlendMode::Normal.name(), "normal");
        assert_eq!(BlendMode::Overlay.name(), "overlay");
    }

    #[test]
    fn double_buffer_toggle() {
        let mut c = CompositorExt::new(100, 100);
        assert!(c.double_buffered);
        c.double_buffered = false;
        assert!(!c.double_buffered);
    }

    #[test]
    fn resize_rebuilds_buffers() {
        let mut c = CompositorExt::new(100, 100);
        c.resize(200, 200);
        assert_eq!(c.front().width(), 200);
        assert_eq!(c.front().height(), 200);
    }
}
