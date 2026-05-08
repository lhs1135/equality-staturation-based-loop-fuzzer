use egg::*;
use super::language::LoopIR;
use super::rules::make_rules;

const DEFAULT_ITER_LIMIT: usize = 10;
const DEFAULT_NODE_LIMIT: usize = 10_000;

pub struct EqsatResult {
    pub egraph: EGraph<LoopIR, ()>,
    pub roots: Vec<Id>,
    #[allow(dead_code)]
    pub stop_reason: StopReason,
}

impl EqsatResult {
    pub fn run(expr: &RecExpr<LoopIR>) -> Self {
        Self::run_inner(expr, make_rules(), DEFAULT_ITER_LIMIT, DEFAULT_NODE_LIMIT)
    }

    fn run_inner(
        expr: &RecExpr<LoopIR>,
        rules: Vec<egg::Rewrite<LoopIR, ()>>,
        iter_limit: usize,
        node_limit: usize,
    ) -> Self {
        let runner = Runner::default()
            .with_expr(expr)
            .with_iter_limit(iter_limit)
            .with_node_limit(node_limit)
            .run(&rules);

        Self {
            roots: runner.roots.clone(),
            stop_reason: runner.stop_reason.clone().unwrap_or(StopReason::Saturated),
            egraph: runner.egraph,
        }
    }
}
