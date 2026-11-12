//! 信号机制。
//!
//! 模拟类 Unix 信号：定义信号枚举（SIGKILL/SIGSTOP/SIGCONT/SIGTERM/SIGINT/
//! SIGSEGV/SIGCHLD 等），支持为进程注册信号处理函数、发送信号、信号屏蔽
//! 掩码、未决信号集与信号投递（deliver_signals）。
//!
//! 信号处理支持三种动作：默认动作、忽略、自定义处理函数。
//!
//! 本模块为纯内存模拟，不操作真实进程，适合信号子系统的原型验证。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 信号枚举
// =====================================================================================

/// 信号编号与名称。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    /// 挂断（终端断开）。
    SIGHUP = 1,
    /// 中断（键盘中断 Ctrl+C）。
    SIGINT = 2,
    /// 退出（quit）。
    SIGQUIT = 3,
    /// 非法指令。
    SIGILL = 4,
    /// 捕获/断点。
    SIGTRAP = 5,
    /// 中止。
    SIGABRT = 6,
    /// 浮点异常。
    SIGFPE = 8,
    /// 无效内存引用（段错误）。
    SIGSEGV = 11,
    /// 终止（可捕获、可阻塞）。
    SIGTERM = 15,
    /// 子进程状态变更。
    SIGCHLD = 17,
    /// 继续（若处于停止状态）。
    SIGCONT = 18,
    /// 停止（不可捕获、不可阻塞）。
    SIGSTOP = 19,
    /// 终止（不可捕获、不可阻塞，强制杀死）。
    SIGKILL = 9,
    /// 用户自定义 1。
    SIGUSR1 = 10,
    /// 用户自定义 2。
    SIGUSR2 = 12,
    /// 管道破裂。
    SIGPIPE = 13,
    /// 报时器超时。
    SIGALRM = 14,
}

impl Signal {
    /// 返回信号编号。
    pub fn number(self) -> i32 {
        self as i32
    }

    /// 返回信号名称。
    pub fn name(self) -> &'static str {
        match self {
            Signal::SIGHUP => "SIGHUP",
            Signal::SIGINT => "SIGINT",
            Signal::SIGQUIT => "SIGQUIT",
            Signal::SIGILL => "SIGILL",
            Signal::SIGTRAP => "SIGTRAP",
            Signal::SIGABRT => "SIGABRT",
            Signal::SIGFPE => "SIGFPE",
            Signal::SIGSEGV => "SIGSEGV",
            Signal::SIGTERM => "SIGTERM",
            Signal::SIGCHLD => "SIGCHLD",
            Signal::SIGCONT => "SIGCONT",
            Signal::SIGSTOP => "SIGSTOP",
            Signal::SIGKILL => "SIGKILL",
            Signal::SIGUSR1 => "SIGUSR1",
            Signal::SIGUSR2 => "SIGUSR2",
            Signal::SIGPIPE => "SIGPIPE",
            Signal::SIGALRM => "SIGALRM",
        }
    }

    /// 是否不可捕获且不可阻塞（SIGKILL / SIGSTOP）。
    pub fn uncatchable(self) -> bool {
        matches!(self, Signal::SIGKILL | Signal::SIGSTOP)
    }

    /// 默认动作描述。
    pub fn default_action_desc(self) -> &'static str {
        match self {
            Signal::SIGKILL | Signal::SIGTERM | Signal::SIGINT | Signal::SIGQUIT => "终止进程",
            Signal::SIGSEGV | Signal::SIGABRT | Signal::SIGILL | Signal::SIGFPE => "终止并转储核心",
            Signal::SIGSTOP => "停止进程",
            Signal::SIGCONT => "继续进程",
            Signal::SIGCHLD => "忽略",
            Signal::SIGHUP => "终止进程",
            Signal::SIGUSR1 | Signal::SIGUSR2 => "终止进程",
            Signal::SIGPIPE => "忽略",
            Signal::SIGALRM => "终止进程",
            Signal::SIGTRAP => "终止并转储核心",
        }
    }
}

// =====================================================================================
// 信号处理方式
// =====================================================================================

/// 信号处理方式。
pub enum SignalAction {
    /// 默认动作。
    Default,
    /// 忽略该信号。
    Ignore,
    /// 自定义处理函数：(signal) -> 是否继续运行。
    Handler(Box<dyn Fn(Signal) -> bool + Send + Sync>),
}

