//! 构建图：任务节点与依赖边（DAG），供 planner 构造、executor 调度。

use std::collections::{BTreeMap, BTreeSet};

/// 节点 ID。
pub type NodeId = usize;

/// 任务种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// 编译任务（单源 / unity）。
    Compile,
    /// 链接 / 归档。
    Link,
    /// 自定义命令。
    Command,
}

/// 图中的任务节点。
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub key: String,
    pub kind: TaskKind,
    /// 输入（源文件、对象等）。
    pub inputs: Vec<std::path::PathBuf>,
    /// 依赖文件（仅编译）。
    pub depfile: Option<std::path::PathBuf>,
    /// 产物。
    pub output: std::path::PathBuf,
    /// 完整外部命令。
    pub command: Vec<String>,
    /// 允许并行。
    pub parallel: bool,
    pub label: String,
    /// 是否已由缓存命中（跳过执行）。
    pub cached: bool,
}

/// 构建图：节点 + 依赖边。
#[derive(Debug, Default, Clone)]
pub struct BuildGraph {
    nodes: Vec<Node>,
    /// node → 依赖节点集合。
    deps: Vec<BTreeSet<NodeId>>,
    /// node → 依赖它的节点集合（反向）。
    rdeps: Vec<BTreeSet<NodeId>>,
    /// 目标产物 → 节点（用于查找）。
    by_output: BTreeMap<std::path::PathBuf, NodeId>,
}

impl BuildGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加节点，返回 ID。
    pub fn add_node(&mut self, node: Node) -> NodeId {
        let id = self.nodes.len();
        self.deps.push(BTreeSet::new());
        self.rdeps.push(BTreeSet::new());
        if !node.output.as_os_str().is_empty() {
            self.by_output.insert(node.output.clone(), id);
        }
        self.nodes.push(node);
        id
    }

    /// 添加依赖边：`dep` 必须先于 `node`。
    pub fn add_dep(&mut self, node: NodeId, dep: NodeId) {
        self.deps[node].insert(dep);
        self.rdeps[dep].insert(node);
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn deps_of(&self, id: NodeId) -> &BTreeSet<NodeId> {
        &self.deps[id]
    }

    pub fn rdeps_of(&self, id: NodeId) -> &BTreeSet<NodeId> {
        &self.rdeps[id]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// 按产物查找节点。
    pub fn node_for_output(&self, out: &std::path::Path) -> Option<NodeId> {
        self.by_output.get(out).copied()
    }

    /// 拓扑排序（Kahn）。检测环。
    pub fn topo_order(&self) -> Result<Vec<NodeId>, String> {
        let n = self.len();
        let mut indeg = vec![0usize; n];
        for i in 0..n {
            indeg[i] = self.deps[i].len();
        }
        let mut queue: std::collections::VecDeque<NodeId> = (0..n)
            .filter(|&i| indeg[i] == 0)
            .collect();
        let mut order = Vec::with_capacity(n);
        while let Some(u) = queue.pop_front() {
            order.push(u);
            for &v in &self.rdeps[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    queue.push_back(v);
                }
            }
        }
        if order.len() != n {
            return Err(format!("构建图存在环: {} 个节点未排入拓扑序", n - order.len()));
        }
        Ok(order)
    }

    /// 验证图一致性。
    pub fn validate(&self) -> Result<(), String> {
        self.topo_order()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(key: &str) -> Node {
        Node {
            id: 0,
            key: key.into(),
            kind: TaskKind::Command,
            inputs: vec![],
            depfile: None,
            output: std::path::PathBuf::new(),
            command: vec![],
            parallel: true,
            label: key.into(),
            cached: false,
        }
    }

    #[test]
    fn topo_respects_deps() {
        let mut g = BuildGraph::new();
        let a = g.add_node(node("a"));
        let b = g.add_node(node("b"));
        let c = g.add_node(node("c"));
        g.add_dep(c, a);
        g.add_dep(c, b);
        let order = g.topo_order().unwrap();
        let pos = |x: NodeId| order.iter().position(|&i| i == x).unwrap();
        assert!(pos(a) < pos(c));
        assert!(pos(b) < pos(c));
    }

    #[test]
    fn detects_cycle() {
        let mut g = BuildGraph::new();
        let a = g.add_node(node("a"));
        let b = g.add_node(node("b"));
        g.add_dep(b, a);
        g.add_dep(a, b);
        assert!(g.topo_order().is_err());
    }
}
