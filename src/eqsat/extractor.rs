use std::cmp::Reverse;
use egg::*;
use super::language::LoopIR;

// ---------------------------------------------------------------------------
// Shared cost logic
// ---------------------------------------------------------------------------

/// Loop nodes carry a base overhead of 100 so the extractor naturally prefers
/// fewer loops (fused form) over more (fissioned form).
/// FusedBody is cheaper than Seq to break ties in favour of the fused form.
fn raw_cost<C>(enode: &LoopIR, mut costs: C) -> usize
where
    C: FnMut(Id) -> usize,
{
    let mut sum_children = |base: usize| {
        enode.children().iter().fold(base, |acc, &id| acc.saturating_add(costs(id)))
    };
    match enode {
        LoopIR::Loop(_)      => sum_children(100),
        LoopIR::Seq(_)       => sum_children(5),
        LoopIR::FusedBody(_) => sum_children(3),
        _                    => sum_children(1),
    }
}

// ---------------------------------------------------------------------------
// Best (fusion-preferred) extractor
// ---------------------------------------------------------------------------

pub struct FusionCost;

impl CostFunction<LoopIR> for FusionCost {
    type Cost = usize;
    fn cost<C>(&mut self, enode: &LoopIR, costs: C) -> usize
    where C: FnMut(Id) -> usize
    {
        raw_cost(enode, costs)
    }
}

// ---------------------------------------------------------------------------
// Worst (fission-preferred) extractor
// ---------------------------------------------------------------------------

pub struct WorstFusionCost;

impl CostFunction<LoopIR> for WorstFusionCost {
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

pub fn extract_best(egraph: &EGraph<LoopIR, ()>, root: Id) -> (usize, RecExpr<LoopIR>) {
    Extractor::new(egraph, FusionCost).find_best(root)
}

pub fn extract_worst(egraph: &EGraph<LoopIR, ()>, root: Id) -> (usize, RecExpr<LoopIR>) {
    let (Reverse(cost), expr) = Extractor::new(egraph, WorstFusionCost).find_best(root);
    (cost, expr)
}
