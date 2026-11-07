//! 感知 / 行动探针 —— 把每个通道接到真实子系统。
//!
//! `probe(name)` 返回该通道的实时快照；`act(name, payload)` 执行一次写操作。
//! 返回 `None` 表示该通道由 shell 层负责 (文件、上下文、行动等需要可变的会话状态)。

use std::io::IsTerminal;

use crate::desktop;
use crate::gpu::capabilities::{GpuCapability, GpuCapabilities};
use crate::gpu::{iommu, vram};
use crate::monitor::get_time_ns;
use crate::net::{ETH_NAME, fmt_ip, fmt_mac, net, parse_ip, parse_mac, Packet, RouteEntry};
use crate::vfs::Vfs;

use crate::bus::bus;
use crate::caps::caps;
use crate::cgroup::cgroups;
use crate::dev::devtree;
use crate::env::envstore;
use crate::fsmeta::fsmeta;
use crate::fw::firewall;
use crate::init::init_mgr;
use crate::ipc::ipc;
use crate::limits::limits;
use crate::mem::mem;
use crate::mount::mounts;
use crate::netif::ifaces;
use crate::pkg::pkgstore;
use crate::power::power;
use crate::proc::proctable;
use crate::procfs::procfs;
use crate::socket::netio;
use crate::sysctl::sysctl;
use crate::time::timekeeper;
use crate::tty::tty;
use crate::user::userdb;

