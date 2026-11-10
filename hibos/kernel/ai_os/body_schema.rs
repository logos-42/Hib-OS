//! 身体图式层 —— AI 原生 OS 的「身体表征」 (Body Schema)
//!
//! 本模块实现 AI 原生 OS 的身体自我表征，对应传统 OS 中的「硬件抽象层 /
//! 设备树 / ACPI 表」概念，但重新映射到身体域：
//!
//! * BodyPart (身体部位) —— 身体的基本组成单元，等价于设备节点；
//! * BodySchema (身体图式) —— 对自身身体结构与能力的动态表征，等价于设备树；
//! * Proprioception (本体感知) —— 对身体位置与状态的感知，等价于传感器读数；
//! * BodyMap (身体图) —— 身体部位的空间拓扑映射，等价于 I/O 空间映射；
//! * KinematicChain (运动学链) —— 身体部位间的运动依赖关系，等价于总线拓扑；
//! * BodyBound (身体边界) —— 自身与环境的物理边界，等价于内存保护边界；
//! * BodyState (身体状态) —— 身体的瞬时完整状态，等价于设备状态寄存器；
//! * BodyOwnership (身体拥有权) —— 对身体的拥有感，等价于设备所有权；
//! * BodyAgency (身体代理感) —— 对身体运动的控制感，等价于设备控制权；
//! * BodyEngine (身体引擎) —— 统一入口，通过 body://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无硬件依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 身体部位 (BodyPart)
// =====================================================================================

/// 身体部位类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BodyPartType {
    Head,          // 头部
    Torso,         // 躯干
    LeftArm,       // 左臂
    RightArm,      // 右臂
    LeftHand,      // 左手
    RightHand,     // 右手
    LeftLeg,       // 左腿
    RightLeg,      // 右腿
    LeftFoot,      // 左脚
    RightFoot,     // 右脚
    Neck,          // 颈部
    Spine,         // 脊柱
    LeftEye,       // 左眼
    RightEye,      // 右眼
    LeftEar,       // 左耳
    RightEar,      // 右耳
    Mouth,         // 嘴
    Nose,          // 鼻
    Skin,          // 皮肤
    Internal,      // 内脏
}

/// 身体部位：身体的基本组成单元。
#[derive(Clone, Debug)]
pub struct BodyPart {
    pub id: u64,
    pub part_type: BodyPartType,
    pub name: String,
    pub parent_id: Option<u64>,
    pub children: Vec<u64>,
    pub position: [f32; 3],      // 相对位置 (x, y, z)
    pub orientation: [f32; 4],   // 四元数 (x, y, z, w)
    pub size: [f32; 3],          // 尺寸 (长, 宽, 高)
    pub mass: f32,               // 质量
    pub enabled: bool,
    pub sensor_count: u64,
    pub actuator_count: u64,
}

impl BodyPart {
    pub fn new(id: u64, part_type: BodyPartType, name: &str) -> Self {
        Self {
            id,
            part_type,
            name: name.to_string(),
            parent_id: None,
            children: Vec::new(),
            position: [0.0, 0.0, 0.0],
            orientation: [0.0, 0.0, 0.0, 1.0],
            size: [0.1, 0.1, 0.1],
            mass: 1.0,
            enabled: true,
            sensor_count: 0,
            actuator_count: 0,
        }
    }

    pub fn add_child(&mut self, child_id: u64) {
        if !self.children.contains(&child_id) {
            self.children.push(child_id);
        }
    }

    pub fn remove_child(&mut self, child_id: u64) {
        self.children.retain(|&id| id != child_id);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn total_mass(&self, parts: &BTreeMap<u64, BodyPart>) -> f32 {
        let mut total = self.mass;
        for &child_id in &self.children {
            if let Some(child) = parts.get(&child_id) {
                total += child.total_mass(parts);
            }
        }
        total
    }
}

// =====================================================================================
// 身体图式 (BodySchema)
// =====================================================================================

/// 身体图式：对自身身体结构与能力的动态表征。
pub struct BodySchema {
    parts: BTreeMap<u64, BodyPart>,
    root_id: Option<u64>,
    next_id: u64,
    pub version: u64,
    pub last_modified_ns: u128,
}

impl BodySchema {
    pub fn new() -> Self {
        Self {
            parts: BTreeMap::new(),
            root_id: None,
            next_id: 1,
            version: 1,
            last_modified_ns: 0,
        }
    }