impl Clone for SignalAction {
    fn clone(&self) -> Self {
        match self {
            SignalAction::Default => SignalAction::Default,
            SignalAction::Ignore => SignalAction::Ignore,
            SignalAction::Handler(_) => SignalAction::Default, // 闭包不可克隆，降级为默认
        }
    }
}

impl std::fmt::Debug for SignalAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalAction::Default => write!(f, "Default"),
            SignalAction::Ignore => write!(f, "Ignore"),
            SignalAction::Handler(_) => write!(f, "Handler(<fn>)"),
        }
    }
}

impl SignalAction {
    /// 返回某信号的默认处理方式。
    pub fn default_for(signal: Signal) -> Self {
        match signal {
            Signal::SIGKILL | Signal::SIGSTOP => SignalAction::Default,
            Signal::SIGCHLD | Signal::SIGPIPE => SignalAction::Ignore,
            _ => SignalAction::Default,
        }
    }
}

// =====================================================================================
// 信号处理器：每进程的信号状态
// =====================================================================================

/// 进程标识符。
pub type Pid = u32;

/// 单进程的信号状态：处理方式表、屏蔽掩码、未决信号集。
#[derive(Clone, Debug)]
pub struct ProcessSignalState {
    pub pid: Pid,
    /// 信号 -> 处理方式。
    actions: BTreeMap<Signal, SignalAction>,
    /// 信号屏蔽掩码（被屏蔽的信号集）。
    mask: HashSet<Signal>,
    /// 未决信号集（已发送但未投递的信号）。
    pending: HashSet<Signal>,
    /// 信号计数：每个信号被发送的次数。
    sent_count: BTreeMap<Signal, u64>,
    /// 信号投递次数。
    delivered_count: BTreeMap<Signal, u64>,
    /// 进程是否已终止（由信号导致）。
    pub terminated: bool,
    /// 终止信号（若 terminated 为 true）。
    pub term_signal: Option<Signal>,
}

impl ProcessSignalState {
    pub fn new(pid: Pid) -> Self {
        Self {
            pid,
            actions: BTreeMap::new(),
            mask: HashSet::new(),
            pending: HashSet::new(),
            sent_count: BTreeMap::new(),
            delivered_count: BTreeMap::new(),
            terminated: false,
            term_signal: None,
        }
    }

    /// 注册信号处理方式。
    pub fn register_action(&mut self, signal: Signal, action: SignalAction) {
        if signal.uncatchable() {
            // SIGKILL / SIGSTOP 不可改变处理方式，保持默认。
            return;
        }
        self.actions.insert(signal, action);
    }

    /// 将信号加入屏蔽掩码。
    pub fn mask_add(&mut self, signal: Signal) {
        if signal.uncatchable() {
            return;
        }
        self.mask.insert(signal);
    }

    /// 将信号从屏蔽掩码移除。
    pub fn mask_remove(&mut self, signal: Signal) {
        self.mask.remove(&signal);
    }

    /// 判断信号当前是否被屏蔽。
    pub fn is_masked(&self, signal: Signal) -> bool {
        self.mask.contains(&signal)
    }

    /// 发送信号：若未被屏蔽则加入未决集；若已屏蔽则仍记入 sent_count 但不进入 pending。
    pub fn send(&mut self, signal: Signal) {
        *self.sent_count.entry(signal).or_insert(0) += 1;
        if self.terminated {
            return;
        }
        if self.is_masked(signal) {
            // 被屏蔽：不进入 pending，但记录已发送。
            return;
        }
        self.pending.insert(signal);
    }

    /// 返回未决信号集。
    pub fn pending(&self) -> HashSet<Signal> {
        self.pending.clone()
    }

