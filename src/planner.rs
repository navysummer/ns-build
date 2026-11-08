//! 规划器：将解析后的脚本目标转换成任务 DAG。
//!
//! 支持：
//! - 分步构建（`kind = "object"`）：只产出 `.o`，供其他 target 链接；
//! - unity build（`groups` / `group_size`）：多源文件合并编译；
//! - 增量构建：用缓存指纹判断哪些任务可跳过。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::compiler::{
    CompileKind, compile_command, is_cxx, make_link_task, unity_compile_command,
    unity_source_content,
};
use crate::graph::{BuildGraph, Node, NodeId, TaskKind};
use crate::script::{ResolvedScript, ResolvedTarget, TargetKind};

/// 规划结果。
pub struct Plan {
    pub graph: BuildGraph,
    /// 每个 target 名 → 其产物节点（最终节点）。
    pub outputs: BTreeMap<String, NodeId>,
    /// 跳过（缓存命中）的任务数。
    pub skipped: usize,
}

/// 从解析后的脚本规划 DAG。
pub fn plan(script: &ResolvedScript, cache: &Cache) -> Result<Plan, String> {
    let mut graph = BuildGraph::new();
    let mut outputs: BTreeMap<String, NodeId> = BTreeMap::new();
    let mut skipped = 0usize;

    let host = script.host;
    let out_dir = &script.out_dir;
    let obj_dir = out_dir.join("obj");

    // 第一遍：为每个 target 生成对象编译节点（含 unity）。
    let mut obj_nodes: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for t in &script.targets {
        if t.kind == TargetKind::Phony {
            continue;
        }
        let objs = plan_object_nodes(t, &obj_dir, &out_dir, &host, cache, &mut skipped)?;
        let mut ids = Vec::new();
        for n in objs {
            let id = graph.add_node(n);
            ids.push(id);
        }
        obj_nodes.insert(t.name.clone(), ids);
    }

    // 第二遍：构造聚合/链接节点，并处理 deps。
    for t in &script.targets {
        match t.kind {
            TargetKind::Phony => {
                let nid = graph.add_node(Node {
                    id: 0,
                    key: format!("phony:{}", t.name),
                    kind: TaskKind::Command,
                    inputs: vec![],
                    depfile: None,
                    output: PathBuf::new(),
                    command: vec![],
                    parallel: false,
                    label: format!("PHONY {}", t.name),
                    cached: false,
                });
                outputs.insert(t.name.clone(), nid);
            }
            TargetKind::Object => {
                let last = *obj_nodes.get(&t.name).unwrap().last().unwrap();
                outputs.insert(t.name.clone(), last);
            }
            _ => {
                let obj_ids = obj_nodes.get(&t.name).cloned().unwrap_or_default();
                // 收集自身对象 + deps 的对象（作为链接输入）。
                let mut all_obj_ids = obj_ids.clone();
                let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
                seen.insert(t.name.clone());
                for dep in &t.deps {
                    if let Some(dep_ids) = obj_nodes.get(dep) {
                        for &d in dep_ids {
                            if !all_obj_ids.contains(&d) {
                                all_obj_ids.push(d);
                            }
                        }
                    }
                    seen.insert(dep.clone());
                }
                let obj_paths: Vec<PathBuf> = all_obj_ids
                    .iter()
                    .map(|&id| graph.node(id).output.clone())
                    .collect();
                let ar = script.script.defaults.ar.clone();
                let link_task = make_link_task(t, obj_paths, host, t.profile, &ar);
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
                    command: link_task.command.clone(),
                    parallel: t.parallel,
                    label: link_task.label.clone(),
                    cached,
                };
                let link_id = graph.add_node(link_node);
                for &o in &all_obj_ids {
                    graph.add_dep(link_id, o);
                }
                outputs.insert(t.name.clone(), link_id);
            }
        }
    }

    // 第三遍：target 间 deps 边 + 钩子。
    for t in &script.targets {
        let Some(&me) = outputs.get(&t.name) else {
            continue;
        };
        for dep in &t.deps {
            let Some(&dn) = outputs.get(dep) else {
                return Err(format!("target '{}' 依赖不存在的 target '{}'", t.name, dep));
            };
            if dn != me {
                graph.add_dep(me, dn);
            }
        }
        if !t.hooks.before.is_empty() {
            for cmd in &t.hooks.before {
                let cid = make_command_node(&t.name, "before", cmd);
                let cid = graph.add_node(cid);
                graph.add_dep(me, cid);
            }
        }
        if !t.hooks.after.is_empty() {
            for cmd in &t.hooks.after {
                let cid = make_command_node(&t.name, "after", cmd);
                let cid = graph.add_node(cid);
                graph.add_dep(cid, me);
            }
        }
    }

    graph.validate()?;
    Ok(Plan { graph, outputs, skipped })
}