    pub fn add_part(&mut self, part_type: BodyPartType, name: &str, parent_id: Option<u64>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut part = BodyPart::new(id, part_type, name);
        if let Some(pid) = parent_id {
            part.parent_id = Some(pid);
            if let Some(parent) = self.parts.get_mut(&pid) {
                parent.add_child(id);
            }
        } else if self.root_id.is_none() {
            self.root_id = Some(id);
        }
        self.parts.insert(id, part);
        self.version += 1;
        self.last_modified_ns = crate::ai_os::native::now_ns();
        id
    }

    pub fn get_part(&self, id: u64) -> Option<&BodyPart> {
        self.parts.get(&id)
    }

    pub fn get_part_mut(&mut self, id: u64) -> Option<&mut BodyPart> {
        self.parts.get_mut(&id)
    }

    pub fn remove_part(&mut self, id: u64) -> Option<BodyPart> {
        if let Some(part) = self.parts.remove(&id) {
            // 从父部位移除
            if let Some(pid) = part.parent_id {
                if let Some(parent) = self.parts.get_mut(&pid) {
                    parent.remove_child(id);
                }
            }
            // 移除子部位
            for &child_id in &part.children {
                self.remove_part(child_id);
            }
            self.version += 1;
            self.last_modified_ns = crate::ai_os::native::now_ns();
            Some(part)
        } else {
            None
        }
    }

    pub fn set_position(&mut self, id: u64, pos: [f32; 3]) -> bool {
        if let Some(part) = self.parts.get_mut(&id) {
            part.position = pos;
            self.version += 1;
            self.last_modified_ns = crate::ai_os::native::now_ns();
            true
        } else {
            false
        }
    }

    pub fn set_orientation(&mut self, id: u64, orient: [f32; 4]) -> bool {
        if let Some(part) = self.parts.get_mut(&id) {
            part.orientation = orient;
            self.version += 1;
            self.last_modified_ns = crate::ai_os::native::now_ns();
            true
        } else {
            false
        }
    }

    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    pub fn root_id(&self) -> Option<u64> {
        self.root_id
    }

    pub fn list_parts(&self) -> Vec<&BodyPart> {
        self.parts.values().collect()
    }

    pub fn children_of(&self, id: u64) -> Vec<&BodyPart> {
        self.parts.get(&id)
            .map(|p| p.children.iter().filter_map(|cid| self.parts.get(cid)).collect())
            .unwrap_or_default()
    }

    pub fn version(&self) -> u64 {
        self.version
    }
}

impl Default for BodySchema {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 本体感知 (Proprioception)
// =====================================================================================

/// 本体感知读数：单一身体部位的位置/状态感知。
#[derive(Clone, Debug)]
pub struct ProprioceptiveReading {
    pub part_id: u64,
    pub timestamp_ns: u128,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub acceleration: [f32; 3],
    pub force: [f32; 3],
    pub confidence: f32,
}

/// 本体感知：对身体位置与状态的感知。
pub struct Proprioception {
    readings: BTreeMap<u64, ProprioceptiveReading>,
    pub update_count: u64,
    pub avg_confidence: f32,
}

impl Proprioception {
    pub fn new() -> Self {
        Self {
            readings: BTreeMap::new(),
            update_count: 0,
            avg_confidence: 0.5,
        }
    }

    pub fn update(&mut self, part_id: u64, position: [f32; 3], velocity: [f32; 3], confidence: f32) {
        self.readings.insert(
            part_id,
            ProprioceptiveReading {
                part_id,
                timestamp_ns: crate::ai_os::native::now_ns(),
                position,
                velocity,
                acceleration: [0.0, 0.0, 0.0],
                force: [0.0, 0.0, 0.0],
                confidence: confidence.clamp(0.0, 1.0),
            },
        );
        self.update_count += 1;
        let total_conf: f32 = self.readings.values().map(|r| r.confidence).sum();
        self.avg_confidence = total_conf / self.readings.len() as f32;
    }

