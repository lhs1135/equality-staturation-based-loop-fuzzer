pub mod extractor;
pub mod language;
pub mod rules;
pub mod runner;
pub mod verify;

pub use extractor::{extract_best, extract_worst};
pub use language::LoopIR;
pub use runner::EqsatResult;

use egg::RecExpr;

/// Run equality saturation with the full rule set and return the
/// LICM-preferred (lowest-cost, hoisted) form.
pub fn optimize(expr_str: &str) -> Result<(usize, String), String> {
    let expr: RecExpr<LoopIR> = expr_str.parse().map_err(|e| format!("{e}"))?;
    let result = EqsatResult::run(&expr);
    let root = result.roots[0];
    let (cost, best) = extract_best(&result.egraph, root);
    Ok((cost, best.to_string()))
}

/// Run equality saturation with safe rules only and return the
/// worst-cost (most complex, non-hoisted) form.
///
/// Safe rules exclude identity rewrites (add-zero, seq-nop, …) that create
/// self-referential e-classes and cause the maximum extractor to diverge.
pub fn optimize_worst(expr_str: &str) -> Result<(usize, String), String> {
    let expr: RecExpr<LoopIR> = expr_str.parse().map_err(|e| format!("{e}"))?;
    let result = EqsatResult::run_for_worst(&expr);
    let root = result.roots[0];
    let (cost, worst) = extract_worst(&result.egraph, root);
    Ok((cost, worst.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best(s: &str)  -> (usize, String) { optimize(s).expect("parse/run failed") }
    fn worst(s: &str) -> (usize, String) { optimize_worst(s).expect("parse/run failed") }

    // --- best extractor (LICM-preferred, hoisted form) ---

    #[test]
    fn test_licm_best_hoists() {
        let (cost, after) = best("(loop 0 10 1 (seq K body))");
        println!("licm-best: {after}  cost={cost}");
        assert!(after.starts_with("(seq K"), "expected K hoisted; got: {after}");
    }

    #[test]
    fn test_arithmetic_simplification() {
        let (_, r) = best("(+ x 0)");  assert_eq!(r, "x");
        let (_, r) = best("(* y 1)");  assert_eq!(r, "y");
        let (_, r) = best("(- z z)");  assert_eq!(r, "0");
    }

    #[test]
    fn test_seq_nop() {
        let (_, r) = best("(seq nop (+ i 1))");
        assert_eq!(r, "(+ i 1)");
    }

    #[test]
    fn test_loop_nop() {
        let (_, r) = best("(loop 0 10 1 nop)");
        assert_eq!(r, "nop");
    }

    // --- worst extractor (non-hoisted, complex form) ---

    #[test]
    fn test_licm_worst_keeps_seq_in_body() {
        // Worst extractor prefers seq inside the loop (higher body cost).
        let (cost, after) = worst("(loop 0 10 1 (seq K body))");
        println!("licm-worst: {after}  cost={cost}");
        assert!(after.starts_with("(loop"), "expected non-hoisted loop; got: {after}");
    }

    #[test]
    fn test_licm_exposes_adjacent_loops() {
        // Both loops must be present regardless of extraction direction.
        let expr = "(seq (loop 0 N 1 (seq body1 K)) (loop 0 N 1 (seq K body2)))";
        let (_, after_w) = worst(expr);
        println!("licm-adjacent worst: {after_w}");
        assert_eq!(after_w.matches("loop").count(), 2, "expected 2 loops; got: {after_w}");
    }
}