/// 为可链接/分步 target 规划对象编译节点。
fn plan_object_nodes(
    t: &ResolvedTarget,
    obj_dir: &Path,
    out_dir: &Path,
    host: &crate::platform::Host,
    cache: &Cache,
    skipped: &mut usize,
) -> Result<Vec<Node>, String> {
    let mut nodes = Vec::new();
    if t.sources.is_empty() {
        // 纯依赖 target（如只链接依赖的对象）。
        return Ok(nodes);
    }
    // 分组的决定。
    let n_src = t.sources.len();
    let groups = decide_groups(t, n_src);
    let use_unity = groups > 1 || (groups == 1 && n_src > 1);

    if use_unity {
        // 将源文件均分为 groups 组，每组一个 .o。
        let per = (n_src + groups - 1) / groups;
        for (gi, chunk) in t.sources.chunks(per).enumerate() {
            let obj = obj_dir.join(format!("{}_{}.o", safe_name(&t.name), gi));
            let depfile = obj_dir.join(format!("{}_{}.d", safe_name(&t.name), gi));
            let unity_dir = out_dir.join("unity");
            let unity_src = unity_dir.join(format!("{}_{}.c", safe_name(&t.name), gi));
            let _ = std::fs::create_dir_all(&unity_dir);
            let _ = std::fs::write(&unity_src, unity_source_content(chunk));
            let cmd = unity_compile_command(&t.cc, chunk, &unity_src, &obj, &depfile, t, t.profile);
            let key = format!("unity:{}:{}", t.name, gi);
            let cmd_hash = hash_cmd(&cmd);
            let cached = cache.hit(&key, chunk, &cmd_hash);
            if cached {
                *skipped += 1;
            }
            let mut inputs: Vec<PathBuf> = chunk.to_vec();
            if !cached {
                inputs = chunk.to_vec();
            } else if let Ok(deps) = crate::depfile::parse_depfile(&depfile) {
                inputs.extend(deps);
            }
            nodes.push(Node {
                id: 0,
                key,
                kind: TaskKind::Compile,
                inputs,
                depfile: Some(depfile),
                output: obj,
                command: cmd,
                parallel: t.parallel,
                label: format!("CC {} [{}]", t.name, chunk.len()),
                cached,
            });
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
            let cmd = compile_command(
                &t.cc,
                src,
                &obj,
                &depfile,
                t,
                *host,
                t.profile,
                is_cxx_src,
            );
            let key = format!("obj:{}:{}", t.name, i);
            let cmd_hash = hash_cmd(&cmd);
            let cached = cache.hit(&key, &[src.clone()], &cmd_hash);
            if cached {
                *skipped += 1;
            }
            let mut inputs = vec![src.clone()];
            if cached {
                if let Ok(deps) = crate::depfile::parse_depfile(&depfile) {
                    inputs = deps.into_iter().collect();
                }
            }
            nodes.push(Node {
                id: 0,
                key,
                kind: TaskKind::Compile,
                inputs,
                depfile: Some(depfile),
                output: obj,
                command: cmd,
                parallel: t.parallel,
                label: format!("CC {} ({})", t.name, src.file_name().and_then(|s| s.to_str()).unwrap_or("")),
                cached,
            });
        }
    }
    Ok(nodes)
}

/// 决定分组数。
fn decide_groups(t: &ResolvedTarget, n_src: usize) -> usize {
    if n_src == 0 {
        return 1;
    }
    if let Some(g) = t.groups {
        return g.max(1).min(n_src);
    }
    if let Some(gs) = t.group_size {
        let g = (n_src + gs - 1) / gs;
        return g.max(1);
    }
    // 默认：unity auto 规则（≥4 个源自动合并为 2 组）。
    if n_src >= 4 {
        2
    } else {
        1
    }
}

/// 生成自定义命令节点（钩子）。
fn make_command_node(target: &str, phase: &str, cmd: &crate::script::Command) -> Node {
    let (program, args): (String, Vec<String>) = match cmd {
        crate::script::Command::Shell(s) => {
            // 简单拆分为 program + args（空格）。
            let mut parts = s.split_whitespace();
            let p = parts.next().unwrap_or("").to_string();
            (p, parts.map(|x| x.to_string()).collect())
        }
        crate::script::Command::Run { program, args } => (program.clone(), args.clone()),
    };
    let mut command = vec![program];
    command.extend(args);
    Node {
        id: 0,
        key: format!("hook:{target}:{phase}"),
        kind: TaskKind::Command,
        inputs: vec![],
        depfile: None,
        output: PathBuf::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_split() {
        let t = ResolvedTarget {
            name: "t".into(),
            kind: TargetKind::Exe,
            cc: "cc".into(),
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
            output: PathBuf::from("out"),
            groups: Some(2),
            group_size: None,
            parallel: true,
            hooks: Default::default(),
            profile: crate::script::Profile::Release,
            strip: false,
            install: true,
        };
        assert_eq!(decide_groups(&t, 5), 2);
    }

    #[test]
    fn group_size_calc() {
        let t = ResolvedTarget {
            name: "t".into(),
            kind: TargetKind::Exe,
            cc: "cc".into(),
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
            output: PathBuf::from("out"),
            groups: None,
            group_size: Some(3),
            parallel: true,
            hooks: Default::default(),
            profile: crate::script::Profile::Release,
            strip: false,
            install: true,
        };
        assert_eq!(decide_groups(&t, 7), 3);
    }
}