    /// 返回是否有未决信号。
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 投递所有未决信号：按优先级顺序处理，执行对应处理方式。
    ///
    /// 返回被投递的信号列表。
    pub fn deliver(&mut self) -> Vec<Signal> {
        let mut delivered: Vec<Signal> = Vec::new();
        // 按信号编号排序，确保确定性顺序。
        let mut pendings: Vec<Signal> = self.pending.iter().copied().collect();
        pendings.sort_by_key(|s| s.number());
        self.pending.clear();
        for signal in pendings {
            if self.terminated {
                break;
            }
            // SIGKILL / SIGSTOP 始终强制执行默认动作。
            let action = if signal.uncatchable() {
                SignalAction::Default
            } else {
                self.actions.get(&signal).cloned().unwrap_or_else(|| SignalAction::default_for(signal))
            };
            *self.delivered_count.entry(signal).or_insert(0) += 1;
            delivered.push(signal);
            match action {
                SignalAction::Default => {
                    self._apply_default_action(signal);
                }
                SignalAction::Ignore => {
                    // 不做任何事。
                }
                SignalAction::Handler(cb) => {
                    let continue_running = cb(signal);
                    if !continue_running {
                        self.terminated = true;
                        self.term_signal = Some(signal);
                        break;
                    }
                }
            }
        }
        delivered
    }

    /// 应用默认动作。
    fn _apply_default_action(&mut self, signal: Signal) {
        match signal {
            Signal::SIGKILL | Signal::SIGTERM | Signal::SIGINT | Signal::SIGQUIT
            | Signal::SIGSEGV | Signal::SIGABRT | Signal::SIGILL | Signal::SIGFPE
            | Signal::SIGALRM | Signal::SIGUSR1 | Signal::SIGUSR2 | Signal::SIGTRAP => {
                self.terminated = true;
                self.term_signal = Some(signal);
            }
            Signal::SIGSTOP => {
                // 停止进程：此处标记为 terminated 以简化模拟。
                self.terminated = true;
                self.term_signal = Some(signal);
            }
            Signal::SIGCONT => {
                // 继续：若之前停止，则恢复（此处仅清除终止标记，模拟恢复）。
                self.terminated = false;
                self.term_signal = None;
            }
            Signal::SIGCHLD | Signal::SIGPIPE => {
                // 忽略。
            }
            Signal::SIGHUP => {
                self.terminated = true;
                self.term_signal = Some(signal);
            }
        }
    }

    /// 返回信号被发送的次数。
    pub fn sent_count(&self, signal: Signal) -> u64 {
        self.sent_count.get(&signal).copied().unwrap_or(0)
    }

    /// 返回信号被投递的次数。
    pub fn delivered_count(&self, signal: Signal) -> u64 {
        self.delivered_count.get(&signal).copied().unwrap_or(0)
    }

    /// 返回进程是否已终止。
    pub fn is_terminated(&self) -> bool {
        self.terminated
    }

    /// 返回终止信号。
    pub fn term_signal(&self) -> Option<Signal> {
        self.term_signal
    }

    /// 返回文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "pid={} terminated={} term_signal={:?} pending={} masked={}",
            self.pid,
            self.terminated,
            self.term_signal.map(|s| s.name()),
            self.pending.len(),
            self.mask.len(),
        )
    }
}

// =====================================================================================
// SignalHandler —— 全局信号处理器
// =====================================================================================

/// 全局信号处理器：管理所有进程的信号状态，提供注册处理函数、发送信号、
/// 屏蔽掩码、未决信号查询与投递接口。
pub struct SignalHandler {
    /// pid -> 进程信号状态。
    processes: BTreeMap<Pid, ProcessSignalState>,
    /// 默认处理方式表（供未注册的进程使用）。
    default_actions: BTreeMap<Signal, SignalAction>,
    /// 全局统计。
    total_sent: u64,
    total_delivered: u64,
}

impl SignalHandler {
    pub fn new() -> Self {
        let mut default_actions = BTreeMap::new();
        for sig in [
            Signal::SIGHUP,
            Signal::SIGINT,
            Signal::SIGQUIT,
            Signal::SIGILL,
            Signal::SIGTRAP,
            Signal::SIGABRT,
            Signal::SIGFPE,
            Signal::SIGSEGV,
            Signal::SIGTERM,
            Signal::SIGCHLD,
            Signal::SIGCONT,
            Signal::SIGSTOP,
            Signal::SIGKILL,
            Signal::SIGUSR1,
            Signal::SIGUSR2,
            Signal::SIGPIPE,
            Signal::SIGALRM,
        ] {
            default_actions.insert(sig, SignalAction::default_for(sig));
        }
        Self {
            processes: BTreeMap::new(),
            default_actions,
            total_sent: 0,
            total_delivered: 0,
        }
    }

    /// 获取或创建进程信号状态。
    fn process_state_mut(&mut self, pid: Pid) -> &mut ProcessSignalState {
        self.processes.entry(pid).or_insert_with(|| ProcessSignalState::new(pid))
    }