/// 感知一个硬件 / 系统通道，返回实时快照。
pub fn probe(name: &str) -> Option<String> {
    match name {
        "gpu://vram" => {
            let v = vram().lock().unwrap();
            let total = v.total_vram();
            let free = v.available();
            Some(format!(
                "total={}MiB used={}MiB free={}MiB partitions={}",
                total / (1024 * 1024),
                (total - free) / (1024 * 1024),
                free / (1024 * 1024),
                v.partition_count()
            ))
        }
        "gpu://capabilities" => {
            let caps = GpuCapabilities::all();
            let names: Vec<&str> = GpuCapability::ALL.iter().map(|c| c.name()).collect();
            Some(format!("mask=0x{:02x} caps=[{}]", caps.bits(), names.join(",")))
        }
        "gpu://framebuffer" => {
            let (w, h, bytes, lit) = desktop::framebuffer_stats();
            Some(format!("{}x{} {}B lit={}", w, h, bytes, lit))
        }
        "gpu://scheduler" | "action://scheduler" => {
            Some("MLFQ + semantic weight, slice=10ms".to_string())
        }
        "mem://system" => {
            let s = crate::monitor::get_stats();
            Some(format!(
                "tokens={} active={} completed={} failed={} avg={}ns vram_used={}B",
                s.total_tokens,
                s.active_tokens,
                s.total_tasks_completed,
                s.total_tasks_failed,
                s.avg_task_time_ns,
                s.total_vram_used
            ))
        }
        "dev://iommu0" => {
            let io = iommu().lock().unwrap();
            Some(format!(
                "domains={} mappings={}",
                io.domain_count(),
                io.mapping_count()
            ))
        }
        "dev://timer" => Some(format!("now={}ns", get_time_ns())),
        "dev://console" => Some(format!(
            "stdin={} stdout=tty ansi=on",
            if std::io::stdin().is_terminal() {
                "tty"
            } else {
                "pipe"
            }
        )),
        "dev://disk" => {
            let p = Vfs::default_state_path();
            let exists = p.exists();
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some(format!("{} exists={} size={}B", p.display(), exists, size))
        }
        "dev://null" => Some("discard (写入即丢弃)".to_string()),
        "dev://gpu0" => {
            let v = vram().lock().unwrap();
            Some(format!(
                "gpu0 total={}GiB caps={} dev-id=0",
                v.total_vram() / (1024 * 1024 * 1024),
                GpuCapability::ALL.len()
            ))
        }
        "dev://monitor" => {
            let s = crate::monitor::get_stats();
            Some(format!("uptime={}ns tasks={}", s.system_uptime_ns, s.total_tasks_completed))
        }
        "interface://abi" => Some("AiSystemCall 10 变体 / AiSystemCallResult".to_string()),
        "gateway://net" => Some("offline (网络栈尚未实现)".to_string()),
        "browser://gateway" => Some("offline (无 HTML/DOM/网络栈, 仅通道占位)".to_string()),
        "driver://gpu" => {
            let v = vram().lock().unwrap();
            Some(format!(
                "gpu-driver total={}GiB caps={} state=loaded",
                v.total_vram() / (1024 * 1024 * 1024),
                GpuCapability::ALL.len()
            ))
        }
        "driver://iommu" => {
            let io = iommu().lock().unwrap();
            Some(format!(
                "iommu-driver domains={} mappings={} state=loaded",
                io.domain_count(),
                io.mapping_count()
            ))
        }
        "trace://exec" => {
            let s = crate::monitor::get_stats();
            Some(format!(
                "uptime={}ns exec_ok={} exec_fail={}",
                s.system_uptime_ns, s.total_tasks_completed, s.total_tasks_failed
            ))
        }
        "net://eth0" => {
            let st = net().lock().unwrap();
            let s = st.stats();
            let arp_lines: Vec<String> = st
                .arp
                .iter()
                .map(|(ip, mac)| format!("  {} -> {}", fmt_ip(*ip), fmt_mac(*mac)))
                .collect();
            let route_lines: Vec<String> = st
                .routes
                .iter()
                .map(|r| {
                    format!(
                        "  {} via {} dev {}",
                        fmt_ip(r.network),
                        fmt_ip(r.gateway),
                        r.iface
                    )
                })
                .collect();
            let arp_block = if arp_lines.is_empty() {
                "  (empty)".to_string()
            } else {
                arp_lines.join("\n")
            };
            let route_block = if route_lines.is_empty() {
                "  (empty)".to_string()
            } else {
                route_lines.join("\n")
            };
            Some(format!(
                "eth0 link={} ip={} mask={} mac={}\ntx={}/{}B rx={}/{}B\nARP:\n{}\nRoutes:\n{}",
                if st.iface.link_up { "up" } else { "down" },
                fmt_ip(st.iface.ip),
                fmt_ip(st.iface.mask),
                fmt_mac(st.iface.mac),
                s.tx_packets,
                s.tx_bytes,
                s.rx_packets,
                s.rx_bytes,
                arp_block,
                route_block
            ))
        }
        // —— AI 原生扩展通道：进程 / 内存 / 权限 / 身份 / 真实网络 ——
        "proc://manager" => {
            let mut t = proctable().lock().unwrap();
            Some(t.tree())
        }
        "mem://ram" => Some(mem().lock().unwrap().stats()),
        "mem://mmap" => Some(mem().lock().unwrap().regions()),
        "fs://meta" => Some(fsmeta().lock().unwrap().list()),
        "user://db" => Some(userdb().lock().unwrap().list()),
        "socket://tcp" => Some(netio().lock().unwrap().last.clone()),
        "net://dns" => Some(netio().lock().unwrap().last.clone()),
        // —— IPC / 安全 / 设备 / init / 时间 / 包管理 / 作业控制 ——
        "ipc://pipe" => Some(ipc().lock().unwrap().pipes()),
        "ipc://shm" => Some(ipc().lock().unwrap().shm_list()),
        "ipc://msgq" => Some(ipc().lock().unwrap().msgq_list()),
        "ipc://sem" => Some(ipc().lock().unwrap().sem_list()),
        "sec://caps" => Some(caps().lock().unwrap().list()),
        "dev://tree" => Some(devtree().lock().unwrap().list()),
        "init://services" => Some(init_mgr().lock().unwrap().list()),
        "time://clock" => Some(timekeeper().lock().unwrap().clock()),
        "time://cron" => Some(timekeeper().lock().unwrap().cron_list()),
        "pkg://store" => Some(pkgstore().lock().unwrap().list()),
        "tty://jobs" => {
            let mut t = tty().lock().unwrap();
            Some(t.jobs())
        }
        // —— 系统设置类 ——
        "sysctl://kernel" => Some(sysctl().lock().unwrap().kernel_list()),
        "config://system" => Some(sysctl().lock().unwrap().identity_list()),
        "env://process" => Some(envstore().lock().unwrap().list()),
        "limits://process" => Some(limits().lock().unwrap().list()),
        "fw://rules" => Some(firewall().lock().unwrap().list()),
        "cgroup://groups" => Some(cgroups().lock().unwrap().list()),
        "power://state" => Some(power().lock().unwrap().status()),
        // —— 第二批系统模块 ——
        "mount://table" => Some(mounts().lock().unwrap().list()),
        "proc://sysinfo" => Some(procfs().lock().unwrap().snapshot()),
        "net://interfaces" => Some(ifaces().lock().unwrap().list()),
        "dev://bus" => Some(bus().lock().unwrap().list()),
        _ => None,
    }
}

