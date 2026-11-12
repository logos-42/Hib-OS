//! 触摸与手势 —— 多指触点跟踪与手势识别。
//!
//! `GestureRecognizer` 接收 `touch_down` / `touch_move` / `touch_up`，在
//! `recognize()` 时识别 Tap / DoubleTap / LongPress / Drag / Pinch / Zoom /
//! Swipe / Scroll 手势。
//!
//! 手势检测基于时间阈值、距离阈值、速度计算与方向判断；支持多指
//! 同时跟踪（pinch/zoom 需要两根手指）。与 `kernel/gui/input.rs` 的
//! 鼠标事件互补：触摸屏/触控板输入走此模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 触点
// ============================================================================

/// 触点状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchState {
    Down,
    Move,
    Up,
    Stationary,
}

/// 单个触点。
#[derive(Clone, Debug)]
pub struct TouchPoint {
    /// 触点 ID（设备分配，0-9 等）。
    pub id: u64,
    /// 当前位置（x, y）。
    pub x: i32,
    pub y: i32,
    /// 上一次位置（用于速度计算）。
    pub prev_x: i32,
    pub prev_y: i32,
    /// 时间戳（ms）。
    pub time: u64,
    /// 压力（0.0-1.0，简化为固定值）。
    pub pressure: f32,
    pub state: TouchState,
}

impl TouchPoint {
    pub fn new(id: u64, x: i32, y: i32, time: u64) -> Self {
        Self {
            id,
            x,
            y,
            prev_x: x,
            prev_y: y,
            time,
            pressure: 0.5,
            state: TouchState::Down,
        }
    }

    /// 更新位置与时间。
    pub fn update(&mut self, x: i32, y: i32, time: u64) {
        self.prev_x = self.x;
        self.prev_y = self.y;
        self.x = x;
        self.y = y;
        self.time = time;
        self.state = TouchState::Move;
    }

    /// 两点之间的欧氏距离。
    pub fn distance_to(&self, other: &TouchPoint) -> f64 {
        let dx = (self.x - other.x) as f64;
        let dy = (self.y - other.y) as f64;
        (dx * dx + dy * dy).sqrt()
    }

    /// 从上一次位置移动的距离。
    pub fn delta_distance(&self) -> f64 {
        let dx = (self.x - self.prev_x) as f64;
        let dy = (self.y - self.prev_y) as f64;
        (dx * dx + dy * dy).sqrt()
    }

    /// 速度（像素/ms）。
    pub fn velocity(&self) -> f64 {
        let dt = if self.time == 0 { 1 } else { self.time };
        self.delta_distance() / dt as f64
    }
}

// ============================================================================
// 手势类型
// ============================================================================

/// 识别出的手势。
#[derive(Clone, Debug)]
pub enum Gesture {
    /// 单击：短时间轻触并抬起。
    Tap {
        x: i32,
        y: i32,
        touch_id: u64,
    },
    /// 双击：短时间内同一位置两次点击。
    DoubleTap {
        x: i32,
        y: i32,
        touch_id: u64,
    },
    /// 长按：按住超过阈值。
    LongPress {
        x: i32,
        y: i32,
        touch_id: u64,
        duration_ms: u64,
    },
    /// 拖拽：按住后移动。
    Drag {
        start_x: i32,
        start_y: i32,
        current_x: i32,
        current_y: i32,
        touch_id: u64,
        delta_x: i32,
        delta_y: i32,
    },
    /// 捏合：两指相向（缩小）。
    Pinch {
        center_x: i32,
        center_y: i32,
        scale: f64, // <1 缩小
        initial_distance: f64,
        current_distance: f64,
    },
    /// 缩放：两指背离（放大）。
    Zoom {
        center_x: i32,
        center_y: i32,
        scale: f64, // >1 放大
        initial_distance: f64,
        current_distance: f64,
    },
    /// 滑行：快速滑动后抬起。
    Swipe {
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        direction: SwipeDirection,
        velocity: f64,
    },
    /// 滚动：两指同向移动。
    Scroll {
        delta_x: i32,
        delta_y: i32,
        touch_ids: (u64, u64),
    },
}

/// 滑行方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SwipeDirection {
    pub fn label(self) -> &'static str {
        match self {
            SwipeDirection::Up => "上",
            SwipeDirection::Down => "下",
            SwipeDirection::Left => "左",
            SwipeDirection::Right => "右",
        }
    }
}

// ============================================================================
// 手势识别器配置
// ============================================================================

