//! WASM 沙箱运行时 —— 加载 AI 生成的 Rust 代码编译产物并执行约定 ABI。
//!
//! 使用 wasmtime + wasm32-unknown-unknown 目标：模块无法访问文件系统/网络/进程，
//! 只能通过宿主显式导出的函数交互（当前版本仅纯计算 ABI）。
//!
//! 四类创造物（App / Plugin / IME / Animation）共用一套加载器，
//! 通过调用不同 ABI 函数实现能力。

use std::sync::{Mutex, OnceLock};

use wasmtime::{Engine, Instance, Module, Store};

/// 创造物类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Plugin,
    Ime,
    Animation,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Plugin => "plugin",
            Kind::Ime => "ime",
            Kind::Animation => "animation",
        }
    }
}

/// 一个已加载的 WASM 实例（线程安全封装）。
pub struct WasmInstance {
    inner: Mutex<InstanceState>,
}

struct InstanceState {
    store: Store<()>,
    instance: Instance,
}

impl WasmInstance {
    /// 从 wasm 字节加载并实例化。
    pub fn load(wasm: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm).map_err(|e| format!("WASM 模块无效: {}", e))?;
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| format!("WASM 实例化失败: {}", e))?;
        Ok(Self {
            inner: Mutex::new(InstanceState { store, instance }),
        })
    }

    /// 调用无参 i32 返回函数（如 init）。
    pub fn call_i32(&self, name: &str) -> Result<i32, String> {
        let mut s = self.inner.lock().unwrap();
        let InstanceState { store, instance } = &mut *s;
        let f = instance
            .get_typed_func::<(), i32>(&mut *store, name)
            .map_err(|_| format!("WASM 缺少 ABI: {}", name))?;
        f.call(&mut *store, ())
            .map_err(|e| format!("WASM {} 调用失败: {}", name, e))
    }

    /// 调用 (i32) -> i32 函数（tick / on_input / map_key）。
    pub fn call_i32_i32(&self, name: &str, a: i32) -> Result<i32, String> {
        let mut s = self.inner.lock().unwrap();
        let InstanceState { store, instance } = &mut *s;
        let f = instance
            .get_typed_func::<i32, i32>(&mut *store, name)
            .map_err(|_| format!("WASM 缺少 ABI: {}", name))?;
        f.call(&mut *store, a)
            .map_err(|e| format!("WASM {} 调用失败: {}", name, e))
    }

    /// 调用 (i32,i32) -> i32 函数（on_event / candidates）。
    pub fn call_i32_i32_i32(&self, name: &str, a: i32, b: i32) -> Result<i32, String> {
        let mut s = self.inner.lock().unwrap();
        let InstanceState { store, instance } = &mut *s;
        let f = instance
            .get_typed_func::<(i32, i32), i32>(&mut *store, name)
            .map_err(|_| format!("WASM 缺少 ABI: {}", name))?;
        f.call(&mut *store, (a, b))
            .map_err(|e| format!("WASM {} 调用失败: {}", name, e))
    }

    /// 调用 (i32,i32,i32) -> () 函数（render）。
    pub fn call_render(&self, ptr: i32, w: i32, h: i32) -> Result<(), String> {
        let mut s = self.inner.lock().unwrap();
        let InstanceState { store, instance } = &mut *s;
        let f = instance
            .get_typed_func::<(i32, i32, i32), ()>(&mut *store, "render")
            .map_err(|_| "WASM 缺少 ABI: render".to_string())?;
        f.call(&mut *store, (ptr, w, h))
            .map_err(|e| format!("WASM render 调用失败: {}", e))
    }

    /// 调用 (i32) -> i64 函数（animate）。
    pub fn call_i32_i64(&self, name: &str, a: i32) -> Result<i64, String> {
        let mut s = self.inner.lock().unwrap();
        let InstanceState { store, instance } = &mut *s;
        let f = instance
            .get_typed_func::<i32, i64>(&mut *store, name)
            .map_err(|_| format!("WASM 缺少 ABI: {}", name))?;
        f.call(&mut *store, a)
            .map_err(|e| format!("WASM {} 调用失败: {}", name, e))
    }

    /// 校验某组 ABI 是否齐全（load 后立即调用）。
    pub fn has_abi(&self, names: &[&str]) -> bool {
        let mut s = self.inner.lock().unwrap();
        let InstanceState { store, instance } = &mut *s;
        names
            .iter()
            .all(|n| instance.get_func(&mut *store, n).is_some())
    }
}

