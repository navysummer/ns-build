//! 规划器：将解析后的脚本目标转换成任务 DAG。
//!
//! 支持：
//! - 分步构建（`kind = "object"`）：只产出 `.o`，供其他 target 链接；
//! - unity build（`groups` / `group_size`）：多源文件合并编译；
//! - 增量构建：用缓存指纹判断哪些任务可跳过。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::compiler::{
    CompileContext, CompileKind, compile_command, is_cxx, make_link_task,
    unity_compile_command, unity_source_content,
};
use crate::graph::{BuildGraph, Node, NodeId, TaskKind};
use crate::script::{Language, ResolvedScript, ResolvedTarget, TargetKind};

/// 规划结果。
pub struct Plan {
    pub graph: BuildGraph,
    /// 每个 target 名 → 其产物节点（最终节点）。
    pub outputs: BTreeMap<String, NodeId>,
    /// 跳过（缓存命中）的任务数。
    pub skipped: usize,
}

/// 生成 CMake 兼容的 `compile_commands.json`（供 clangd / IDE 使用）。
/// 从构建图中所有编译任务收集命令。
pub fn generate_compile_commands(plan: &Plan, cwd: &std::path::Path) -> String {
    use serde_json::json;
    let mut entries = Vec::new();
    for node in plan.graph.nodes() {
        if node.kind != TaskKind::Compile {
            continue;
        }
        let file = node
            .inputs
            .first()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let directory = cwd.to_string_lossy().into_owned();
        let command = node.command.join(" ");
        entries.push(json!({
            "directory": directory,
            "command": command,
            // arguments 数组：参数含空格时不会被错误切分（clangd 优先使用它）。
            "arguments": node.command,
            "file": file,
            "output": node.output.to_string_lossy(),
        }));
    }
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".into())
}

