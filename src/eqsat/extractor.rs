use std::cmp::Reverse;
use egg::*;
use super::language::LoopIR;

// ---------------------------------------------------------------------------
// Shared cost logic
// ---------------------------------------------------------------------------

/// Compute the raw LICM cost for one e-node.
///
/// The loop body cost is multiplied by 2 so that a loop with a complex body
/// (e.g. a `seq` inside) is more expensive than one whose `seq` has been
/// hoisted out by LICM.
fn raw_cost<C>(enode: &LoopIR, mut costs: C) -> usize
where
    C: FnMut(Id) -> usize,
{
    match enode {
        LoopIR::Loop([s, e, st, body]) => 100_usize
            .saturating_add(costs(*s))
            .saturating_add(costs(*e))
            .saturating_add(costs(*st))
            .saturating_add(costs(*body).saturating_mul(2)),
        LoopIR::Seq(_) => enode.children().iter().fold(2, |acc, &id| acc.saturating_add(costs(id))),
        LoopIR::Nop    => 0,
        _              => enode.children().iter().fold(1, |acc, &id| acc.saturating_add(costs(id))),
    }
}

// ---------------------------------------------------------------------------
// Best (LICM-preferred) extractor
// ---------------------------------------------------------------------------

/// Minimises cost → prefers the LICM-hoisted form (simpler loop body).
pub struct LicmCost;

impl CostFunction<LoopIR> for LicmCost {
    type Cost = usize;
    fn cost<C>(&mut self, enode: &LoopIR, costs: C) -> usize
    where C: FnMut(Id) -> usize
    {
        raw_cost(enode, costs)
    }
}

// ---------------------------------------------------------------------------
// Worst (most complex) extractor
// ---------------------------------------------------------------------------

/// Maximises cost → prefers the least-optimised form (complex loop body).
///
/// Implemented by wrapping the raw cost in `Reverse` so that egg's
/// built-in minimiser effectively finds the maximum.
pub struct WorstLicmCost;

impl CostFunction<LoopIR> for WorstLicmCost {
    type Cost = Reverse<usize>;
    fn cost<C>(&mut self, enode: &LoopIR, mut costs: C) -> Reverse<usize>
    where C: FnMut(Id) -> Reverse<usize>
    {
        Reverse(raw_cost(enode, |id| costs(id).0))
    }
}

// ---------------------------------------------------------------------------
// Public extraction helpers
// ---------------------------------------------------------------------------

/// Extract the LICM-preferred (lowest-cost, hoisted) expression.
pub fn extract_best(egraph: &EGraph<LoopIR, ()>, root: Id) -> (usize, RecExpr<LoopIR>) {
    Extractor::new(egraph, LicmCost).find_best(root)
}

/// Extract the worst-cost (most complex, non-hoisted) expression.
pub fn extract_worst(egraph: &EGraph<LoopIR, ()>, root: Id) -> (usize, RecExpr<LoopIR>) {
    let (Reverse(cost), expr) = Extractor::new(egraph, WorstLicmCost).find_best(root);
    (cost, expr)
}

#[allow(dead_code)]
/// Extract the smallest AST (standard `AstSize` metric) from the e-graph.
pub fn extract_smallest(egraph: &EGraph<LoopIR, ()>, root: Id) -> (usize, RecExpr<LoopIR>) {
    Extractor::new(egraph, AstSize).find_best(root)
}