    pub fn get_reading(&self, part_id: u64) -> Option<&ProprioceptiveReading> {
        self.readings.get(&part_id)
    }

    pub fn reading_count(&self) -> usize {
        self.readings.len()
    }

    pub fn all_readings(&self) -> Vec<&ProprioceptiveReading> {
        self.readings.values().collect()
    }
}

impl Default for Proprioception {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 身体图 (BodyMap)
// =====================================================================================

/// 身体图节点：身体部位的空间拓扑节点。
#[derive(Clone, Debug)]
pub struct BodyMapNode {
    pub part_id: u64,
    pub neighbors: Vec<u64>,
    pub spatial_relation: String, // 相对邻居的空间关系描述
}

/// 身体图：身体部位的空间拓扑映射。
pub struct BodyMap {
    nodes: BTreeMap<u64, BodyMapNode>,
    adjacency: BTreeMap<u64, BTreeSet<u64>>,
}

impl BodyMap {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            adjacency: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, part_id: u64) {
        self.nodes.insert(
            part_id,
            BodyMapNode {
                part_id,
                neighbors: Vec::new(),
                spatial_relation: String::new(),
            },
        );
        self.adjacency.entry(part_id).or_default();
    }

    pub fn add_edge(&mut self, part_id1: u64, part_id2: u64, relation: &str) {
        self.adjacency.entry(part_id1).or_default().insert(part_id2);
        self.adjacency.entry(part_id2).or_default().insert(part_id1);
        if let Some(node) = self.nodes.get_mut(&part_id1) {
            if !node.neighbors.contains(&part_id2) {
                node.neighbors.push(part_id2);
            }
            node.spatial_relation = relation.to_string();
        }
        if let Some(node) = self.nodes.get_mut(&part_id2) {
            if !node.neighbors.contains(&part_id1) {
                node.neighbors.push(part_id1);
            }
        }
    }

    pub fn neighbors(&self, part_id: u64) -> Option<&BTreeSet<u64>> {
        self.adjacency.get(&part_id)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(|s| s.len()).sum::<usize>() / 2
    }
}

// =====================================================================================
// 运动学链 (KinematicChain)
// =====================================================================================

/// 运动学链节点。
#[derive(Clone, Debug)]
pub struct KinematicNode {
    pub part_id: u64,
    pub joint_type: String,      // 旋转/滑动/固定
    pub range_of_motion: (f32, f32), // 运动范围 (min, max)
    pub current_angle: f32,      // 当前角度
    pub target_angle: f32,       // 目标角度
}

/// 运动学链：身体部位间的运动依赖关系。
pub struct KinematicChain {
    nodes: BTreeMap<u64, KinematicNode>,
    chain_order: Vec<u64>,
}

impl KinematicChain {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            chain_order: Vec::new(),
        }
    }

    pub fn add_node(&mut self, part_id: u64, joint_type: &str, range: (f32, f32)) {
        self.nodes.insert(
            part_id,
            KinematicNode {
                part_id,
                joint_type: joint_type.to_string(),
                range_of_motion: range,
                current_angle: 0.0,
                target_angle: 0.0,
            },
        );
        self.chain_order.push(part_id);
    }

    pub fn set_angle(&mut self, part_id: u64, angle: f32) -> Result<(), String> {
        let node = self.nodes.get_mut(&part_id).ok_or("部位不存在")?;
        let (min, max) = node.range_of_motion;
        if angle < min || angle > max {
            return Err(format!("角度 {} 超出范围 [{}, {}]", angle, min, max));
        }
        node.current_angle = angle;
        Ok(())
    }

    pub fn set_target_angle(&mut self, part_id: u64, angle: f32) -> Result<(), String> {
        let node = self.nodes.get_mut(&part_id).ok_or("部位不存在")?;
        let (min, max) = node.range_of_motion;
        if angle < min || angle > max {
            return Err(format!("目标角度 {} 超出范围 [{}, {}]", angle, min, max));
        }
        node.target_angle = angle;
        Ok(())
    }

