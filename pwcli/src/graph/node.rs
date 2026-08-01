/// 节点标识。不是 trait object——节点行为由 executor 硬编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeId {
    Agent,
    Decision,
    Tool,
    End,
}

/// 节点执行后的路由决策
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    Goto(NodeId),
    End,
}