/// 对硬件 / 系统通道执行一次行动。
pub fn act(name: &str, payload: &str) -> Option<Result<String, String>> {
    match name {
        "gpu://framebuffer" | "desktop://framebuffer" => {
            let mut fb = desktop::framebuffer().lock().unwrap();
            fb.clear();
            let end = fb.draw_text(2, 2, payload);
            Some(Ok(format!(
                "已光栅化 {:?} 到帧缓冲 (结束 x={}, 点亮 {} 像素)",
                payload,
                end,
                fb.lit_pixels()
            )))
        }
        "dev://timer" => Some(Ok(format!("now={}ns", get_time_ns()))),
        "dev://null" => Some(Ok(format!("已丢弃 {} 字节", payload.len()))),
        "gpu://vram" => {
            let v = vram().lock().unwrap();
            Some(Ok(format!(
                "显存通道已确认写入 {} 字节, 剩余 {}MiB",
                payload.len(),
                v.available() / (1024 * 1024)
            )))
        }
        "net://eth0" => {
            let mut it = payload.split_whitespace();
            match it.next() {
                Some("stats") => {
                    let st = net().lock().unwrap();
                    let s = st.stats();
                    Some(Ok(format!(
                        "tx={}/{}B rx={}/{}B link={}",
                        s.tx_packets,
                        s.tx_bytes,
                        s.rx_packets,
                        s.rx_bytes,
                        if st.iface.link_up { "up" } else { "down" }
                    )))
                }
                Some("arp") => {
                    let mut st = net().lock().unwrap();
                    match it.next() {
                        Some("add") => {
                            let ip = parse_ip(it.next().unwrap_or("0.0.0.0"));
                            let mac = parse_mac(it.next().unwrap_or("00:00:00:00:00:00"));
                            st.arp_add(ip, mac);
                            Some(Ok(format!(
                                "ARP {} -> {} 已添加",
                                fmt_ip(ip),
                                fmt_mac(mac)
                            )))
                        }
                        Some("del") => {
                            let ip = parse_ip(it.next().unwrap_or("0.0.0.0"));
                            st.arp_del(ip);
                            Some(Ok(format!("ARP {} 已删除", fmt_ip(ip))))
                        }
                        Some("show") | None => {
                            let st = net().lock().unwrap();
                            if st.arp.is_empty() {
                                Some(Ok("ARP 表为空".to_string()))
                            } else {
                                let lines: Vec<String> = st
                                    .arp
                                    .iter()
                                    .map(|(ip, mac)| format!("  {} -> {}", fmt_ip(*ip), fmt_mac(*mac)))
                                    .collect();
                                Some(Ok(lines.join("\n")))
                            }
                        }
                        _ => Some(Err("用法: act net://eth0 arp add <ip> <mac> | del <ip> | show".to_string())),
                    }
                }
                Some("route") => {
                    let mut st = net().lock().unwrap();
                    match it.next() {
                        Some("add") => {
                            let network = parse_ip(it.next().unwrap_or("0.0.0.0"));
                            let mask = parse_ip(it.next().unwrap_or("0.0.0.0"));
                            let gateway = parse_ip(it.next().unwrap_or("0.0.0.0"));
                            st.route_add(RouteEntry {
                                network,
                                mask,
                                gateway,
                                iface: ETH_NAME.to_string(),
                            });
                            Some(Ok(format!(
                                "路由 {} via {} 已添加",
                                fmt_ip(network),
                                fmt_ip(gateway)
                            )))
                        }
                        Some("del") => {
                            let network = parse_ip(it.next().unwrap_or("0.0.0.0"));
                            st.route_del(network);
                            Some(Ok(format!("路由 {} 已删除", fmt_ip(network))))
                        }
                        Some("show") | None => {
                            let st = net().lock().unwrap();
                            if st.routes.is_empty() {
                                Some(Ok("路由表为空".to_string()))
                            } else {
                                let lines: Vec<String> = st
                                    .routes
                                    .iter()
                                    .map(|r| {
                                        format!(
                                            "  {} via {} dev {}",
                                            fmt_ip(r.network),
                                            fmt_ip(r.gateway),
                                            r.iface
                                        )
                                    })
                                    .collect();
                                Some(Ok(lines.join("\n")))
                            }
                        }
                        _ => Some(Err(
                            "用法: act net://eth0 route add <net> <mask> <gw> | del <net> | show"
                                .to_string(),
                        )),
                    }
                }
                Some("ping") => {
                    let host = it.next().unwrap_or("10.0.0.1");
                    let dst = parse_ip(host);
                    let mut st = net().lock().unwrap();
                    let r = st.send(dst, b"ping".to_vec());
                    let time_ms = (get_time_ns() % 1_000_000) / 1000;
                    match r {
                        Ok(()) => {
                            let own_ip = st.iface.ip;
                            if dst == own_ip {
                                st.rx.push_back(Packet {
                                    src: dst,
                                    dst: own_ip,
                                    ttl: 64,
                                    payload: b"echo".to_vec(),
                                });
                                st.stats.rx_packets += 1;
                                st.stats.rx_bytes += 4;
                            }
                            Some(Ok(format!(
                                "64 bytes from {}: icmp_seq=1 ttl=64 time={} ms",
                                fmt_ip(dst),
                                time_ms
                            )))
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
                Some("send") => {
                    let host = it.next().unwrap_or("255.255.255.255");
                    let dst = parse_ip(host);
                    let msg = it.next().unwrap_or("").as_bytes().to_vec();
                    let msg_len = msg.len();
                    let mut st = net().lock().unwrap();
                    match st.send(dst, msg) {
                        Ok(()) => {
                            let nh = st.lookup_route(dst).unwrap_or(dst);
                            Some(Ok(format!(
                                "已向 {} (next-hop {}) 发送 {} 字节",
                                fmt_ip(dst),
                                fmt_ip(nh),
                                msg_len
                            )))
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
                Some("link") => {
                    let mut st = net().lock().unwrap();
                    match it.next() {
                        Some("up") => {
                            st.iface.link_up = true;
                            Some(Ok("链路已启通".to_string()))
                        }
                        Some("down") => {
                            st.iface.link_up = false;
                            Some(Ok("链路已断开".to_string()))
                        }
                        _ => Some(Err("用法: act <通道> link up|down".to_string())),
                    }
                }
                _ => Some(Err("未知操作 (ping/send/stats/link up|down)".to_string())),
            }
        }
        // —— AI 原生扩展通道 ——
        "proc://manager" => Some(act_proc(payload)),
        "mem://ram" => Some(act_ram(payload)),
        "mem://mmap" => Some(act_mmap(payload)),
        "fs://meta" => Some(act_fsmeta(payload)),
        "user://db" => Some(act_user(payload)),
        "socket://tcp" => Some(act_socket(payload)),
        "net://dns" => Some(act_dns(payload)),
        "ipc://pipe" => Some(act_ipc_pipe(payload)),
        "ipc://shm" => Some(act_ipc_shm(payload)),
        "ipc://msgq" => Some(act_ipc_msgq(payload)),
        "ipc://sem" => Some(act_ipc_sem(payload)),
        "sec://caps" => Some(act_caps(payload)),
        "dev://tree" => Some(act_dev(payload)),
        "init://services" => Some(act_init(payload)),
        "time://clock" => Some(Ok(timekeeper().lock().unwrap().clock())),
        "time://cron" => Some(act_cron(payload)),
        "pkg://store" => Some(act_pkg(payload)),
        "tty://jobs" => Some(act_tty(payload)),
        "sysctl://kernel" => Some(act_sysctl(payload, true)),
        "config://system" => Some(act_sysctl(payload, false)),
        "env://process" => Some(act_env(payload)),
        "limits://process" => Some(act_limits(payload)),
        "fw://rules" => Some(act_fw(payload)),
        "cgroup://groups" => Some(act_cgroup(payload)),
        "power://state" => Some(act_power(payload)),
        "mount://table" => Some(act_mount(payload)),
        "proc://sysinfo" => Some(act_procfs(payload)),
        "net://interfaces" => Some(act_netif(payload)),
        "dev://bus" => Some(act_bus(payload)),
        _ => None,
    }
}

// ---- AI 原生扩展通道的行动实现 ----

fn parse_mib(s: &str) -> Result<u64, String> {
    s.parse::<u64>()
        .map(|n| n * 1024 * 1024)
        .map_err(|_| format!("无效数值(MiB): {}", s))
}

fn act_proc(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "spawn" => proctable().lock().unwrap().spawn(rest),
        "signal" | "kill" => {
            let mut a = rest.split_whitespace();
            let pid = a
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("用法: signal <pid> <sig>")?;
            let sig = a.next().unwrap_or(if sub == "kill" { "KILL" } else { "TERM" });
            proctable().lock().unwrap().signal(pid, sig)
        }
        "tree" => Ok(proctable().lock().unwrap().tree()),
        "list" | "" => Ok(proctable().lock().unwrap().list()),
        _ => Err(
            "用法: act proc://manager spawn <命令> | signal <pid> <sig> | kill <pid> | tree | list"
                .to_string(),
        ),
    }
}

fn act_ram(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("alloc") => mem().lock().unwrap().alloc(parse_mib(it.next().unwrap_or("0"))?),
        Some("free") => mem().lock().unwrap().free(parse_mib(it.next().unwrap_or("0"))?),
        _ => Err("用法: act mem://ram alloc|free <MiB>".to_string()),
    }
}

fn act_mmap(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("mmap") => {
            let bytes = parse_mib(it.next().unwrap_or("0"))?;
            let perms = it.next().unwrap_or("rw");
            let uid = userdb().lock().unwrap().current_uid;
            mem().lock().unwrap().mmap(bytes, perms, uid)
        }
        Some("munmap") => {
            let a = it.next().unwrap_or("0");
            let addr = u64::from_str_radix(a.trim_start_matches("0x"), 16)
                .map_err(|_| format!("无效地址: {}", a))?;
            mem().lock().unwrap().munmap(addr)
        }
        _ => Err("用法: act mem://mmap mmap <MiB> [perms] | munmap <addr>".to_string()),
    }
}

fn act_fsmeta(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("chmod") => {
            let mode_s = it.next().ok_or("用法: chmod <mode> <路径>")?;
            let path = it.next().ok_or("用法: chmod <mode> <路径>")?;
            let mode =
                u16::from_str_radix(mode_s, 8).map_err(|_| format!("无效 mode: {}", mode_s))?;
            Ok(fsmeta().lock().unwrap().chmod(mode, path))
        }
        Some("chown") => {
            let uid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: chown <uid> <gid> <路径>")?;
            let gid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: chown <uid> <gid> <路径>")?;
            let path = it.next().ok_or("用法: chown <uid> <gid> <路径>")?;
            Ok(fsmeta().lock().unwrap().chown(uid, gid, path))
        }
        Some("ln") => {
            let target = it.next().ok_or("用法: ln <目标> <链接>")?;
            let link = it.next().ok_or("用法: ln <目标> <链接>")?;
            Ok(fsmeta().lock().unwrap().ln(target, link))
        }
        Some("mount") => {
            let src = it.next().ok_or("用法: mount <源> <目标>")?;
            let dst = it.next().ok_or("用法: mount <源> <目标>")?;
            Ok(fsmeta().lock().unwrap().mount(src, dst))
        }
        _ => Err("用法: act fs://meta chmod <mode> <路径> | chown <uid> <gid> <路径> | ln <目标> <链接> | mount <源> <目标>".to_string()),
    }
}

fn act_user(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("adduser") | Some("useradd") => {
            let name = it.next().ok_or("用法: adduser <名称>")?;
            let uid = userdb().lock().unwrap().adduser(name)?;
            Ok(format!("已创建用户 {} uid={}", name, uid))
        }
        Some("su") => {
            let who = it.next().ok_or("用法: su <名称|uid>")?;
            userdb().lock().unwrap().su(who)
        }
        Some("sudo") => Ok(userdb().lock().unwrap().sudo()),
        Some("whoami") | Some("") | None => Ok(userdb().lock().unwrap().whoami()),
        _ => Err("用法: act user://db adduser <名> | su <名|uid> | sudo | whoami".to_string()),
    }
}

fn act_socket(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("connect") => {
            let hostport = it.next().ok_or("用法: connect <host:port> [数据]")?;
            let data = it.next();
            netio().lock().unwrap().connect(hostport, data)
        }
        _ => Err("用法: act socket://tcp connect <host:port> [数据]".to_string()),
    }
}