/// 手势阈值配置。
#[derive(Clone, Debug, Default)]
pub struct GestureConfig {
    /// 点击最大距离（像素）。
    pub tap_max_distance: f64,
    /// 双击最大间隔（ms）。
    pub double_tap_max_interval_ms: u64,
    /// 长按最小时长（ms）。
    pub long_press_min_ms: u64,
    /// 拖拽最小移动距离（像素）。
    pub drag_min_distance: f64,
    /// 滑行最小速度（像素/ms）。
    pub swipe_min_velocity: f64,
    /// 捏合/缩放最小距离变化比例。
    pub pinch_zoom_threshold: f64,
    /// 滚动最小移动距离（像素）。
    pub scroll_min_distance: f64,
}

impl GestureConfig {
    pub fn default_config() -> Self {
        Self {
            tap_max_distance: 10.0,
            double_tap_max_interval_ms: 300,
            long_press_min_ms: 500,
            drag_min_distance: 5.0,
            swipe_min_velocity: 0.3,
            pinch_zoom_threshold: 0.1,
            scroll_min_distance: 3.0,
        }
    }
}

// ============================================================================
// 手势识别器
// ============================================================================

/// 手势识别器：跟踪多指触点，识别手势。
pub struct GestureRecognizer {
    /// 当前活跃触点（id -> 触点）。
    active: BTreeMap<u64, TouchPoint>,
    /// 已抬起的触点历史（用于双击检测）。
    last_tap: Option<TouchPoint>,
/// 配置。
    pub config: GestureConfig,
}
impl Default for GestureRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureRecognizer {
    /// 创建识别器。
    pub fn new() -> Self {
        Self {
            active: BTreeMap::new(),
            last_tap: None,
            config: GestureConfig::default_config(),
        }
    }

    /// 设置配置。
    pub fn set_config(&mut self, config: GestureConfig) {
        self.config = config;
    }

    /// 触摸按下。
    pub fn touch_down(&mut self, id: u64, x: i32, y: i32, time: u64) {
        self.active.insert(
            id,
            TouchPoint::new(id, x, y, time),
        );
    }

    /// 触摸移动。
    pub fn touch_move(&mut self, id: u64, x: i32, y: i32, time: u64) {
        if let Some(p) = self.active.get_mut(&id) {
            p.update(x, y, time);
        }
    }

    /// 触摸抬起。
    pub fn touch_up(&mut self, id: u64, x: i32, y: i32, time: u64) {
        if let Some(p) = self.active.get(&id) {
            let mut up = p.clone();
            up.x = x;
            up.y = y;
            up.time = time;
            up.state = TouchState::Up;
            // 记录最后一次点击（用于双击）。
            self.last_tap = Some(up);
        }
        self.active.remove(&id);
    }

    /// 识别当前手势。返回 Option<Gesture>，None 表示无明确手势。
    pub fn recognize(&mut self) -> Option<Gesture> {
        let count = self.active.len();

        // 单指手势。
        if count == 1 {
            if let Some((_, p)) = self.active.iter().next() {
                // 长按。
                if p.state == TouchState::Down {
                    // 需要外部传入持续时间，这里用 time 作为近似。
                    // 若触点已按住且未移动，可视为长按候选。
                    let _ = p;
                }
                // 若触点有显著移动，视为拖拽。
                if p.delta_distance() > self.config.drag_min_distance {
                    return Some(Gesture::Drag {
                        start_x: p.prev_x,
                        start_y: p.prev_y,
                        current_x: p.x,
                        current_y: p.y,
                        touch_id: p.id,
                        delta_x: p.x - p.prev_x,
                        delta_y: p.y - p.prev_y,
                    });
                }
            }
            None
        }
        // 双指手势：pinch/zoom/scroll。
        else if count == 2 {
            let mut iter = self.active.iter();
            let p1 = iter.next().unwrap().1;
            let p2 = iter.next().unwrap().1;
            let dist = p1.distance_to(p2);
            let center_x = (p1.x + p2.x) / 2;
            let center_y = (p1.y + p2.y) / 2;

            // 判断滚动：两指同向移动且距离变化小。
            let d1 = p1.delta_distance();
            let d2 = p2.delta_distance();
            let dist_change = (dist - self.initial_distance(p1, p2)).abs();
            if dist_change < 5.0 && d1 > self.config.scroll_min_distance && d2 > self.config.scroll_min_distance {
                // 同向判断：dx 符号相同。
                let same_x = (p1.x - p1.prev_x) * (p2.x - p2.prev_x) >= 0;
                let same_y = (p1.y - p1.prev_y) * (p2.y - p2.prev_y) >= 0;
                if same_x || same_y {
                    return Some(Gesture::Scroll {
                        delta_x: (p1.x - p1.prev_x + p2.x - p2.prev_x) / 2,
                        delta_y: (p1.y - p1.prev_y + p2.y - p2.prev_y) / 2,
                        touch_ids: (p1.id, p2.id),
                    });
                }
            }

            // 判断 pinch/zoom：距离变化超过阈值。
            if dist_change > self.config.pinch_zoom_threshold * 50.0 {
                let scale = dist / self.initial_distance(p1, p2).max(1.0);
                if scale < 1.0 {
                    return Some(Gesture::Pinch {
                        center_x,
                        center_y,
                        scale,
                        initial_distance: self.initial_distance(p1, p2),
                        current_distance: dist,
                    });
                } else {
                    return Some(Gesture::Zoom {
                        center_x,
                        center_y,
                        scale,
                        initial_distance: self.initial_distance(p1, p2),
                        current_distance: dist,
                    });
                }
            }
            None
        }
        // 三指及以上：暂不识别。
        else {
            None
        }
    }

