//! GPU 能力位定义。
//!
//! 不使用外部 `bitfield` crate，改为手工实现的轻量位集，
//! 保证项目零第三方依赖即可编译。

/// 单个 GPU 能力位。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GpuCapability {
    /// 通用 GPU 计算 (矩阵运算、非张量)
    Compute = 0,
    /// AI 张量运算 (FP16/BF16/FP32 矩阵乘法)
    Tensor = 1,
    /// 显示输出 (帧缓冲、显示管道)
    Display = 2,
    /// 媒体处理 (视频编解码、图像处理)
    Media = 3,
    /// 寄存器读写 (调试、配置)
    RegisterRw = 4,
    /// 安全开关 (IOMMU 配置、策略切换)
    SecureSwitch = 5,
    /// 超频/功率控制 (受限访问)
    PowerControl = 6,
    /// 虚拟化扩展 (多个虚拟 GPU)
    Virtualization = 7,
}

impl GpuCapability {
    /// 全部能力位。
    pub const ALL: [GpuCapability; 8] = [
        GpuCapability::Compute,
        GpuCapability::Tensor,
        GpuCapability::Display,
        GpuCapability::Media,
        GpuCapability::RegisterRw,
        GpuCapability::SecureSwitch,
        GpuCapability::PowerControl,
        GpuCapability::Virtualization,
    ];

    /// 该能力对应的单比特掩码。
    pub const fn mask(self) -> u8 {
        1u8 << (self as u8)
    }

    /// 人类可读名称。
    pub const fn name(self) -> &'static str {
        match self {
            GpuCapability::Compute => "Compute",
            GpuCapability::Tensor => "Tensor",
            GpuCapability::Display => "Display",
            GpuCapability::Media => "Media",
            GpuCapability::RegisterRw => "RegisterRw",
            GpuCapability::SecureSwitch => "SecureSwitch",
            GpuCapability::PowerControl => "PowerControl",
            GpuCapability::Virtualization => "Virtualization",
        }
    }
}

/// GPU 能力位集合 (u8 位图)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GpuCapabilities(u8);

impl GpuCapabilities {
    /// 空集合。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 全能力集合。
    pub const fn all() -> Self {
        Self(0xFF)
    }

    /// 从单个能力构造。
    pub const fn from_cap(cap: GpuCapability) -> Self {
        Self(cap.mask())
    }

    /// 从能力列表构造。
    pub fn from_caps(caps: &[GpuCapability]) -> Self {
        let mut bits = 0u8;
        for c in caps {
            bits |= c.mask();
        }
        Self(bits)
    }

    /// 是否包含某个能力。
    pub const fn contains(self, cap: GpuCapability) -> bool {
        self.0 & cap.mask() != 0
    }

    /// 插入某个能力。
    pub fn insert(&mut self, cap: GpuCapability) {
        self.0 |= cap.mask();
    }

    /// 移除某个能力。
    pub fn remove(&mut self, cap: GpuCapability) {
        self.0 &= !cap.mask();
    }

    /// 是否是空集。
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// 原始位值。
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// 所需能力是否全部被当前集合覆盖。
    pub fn covers(self, required: Self) -> bool {
        (required.0 & !self.0) == 0
    }
}

impl From<GpuCapability> for GpuCapabilities {
    fn from(cap: GpuCapability) -> Self {
        Self::from_cap(cap)
    }
}

impl core::ops::BitOr for GpuCapabilities {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitAnd for GpuCapabilities {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl core::ops::Not for GpuCapabilities {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0)
    }
}

/// 检查是否拥有所需能力。
pub fn has_capability(caps: GpuCapabilities, required: GpuCapabilities) -> bool {
    caps.covers(required)
}

/// 能力并集。
pub fn combine_caps(a: GpuCapabilities, b: GpuCapabilities) -> GpuCapabilities {
    a | b
}

/// 能力差集。
pub fn remove_caps(caps: GpuCapabilities, to_remove: GpuCapabilities) -> GpuCapabilities {
    caps & !to_remove
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_all() {
        let e = GpuCapabilities::empty();
        assert!(e.is_empty());
        assert!(!e.contains(GpuCapability::Compute));

        let a = GpuCapabilities::all();
        for cap in GpuCapability::ALL {
            assert!(a.contains(cap), "缺少能力 {:?}", cap);
        }
    }

    #[test]
    fn from_caps_and_set_ops() {
        let c = GpuCapabilities::from_caps(&[GpuCapability::Compute, GpuCapability::Tensor]);
        assert!(c.contains(GpuCapability::Compute));
        assert!(c.contains(GpuCapability::Tensor));
        assert!(!c.contains(GpuCapability::Display));

        let union = c | GpuCapabilities::from_cap(GpuCapability::Display);
        assert!(union.contains(GpuCapability::Display));

        let diff = remove_caps(union, GpuCapabilities::from_cap(GpuCapability::Tensor));
        assert!(!diff.contains(GpuCapability::Tensor));
        assert!(diff.contains(GpuCapability::Compute));
    }

    #[test]
    fn covers_means_subset() {
        let have = GpuCapabilities::from_caps(&[
            GpuCapability::Compute,
            GpuCapability::Tensor,
            GpuCapability::Media,
        ]);
        let need =
            GpuCapabilities::from_caps(&[GpuCapability::Compute, GpuCapability::Tensor]);
        assert!(have.covers(need));
        assert!(has_capability(have, need));
        assert!(!need.covers(have));
    }

    #[test]
    fn insert_and_remove() {
        let mut c = GpuCapabilities::empty();
        c.insert(GpuCapability::SecureSwitch);
        assert!(c.contains(GpuCapability::SecureSwitch));
        c.remove(GpuCapability::SecureSwitch);
        assert!(!c.contains(GpuCapability::SecureSwitch));
    }
}