fn act_dns(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("resolve") => {
            let host = it.next().ok_or("用法: resolve <域名>")?;
            netio().lock().unwrap().resolve(host)
        }
        _ => Err("用法: act net://dns resolve <域名>".to_string()),
    }
}

fn act_ipc_pipe(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("create") | None | Some("") => Ok(ipc().lock().unwrap().pipe_create()),
        Some("write") => {
            let id = it.next().and_then(|s| s.parse().ok()).ok_or("用法: write <id> <data>")?;
            let data = it.next().unwrap_or("");
            ipc().lock().unwrap().pipe_write(id, data)
        }
        Some("read") => {
            let id = it.next().and_then(|s| s.parse().ok()).ok_or("用法: read <id>")?;
            ipc().lock().unwrap().pipe_read(id)
        }
        _ => Err("用法: act ipc://pipe create | write <id> <data> | read <id>".to_string()),
    }
}

fn act_ipc_shm(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("create") => {
            let name = it.next().ok_or("用法: create <name> <size>")?;
            let size = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(4096);
            Ok(ipc().lock().unwrap().shm_create(name, size))
        }
        Some("write") => {
            let name = it.next().ok_or("用法: write <name> <data>")?;
            let data = it.next().unwrap_or("");
            ipc().lock().unwrap().shm_write(name, data)
        }
        Some("read") => {
            let name = it.next().ok_or("用法: read <name>")?;
            ipc().lock().unwrap().shm_read(name)
        }
        _ => Err("用法: act ipc://shm create <name> <size> | write <name> <data> | read <name>".to_string()),
    }
}