/// 从解析后的脚本规划 DAG。
pub fn plan(script: &ResolvedScript, cache: &Cache) -> Result<Plan, String> {
    let mut graph = BuildGraph::new();
    let mut outputs: BTreeMap<String, NodeId> = BTreeMap::new();
    let mut skipped = 0usize;

    let out_dir = &script.out_dir;
    let obj_dir = out_dir.join("obj");

    // 第零遍：自定义命令节点（生成生成文件，targets 引用其产物）。
    let mut custom_by_out: BTreeMap<PathBuf, NodeId> = BTreeMap::new();
    let mut custom_by_name: BTreeMap<String, NodeId> = BTreeMap::new();
    for cc in &script.custom_commands {
        let argv = command_argv(&cc.command, &script.host);
        let cmd_hash = hash_cmd(&argv);
        let key = format!("custom:{}", cc.name);
        let cached = cache.hit(&key, &cc.depends, &cmd_hash);
        if cached {
            skipped += 1;
        }
        let out0 = cc.outputs.first().cloned().unwrap_or_default();
        let node = Node {
            id: 0,
            key,
            kind: TaskKind::Command,
            inputs: cc.depends.clone(),
            depfile: None,
            output: out0,
            extra_outputs: cc.outputs.iter().skip(1).cloned().collect(),
            command: argv,
            parallel: true,
            label: format!("GEN {}", cc.name),
            cached,
        };
        let nid = graph.add_node(node);
        for o in &cc.outputs {
            custom_by_out.insert(o.clone(), nid);
        }
        custom_by_name.insert(cc.name.clone(), nid);
    }
    // 自定义命令之间的依赖（按名称）。
    for cc in &script.custom_commands {
        let id = custom_by_name[&cc.name];
        for d in &cc.deps {
            let Some(&dep_id) = custom_by_name.get(d) else {
                return Err(format!(
                    "custom_command '{}' 依赖不存在的自定义命令 '{}'",
                    cc.name, d
                ));
            };
            graph.add_dep(id, dep_id);
        }
    }

    // 第零遍：为所有 target 创建 before 钩子节点（无依赖，先落地）。
    // before 钩子保证同 target 内的编译/链接节点之前先执行，
    // 常用于生成头文件 / 编译前的环境准备。
    let mut before_hooks: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for t in &script.targets {
        if t.hooks.before.is_empty() {
            continue;
        }
        let mut ids = Vec::with_capacity(t.hooks.before.len());
        for cmd in &t.hooks.before {
            let cid = make_command_node(&t.name, "before", cmd, &t.host);
            ids.push(graph.add_node(cid));
        }
        before_hooks.insert(t.name.clone(), ids);
    }

    // 第一遍：为每个 target 生成对象编译节点（含 unity）。
    let mut obj_nodes: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for t in &script.targets {
        let hook_ids = before_hooks.get(&t.name).cloned().unwrap_or_default();
        // phony / interface / alias 无编译产物。
        if matches!(
            t.kind,
            TargetKind::Phony | TargetKind::Interface | TargetKind::Alias
        ) {
            obj_nodes.insert(t.name.clone(), vec![]);
            continue;
        }
        // 非 C/C++ 语言：直接由链接任务处理源文件，不产生对象节点。
        if !is_native_c(t) {
            obj_nodes.insert(t.name.clone(), vec![]);
            continue;
        }
        let ctx = ObjCompileCtx {
            obj_dir: &obj_dir,
            out_dir,
            unity_mode: &script.script.defaults.unity,
            parallel: script.parallel,
        };
        let objs = plan_object_nodes(t, &ctx, cache, &mut skipped, &custom_by_out)?;
        let mut ids = Vec::new();
        for (n, customs) in objs {
            let id = graph.add_node(n);
            for c in customs {
                graph.add_dep(id, c);
            }
            // before 钩子全部就绪后再编译该 target 的对象文件。
            for &h in &hook_ids {
                graph.add_dep(id, h);
            }
            ids.push(id);
        }
        obj_nodes.insert(t.name.clone(), ids);
    }

    // 目标名 → 定义，供依赖解析（alias → 最终目标）。
    let targets_by_name: BTreeMap<&str, &ResolvedTarget> =
        script.targets.iter().map(|t| (t.name.as_str(), t)).collect();
    // 已声明过的 target 全集（含平台/when 过滤掉的）：用于区分"未声明"与"条件过滤"。
    let declared: BTreeSet<&str> = script.declared.iter().map(|s| s.as_str()).collect();

    // 传递求值：target 或其（传递）依赖含 C++ 源/语言 → 链接用 C++ 链接器。
    // 静态库链里的 C++ 对象必须用 c++ 链接，否则 undefined symbols。
    let unaliases: BTreeMap<String, String> = script
        .targets
        .iter()
        .map(|t| (t.name.clone(), crate::script::unalias(&t.name, &script.targets)))
        .collect();
    let unalias_of = |dep: &str| -> String {
        unaliases.get(dep).cloned().unwrap_or_else(|| dep.to_string())
    };
    let mut has_cxx_map: BTreeMap<String, bool> = script
        .targets
        .iter()
        .map(|t| {
            (
                t.name.clone(),
                t.sources.iter().any(|s| is_cxx(s))
                    || matches!(t.language, Language::Cpp),
            )
        })
        .collect();
    for _ in 0..script.targets.len().max(1) {
        let snapshot = has_cxx_map.clone();
        let mut changed = false;
        for t in &script.targets {
            if has_cxx_map[&t.name] {
                continue;
            }
            for dep in t.deps.iter().chain(t.private_deps.iter()) {
                let ud = unalias_of(dep);
                if snapshot.get(&ud).copied().unwrap_or(false) {
                    has_cxx_map.insert(t.name.clone(), true);
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // 第二遍：构造聚合/链接节点，并处理 deps。
    for t in &script.targets {
        match t.kind {
            // alias 无产物：指向底层节点（第一遍结束后统一登记，见下）。
            TargetKind::Alias => {}
            // phony / interface 无产物：生成空命令节点，保证依赖边可解析。
            TargetKind::Phony | TargetKind::Interface => {
                let kind_name = if t.kind == TargetKind::Phony {
                    "PHONY"
                } else {
                    "INTERFACE"
                };
                let nid = graph.add_node(Node {
                    id: 0,
                    key: format!("phony:{}", t.name),
                    kind: TaskKind::Command,
                    inputs: vec![],
                    depfile: None,
                    output: PathBuf::new(),
                    extra_outputs: vec![],
                    command: vec![],
                    parallel: false,
                    label: format!("{kind_name} {}", t.name),
                    cached: true,
                });
                outputs.insert(t.name.clone(), nid);
            }
            TargetKind::Object => {
                if is_native_c(t) {
                    let ids = obj_nodes.get(&t.name).cloned().unwrap_or_default();
                    if let Some(&last) = ids.last() {
                        outputs.insert(t.name.clone(), last);
                    } else {
                        // 无源文件的空 object：登记 no-op 节点。
                        let nid = graph.add_node(noop_node(&t.name, "OBJECT"));
                        outputs.insert(t.name.clone(), nid);
                    }
                } else {
                    // 非 C/C++：object 也走链接/打包任务。
                    let link_id = make_simple_link(t, cache, &mut graph, &mut skipped, has_cxx_map.get(&t.name).copied().unwrap_or(false))?;
                    for s in &t.sources {
                        if let Some(&c) = custom_by_out.get(s) {
                            graph.add_dep(link_id, c);
                        }
                    }
                    outputs.insert(t.name.clone(), link_id);
                }
            }
            _ => {
                let obj_ids = obj_nodes.get(&t.name).cloned().unwrap_or_default();
                // 链接输入：自身对象 + 依赖的对象/静态库/动态库。
                let mut link_inputs: Vec<PathBuf> = Vec::new();
                let mut node_inputs: Vec<NodeId> = obj_ids.clone();
                if is_native_c(t) {
                    for &id in &obj_ids {
                        link_inputs.push(graph.node(id).output.clone());
                    }
                } else {
                    // 非 C/C++：链接/打包输入是源文件本身。
                    link_inputs.extend(t.sources.iter().cloned());
                }
                let mut seen_libs: BTreeSet<String> = BTreeSet::new();
                for dep in t.deps.iter().chain(t.private_deps.iter()) {
                    let ud = unalias_of(dep);
                    let Some(dep_t) = targets_by_name.get(ud.as_str()).copied() else {
                        // 声明但被条件过滤的依赖：跳过（不链接、不建边）。
                        if declared.contains(ud.as_str()) {
                            continue;
                        }
                        return Err(format!("target '{}' 依赖不存在的 target '{}'", t.name, dep));
                    };
                    let dep_ids = obj_nodes.get(ud.as_str()).cloned().unwrap_or_default();
                    match dep_t.kind {
                        // object 库：把对象并入链接（静态库归档也并入成员）。
                        TargetKind::Object => {
                            for &id in &dep_ids {
                                if !node_inputs.contains(&id) {
                                    node_inputs.push(id);
                                }
                                if is_native_c(t) {
                                    let out = graph.node(id).output.clone();
                                    if !link_inputs.contains(&out) {
                                        link_inputs.push(out);
                                    }
                                }
                            }
                        }
                        // 静态/动态库：可执行/共享库直接链接其产物，并递归
                        // 收集它依赖的库（静态库链式依赖）。
                        TargetKind::Static | TargetKind::Lib | TargetKind::Module
                            if t.kind != TargetKind::Static && is_native_c(t) => {
                                collect_link_libs(
                                    &ud,
                                    &targets_by_name,
                                    &mut link_inputs,
                                    &mut seen_libs,
                                    &unaliases,
                                );
                            }
                        _ => {}
                    }
                }
                let link_is_cpp = has_cxx_map.get(&t.name).copied().unwrap_or(false);
                let link_task = make_link_task(t, link_inputs, t.host, t.profile, &t.ar, link_is_cpp);
                let link_cmd_hash = hash_cmd(&link_task.command);
                let link_inputs = link_task.inputs.clone();
                let cached = cache.hit(&link_task.key, &link_inputs, &link_cmd_hash);
                if cached {
                    skipped += 1;
                }
                let link_node = Node {
                    id: 0,
                    key: link_task.key.clone(),
                    kind: match link_task.kind {
                        CompileKind::Archive => TaskKind::Command,
                        _ => TaskKind::Link,
                    },
                    inputs: link_inputs.clone(),
                    depfile: None,
                    output: link_task.output.clone(),
                    extra_outputs: vec![],
                    command: link_task.command.clone(),
                    parallel: t.parallel,
                    label: link_task.label.clone(),
                    cached,
                };
                let link_id = graph.add_node(link_node);
                for &o in &node_inputs {
                    graph.add_dep(link_id, o);
                }
                for s in &t.sources {
                    if let Some(&c) = custom_by_out.get(s) {
                        graph.add_dep(link_id, c);
                    }
                }
                outputs.insert(t.name.clone(), link_id);
            }
        }
    }

    // alias 登记：alias 名 → 底层 target 的节点（共享同一构建节点）。
    for t in &script.targets {
        if t.kind != TargetKind::Alias {
            continue;
        }
        let ud = unalias_of(&t.name);
        match outputs.get(ud.as_str()) {
            Some(&nid) => {
                outputs.insert(t.name.clone(), nid);
            }
            None => {
                return Err(format!(
                    "alias 目标 '{}' 指向不存在的 target '{}'",
                    t.name, ud
                ));
            }
        }
    }

    // 第三遍：target 间 deps 边 + 钩子。
    for t in &script.targets {
        let Some(&me) = outputs.get(&t.name) else {
            continue;
        };
        // before 钩子：me 必须等钩子跑完。obj 节点在第一遍已挂依赖，此处覆盖
        // phony/interface/alias/Non-C 等无 obj 节点的目标。
        if let Some(hooks) = before_hooks.get(&t.name) {
            for &h in hooks {
                graph.add_dep(me, h);
            }
        }
        for dep in t.deps.iter().chain(t.private_deps.iter()) {
            let ud = unalias_of(dep);
            let Some(&dn) = outputs.get(ud.as_str()) else {
                if declared.contains(ud.as_str()) {
                    continue; // 声明但被条件过滤：跳过
                }
                return Err(format!("target '{}' 依赖不存在的 target '{}'", t.name, dep));
            };
            if dn != me {
                graph.add_dep(me, dn);
            }
        }
        if !t.hooks.after.is_empty() {
            for cmd in &t.hooks.after {
                let cid = make_command_node(&t.name, "after", cmd, &t.host);
                let cid = graph.add_node(cid);
                graph.add_dep(cid, me);
            }
        }
    }

    graph.validate()?;
    Ok(Plan { graph, outputs, skipped })
}

/// 对象编译节点生成的共享上下文。
#[derive(Clone, Copy)]
struct ObjCompileCtx<'a> {
    obj_dir: &'a Path,
    out_dir: &'a Path,
    unity_mode: &'a str,
    parallel: usize,
}

/// 为可链接/分步 target 规划对象编译节点。
/// 返回 (节点, 其依赖的自定义命令节点) 列表。
fn plan_object_nodes(
    t: &ResolvedTarget,
    ctx: &ObjCompileCtx<'_>,
    cache: &Cache,
    skipped: &mut usize,
    custom_by_out: &BTreeMap<PathBuf, NodeId>,
) -> Result<Vec<(Node, Vec<NodeId>)>, String> {
    let ObjCompileCtx {
        obj_dir,
        out_dir,
        unity_mode,
        parallel,
    } = *ctx;
    let mut nodes: Vec<(Node, Vec<NodeId>)> = Vec::new();
    if t.sources.is_empty() {
        // 纯依赖 target（如只链接依赖的对象）。
        return Ok(nodes);
    }
    // 分组的决定：None = 逐文件编译，Some(n) = n 组 unity。
    let n_src = t.sources.len();
    let groups = decide_groups(t, n_src, unity_mode, parallel);

    if let Some(groups) = groups {
        // 将源文件按语言拆分后再均分为 groups 组（C 与 C++ 绝不混合编译）。
        let per = n_src.div_ceil(groups);
        let unity_dir = out_dir.join("unity");
        let _ = std::fs::create_dir_all(&unity_dir);
        let mut lang_groups: Vec<(bool, Vec<PathBuf>)> =
            vec![(false, vec![]), (true, vec![])];
        for s in &t.sources {
            lang_groups[is_cxx(s) as usize].1.push(s.clone());
        }
        let mut gi = 0usize;
        for (chunk_cxx, srcs) in lang_groups {
            if srcs.is_empty() {
                continue;
            }
            for chunk in srcs.chunks(per) {
                let ext = if chunk_cxx { "cpp" } else { "c" };
                let obj = obj_dir.join(format!("{}_{}.o", safe_name(&t.name), gi));
                let depfile = obj_dir.join(format!("{}_{}.d", safe_name(&t.name), gi));
                let unity_src = unity_dir.join(format!("{}_{}.{ext}", safe_name(&t.name), gi));
                gi += 1;
                let _ = std::fs::write(&unity_src, unity_source_content(chunk));
                let compiler = if chunk_cxx { &t.cxx } else { &t.cc };
                let node_depfile = if crate::compiler::is_msvc_compiler(compiler) { None } else { Some(depfile.clone()) };
                let cmd = unity_compile_command(
                    &CompileContext {
                        cc: compiler,
                        target: t,
                        profile: t.profile,
                        is_cxx: chunk_cxx,
                    },
                    &unity_src,
                    &obj,
                    &depfile,
                );
                let key = format!("unity:{}:{}", t.name, gi - 1);
                let cmd_hash = hash_cmd(&cmd);
                // 缓存判定需把 depfile 里的头文件算进输入（否则改头文件不重编）。
                let hit_inputs = effective_inputs(chunk, &depfile);
                let cached = cache.hit(&key, &hit_inputs, &cmd_hash);
                if cached {
                    *skipped += 1;
                }
                let inputs = if cached { hit_inputs } else { chunk.to_vec() };
                let custom_deps: Vec<NodeId> = {
                    let mut v: Vec<NodeId> = chunk
                        .iter()
                        .filter_map(|s| custom_by_out.get(s).copied())
                        .collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                nodes.push((Node {
                    id: 0,
                    key,
                    kind: TaskKind::Compile,
                    inputs,
                    depfile: node_depfile,
                    output: obj,
                    extra_outputs: vec![],
                    command: cmd,
                    parallel: t.parallel,
                    label: format!("CC {} [{}]", t.name, chunk.len()),
                    cached,
                }, custom_deps));
            }
        }
    } else {
        for (i, src) in t.sources.iter().enumerate() {
            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("src");
            let obj = obj_dir.join(format!("{}_{}_{}.o", safe_name(&t.name), i, safe_name(stem)));
            let depfile = obj_dir.join(format!("{}_{}_{}.d", safe_name(&t.name), i, safe_name(stem)));
            let is_cxx_src = is_cxx(src);
            let compiler = if is_cxx_src { &t.cxx } else { &t.cc };
            let node_depfile = if crate::compiler::is_msvc_compiler(compiler) { None } else { Some(depfile.clone()) };
            let cmd = compile_command(
                &CompileContext {
                    cc: compiler,
                    target: t,
                    profile: t.profile,
                    is_cxx: is_cxx_src,
                },
                src,
                &obj,
                &depfile,
            );
            let key = format!("obj:{}:{}", t.name, i);
            let cmd_hash = hash_cmd(&cmd);
            let hit_inputs = effective_inputs(std::slice::from_ref(src), &depfile);
            let cached = cache.hit(&key, &hit_inputs, &cmd_hash);
            if cached {
                *skipped += 1;
            }
            let inputs = if cached { hit_inputs } else { vec![src.clone()] };
            let custom_deps: Vec<NodeId> = custom_by_out
                .get(src)
                .copied()
                .into_iter()
                .collect();
            nodes.push((Node {
                id: 0,
                key,
                kind: TaskKind::Compile,
                inputs,
                depfile: node_depfile,
                output: obj,
                extra_outputs: vec![],
                command: cmd,
                parallel: t.parallel,
                label: format!("CC {} ({})", t.name, src.file_name().and_then(|s| s.to_str()).unwrap_or("")),
                cached,
            }, custom_deps));
        }
    }
    Ok(nodes)
}

/// 递归收集 target 依赖的静态/动态库产物（用于最终链接）。
/// `unaliases` 来自预计算表（alias → 最终目标名）。
fn collect_link_libs(
    name: &str,
    targets: &BTreeMap<&str, &ResolvedTarget>,
    out: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<String>,
    unaliases: &BTreeMap<String, String>,
) {
    let ud = unaliases.get(name).cloned().unwrap_or_else(|| name.to_string());
    if !seen.insert(ud.clone()) {
        return;
    }
    let Some(t) = targets.get(ud.as_str()) else {
        return;
    };
    if matches!(
        t.kind,
        TargetKind::Static | TargetKind::Lib | TargetKind::Module
    ) && !out.contains(&t.output)
    {
        out.push(t.output.clone());
    }
    for dep in t.deps.iter().chain(t.private_deps.iter()) {
        collect_link_libs(dep, targets, out, seen, unaliases);
    }
}

/// 是否 C/C++ 原生编译语言（需要对象编译步骤）。
fn is_native_c(t: &ResolvedTarget) -> bool {
    matches!(t.language, Language::C | Language::Cpp)
}

/// 非 C/C++：直接构建一个链接/打包任务（输入为源文件）。
fn make_simple_link(
    t: &ResolvedTarget,
    cache: &Cache,
    graph: &mut BuildGraph,
    skipped: &mut usize,
    link_is_cpp: bool,
) -> Result<NodeId, String> {
    let link_task = make_link_task(t, t.sources.clone(), t.host, t.profile, &t.ar, link_is_cpp);
    let cmd_hash = hash_cmd(&link_task.command);
    let inputs = link_task.inputs.clone();
    let cached = cache.hit(&link_task.key, &inputs, &cmd_hash);
    if cached {
        *skipped += 1;
    }
    let link_node = Node {
        id: 0,
        key: link_task.key.clone(),
        kind: TaskKind::Link,
        inputs: link_task.inputs.clone(),
        depfile: None,
        output: link_task.output.clone(),
        extra_outputs: vec![],
        command: link_task.command.clone(),
        parallel: t.parallel,
        label: link_task.label.clone(),
        cached,
    };
    Ok(graph.add_node(link_node))
}

/// 决定 unity 分组：`None` 表示逐文件编译，`Some(n)` 表示 n 组。
fn decide_groups(
    t: &ResolvedTarget,
    n_src: usize,
    unity_mode: &str,
    parallel: usize,
) -> Option<usize> {
    if n_src == 0 {
        return None;
    }
    if let Some(g) = t.groups {
        return Some(g.max(1).min(n_src));
    }
    if let Some(gs) = t.group_size {
        let gs = gs.max(1);
        return Some(n_src.div_ceil(gs).max(1).min(n_src));
    }
    match unity_mode {
        "off" | "false" | "0" => None,
        "on" | "true" | "1" => Some(n_src.min(parallel.max(1))),
        // auto：源文件足够多时按每 4 个一组切分，最多并行度组。
        _ => {
            if n_src >= 4 {
                let mut g = n_src.div_ceil(4);
                g = g.max(2).min(parallel.max(1)).min(n_src);
                Some(g)
            } else {
                None
            }
        }
    }
}

/// 空命令节点（phony / interface / 空 object）。
fn noop_node(name: &str, kind: &str) -> Node {
    Node {
        id: 0,
        key: format!("phony:{}", name),
        kind: TaskKind::Command,
        inputs: vec![],
        depfile: None,
        output: PathBuf::new(),
        extra_outputs: vec![],
        command: vec![],
        parallel: false,
        label: format!("{kind} {name}"),
        cached: true,
    }
}

/// 把 `Command` 转成 argv（Shell → sh -c / cmd /C）。
fn command_argv(cmd: &crate::script::Command, host: &crate::platform::Host) -> Vec<String> {
    match cmd {
        crate::script::Command::Shell(s) => {
            if host.os.is_windows_family() {
                vec!["cmd".to_string(), "/C".to_string(), s.clone()]
            } else {
                vec!["sh".to_string(), "-c".to_string(), s.clone()]
            }
        }
        crate::script::Command::Run { program, args } => {
            let mut c = vec![program.clone()];
            c.extend(args.iter().cloned());
            c
        }
    }
}

/// 生成自定义命令节点（钩子）。
fn make_command_node(
    target: &str,
    phase: &str,
    cmd: &crate::script::Command,
    host: &crate::platform::Host,
) -> Node {
    let command = command_argv(cmd, host);
    Node {
        id: 0,
        key: format!("hook:{target}:{phase}"),
        kind: TaskKind::Command,
        inputs: vec![],
        depfile: None,
        output: PathBuf::new(),
        extra_outputs: vec![],
        command,
        parallel: false,
        label: format!("HOOK {target} {phase}"),
        cached: false,
    }
}

/// 命令指纹（内容哈希）。
pub fn hash_cmd(cmd: &[String]) -> String {
    let joined = cmd.join("\u{0}");
    blake3::hash(joined.as_bytes()).to_hex().to_string()
}

/// 安全文件名。
pub fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}
/// 计算编译任务的缓存输入：基础输入（源文件）+ depfile 里解析出的头文件（去重）。
/// 改头文件后重跑时 depfile 仍记录旧的头文件集合；头内容变化使哈希比对失败 → 重编。
fn effective_inputs(base: &[PathBuf], depfile: &Path) -> Vec<PathBuf> {
    let mut v = base.to_vec();
    if let Ok(deps) = crate::depfile::parse_depfile(depfile) {
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
    use crate::script::Language;

    #[test]
    fn groups_split() {
        let t = ResolvedTarget {
            name: "t".into(),
            kind: TargetKind::Exe,
            cc: "cc".into(),
            cxx: "c++".into(),
            ar: "ar".into(),
            host: crate::platform::Host::current(),
            sources: vec![
                PathBuf::from("a.c"),
                PathBuf::from("b.c"),
                PathBuf::from("c.c"),
                PathBuf::from("d.c"),
                PathBuf::from("e.c"),
            ],
            include_dirs: vec![],
            flags: vec![],
            cflags: vec![],
            cxxflags: vec![],
            link: vec![],
            link_dirs: vec![],
            link_flags: vec![],
            defines: vec![],
            deps: vec![],
            private_deps: vec![],
            output: PathBuf::from("out"),
            alias: None,
            groups: Some(2),
            group_size: None,
            parallel: true,
            hooks: Default::default(),
            profile: crate::script::Profile::Release,
            strip: false,
            warnings: true,
            werror: false,
            lto: false,
            rpath: vec![],
            soversion: None,
            install: true,
            language: Language::C,
            c_std: None,
            cxx_std: None,
            pkg_config: vec![],
            pkg_config_static: vec![],
            when: None,
            pch: vec![],
        };
        assert_eq!(decide_groups(&t, 5, "auto", 8), Some(2));
    }

    #[test]
    fn group_size_calc() {
        let t = ResolvedTarget {
            name: "t".into(),
            kind: TargetKind::Exe,
            cc: "cc".into(),
            cxx: "c++".into(),
            ar: "ar".into(),
            host: crate::platform::Host::current(),
            sources: vec![],
            include_dirs: vec![],
            flags: vec![],
            cflags: vec![],
            cxxflags: vec![],
            link: vec![],
            link_dirs: vec![],
            link_flags: vec![],
            defines: vec![],
            deps: vec![],
            private_deps: vec![],
            output: PathBuf::from("out"),
            alias: None,
            groups: None,
            group_size: Some(3),
            parallel: true,
            hooks: Default::default(),
            profile: crate::script::Profile::Release,
            strip: false,
            warnings: true,
            werror: false,
            lto: false,
            rpath: vec![],
            soversion: None,
            install: true,
            language: Language::C,
            c_std: None,
            cxx_std: None,
            pkg_config: vec![],
            pkg_config_static: vec![],
            when: None,
            pch: vec![],
        };
        assert_eq!(decide_groups(&t, 7, "auto", 8), Some(3));
    }

    #[test]
    fn unity_modes_auto_on_off() {
        let t = ResolvedTarget {
            name: "t".into(),
            kind: TargetKind::Exe,
            cc: "cc".into(),
            cxx: "c++".into(),
            ar: "ar".into(),
            host: crate::platform::Host::current(),
            sources: vec![],
            include_dirs: vec![],
            flags: vec![],
            cflags: vec![],
            cxxflags: vec![],
            link: vec![],
            link_dirs: vec![],
            link_flags: vec![],
            defines: vec![],
            deps: vec![],
            private_deps: vec![],
            output: PathBuf::from("out"),
            alias: None,
            groups: None,
            group_size: None,
            parallel: true,
            hooks: Default::default(),
            profile: crate::script::Profile::Release,
            strip: false,
            warnings: true,
            werror: false,
            lto: false,
            rpath: vec![],
            soversion: None,
            install: true,
            language: Language::C,
            c_std: None,
            cxx_std: None,
            pkg_config: vec![],
            pkg_config_static: vec![],
            when: None,
            pch: vec![],
        };
        assert_eq!(decide_groups(&t, 300, "auto", 8), Some(8));
        assert_eq!(decide_groups(&t, 300, "off", 8), None);
        assert_eq!(decide_groups(&t, 100, "on", 8), Some(8));
        assert_eq!(decide_groups(&t, 3, "auto", 8), None);
        assert_eq!(decide_groups(&t, 300, "auto", 1), Some(1));
    }
}