    /// 获取进程信号状态（不可变）。
    fn process_state(&self, pid: Pid) -> Option<&ProcessSignalState> {
        self.processes.get(&pid)
    }

    /// 为进程注册信号处理方式。
    pub fn register_handler(&mut self, pid: Pid, signal: Signal, action: SignalAction) {
        self.process_state_mut(pid).register_action(signal, action);
    }

    /// 为进程注册自定义处理函数。
    pub fn register_handler_fn<F>(&mut self, pid: Pid, signal: Signal, handler: F)
    where
        F: Fn(Signal) -> bool + Send + Sync + 'static,
    {
        self.process_state_mut(pid)
            .register_action(signal, SignalAction::Handler(Box::new(handler)));
    }

    /// 返回进程对某信号的处理方式（未注册则返回默认）。
    pub fn default_handler(&self, signal: Signal) -> SignalAction {
        self.default_actions
            .get(&signal)
            .cloned()
            .unwrap_or(SignalAction::Default)
    }

    /// 向进程发送信号。
    pub fn send_signal(&mut self, pid: Pid, signal: Signal) {
        self.total_sent += 1;
        self.process_state_mut(pid).send(signal);
    }

    /// 将信号加入进程屏蔽掩码。
    pub fn signal_mask_add(&mut self, pid: Pid, signal: Signal) {
        self.process_state_mut(pid).mask_add(signal);
    }

    /// 将信号从进程屏蔽掩码移除。
    pub fn signal_mask_remove(&mut self, pid: Pid, signal: Signal) {
        self.process_state_mut(pid).mask_remove(signal);
    }

    /// 返回进程的未决信号集。
    pub fn pending_signals(&self, pid: Pid) -> HashSet<Signal> {
        self.process_state(pid)
            .map(|s| s.pending())
            .unwrap_or_default()
    }

    /// 投递指定进程的所有未决信号，返回已投递信号列表。
    pub fn deliver_signals(&mut self, pid: Pid) -> Vec<Signal> {
        let delivered = self.process_state_mut(pid).deliver();
        self.total_delivered += delivered.len() as u64;
        delivered
    }

    /// 返回进程是否已终止。
    pub fn is_terminated(&self, pid: Pid) -> bool {
        self.process_state(pid)
            .map(|s| s.is_terminated())
            .unwrap_or(false)
    }

    /// 返回进程的终止信号。
    pub fn term_signal(&self, pid: Pid) -> Option<Signal> {
        self.process_state(pid).and_then(|s| s.term_signal())
    }

    /// 返回进程信号状态摘要。
    pub fn process_summary(&self, pid: Pid) -> Option<String> {
        self.process_state(pid).map(|s| s.summary())
    }

    /// 返回总发送次数。
    pub fn total_sent(&self) -> u64 {
        self.total_sent
    }

    /// 返回指定进程对指定信号的发送次数。
    pub fn sent_count(&self, pid: Pid, signal: Signal) -> u64 {
        self.process_state(pid)
            .map(|s| s.sent_count(signal))
            .unwrap_or(0)
    }

    /// 返回总投递次数。
    pub fn total_delivered(&self) -> u64 {
        self.total_delivered
    }

    /// 返回文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "signal_handler processes={} total_sent={} total_delivered={}",
            self.processes.len(),
            self.total_sent,
            self.total_delivered,
        )
    }
}

impl Default for SignalHandler {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static SIGNAL_HANDLER: OnceLock<Mutex<SignalHandler>> = OnceLock::new();

fn signal_handler() -> &'static Mutex<SignalHandler> {
    SIGNAL_HANDLER.get_or_init(|| Mutex::new(SignalHandler::new()))
}

/// 为进程注册信号处理方式。
pub fn signal_register_handler(pid: Pid, signal: Signal, action: SignalAction) {
    signal_handler().lock().unwrap().register_handler(pid, signal, action);
}

/// 为进程注册自定义处理函数。
pub fn signal_register_handler_fn<F>(pid: Pid, signal: Signal, handler: F)
where
    F: Fn(Signal) -> bool + Send + Sync + 'static,
{
    signal_handler().lock().unwrap().register_handler_fn(pid, signal, handler);
}