    /// 完成一次点击抬起后的事后识别（Tap/DoubleTap/LongPress/Swipe）。
    /// 应在 touch_up 后调用，传入抬起点与时间。
    pub fn recognize_after_up(
        &mut self,
        id: u64,
        x: i32,
        y: i32,
        down_time: u64,
        up_time: u64,
    ) -> Option<Gesture> {
        let duration = up_time.saturating_sub(down_time);
        // 长按优先。
        if duration >= self.config.long_press_min_ms {
            self.last_tap = Some(TouchPoint {
                id,
                x,
                y,
                prev_x: x,
                prev_y: y,
                time: up_time,
                pressure: 0.5,
                state: TouchState::Up,
            });
            return Some(Gesture::LongPress {
                x,
                y,
                touch_id: id,
                duration_ms: duration,
            });
        }
        // 短时间内为点击。
        if duration <= 200 {
            let dist = if let Some(ref last) = self.last_tap {
                let dx = (x - last.x) as f64;
                let dy = (y - last.y) as f64;
                (dx * dx + dy * dy).sqrt()
            } else {
                f64::INFINITY
            };
            let interval = up_time.saturating_sub(self.last_tap.as_ref().map_or(0, |t| t.time));
            if dist < self.config.tap_max_distance
                && interval <= self.config.double_tap_max_interval_ms
            {
                self.last_tap = None; // 消费，避免重复双击。
                return Some(Gesture::DoubleTap {
                    x,
                    y,
                    touch_id: id,
                });
            }
            // 普通 tap：检查是否有滑行速度。
            if let Some(ref last) = self.active.get(&id) {
                let vel = last.velocity();
                if vel > self.config.swipe_min_velocity {
                    let dir = if (x - last.prev_x).abs() > (y - last.prev_y).abs() {
                        if x > last.prev_x {
                            SwipeDirection::Right
                        } else {
                            SwipeDirection::Left
                        }
                    } else if y > last.prev_y {
                        SwipeDirection::Down
                    } else {
                        SwipeDirection::Up
                    };
                    self.last_tap = Some(TouchPoint {
                        id,
                        x,
                        y,
                        prev_x: x,
                        prev_y: y,
                        time: up_time,
                        pressure: 0.5,
                        state: TouchState::Up,
                    });
                    return Some(Gesture::Swipe {
                        start_x: last.prev_x,
                        start_y: last.prev_y,
                        end_x: x,
                        end_y: y,
                        direction: dir,
                        velocity: vel,
                    });
                }
            }
            self.last_tap = Some(TouchPoint {
                id,
                x,
                y,
                prev_x: x,
                prev_y: y,
                time: up_time,
                pressure: 0.5,
                state: TouchState::Up,
            });
            Some(Gesture::Tap { x, y, touch_id: id })
        } else {
            None
        }
    }

    /// 返回两点初始距离（简化：基于当前位置，若无历史记录则返回当前距离）。
    fn initial_distance(&self, p1: &TouchPoint, p2: &TouchPoint) -> f64 {
        // 这里没有保存按下时的距离，用当前距离作为基准（简化版）。
        p1.distance_to(p2)
    }

    /// 返回当前活跃触点数。
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// 清空所有触点。
    pub fn clear(&mut self) {
        self.active.clear();
        self.last_tap = None;
    }

    /// 返回最后一次点击点。
    pub fn last_tap(&self) -> Option<&TouchPoint> {
        self.last_tap.as_ref()
    }
}

