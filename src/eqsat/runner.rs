use egg::*;
use super::language::LoopIR;
use super::rules::{make_rules, make_safe_rules};

const DEFAULT_ITER_LIMIT: usize = 10;
const DEFAULT_NODE_LIMIT: usize = 10_000;

/// Wraps an egg `Runner` result after equality saturation completes.
pub struct EqsatResult {
    pub egraph: EGraph<LoopIR, ()>,
    /// Root e-class ID for each expression passed to the runner.
    pub roots: Vec<Id>,
    #[allow(dead_code)]
    pub stop_reason: StopReason,
}

impl EqsatResult {
    /// Run equality saturation with the full rule set (best extraction).
    pub fn run(expr: &RecExpr<LoopIR>) -> Self {
        Self::run_inner(expr, make_rules(), DEFAULT_ITER_LIMIT, DEFAULT_NODE_LIMIT)
    }

    /// Run equality saturation with only the safe rules (worst extraction).
    ///
    /// Identity rules are omitted to prevent the maximum-cost extractor from
    /// diverging on self-referential e-classes.
    pub fn run_for_worst(expr: &RecExpr<LoopIR>) -> Self {
        Self::run_inner(expr, make_safe_rules(), DEFAULT_ITER_LIMIT, DEFAULT_NODE_LIMIT)
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