fn act_ipc_msgq(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("send") => {
            let name = it.next().ok_or("用法: send <name> <msg>")?;
            let msg = it.next().unwrap_or("");
            ipc().lock().unwrap().msgq_send(name, msg)
        }
        Some("recv") => {
            let name = it.next().ok_or("用法: recv <name>")?;
            ipc().lock().unwrap().msgq_recv(name)
        }
        _ => Err("用法: act ipc://msgq send <name> <msg> | recv <name>".to_string()),
    }
}

fn act_ipc_sem(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("init") => {
            let name = it.next().ok_or("用法: init <name> <n>")?;
            let n = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(1);
            Ok(ipc().lock().unwrap().sem_init(name, n))
        }
        Some("post") => {
            let name = it.next().ok_or("用法: post <name>")?;
            ipc().lock().unwrap().sem_post(name)
        }
        Some("wait") => {
            let name = it.next().ok_or("用法: wait <name>")?;
            ipc().lock().unwrap().sem_wait(name)
        }
        _ => Err("用法: act ipc://sem init <name> <n> | post <name> | wait <name>".to_string()),
    }
}

fn act_caps(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("grant") => {
            let cap = it.next().ok_or("用法: grant <CAP_XXX>")?;
            caps().lock().unwrap().grant(cap)
        }
        Some("revoke") => {
            let cap = it.next().ok_or("用法: revoke <CAP_XXX>")?;
            caps().lock().unwrap().revoke(cap)
        }
        Some("check") => {
            let cap = it.next().ok_or("用法: check <CAP_XXX>")?;
            Ok(caps().lock().unwrap().check(cap))
        }
        _ => Ok(caps().lock().unwrap().list()),
    }
}