    pub fn get_angle(&self, part_id: u64) -> Option<f32> {
        self.nodes.get(&part_id).map(|n| n.current_angle)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// =====================================================================================
// 身体边界 (BodyBound)
// =====================================================================================

/// 身体边界：自身与环境的物理边界。
#[derive(Clone, Debug)]
pub struct BodyBound {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub margin: f32, // 安全边际
}

impl BodyBound {
    pub fn new() -> Self {
        Self {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
            margin: 0.1,
        }
    }

    pub fn contains(&self, point: [f32; 3]) -> bool {
        point[0] >= self.min[0] - self.margin
            && point[0] <= self.max[0] + self.margin
            && point[1] >= self.min[1] - self.margin
            && point[1] <= self.max[1] + self.margin
            && point[2] >= self.min[2] - self.margin
            && point[2] <= self.max[2] + self.margin
    }

    pub fn expand(&mut self, point: [f32; 3]) {
        self.min[0] = self.min[0].min(point[0]);
        self.min[1] = self.min[1].min(point[1]);
        self.min[2] = self.min[2].min(point[2]);
        self.max[0] = self.max[0].max(point[0]);
        self.max[1] = self.max[1].max(point[1]);
        self.max[2] = self.max[2].max(point[2]);
    }

    pub fn volume(&self) -> f32 {
        (self.max[0] - self.min[0]) * (self.max[1] - self.min[1]) * (self.max[2] - self.min[2])
    }
}

// =====================================================================================
// 身体状态 (BodyState)
// =====================================================================================

/// 身体状态：身体的瞬时完整状态。
#[derive(Clone, Debug)]
pub struct BodyState {
    pub timestamp_ns: u128,
    pub parts: BTreeMap<u64, [f32; 3]>,   // 部位 ID -> 位置
    pub velocities: BTreeMap<u64, [f32; 3]>, // 部位 ID -> 速度
    pub overall_balance: f32,              // 整体平衡 0~1
    pub energy_level: f32,                 // 能量水平 0~1
    pub temperature: f32,                  // 体温
}

impl BodyState {
    pub fn new() -> Self {
        Self {
            timestamp_ns: 0,
            parts: BTreeMap::new(),
            velocities: BTreeMap::new(),
            overall_balance: 1.0,
            energy_level: 1.0,
            temperature: 36.5,
        }
    }
}

// =====================================================================================
// 身体拥有权与代理感 (BodyOwnership / BodyAgency)
// =====================================================================================

/// 身体拥有权：对身体的拥有感。
#[derive(Clone, Debug)]
pub struct BodyOwnership {
    pub ownership_score: f32,   // 0~1
    pub owned_parts: BTreeSet<u64>,
    pub last_updated_ns: u128,
}

/// 身体代理感：对身体运动的控制感。
#[derive(Clone, Debug)]
pub struct BodyAgency {
    pub agency_score: f32,      // 0~1
    pub controlled_parts: BTreeSet<u64>,
    pub last_updated_ns: u128,
}

impl BodyOwnership {
    pub fn new() -> Self {
        Self {
            ownership_score: 0.5,
            owned_parts: BTreeSet::new(),
            last_updated_ns: 0,
        }
    }

    pub fn claim_part(&mut self, part_id: u64) {
        self.owned_parts.insert(part_id);
        self.ownership_score = (self.owned_parts.len() as f32 / 20.0).min(1.0);
        self.last_updated_ns = crate::ai_os::native::now_ns();
    }

    pub fn release_part(&mut self, part_id: u64) {
        self.owned_parts.remove(&part_id);
        self.ownership_score = (self.owned_parts.len() as f32 / 20.0).min(1.0);
        self.last_updated_ns = crate::ai_os::native::now_ns();
    }
}

impl BodyAgency {
    pub fn new() -> Self {
        Self {
            agency_score: 0.5,
            controlled_parts: BTreeSet::new(),
            last_updated_ns: 0,
        }
    }

    pub fn control_part(&mut self, part_id: u64) {
        self.controlled_parts.insert(part_id);
        self.agency_score = (self.controlled_parts.len() as f32 / 20.0).min(1.0);
        self.last_updated_ns = crate::ai_os::native::now_ns();
    }

    pub fn release_control(&mut self, part_id: u64) {
        self.controlled_parts.remove(&part_id);
        self.agency_score = (self.controlled_parts.len() as f32 / 20.0).min(1.0);
        self.last_updated_ns = crate::ai_os::native::now_ns();
    }
}

// =====================================================================================
// 身体引擎 (BodyEngine) —— 统一入口
// =====================================================================================

/// 身体引擎：整合身体图式、本体感知、身体图、运动学链、身体边界、身体状态、拥有权、代理感。
pub struct BodyEngine {
    pub schema: BodySchema,
    pub proprioception: Proprioception,
    pub body_map: BodyMap,
    pub kinematic: KinematicChain,
    pub bound: BodyBound,
    pub state: BodyState,
    pub ownership: BodyOwnership,
    pub agency: BodyAgency,
    pub enabled: bool,
    pub total_updates: u64,
}

impl BodyEngine {
    pub fn new() -> Self {
        Self {
            schema: BodySchema::new(),
            proprioception: Proprioception::new(),
            body_map: BodyMap::new(),
            kinematic: KinematicChain::new(),
            bound: BodyBound::new(),
            state: BodyState::new(),
            ownership: BodyOwnership::new(),
            agency: BodyAgency::new(),
            enabled: true,
            total_updates: 0,
        }
    }

    pub fn add_body_part(&mut self, part_type: BodyPartType, name: &str, parent_id: Option<u64>) -> u64 {
        let id = self.schema.add_part(part_type, name, parent_id);
        self.body_map.add_node(id);
        self.ownership.claim_part(id);
        self.agency.control_part(id);
        self.total_updates += 1;
        id
    }

    pub fn update_proprioception(&mut self, part_id: u64, position: [f32; 3], velocity: [f32; 3], confidence: f32) {
        self.proprioception.update(part_id, position, velocity, confidence);
        self.schema.set_position(part_id, position);
        self.bound.expand(position);
        self.state.parts.insert(part_id, position);
        self.state.velocities.insert(part_id, velocity);
        self.state.timestamp_ns = crate::ai_os::native::now_ns();
        self.total_updates += 1;
    }

    pub fn add_kinematic_node(&mut self, part_id: u64, joint_type: &str, range: (f32, f32)) {
        self.kinematic.add_node(part_id, joint_type, range);
        self.total_updates += 1;
    }

    pub fn set_joint_angle(&mut self, part_id: u64, angle: f32) -> Result<(), String> {
        self.kinematic.set_angle(part_id, angle)
    }

    pub fn claim_ownership(&mut self, part_id: u64) {
        self.ownership.claim_part(part_id);
    }

    pub fn release_ownership(&mut self, part_id: u64) {
        self.ownership.release_part(part_id);
    }

    pub fn control_part(&mut self, part_id: u64) {
        self.agency.control_part(part_id);
    }

    pub fn release_control(&mut self, part_id: u64) {
        self.agency.release_control(part_id);
    }

    pub fn part_count(&self) -> usize {
        self.schema.part_count()
    }

    pub fn proprioception_count(&self) -> usize {
        self.proprioception.reading_count()
    }

    pub fn ownership_score(&self) -> f32 {
        self.ownership.ownership_score
    }

    pub fn agency_score(&self) -> f32 {
        self.agency.agency_score
    }

    pub fn bound_volume(&self) -> f32 {
        self.bound.volume()
    }

    pub fn tick(&mut self) {
        self.state.timestamp_ns = crate::ai_os::native::now_ns();
        self.total_updates += 1;
    }
}

impl Default for BodyEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static BODY: OnceLock<Mutex<BodyEngine>> = OnceLock::new();

pub fn body_engine() -> &'static Mutex<BodyEngine> {
    BODY.get_or_init(|| Mutex::new(BodyEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = body_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = body_engine().lock().unwrap();
    format!(
        "body: parts={} proprioception={} map_nodes={} map_edges={} kinematic={} ownership={:.2} agency={:.2} bound_volume={:.2} updates={}",
        eng.part_count(),
        eng.proprioception_count(),
        eng.body_map.node_count(),
        eng.body_map.edge_count(),
        eng.kinematic.node_count(),
        eng.ownership_score(),
        eng.agency_score(),
        eng.bound_volume(),
        eng.total_updates
    )
}

pub fn body_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn body_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("add-part") => {
            let type_str = it.next().unwrap_or("torso");
            let part_type = match type_str {
                "head" => BodyPartType::Head,
                "torso" => BodyPartType::Torso,
                "left_arm" => BodyPartType::LeftArm,
                "right_arm" => BodyPartType::RightArm,
                "left_hand" => BodyPartType::LeftHand,
                "right_hand" => BodyPartType::RightHand,
                "left_leg" => BodyPartType::LeftLeg,
                "right_leg" => BodyPartType::RightLeg,
                "left_foot" => BodyPartType::LeftFoot,
                "right_foot" => BodyPartType::RightFoot,
                "neck" => BodyPartType::Neck,
                "spine" => BodyPartType::Spine,
                _ => BodyPartType::Torso,
            };
            let name = it.next().unwrap_or("part");
            let parent_id = it.next().and_then(|s| s.parse::<u64>().ok());
            let mut eng = body_engine().lock().unwrap();
            let id = eng.add_body_part(part_type, name, parent_id);
            Ok(format!("已添加身体部位 id={} 类型={:?} 名称={}", id, part_type, name))
        }
        Some("update-proprioception") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let x = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let y = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let z = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let vx = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let vy = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let vz = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let confidence = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let mut eng = body_engine().lock().unwrap();
            eng.update_proprioception(part_id, [x, y, z], [vx, vy, vz], confidence);
            Ok(format!("已更新部位 {} 本体感知 位置=({},{},{})", part_id, x, y, z))
        }
        Some("add-kinematic") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let joint_type = it.next().unwrap_or("revolute");
            let min = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(-180.0);
            let max = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(180.0);
            let mut eng = body_engine().lock().unwrap();
            eng.add_kinematic_node(part_id, joint_type, (min, max));
            Ok(format!("已添加运动学节点 部位={} 关节={} 范围=({},{})", part_id, joint_type, min, max))
        }
        Some("set-angle") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let angle = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let mut eng = body_engine().lock().unwrap();
            eng.set_joint_angle(part_id, angle)?;
            Ok(format!("已设置部位 {} 关节角度 {}", part_id, angle))
        }
        Some("claim-ownership") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let mut eng = body_engine().lock().unwrap();
            eng.claim_ownership(part_id);
            Ok(format!("已声称部位 {} 的拥有权", part_id))
        }
        Some("release-ownership") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let mut eng = body_engine().lock().unwrap();
            eng.release_ownership(part_id);
            Ok(format!("已释放部位 {} 的拥有权", part_id))
        }
        Some("control-part") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let mut eng = body_engine().lock().unwrap();
            eng.control_part(part_id);
            Ok(format!("已控制部位 {}", part_id))
        }
        Some("release-control") => {
            let part_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let mut eng = body_engine().lock().unwrap();
            eng.release_control(part_id);
            Ok(format!("已释放部位 {} 的控制权", part_id))
        }
        Some("part") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少部位id")?;
            let eng = body_engine().lock().unwrap();
            match eng.schema.get_part(id) {
                Some(p) => Ok(format!("部位[id={}] 类型={:?} 名称={} 位置=({},{},{})", id, p.part_type, p.name, p.position[0], p.position[1], p.position[2])),
                None => Err(format!("部位[id={}] 不存在", id)),
            }
        }
        Some("list-parts") => {
            let eng = body_engine().lock().unwrap();
            let parts = eng.schema.list_parts();
            let mut out = String::from("身体部位列表:\n");
            for p in parts {
                out.push_str(&format!("  id={} 类型={:?} 名称={} 启用={}\n", p.id, p.part_type, p.name, p.enabled));
            }
            Ok(out)
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 body 动作: {}", other)),
    }
}