/// 返回默认处理方式。
pub fn signal_default_handler(signal: Signal) -> SignalAction {
    signal_handler().lock().unwrap().default_handler(signal)
}

/// 向进程发送信号。
pub fn signal_send(pid: Pid, signal: Signal) {
    signal_handler().lock().unwrap().send_signal(pid, signal);
}

/// 将信号加入屏蔽掩码。
pub fn signal_mask_add(pid: Pid, signal: Signal) {
    signal_handler().lock().unwrap().signal_mask_add(pid, signal);
}

/// 将信号从屏蔽掩码移除。
pub fn signal_mask_remove(pid: Pid, signal: Signal) {
    signal_handler().lock().unwrap().signal_mask_remove(pid, signal);
}

/// 返回进程的未决信号集。
pub fn signal_pending(pid: Pid) -> HashSet<Signal> {
    signal_handler().lock().unwrap().pending_signals(pid)
}

/// 投递进程所有未决信号，返回已投递信号列表。
pub fn signal_deliver(pid: Pid) -> Vec<Signal> {
    signal_handler().lock().unwrap().deliver_signals(pid)
}

/// 返回进程是否已终止。
pub fn signal_is_terminated(pid: Pid) -> bool {
    signal_handler().lock().unwrap().is_terminated(pid)
}

/// 返回信号处理器摘要。
pub fn signal_summary() -> String {
    signal_handler().lock().unwrap().summary()
}

// =====================================================================================
// 信号集操作（POSIX 兼容：sigaddset/sigdelset/sigismember/sigemptyset/sigfillset）
// =====================================================================================

/// 信号集：位掩码表示，用于屏蔽/未决/等待信号集合。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigSet {
    bits: u128, // 足够容纳所有信号编号（最大 17）
}

impl SigSet {
    /// 空信号集。
    pub fn empty() -> Self {
        Self { bits: 0 }
    }

    /// 全集（所有信号）。
    pub fn full() -> Self {
        Self { bits: !0u128 }
    }

    /// 将信号加入集合。
    pub fn add(&mut self, sig: Signal) {
        self.bits |= 1u128 << sig.number();
    }

    /// 将信号从集合移除。
    pub fn del(&mut self, sig: Signal) {
        self.bits &= !(1u128 << sig.number());
    }

    /// 判断信号是否在集合中。
    pub fn is_member(&self, sig: Signal) -> bool {
        (self.bits & (1u128 << sig.number())) != 0
    }

    /// 返回集合中信号数量。
    pub fn count(&self) -> u32 {
        self.bits.count_ones()
    }

    /// 返回集合中的所有信号。
    pub fn signals(&self) -> Vec<Signal> {
        let mut out = Vec::new();
        for sig in [
            Signal::SIGHUP, Signal::SIGINT, Signal::SIGQUIT, Signal::SIGILL,
            Signal::SIGTRAP, Signal::SIGABRT, Signal::SIGFPE, Signal::SIGSEGV,
            Signal::SIGTERM, Signal::SIGCHLD, Signal::SIGCONT, Signal::SIGSTOP,
            Signal::SIGKILL, Signal::SIGUSR1, Signal::SIGUSR2, Signal::SIGPIPE,
            Signal::SIGALRM,
        ] {
            if self.is_member(sig) {
                out.push(sig);
            }
        }
        out
    }
}

/// 将信号集中的所有信号加入进程屏蔽掩码。
pub fn signal_mask_add_set(pid: Pid, set: &SigSet) {
    let mut h = signal_handler().lock().unwrap();
    for sig in set.signals() {
        h.signal_mask_add(pid, sig);
    }
}

/// 将信号集中的所有信号从进程屏蔽掩码移除。
pub fn signal_mask_del_set(pid: Pid, set: &SigSet) {
    let mut h = signal_handler().lock().unwrap();
    for sig in set.signals() {
        h.signal_mask_remove(pid, sig);
    }
}

/// 向进程发送一组信号。
pub fn signal_send_set(pid: Pid, set: &SigSet) {
    let mut h = signal_handler().lock().unwrap();
    for sig in set.signals() {
        h.send_signal(pid, sig);
    }
}