fn act_dev(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("attach") => {
            let name = it.next().ok_or("用法: attach <name>")?;
            devtree().lock().unwrap().attach(name)
        }
        Some("detach") => {
            let name = it.next().ok_or("用法: detach <name>")?;
            devtree().lock().unwrap().detach(name)
        }
        _ => Ok(devtree().lock().unwrap().list()),
    }
}

fn act_init(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("start") => {
            let n = it.next().ok_or("用法: start <name>")?;
            init_mgr().lock().unwrap().start(n)
        }
        Some("stop") => {
            let n = it.next().ok_or("用法: stop <name>")?;
            init_mgr().lock().unwrap().stop(n)
        }
        Some("enable") => {
            let n = it.next().ok_or("用法: enable <name>")?;
            init_mgr().lock().unwrap().enable(n)
        }
        Some("disable") => {
            let n = it.next().ok_or("用法: disable <name>")?;
            init_mgr().lock().unwrap().disable(n)
        }
        _ => Ok(init_mgr().lock().unwrap().list()),
    }
}

fn act_cron(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("add") => {
            let interval = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: add <interval_secs> <cmd>")?;
            let cmd = it.next().unwrap_or("");
            Ok(timekeeper().lock().unwrap().cron_add(interval, cmd))
        }
        Some("del") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: del <id>")?;
            timekeeper().lock().unwrap().cron_del(id)
        }
        Some("run") => Ok(timekeeper().lock().unwrap().cron_run_due()),
        _ => Ok(timekeeper().lock().unwrap().cron_list()),
    }
}

fn act_pkg(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("install") => {
            let n = it.next().ok_or("用法: install <name>")?;
            pkgstore().lock().unwrap().install(n)
        }
        Some("remove") => {
            let n = it.next().ok_or("用法: remove <name>")?;
            pkgstore().lock().unwrap().remove(n)
        }
        Some("search") => {
            let k = it.next().ok_or("用法: search <kw>")?;
            Ok(pkgstore().lock().unwrap().search(k))
        }
        _ => Ok(pkgstore().lock().unwrap().list()),
    }
}

fn act_tty(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("fg") => {
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: fg <pid>")?;
            Ok(tty().lock().unwrap().fg(pid))
        }
        Some("bg") => {
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: bg <pid>")?;
            Ok(tty().lock().unwrap().bg(pid))
        }
        Some("kill") => {
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: kill <pid>")?;
            tty().lock().unwrap().kill(pid)
        }
        _ => {
            let mut t = tty().lock().unwrap();
            Ok(t.jobs())
        }
    }
}

fn act_sysctl(payload: &str, kernel: bool) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("set") => {
            let k = it.next().ok_or("用法: set <key> <value>")?;
            let v = it.next().ok_or("用法: set <key> <value>")?;
            if kernel {
                sysctl().lock().unwrap().set_kernel(k, v)
            } else {
                sysctl().lock().unwrap().set_identity(k, v)
            }
        }
        _ => {
            if kernel {
                Ok(sysctl().lock().unwrap().kernel_list())
            } else {
                Ok(sysctl().lock().unwrap().identity_list())
            }
        }
    }
}

fn act_env(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("set") => {
            let k = it.next().ok_or("用法: set <key> <value>")?;
            let v = it.next().ok_or("用法: set <key> <value>")?;
            Ok(envstore().lock().unwrap().set(k, v))
        }
        Some("get") => {
            let k = it.next().ok_or("用法: get <key>")?;
            Ok(envstore().lock().unwrap().get(k))
        }
        Some("unset") => {
            let k = it.next().ok_or("用法: unset <key>")?;
            Ok(envstore().lock().unwrap().unset(k))
        }
        _ => Ok(envstore().lock().unwrap().list()),
    }
}

fn act_limits(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("set") => {
            let k = it.next().ok_or("用法: set <key> <value>")?;
            let v = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: set <key> <value>")?;
            limits().lock().unwrap().set(k, v)
        }
        Some("get") => {
            let k = it.next().ok_or("用法: get <key>")?;
            Ok(limits().lock().unwrap().get(k))
        }
        _ => Ok(limits().lock().unwrap().list()),
    }
}