/// 全局加载实例注册表（name -> instance）。
static INSTANCES: OnceLock<Mutex<std::collections::BTreeMap<String, std::sync::Arc<WasmInstance>>>> =
    OnceLock::new();

pub fn instances() -> &'static Mutex<std::collections::BTreeMap<String, std::sync::Arc<WasmInstance>>> {
    INSTANCES.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
}

pub fn register(name: &str, inst: WasmInstance) {
    instances()
        .lock()
        .unwrap()
        .insert(name.to_string(), std::sync::Arc::new(inst));
}

pub fn get(name: &str) -> Option<std::sync::Arc<WasmInstance>> {
    instances().lock().unwrap().get(name).cloned()
}

pub fn list() -> Vec<String> {
    instances().lock().unwrap().keys().cloned().collect()
}

/// 所需 ABI 集合。
pub fn required_abi(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::App => &["init", "tick", "render", "on_input"],
        Kind::Plugin => &["init", "on_event"],
        Kind::Ime => &["init", "map_key"],
        Kind::Animation => &["init", "animate"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_WAT: &str = r#"(module
        (func (export "init") (result i32) i32.const 7)
    )"#;

    #[test]
    fn load_and_call_init() {
        let wasm = wat::parse_str(MIN_WAT).unwrap();
        let inst = WasmInstance::load(&wasm).unwrap();
        assert_eq!(inst.call_i32("init").unwrap(), 7);
    }

    #[test]
    fn missing_abi_reports_error() {
        let wasm = wat::parse_str(MIN_WAT).unwrap();
        let inst = WasmInstance::load(&wasm).unwrap();
        assert!(inst.call_i32("tick").is_err());
        assert!(!inst.has_abi(&["tick"]));
    }

    #[test]
    fn kind_names() {
        assert_eq!(Kind::App.name(), "app");
        assert_eq!(Kind::Plugin.name(), "plugin");
        assert_eq!(Kind::Ime.name(), "ime");
        assert_eq!(Kind::Animation.name(), "animation");
    }

    #[test]
    fn app_abi_tick_render_input() {
        let wat_str = r#"(module
            (func (export "init") (result i32) i32.const 1)
            (func (export "tick") (param i32))
            (func (export "render") (param i32 i32 i32))
            (func (export "on_input") (param i32) (result i32) local.get 0)
        )"#;
        let wasm = wat::parse_str(wat_str).unwrap();
        let inst = WasmInstance::load(&wasm).unwrap();
        assert!(inst.has_abi(required_abi(Kind::App)));
        assert_eq!(inst.call_i32_i32("on_input", 42).unwrap(), 42);
        assert!(inst.call_render(0, 10, 10).is_ok());
    }

    #[test]
    fn plugin_ime_animation_abi() {
        let wat_str = r#"(module
            (func (export "init") (result i32) i32.const 1)
            (func (export "on_event") (param i32 i32) (result i32) i32.const 9)
            (func (export "map_key") (param i32) (result i32) i32.const 65)
            (func (export "animate") (param i32) (result i64) i64.const 123)
        )"#;
        let wasm = wat::parse_str(wat_str).unwrap();
        let inst = WasmInstance::load(&wasm).unwrap();
        assert!(inst.has_abi(required_abi(Kind::Plugin)));
        assert!(inst.has_abi(required_abi(Kind::Ime)));
        assert!(inst.has_abi(required_abi(Kind::Animation)));
        assert_eq!(inst.call_i32_i32_i32("on_event", 0, 0).unwrap(), 9);
        assert_eq!(inst.call_i32_i32("map_key", 0).unwrap(), 65);
        assert_eq!(inst.call_i32_i64("animate", 0).unwrap(), 123);
    }
}