/// 返回进程当前屏蔽掩码的 SigSet 视图。
pub fn signal_mask_set(pid: Pid) -> SigSet {
    let h = signal_handler().lock().unwrap();
    let state = h.process_state(pid);
    match state {
        Some(s) => {
            let mut set = SigSet::empty();
            for sig in [
                Signal::SIGHUP, Signal::SIGINT, Signal::SIGQUIT, Signal::SIGILL,
                Signal::SIGTRAP, Signal::SIGABRT, Signal::SIGFPE, Signal::SIGSEGV,
                Signal::SIGTERM, Signal::SIGCHLD, Signal::SIGCONT, Signal::SIGSTOP,
                Signal::SIGKILL, Signal::SIGUSR1, Signal::SIGUSR2, Signal::SIGPIPE,
                Signal::SIGALRM,
            ] {
                if s.is_masked(sig) {
                    set.add(sig);
                }
            }
            set
        }
        None => SigSet::empty(),
    }
}

/// 等待信号：若进程有未决信号则立即返回其中一个；否则返回 None（简化版 sigsuspend）。
pub fn signal_suspend(pid: Pid) -> Option<Signal> {
    let mut h = signal_handler().lock().unwrap();
    let pending = h.pending_signals(pid);
    if let Some(sig) = pending.into_iter().next() {
        let _ = h.deliver_signals(pid);
        Some(sig)
    } else {
        None
    }
}

