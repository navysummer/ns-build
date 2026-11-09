//! 执行器：并行调度任务 DAG。
//!
//! Kahn 式就绪队列 + 有界并发：依赖满足即派发，运行中任务数 ≤ `parallel`。
//! 缓存命中任务直接跳过；失败后停止派发新任务，等待在途完成。

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::cache::Cache;
use crate::graph::{BuildGraph, NodeId};
use crate::planner::{hash_cmd, Plan};

/// 执行结果统计。
#[derive(Debug, Default, Clone)]
pub struct ExecReport {
    pub built: usize,
    pub skipped: usize,
    pub failed: usize,
    pub total: usize,
    /// 总耗时（秒）。
    pub elapsed: f64,
}

/// 执行选项。
#[derive(Debug, Clone)]
pub struct ExecOptions {
    /// 并行度。
    pub parallel: usize,
    /// 不打印任务标签。
    pub quiet: bool,
    /// 打印每条完整命令。
    pub verbose: bool,
    /// 目标过滤（None = 全部；否则匹配 target 名）。
    pub targets: Option<Vec<String>>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            parallel: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
            quiet: false,
            verbose: false,
            targets: None,
        }
    }
}

/// 单任务结果。
struct TaskResult {
    id: NodeId,
    ok: bool,
    inputs: Vec<PathBuf>,
}

