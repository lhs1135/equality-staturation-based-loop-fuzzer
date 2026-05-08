use egg::*;
use super::language::LoopIR;

/// Loop-fusion rule.
///
/// Fires when two loops in sequence are *adjacent* — the exit of the first loop
/// (`?mid`) equals the preheader of the second loop (`?mid`).
/// The fused loop keeps loop-1's structural fields (header, preheader, phis)
/// and loop-2's latch and exit, combining the bodies into a `fused-body`.
///
/// Alive2 is the semantic oracle: it verifies that the fused IR is equivalent
/// to the two-loop IR before the transformation fires.
pub fn make_rules() -> Vec<Rewrite<LoopIR, ()>> {
    vec![
        rewrite!("loop-fusion";
            "(seq
               (loop ?h1 ?pre1 ?lat1 ?mid  ?phis1 ?body1)
               (loop ?h2 ?mid  ?lat2 ?exit ?phis2 ?body2))"
            =>
            "(loop ?h1 ?pre1 ?lat2 ?exit ?phis1 (fused-body ?body1 ?body2))"
        ),
    ]
}