fn act_fw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("add") => {
            let action = it.next().ok_or("用法: add <allow|deny> <proto> <port> [src]")?;
            let proto = it.next().ok_or("用法: add <allow|deny> <proto> <port> [src]")?;
            let port = it.next().ok_or("用法: add <allow|deny> <proto> <port> [src]")?;
            let src = it.next().unwrap_or("any");
            Ok(firewall().lock().unwrap().add(action, proto, port, src))
        }
        Some("del") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: del <id>")?;
            firewall().lock().unwrap().del(id)
        }
        Some("policy") => {
            let p = it.next().ok_or("用法: policy <ACCEPT|DROP>")?;
            firewall().lock().unwrap().set_policy(p)
        }
        _ => Ok(firewall().lock().unwrap().list()),
    }
}

fn act_cgroup(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("create") => {
            let n = it.next().ok_or("用法: create <name>")?;
            cgroups().lock().unwrap().create(n)
        }
        Some("del") => {
            let n = it.next().ok_or("用法: del <name>")?;
            cgroups().lock().unwrap().delete(n)
        }
        Some("set") => {
            let n = it.next().ok_or("用法: set <name> <cpu%> <memMiB>")?;
            let cpu = it.next().and_then(|s| s.parse().ok()).ok_or("用法: set <name> <cpu%> <memMiB>")?;
            let mem = it.next().and_then(|s| s.parse().ok()).ok_or("用法: set <name> <cpu%> <memMiB>")?;
            cgroups().lock().unwrap().set_limit(n, cpu, mem)
        }
        Some("attach") => {
            let n = it.next().ok_or("用法: attach <name> <pid>")?;
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: attach <name> <pid>")?;
            cgroups().lock().unwrap().attach(n, pid)
        }
        _ => Ok(cgroups().lock().unwrap().list()),
    }
}

fn act_power(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("suspend") => power().lock().unwrap().set("suspend"),
        Some("reboot") => power().lock().unwrap().set("reboot"),
        Some("shutdown") => power().lock().unwrap().set("shutdown"),
        Some("wake") => Ok(power().lock().unwrap().wake()),
        _ => Ok(power().lock().unwrap().status()),
    }
}

fn act_mount(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("mount") => {
            let src = it.next().ok_or("用法: mount <源> <目标> [类型] [ro|rw]")?;
            let target = it.next().ok_or("用法: mount <源> <目标> [类型] [ro|rw]")?;
            let mut fstype = "auto";
            let mut ro = false;
            for arg in it {
                match arg {
                    "ro" => ro = true,
                    "rw" => ro = false,
                    other => fstype = other,
                }
            }
            mounts().lock().unwrap().mount(src, target, fstype, ro)
        }
        Some("umount") | Some("unmount") => {
            let target = it.next().ok_or("用法: umount <目标>")?;
            mounts().lock().unwrap().umount(target)
        }
        Some("remount") => {
            let target = it.next().ok_or("用法: remount <目标> <ro|rw>")?;
            let mode = it.next().unwrap_or("rw");
            let ro = match mode {
                "ro" => true,
                "rw" => false,
                _ => return Err("用法: remount <目标> <ro|rw>".to_string()),
            };
            mounts().lock().unwrap().remount(target, ro)
        }
        _ => Ok(mounts().lock().unwrap().list()),
    }
}

fn act_procfs(payload: &str) -> Result<String, String> {
    match payload.trim() {
        "" | "snapshot" | "read" => Ok(procfs().lock().unwrap().snapshot()),
        "refresh" => Ok(procfs().lock().unwrap().refresh()),
        _ => Err("用法: act proc://sysinfo refresh|snapshot".to_string()),
    }
}

fn act_netif(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("up") => {
            let n = it.next().ok_or("用法: up <网卡>")?;
            ifaces().lock().unwrap().set_link(n, true)
        }
        Some("down") => {
            let n = it.next().ok_or("用法: down <网卡>")?;
            ifaces().lock().unwrap().set_link(n, false)
        }
        Some("addr") => {
            let n = it.next().ok_or("用法: addr <网卡> <ip>/<前缀>")?;
            let cidr = it.next().ok_or("用法: addr <网卡> <ip>/<前缀>")?;
            ifaces().lock().unwrap().set_addr(n, cidr)
        }
        Some("mtu") => {
            let n = it.next().ok_or("用法: mtu <网卡> <68-65535>")?;
            let mtu = it
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("用法: mtu <网卡> <68-65535>")?;
            ifaces().lock().unwrap().set_mtu(n, mtu)
        }
        Some("add") => {
            let n = it.next().ok_or("用法: add <网卡> [mac]")?;
            let mac = it.next().unwrap_or("02:00:00:00:00:ff");
            ifaces().lock().unwrap().add(n, mac)
        }
        Some("del") | Some("remove") => {
            let n = it.next().ok_or("用法: del <网卡>")?;
            ifaces().lock().unwrap().del(n)
        }
        _ => Ok(ifaces().lock().unwrap().list()),
    }
}