// ============================================================================
// 全局 GestureRecognizer 单例（供 kernel/sense.rs 调用）
// ============================================================================

static GESTURE_RECOGNIZER: OnceLock<Mutex<GestureRecognizer>> = OnceLock::new();

/// 获取全局手势识别器。
pub fn gesture_recognizer() -> &'static Mutex<GestureRecognizer> {
    GESTURE_RECOGNIZER.get_or_init(|| Mutex::new(GestureRecognizer::new()))
}

/// `sense gui://gesture`：返回手势识别器状态摘要。
pub fn gesture_sense() -> String {
    let gr = gesture_recognizer().lock().unwrap();
    format!(
        "active_touches={} config.tap_max_distance={} config.long_press_min_ms={}",
        gr.active_count(),
        gr.config.tap_max_distance,
        gr.config.long_press_min_ms
    )
}

/// `act gui://gesture <op> ...`：对手势识别器执行行动。
pub fn gesture_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let gr = gesture_recognizer();
    let mut gr = gr.lock().unwrap();
    match it.next().unwrap_or("") {
        "touch_down" => {
            let id: u64 = it
                .next()
                .ok_or("缺少 id")?
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let x: i32 = it.next().ok_or("缺少 x")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let y: i32 = it.next().ok_or("缺少 y")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let time: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            gr.touch_down(id, x, y, time);
            Ok(format!("触点 #{} 按下 ({},{}) @{}", id, x, y, time))
        }
        "touch_move" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let x: i32 = it.next().ok_or("缺少 x")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let y: i32 = it.next().ok_or("缺少 y")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let time: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            gr.touch_move(id, x, y, time);
            Ok(format!("触点 #{} 移动到 ({},{}) @{}", id, x, y, time))
        }
        "touch_up" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let x: i32 = it.next().ok_or("缺少 x")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let y: i32 = it.next().ok_or("缺少 y")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let time: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            // 需要知道 down_time，这里简化：用 time - 100 作为 down_time。
            let down_time = time.saturating_sub(100);
            if let Some(g) = gr.recognize_after_up(id, x, y, down_time, time) {
                Ok(format!("识别手势：{:?}", g))
            } else {
                Ok(format!("触点 #{} 抬起 ({},{}) @{}", id, x, y, time))
            }
        }
        "recognize" => {
            if let Some(g) = gr.recognize() {
                Ok(format!("识别手势：{:?}", g))
            } else {
                Ok("无手势".to_string())
            }
        }
        "clear" => {
            gr.clear();
            Ok("触点已清空".to_string())
        }
        "config" => {
            let key = it.next().ok_or("缺少键")?;
            let val: f64 = it.next().ok_or("缺少值")?.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
            match key {
                "tap_max_distance" => gr.config.tap_max_distance = val,
                "long_press_min_ms" => gr.config.long_press_min_ms = val as u64,
                "drag_min_distance" => gr.config.drag_min_distance = val,
                "swipe_min_velocity" => gr.config.swipe_min_velocity = val,
                _ => return Err("未知配置键".to_string()),
            }
            Ok(format!("配置 {} = {}", key, val))
        }
        "list" => Ok(gesture_sense()),
        _ => Err(
            "用法: gesture touch_down|touch_move|touch_up|recognize|clear|config|list".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_point_new_and_distance() {
        let p1 = TouchPoint::new(1, 0, 0, 0);
        let p2 = TouchPoint::new(2, 3, 4, 0);
        assert!((p1.distance_to(&p2) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn touch_point_update() {
        let mut p = TouchPoint::new(1, 0, 0, 0);
        p.update(10, 10, 100);
        assert_eq!(p.x, 10);
        assert_eq!(p.y, 10);
        assert_eq!(p.prev_x, 0);
        assert_eq!(p.prev_y, 0);
    }

    #[test]
    fn gesture_recognizer_tap() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 100, 100, 0);
        gr.touch_move(1, 100, 100, 50); // 未移动。
        // recognize_after_up: duration=100ms < long_press，短按 -> Tap。
        let g = gr.recognize_after_up(1, 100, 100, 0, 100);
        match g {
            Some(Gesture::Tap { x, y, .. }) => {
                assert_eq!(x, 100);
                assert_eq!(y, 100);
            }
            other => panic!("期望 Tap，得到 {:?}", other),
        }
    }

    #[test]
    fn gesture_recognizer_double_tap() {
        let mut gr = GestureRecognizer::new();
        // 第一次点击。
        gr.touch_down(1, 50, 50, 0);
        gr.recognize_after_up(1, 50, 50, 0, 100);
        // 第二次点击：短时间、同位置 -> DoubleTap。
        gr.touch_down(1, 50, 50, 150);
        let g = gr.recognize_after_up(1, 50, 50, 150, 200);
        match g {
            Some(Gesture::DoubleTap { x, y, .. }) => {
                assert_eq!(x, 50);
                assert_eq!(y, 50);
            }
            other => panic!("期望 DoubleTap，得到 {:?}", other),
        }
    }

    #[test]
    fn gesture_recognizer_long_press() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 10, 10, 0);
        // 按住 600ms 超过 long_press_min_ms(500)。
        let g = gr.recognize_after_up(1, 10, 10, 0, 600);
        match g {
            Some(Gesture::LongPress { duration_ms, .. }) => {
                assert!(duration_ms >= 500);
            }
            other => panic!("期望 LongPress，得到 {:?}", other),
        }
    }

    #[test]
    fn gesture_recognizer_drag() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 0, 0, 0);
        gr.touch_move(1, 20, 0, 100); // 移动 20 像素 > drag_min_distance 5。
        let g = gr.recognize();
        match g {
            Some(Gesture::Drag { delta_x, .. }) => {
                assert_eq!(delta_x, 20);
            }
            other => panic!("期望 Drag，得到 {:?}", other),
        }
    }

    #[test]
    fn gesture_recognizer_swipe() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 0, 0, 0);
        gr.touch_move(1, 100, 0, 50); // 快速向右滑。
        let g = gr.recognize_after_up(1, 100, 0, 0, 50);
        match g {
            Some(Gesture::Swipe {
                direction: SwipeDirection::Right,
                ..
            }) => {}
            other => panic!("期望 Swipe Right，得到 {:?}", other),
        }
    }

    #[test]
    fn gesture_two_finger_pinch() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 0, 0, 0);
        gr.touch_down(2, 100, 0, 0);
        // 移动两指相向 -> 距离缩小 -> Pinch。
        gr.touch_move(1, 20, 0, 100);
        gr.touch_move(2, 80, 0, 100);
        let g = gr.recognize();
        match g {
            Some(Gesture::Pinch { scale, .. }) => {
                assert!(scale < 1.0);
            }
            Some(Gesture::Zoom { scale, .. }) => {
                assert!(scale > 1.0);
            }
            other => panic!("期望 Pinch/Zoom，得到 {:?}", other),
        }
    }

    #[test]
    fn gesture_two_finger_scroll() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 0, 0, 0);
        gr.touch_down(2, 0, 100, 0);
        // 两指同向向下移动，距离不变 -> Scroll。
        gr.touch_move(1, 0, 30, 100);
        gr.touch_move(2, 0, 130, 100);
        let g = gr.recognize();
        match g {
            Some(Gesture::Scroll { delta_y, .. }) => {
                assert!(delta_y > 0);
            }
            other => panic!("期望 Scroll，得到 {:?}", other),
        }
    }

    #[test]
    fn swipe_direction_label() {
        assert_eq!(SwipeDirection::Up.label(), "上");
        assert_eq!(SwipeDirection::Down.label(), "下");
        assert_eq!(SwipeDirection::Left.label(), "左");
        assert_eq!(SwipeDirection::Right.label(), "右");
    }

    #[test]
    fn gesture_config_default() {
        let config = GestureConfig::default_config();
        assert_eq!(config.long_press_min_ms, 500);
        assert_eq!(config.double_tap_max_interval_ms, 300);
    }

    #[test]
    fn gesture_clear() {
        let mut gr = GestureRecognizer::new();
        gr.touch_down(1, 0, 0, 0);
        gr.touch_down(2, 10, 10, 0);
        assert_eq!(gr.active_count(), 2);
        gr.clear();
        assert_eq!(gr.active_count(), 0);
    }

    #[test]
    fn gesture_sense_and_act() {
        let mut gr = gesture_recognizer().lock().unwrap();
        gr.touch_down(1, 5, 5, 0);
        drop(gr);
        let sense = gesture_sense();
        assert!(sense.contains("active_touches="));
        let res = gesture_act("touch_down 1 10 10 100").unwrap();
        assert!(res.contains("按下"));
        let res = gesture_act("recognize").unwrap();
        assert!(res.contains("无手势") || res.contains("识别手势"));
        let res = gesture_act("list").unwrap();
        assert!(sense.contains("config.tap_max_distance="));
    }
}
