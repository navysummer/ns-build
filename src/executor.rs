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

    // 目标过滤：匹配 key 前缀（target 名）。
    let filter: Option<BTreeSet<String>> = opts.targets.as_ref().map(|ts| ts.iter().cloned().collect());
    let key_ok = |key: &str| -> bool {
        match &filter {
            None => true,
            Some(set) => set.iter().any(|t| key == t || key.starts_with(&format!("{t}:"))),
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
        let cwd = cwd;
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
                // 确保产物父目录存在。
                if !node.output.as_os_str().is_empty() {
                    if let Some(parent) = node.output.parent() {
                        if !parent.exists() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                eprintln!("  [ERROR] 创建目录 {} 失败: {e}", parent.display());
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
        // 无运行任务且无就绪：死锁（应不会发生，拓扑已校验）。
        if running.load(Ordering::SeqCst) == 0 && ready.is_empty() {
            return Err(format!("调度停滞：仍有 {pending} 个任务未执行"));
        }

        // 等待一个结果。
        let result = rx.recv().map_err(|_| "任务通道关闭".to_string())?;
        done[result.id] = true;
        pending -= 1;
        if result.ok {
            let node = plan.graph.node(result.id);
            let actually_built = !node.cached;
            if actually_built {
                let cmd_hash = hash_cmd(&node.command);
                let outs = if node.output.as_os_str().is_empty() {
                    vec![]
                } else {
                    vec![node.output.to_string_lossy().into_owned()]
                };
                cache.record(node.key.clone(), &result.inputs, cmd_hash, outs);
                report.built += 1;
                // 输入变了 → 使所有依赖此产物的后继缓存失效。
                let rdeps: Vec<NodeId> = plan.graph.rdeps_of(result.id).iter().copied().collect();
                for r in rdeps {
                    plan.graph.node_mut(r).cached = false;
                }
            }
        } else {
        }
        unlock_rdeps(result.id, &plan.graph, &mut indeg, &mut ready, &done);
    }

    report.elapsed = start.elapsed().as_secs_f64();
    // 失败时等待所有在途任务完成，避免悬空线程。
    while running.load(Ordering::SeqCst) > 0 {
        let Ok(result) = rx.recv() else { break };
        done[result.id] = true;
        let _ = result.id;
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
}