fn act_bus(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("bind") => {
            let addr = it.next().ok_or("用法: bind <地址> <驱动>")?;
            let driver = it.next().ok_or("用法: bind <地址> <驱动>")?;
            bus().lock().unwrap().bind(addr, driver)
        }
        Some("unbind") => {
            let addr = it.next().ok_or("用法: unbind <地址>")?;
            bus().lock().unwrap().unbind(addr)
        }
        Some("rescan") => bus().lock().unwrap().rescan(),
        Some("add") => {
            let addr = it.next().ok_or("用法: add <地址> <vendor:device> <类别> [驱动]")?;
            let id = it.next().ok_or("用法: add <地址> <vendor:device> <类别> [驱动]")?;
            let class = it.next().unwrap_or("misc");
            let driver = it.next();
            bus().lock().unwrap().add(addr, id, class, driver)
        }
        Some("remove") | Some("del") => {
            let addr = it.next().ok_or("用法: remove <地址>")?;
            bus().lock().unwrap().remove(addr)
        }
        _ => Ok(bus().lock().unwrap().list()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_probes_return_data() {
        for name in [
            "gpu://vram",
            "gpu://capabilities",
            "gpu://framebuffer",
            "mem://system",
            "dev://iommu0",
            "dev://timer",
            "dev://console",
            "dev://disk",
            "dev://null",
            "dev://gpu0",
            "dev://monitor",
            "interface://abi",
            "gateway://net",
            "browser://gateway",
            "driver://gpu",
            "driver://iommu",
        "trace://exec",
        "net://eth0",
    ] {
            let out = probe(name).unwrap_or_else(|| panic!("通道 {} 无探针", name));
            assert!(!out.is_empty(), "通道 {} 探针为空", name);
        }
        assert!(probe("file://vfs").is_none(), "文件通道应由 shell 处理");
    }

    #[test]
    fn framebuffer_act_draws_pixels() {
        let r = act("gpu://framebuffer", "HELLO").unwrap().unwrap();
        assert!(r.contains("HELLO"));
        let (_, _, _, lit) = desktop::framebuffer_stats();
        assert!(lit > 0);
    }

    #[test]
    fn net_act_ping_send_stats() {
        assert!(act("net://eth0", "stats").unwrap().is_ok());
        let r = act("net://eth0", "ping 10.0.0.2").unwrap().unwrap();
        assert!(r.contains("bytes from"), "{}", r);
        let r = act("net://eth0", "send 10.0.0.3 hello-net").unwrap().unwrap();
        assert!(r.contains("10.0.0.3"), "{}", r);
        let r = act("net://eth0", "ping 10.0.0.2").unwrap().unwrap();
        assert!(r.contains("bytes from"), "{}", r);
        // 链路断开后 ping 应失败
        let r = act("net://eth0", "link down").unwrap().unwrap();
        assert_eq!(r, "链路已断开");
        assert!(act("net://eth0", "ping 10.0.0.2").unwrap().is_err());
        let r = act("net://eth0", "link up").unwrap().unwrap();
        assert_eq!(r, "链路已启通");
    }

    #[test]
    fn new_system_modules_probe_and_act() {
        // 四个新通道都必须有非空探针
        for name in [
            "mount://table",
            "proc://sysinfo",
            "net://interfaces",
            "dev://bus",
        ] {
            let out = probe(name).unwrap_or_else(|| panic!("通道 {} 无探针", name));
            assert!(!out.is_empty(), "通道 {} 探针为空", name);
        }

        // 挂载表：挂载 -> 卸载，根目录不可卸载
        let r = act("mount://table", "mount tmpfs /cache tmpfs ro")
            .unwrap()
            .unwrap();
        assert!(r.contains("/cache"), "{}", r);
        assert!(act("mount://table", "umount /").unwrap().is_err());
        assert!(act("mount://table", "umount /cache").unwrap().is_ok());

        // procfs：刷新递增采样计数
        let r = act("proc://sysinfo", "refresh").unwrap().unwrap();
        assert!(r.contains("loadavg:") && r.contains("meminfo:"), "{}", r);

        // 多网卡：MTU 与链路状态
        let r = act("net://interfaces", "mtu wlan0 9000").unwrap().unwrap();
        assert!(r.contains("9000"), "{}", r);
        assert!(act("net://interfaces", "mtu wlan0 1").unwrap().is_err());

        // 设备总线：扫描与非法绑定
        let r = act("dev://bus", "rescan").unwrap().unwrap();
        assert!(r.contains("总线扫描"), "{}", r);
        assert!(act("dev://bus", "bind 00:99.0 x").unwrap().is_err());
    }

    #[test]
    fn unknown_channel_returns_none() {
        assert!(act("nope://x", "y").is_none());
    }
}