/// 返回信号集文本摘要。
pub fn signal_set_summary(set: &SigSet) -> String {
    let sigs: Vec<String> = set.signals().iter().map(|s| s.name().to_string()).collect();
    format!("sigset count={} signals=[{}]", set.count(), sigs.join(","))
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn signal_number_and_name() {
        assert_eq!(Signal::SIGKILL.number(), 9);
        assert_eq!(Signal::SIGSEGV.number(), 11);
        assert_eq!(Signal::SIGINT.name(), "SIGINT");
        assert_eq!(Signal::SIGKILL.name(), "SIGKILL");
    }

    #[test]
    fn uncatchable_signals() {
        assert!(Signal::SIGKILL.uncatchable());
        assert!(Signal::SIGSTOP.uncatchable());
        assert!(!Signal::SIGTERM.uncatchable());
        assert!(!Signal::SIGINT.uncatchable());
    }

    #[test]
    fn default_action() {
        assert!(matches!(
            signal_default_handler(Signal::SIGKILL),
            SignalAction::Default
        ));
        assert!(matches!(
            signal_default_handler(Signal::SIGCHLD),
            SignalAction::Ignore
        ));
    }

    #[test]
    fn send_and_pending() {
        let mut h = SignalHandler::new();
        signal_send(1, Signal::SIGINT);
        signal_send(1, Signal::SIGTERM);
        let pending = signal_pending(1);
        assert!(pending.contains(&Signal::SIGINT));
        assert!(pending.contains(&Signal::SIGTERM));
        assert_eq!(h.sent_count(1, Signal::SIGINT), 1);
    }

    #[test]
    fn masked_signal_not_pending() {
        let mut h = SignalHandler::new();
        signal_mask_add(1, Signal::SIGINT);
        signal_send(1, Signal::SIGINT);
        let pending = signal_pending(1);
        assert!(!pending.contains(&Signal::SIGINT));
        // 但 sent_count 仍记录
        assert_eq!(h.sent_count(1, Signal::SIGINT), 1);
    }

    #[test]
    fn deliver_signals_invokes_handler() {
        let mut h = SignalHandler::new();
        let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let invoked2 = invoked.clone();
        signal_register_handler_fn(1, Signal::SIGUSR1, move |sig| {
            assert_eq!(sig, Signal::SIGUSR1);
            invoked2.store(true, Ordering::SeqCst);
            true
        });
        signal_send(1, Signal::SIGUSR1);
        assert!(signal_pending(1).contains(&Signal::SIGUSR1));
        let delivered = signal_deliver(1);
        assert!(delivered.contains(&Signal::SIGUSR1));
        assert!(invoked.load(Ordering::SeqCst));
        assert!(!signal_pending(1).contains(&Signal::SIGUSR1));
    }

    #[test]
    fn sigkill_always_default_terminates() {
        let mut h = SignalHandler::new();
        // 即使注册了自定义处理，SIGKILL 也应终止进程
        signal_register_handler_fn(1, Signal::SIGKILL, |_| true);
        signal_send(1, Signal::SIGKILL);
        signal_deliver(1);
        assert!(signal_is_terminated(1));
        assert_eq!(h.term_signal(1), Some(Signal::SIGKILL));
    }

    #[test]
    fn sigcont_resumes() {
        let mut h = SignalHandler::new();
        signal_send(1, Signal::SIGSTOP);
        signal_deliver(1);
        assert!(signal_is_terminated(1));
        signal_send(1, Signal::SIGCONT);
        signal_deliver(1);
        assert!(!signal_is_terminated(1));
    }

    #[test]
    fn handler_return_false_terminates() {
        let mut h = SignalHandler::new();
        signal_register_handler_fn(1, Signal::SIGUSR2, |_| false);
        signal_send(1, Signal::SIGUSR2);
        signal_deliver(1);
        assert!(signal_is_terminated(1));
        assert_eq!(h.term_signal(1), Some(Signal::SIGUSR2));
    }

    #[test]
    fn pending_signals_cleared_after_deliver() {
        let mut h = SignalHandler::new();
        signal_send(1, Signal::SIGINT);
        signal_send(1, Signal::SIGINT); // 重复发送同一信号，pending 是 set，只保留一份
        let _d1 = signal_deliver(1);
        assert!(!signal_pending(1).contains(&Signal::SIGINT));
    }

    #[test]
    fn global_signal_summary() {
        let _ = signal_summary();
    }

    #[test]
    fn sigset_empty_full_member() {
        let mut set = SigSet::empty();
        assert_eq!(set.count(), 0);
        assert!(!set.is_member(Signal::SIGINT));
        set.add(Signal::SIGINT);
        set.add(Signal::SIGTERM);
        assert_eq!(set.count(), 2);
        assert!(set.is_member(Signal::SIGINT));
        assert!(set.is_member(Signal::SIGTERM));
        set.del(Signal::SIGINT);
        assert!(!set.is_member(Signal::SIGINT));
        assert!(set.is_member(Signal::SIGTERM));
    }

    #[test]
    fn sigset_signals_list() {
        let mut set = SigSet::empty();
        set.add(Signal::SIGKILL);
        set.add(Signal::SIGSTOP);
        let sigs = set.signals();
        assert_eq!(sigs.len(), 2);
        assert!(sigs.contains(&Signal::SIGKILL));
        assert!(sigs.contains(&Signal::SIGSTOP));
    }

    #[test]
    fn sigset_full_count() {
        let set = SigSet::full();
        assert!(set.count() >= 17);
    }

    #[test]
    fn signal_mask_set_and_operations() {
        let mut h = SignalHandler::new();
        signal_mask_add(1, Signal::SIGINT);
        signal_mask_add(1, Signal::SIGTERM);
        let set = signal_mask_set(1);
        assert!(set.is_member(Signal::SIGINT));
        assert!(set.is_member(Signal::SIGTERM));
        assert!(!set.is_member(Signal::SIGKILL));

        signal_mask_del_set(1, &set);
        let set2 = signal_mask_set(1);
        assert!(!set2.is_member(Signal::SIGINT));
    }

    #[test]
    fn signal_send_set_delivers() {
        let mut h = SignalHandler::new();
        let mut set = SigSet::empty();
        set.add(Signal::SIGUSR1);
        set.add(Signal::SIGUSR2);
        signal_send_set(1, &set);
        let pending = signal_pending(1);
        assert!(pending.contains(&Signal::SIGUSR1));
        assert!(pending.contains(&Signal::SIGUSR2));
        let delivered = signal_deliver(1);
        assert!(delivered.contains(&Signal::SIGUSR1));
        assert!(delivered.contains(&Signal::SIGUSR2));
    }

    #[test]
    fn signal_suspend_returns_pending() {
        let mut h = SignalHandler::new();
        signal_send(1, Signal::SIGALRM);
        let sig = signal_suspend(1);
        assert_eq!(sig, Some(Signal::SIGALRM));
        assert!(!signal_pending(1).contains(&Signal::SIGALRM));
    }

    #[test]
    fn signal_suspend_none_when_no_pending() {
        let sig = signal_suspend(1);
        assert_eq!(sig, None);
    }

    #[test]
    fn signal_set_summary_text() {
        let mut set = SigSet::empty();
        set.add(Signal::SIGINT);
        let s = signal_set_summary(&set);
        assert!(s.contains("SIGINT"));
        assert!(s.contains("count=1"));
    }
}