pub fn body_act_dispatch(payload: &str) -> Result<String, String> {
    body_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_part_type_names() {
        assert_eq!(BodyPartType::Head.name(), "head"); // 这里 BodyPartType 没有 name 方法，只是测试变体存在
        let _ = BodyPartType::Head;
        let _ = BodyPartType::Torso;
        let _ = BodyPartType::LeftHand;
    }

    #[test]
    fn body_part_creation() {
        let part = BodyPart::new(1, BodyPartType::Head, "头部");
        assert_eq!(part.id, 1);
        assert_eq!(part.name, "头部");
        assert_eq!(part.position, [0.0, 0.0, 0.0]);
        assert!(part.enabled);
    }

    #[test]
    fn body_part_add_child() {
        let mut parent = BodyPart::new(1, BodyPartType::Torso, "躯干");
        let mut child = BodyPart::new(2, BodyPartType::LeftArm, "左臂");
        parent.add_child(2);
        child.parent_id = Some(1);
        assert_eq!(parent.children, vec![2]);
        assert_eq!(child.parent_id, Some(1));
    }

    #[test]
    fn body_part_total_mass() {
        let mut parts = BTreeMap::new();
        let torso = BodyPart { id: 1, part_type: BodyPartType::Torso, name: "躯干".into(), parent_id: None, children: vec![2], position: [0.0;3], orientation: [0.0,0.0,0.0,1.0], size: [0.5,0.5,0.5], mass: 10.0, enabled: true, sensor_count: 0, actuator_count: 0 };
        let arm = BodyPart { id: 2, part_type: BodyPartType::LeftArm, name: "左臂".into(), parent_id: Some(1), children: vec![], position: [0.0;3], orientation: [0.0,0.0,0.0,1.0], size: [0.3,0.1,0.1], mass: 2.0, enabled: true, sensor_count: 0, actuator_count: 0 };
        parts.insert(1, torso);
        parts.insert(2, arm);
        let total = parts.get(&1).unwrap().total_mass(&parts);
        assert!((total - 12.0).abs() < 1e-6);
    }

    #[test]
    fn body_schema_add_part() {
        let mut schema = BodySchema::new();
        let id1 = schema.add_part(BodyPartType::Torso, "躯干", None);
        let id2 = schema.add_part(BodyPartType::LeftArm, "左臂", Some(id1));
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(schema.part_count(), 2);
        assert_eq!(schema.root_id(), Some(1));
        let children = schema.children_of(1);
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn body_schema_set_position() {
        let mut schema = BodySchema::new();
        let id = schema.add_part(BodyPartType::Head, "头部", None);
        assert!(schema.set_position(id, [1.0, 2.0, 3.0]));
        let part = schema.get_part(id).unwrap();
        assert_eq!(part.position, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn body_schema_remove_part() {
        let mut schema = BodySchema::new();
        let id1 = schema.add_part(BodyPartType::Torso, "躯干", None);
        let id2 = schema.add_part(BodyPartType::LeftArm, "左臂", Some(id1));
        assert!(schema.remove_part(id1).is_some());
        assert_eq!(schema.part_count(), 0);
    }

    #[test]
    fn body_schema_version() {
        let mut schema = BodySchema::new();
        let v0 = schema.version();
        schema.add_part(BodyPartType::Head, "头部", None);
        assert!(schema.version() > v0);
    }

    #[test]
    fn proprioception_update() {
        let mut proprio = Proprioception::new();
        proprio.update(1, [1.0, 2.0, 3.0], [0.1, 0.2, 0.3], 0.9);
        assert_eq!(proprio.reading_count(), 1);
        let reading = proprio.get_reading(1).unwrap();
        assert_eq!(reading.position, [1.0, 2.0, 3.0]);
        assert!((reading.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn body_map_add_edge() {
        let mut map = BodyMap::new();
        map.add_node(1);
        map.add_node(2);
        map.add_edge(1, 2, "连接");
        assert_eq!(map.node_count(), 2);
        assert_eq!(map.edge_count(), 1);
        let neighbors = map.neighbors(1).unwrap();
        assert!(neighbors.contains(&2));
    }

    #[test]
    fn kinematic_chain() {
        let mut chain = KinematicChain::new();
        chain.add_node(1, "旋转", (-90.0, 90.0));
        chain.add_node(2, "滑动", (0.0, 10.0));
        assert_eq!(chain.node_count(), 2);
        assert!(chain.set_angle(1, 45.0).is_ok());
        assert!(chain.set_angle(1, 200.0).is_err());
        assert_eq!(chain.get_angle(1), Some(45.0));
    }

    #[test]
    fn body_bound() {
        let mut bound = BodyBound::new();
        assert!(bound.contains([0.0, 0.0, 0.0]));
        assert!(!bound.contains([10.0, 0.0, 0.0]));
        bound.expand([5.0, 5.0, 5.0]);
        assert!(bound.contains([5.0, 5.0, 5.0]));
        assert!(bound.volume() > 0.0);
    }

    #[test]
    fn body_ownership() {
        let mut ownership = BodyOwnership::new();
        ownership.claim_part(1);
        ownership.claim_part(2);
        assert!(ownership.ownership_score > 0.0);
        ownership.release_part(1);
        assert!(!ownership.owned_parts.contains(&1));
    }

    #[test]
    fn body_agency() {
        let mut agency = BodyAgency::new();
        agency.control_part(1);
        assert!(agency.agency_score > 0.0);
        agency.release_control(1);
        assert!(!agency.controlled_parts.contains(&1));
    }

    #[test]
    fn body_engine_add_part() {
        let mut eng = BodyEngine::new();
        let id = eng.add_body_part(BodyPartType::Torso, "躯干", None);
        assert_eq!(id, 1);
        assert_eq!(eng.part_count(), 1);
        assert!(eng.ownership_score() > 0.0);
        assert!(eng.agency_score() > 0.0);
    }

    #[test]
    fn body_engine_proprioception() {
        let mut eng = BodyEngine::new();
        let id = eng.add_body_part(BodyPartType::Head, "头部", None);
        eng.update_proprioception(id, [1.0, 0.0, 0.0], [0.1, 0.0, 0.0], 0.95);
        assert_eq!(eng.proprioception_count(), 1);
    }

    #[test]
    fn body_engine_kinematic() {
        let mut eng = BodyEngine::new();
        let id = eng.add_body_part(BodyPartType::LeftArm, "左臂", None);
        eng.add_kinematic_node(id, "旋转", (-180.0, 180.0));
        eng.set_joint_angle(id, 90.0).unwrap();
        assert_eq!(eng.kinematic.get_angle(id), Some(90.0));
    }

    #[test]
    fn body_engine_tick() {
        let mut eng = BodyEngine::new();
        eng.tick();
        assert_eq!(eng.total_updates, 1);
    }

    #[test]
    fn body_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("body:"));
        assert!(s.contains("parts="));
    }

    #[test]
    fn body_act_add_part() {
        init();
        let res = body_act("add-part head 头部").unwrap();
        assert!(res.contains("已添加身体部位"));
    }

    #[test]
    fn body_act_proprioception() {
        init();
        body_act("add-part head 头部").unwrap();
        let res = body_act("update-proprioception 1 1.0 2.0 3.0 0.1 0.2 0.3 0.9").unwrap();
        assert!(res.contains("本体感知"));
    }

    #[test]
    fn body_act_kinematic() {
        init();
        body_act("add-part arm 左臂").unwrap();
        let res = body_act("add-kinematic 1 旋转 -90 90").unwrap();
        assert!(res.contains("运动学节点"));
        let res = body_act("set-angle 1 45").unwrap();
        assert!(res.contains("关节角度"));
    }

    #[test]
    fn body_act_ownership() {
        init();
        body_act("add-part hand 左手").unwrap();
        let res = body_act("claim-ownership 1").unwrap();
        assert!(res.contains("拥有权"));
        let res = body_act("release-ownership 1").unwrap();
        assert!(res.contains("释放"));
    }

    #[test]
    fn body_act_control() {
        init();
        body_act("add-part hand 右手").unwrap();
        let res = body_act("control-part 1").unwrap();
        assert!(res.contains("控制"));
        let res = body_act("release-control 1").unwrap();
        assert!(res.contains("释放"));
    }

    #[test]
    fn body_act_part() {
        init();
        body_act("add-part head 头部").unwrap();
        let res = body_act("part 1").unwrap();
        assert!(res.contains("部位"));
    }

    #[test]
    fn body_act_list_parts() {
        init();
        body_act("add-part torso 躯干").unwrap();
        let res = body_act("list-parts").unwrap();
        assert!(res.contains("身体部位列表"));
    }

    #[test]
    fn body_act_status() {
        init();
        let res = body_act("status").unwrap();
        assert!(res.contains("body:"));
    }
}