/// 执行构建图。
pub fn execute(
    plan: &mut Plan,
    cache: &mut Cache,
    opts: &ExecOptions,
    cwd: &Path,
) -> Result<ExecReport, String> {
    let start = std::time::Instant::now();
    let n = plan.graph.len();
    let mut report = ExecReport { total: n, ..Default::default() };

    // 拓扑排序（校验无环）。
    let order = plan.graph.topo_order()?;

    // 目标过滤：key 形如 `<kind>:<target>:<subtask>`，取第二段匹配目标名。
    let filter: Option<BTreeSet<String>> = opts.targets.as_ref().map(|ts| ts.iter().cloned().collect());
    let key_ok = |key: &str| -> bool {
        match &filter {
            None => true,
            Some(set) => key
                .split(':')
                .nth(1)
                .map(|t| set.contains(t))
                .unwrap_or(false),
        }
    };

    // 入度与就绪队列。
    let mut indeg: Vec<usize> = (0..n).map(|i| plan.graph.deps_of(i).len()).collect();
    let mut ready: VecDeque<NodeId> = VecDeque::new();
    for &i in &order {
        if indeg[i] == 0 {
            ready.push_back(i);
        }
    }

    let mut done = vec![false; n];
    let mut pending = n;
    let running = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<TaskResult>();

    // 派发一个任务（若并发许可）。
    let spawn = |id: NodeId,
                 graph: &BuildGraph,
                 tx: mpsc::Sender<TaskResult>,
                 running: &Arc<AtomicUsize>,
                 opts: &ExecOptions,
                 cwd: PathBuf| {
        let node = graph.node(id).clone();
        let tx = tx.clone();
        let running = running.clone();
        let quiet = opts.quiet;
        let verbose = opts.verbose;
        running.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || {
            if !node.cached {
                if !quiet {
                    eprintln!("  [{}]", node.label);
                }
                if verbose {
                    eprintln!("    {}", node.command.join(" "));
                }
            }
            let ok = if node.cached {
                true
            } else {
                // 确保产物父目录存在（含自定义命令的额外产物）。
                for out in std::iter::once(&node.output).chain(node.extra_outputs.iter()) {
                    if !out.as_os_str().is_empty() {
                        if let Some(parent) = out.parent() {
                            if !parent.exists() {
                                if let Err(e) = std::fs::create_dir_all(parent) {
                                    eprintln!("  [ERROR] 创建目录 {} 失败: {e}", parent.display());
                                }
                            }
                        }
                    }
                }
                run_command(&node.command, &cwd)
            };
            let _ = tx.send(TaskResult { id, ok, inputs: node.inputs.clone() });
            running.fetch_sub(1, Ordering::SeqCst);
        });
    };

    while pending > 0 {
        // 尽可能派发就绪任务。
        while running.load(Ordering::SeqCst) < opts.parallel {
            let Some(id) = ready.pop_front() else { break };
            let node = plan.graph.node(id);
            if failed.load(Ordering::SeqCst) {
                // 失败后：标记为 done，不执行。
                done[id] = true;
                pending -= 1;
                report.skipped += 1;
                unlock_rdeps(id, &plan.graph, &mut indeg, &mut ready, &done);
                continue;
            }
            if !key_ok(&node.key) {
                done[id] = true;
                pending -= 1;
                unlock_rdeps(id, &plan.graph, &mut indeg, &mut ready, &done);
                continue;
            }
            if node.cached {
                done[id] = true;
                pending -= 1;
                report.skipped += 1;
                unlock_rdeps(id, &plan.graph, &mut indeg, &mut ready, &done);
                continue;
            }
            if !node.parallel && running.load(Ordering::SeqCst) > 0 {
                // 串行任务须等并发任务清空。
                ready.push_front(id);
                break;
            }
            spawn(id, &plan.graph, tx.clone(), &running, opts, cwd.to_path_buf());
        }

        // 全部完成。
        if pending == 0 {
            break;
        }
        // 无运行任务且无就绪：先尝试收一个已完成的结果（线程可能刚结束，
        // result 尚在通道中），避免误判停滞。
        if running.load(Ordering::SeqCst) == 0 && ready.is_empty() {
            match rx.try_recv() {
                Ok(result) => {
                    done[result.id] = true;
                    pending -= 1;
                    if result.ok {
                        let node = plan.graph.node(result.id);
                        if !node.cached {
                            if !node.output.as_os_str().is_empty() {
                                let cmd_hash = hash_cmd(&node.command);
                                let mut outs: Vec<String> =
                                    node.extra_outputs.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                                outs.insert(0, node.output.to_string_lossy().into_owned());
                                let rec_inputs = refresh_compile_inputs(node, &result.inputs);
                                cache.record(node.key.clone(), &rec_inputs, cmd_hash, outs);
                                // 产物变化 → 使所有依赖此产物的后继缓存失效。
                                let rdeps: Vec<NodeId> =
                                    plan.graph.rdeps_of(result.id).iter().copied().collect();
                                for r in rdeps {
                                    plan.graph.node_mut(r).cached = false;
                                }
                            }
                            report.built += 1;
                        }
                    } else {
                        failed.store(true, Ordering::SeqCst);
                        report.failed += 1;
                    }
                    unlock_rdeps(result.id, &plan.graph, &mut indeg, &mut ready, &done);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("任务通道关闭".to_string());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    return Err(format!("调度停滞：仍有 {pending} 个任务未执行"));
                }
            }
        }

        // 等待一个结果。用短超时 + 回到顶部复查：工作线程先 send 后
        // fetch_sub，存在 running 过期（偏高）的窗口；若在此窗口内阻塞
        // recv 且该线程已发送最后一条结果后不再发送，会永久等待。
        // 短超时回到循环顶部复查 running/ready 即可自愈（同排空循环）。
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(result) => {
                done[result.id] = true;
                pending -= 1;
                if result.ok {
                    let node = plan.graph.node(result.id);
                    let actually_built = !node.cached;
                    if actually_built {
                        if !node.output.as_os_str().is_empty() {
                            let cmd_hash = hash_cmd(&node.command);
                            let mut outs: Vec<String> =
                                node.extra_outputs.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                            outs.insert(0, node.output.to_string_lossy().into_owned());
                            let rec_inputs = refresh_compile_inputs(node, &result.inputs);
                            cache.record(node.key.clone(), &rec_inputs, cmd_hash, outs);
                            // 输入变了 → 使所有依赖此产物的后继缓存失效。
                            let rdeps: Vec<NodeId> =
                                plan.graph.rdeps_of(result.id).iter().copied().collect();
                            for r in rdeps {
                                plan.graph.node_mut(r).cached = false;
                            }
                        }
                        report.built += 1;
                    }
                } else {
                    failed.store(true, Ordering::SeqCst);
                    report.failed += 1;
                }
                unlock_rdeps(result.id, &plan.graph, &mut indeg, &mut ready, &done);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("任务通道关闭".to_string());
            }
        }
    }

    report.elapsed = start.elapsed().as_secs_f64();
    // 失败时等待所有在途任务完成，避免悬空线程。
    // 注意：工作线程先 send 再 fetch_sub。存在一个窗口：主线程看到
    // running==0（本线程递减已提交）但最后一条结果尚未被本循环消费；
    // 此时不能无条件阻塞 rx.recv()（会永久等待），而应短超时重查 running。
    loop {
        if running.load(Ordering::SeqCst) == 0 {
            // 在途计数已清零；消费残留结果（若存在），然后结束。
            loop {
                match rx.try_recv() {
                    Ok(result) => {
                        done[result.id] = true;
                        unlock_rdeps(result.id, &plan.graph, &mut indeg, &mut ready, &done);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            }
            break;
        }
        // 仍有在途任务：阻塞等待一条结果；短超时后回到顶部复查 running。
        let Some(result) = rx.recv_timeout(std::time::Duration::from_millis(50)).ok() else {
            continue;
        };
        done[result.id] = true;
        unlock_rdeps(result.id, &plan.graph, &mut indeg, &mut ready, &done);
    }
    Ok(report)
}

/// 解锁依赖节点的所有后继。
fn unlock_rdeps(
    id: NodeId,
    graph: &BuildGraph,
    indeg: &mut [usize],
    ready: &mut VecDeque<NodeId>,
    done: &[bool],
) {
    for r in graph.rdeps_of(id).iter().copied() {
        if done[r] {
            continue;
        }
        indeg[r] = indeg[r].saturating_sub(1);
        if indeg[r] == 0 {
            ready.push_back(r);
        }
    }
}

/// 运行外部命令，打印 stdout/stderr。
pub fn run_command(cmd: &[String], cwd: &Path) -> bool {
    // 空命令（phony / interface / 空 object）视为成功。
    if cmd.is_empty() {
        return true;
    }
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    c.current_dir(cwd);
    c.stdin(Stdio::null());
    match c.output() {
        Ok(o) => {
            if !o.stdout.is_empty() {
                print!("{}", String::from_utf8_lossy(&o.stdout));
            }
            if !o.stderr.is_empty() {
                eprint!("{}", String::from_utf8_lossy(&o.stderr));
            }
            o.status.success()
        }
        Err(e) => {
            eprintln!("  [ERROR] 执行 {} 失败: {e}", cmd[0]);
            false
        }
    }
}

/// 编译任务成功后：用最新 .d 文件刷新缓存输入（源文件 + 头文件去重合并）。
/// 这样下次构建时头文件变化能命中"输入哈希不一致"而被重编译。
fn refresh_compile_inputs(node: &crate::graph::Node, base: &[PathBuf]) -> Vec<PathBuf> {
    let Some(d) = &node.depfile else {
        return base.to_vec();
    };
    let mut v = base.to_vec();
    if let Ok(deps) = crate::depfile::parse_depfile(d) {
        for dep in deps {
            if !v.iter().any(|x| x == &dep) {
                v.push(dep);
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{BuildGraph, Node, TaskKind};

    fn simple_node(key: &str, cmd: Vec<String>) -> Node {
        Node {
            id: 0,
            key: key.into(),
            kind: TaskKind::Command,
            inputs: vec![],
            depfile: None,
            output: PathBuf::new(),
            extra_outputs: vec![],
            command: cmd,
            parallel: true,
            label: key.into(),
            cached: false,
        }
    }

    #[test]
    fn runs_dag_in_order() {
        let mut g = BuildGraph::new();
        let a = g.add_node(simple_node(
            "a",
            vec!["sh".into(), "-c".into(), "echo a >> order.txt".into()],
        ));
        let b = g.add_node(simple_node(
            "b",
            vec!["sh".into(), "-c".into(), "echo b >> order.txt".into()],
        ));
        g.add_dep(b, a);
        let mut plan = Plan { graph: g, outputs: Default::default(), skipped: 0 };
        let dir = std::env::temp_dir();
        let cache = dir.join("nsbuild-exec-test-cache");
        let mut cache = Cache::load(&cache).unwrap();
        let opts = ExecOptions { parallel: 4, quiet: true, verbose: false, targets: None };
        let rep = execute(&mut plan, &mut cache, &opts, &dir).unwrap();
        assert_eq!(rep.built, 2);
    }

    /// 回归测试：主循环退出后进入排空循环时，最后一个工作线程可能处于
    /// "已 send 未 fetch_sub" 的窗口。排空循环必须自我修复（running==0 时
    /// 消费残留并结束），不能无条件阻塞 rx.recv()。多次迭代放大命中概率。
    #[test]
    fn drain_loop_does_not_deadlock_on_last_task() {
        for i in 0..200 {
            let mut g = BuildGraph::new();
            let _a = g.add_node(simple_node(
                &format!("t{i}"),
                vec!["sh".into(), "-c".into(), "true".into()],
            ));
            let mut plan = Plan { graph: g, outputs: Default::default(), skipped: 0 };
            let dir = std::env::temp_dir();
            let cache = dir.join("nsbuild-exec-drain-test");
            let mut cache = Cache::load(&cache).unwrap();
            let opts = ExecOptions { parallel: 1, quiet: true, verbose: false, targets: None };
            let rep = execute(&mut plan, &mut cache, &opts, &dir).unwrap();
            assert_eq!(rep.built, 1, "迭代 {i} 应构建 1 个任务");
        }
    }

    /// 回归测试：串行任务等待并发任务清空时，主线程可能在 running 过期
    /// （偏高）窗口内阻塞 rx.recv()（原第 226 行），而该工作线程已发送最后
    /// 一条结果、不会再发送 → 永久死锁。并发节点 + 串行节点 + 多次迭代放大。
    #[test]
    fn serial_task_after_concurrent_does_not_deadlock() {
        for i in 0..200 {
            let mut g = BuildGraph::new();
            // 一个并发任务（快速空命令），随后一个串行任务依赖它。
            let c = g.add_node(simple_node(
                &format!("c{i}"),
                vec!["sh".into(), "-c".into(), "true".into()],
            ));
            let mut s = simple_node(&format!("s{i}"), vec!["sh".into(), "-c".into(), "true".into()]);
            s.parallel = false; // 串行任务
            let s = g.add_node(s);
            g.add_dep(s, c);
            let mut plan = Plan { graph: g, outputs: Default::default(), skipped: 0 };
            let dir = std::env::temp_dir();
            let cache = dir.join("nsbuild-exec-serial-test");
            let mut cache = Cache::load(&cache).unwrap();
            // parallel=2 让并发任务先跑，串行任务在 ready 中等待 running 清空。
            let opts = ExecOptions { parallel: 2, quiet: true, verbose: false, targets: None };
            let rep = execute(&mut plan, &mut cache, &opts, &dir).unwrap();
            assert_eq!(rep.built, 2, "迭代 {i} 应构建 2 个任务");
        }
    }
}
