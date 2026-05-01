use egg::*;
use super::language::LoopIR;

/// Full rule set — used when extracting the *best* (LICM-preferred) form.
///
/// Includes identity rules (add-zero, seq-nop, …) which are fine for the
/// minimum extractor but cause the maximum extractor to diverge (see below).
pub fn make_rules() -> Vec<Rewrite<LoopIR, ()>> {
    let mut rules = make_safe_rules();
    rules.extend(make_identity_rules());
    rules
}

/// Rules safe for the *worst* (maximum-cost) extractor.
///
/// Identity rules such as `(+ a 0) => a` merge a longer node into the same
/// e-class as its shorter equivalent.  When the extractor then tries to
/// maximise cost it finds the longer form "worse", raises the class cost,
/// re-evaluates the longer form against the new (higher) child cost, gets a
/// still-worse result, and diverges.
///
/// The rules below are safe because they only rewrite between nodes of the
/// same "depth" (commutativity, associativity, LICM hoisting) — they never
/// merge an expression into the e-class of a strict sub-expression.
pub fn make_safe_rules() -> Vec<Rewrite<LoopIR, ()>> {
    vec![
        // Commutativity
        rewrite!("add-comm"; "(+ ?a ?b)" => "(+ ?b ?a)"),
        rewrite!("mul-comm"; "(* ?a ?b)" => "(* ?b ?a)"),

        // Associativity
        rewrite!("add-assoc";   "(+ ?a (+ ?b ?c))"    => "(+ (+ ?a ?b) ?c)"),
        rewrite!("mul-assoc";   "(* ?a (* ?b ?c))"    => "(* (* ?a ?b) ?c)"),
        rewrite!("seq-assoc";   "(seq (seq ?a ?b) ?c)" => "(seq ?a (seq ?b ?c))"),
        rewrite!("seq-assoc-r"; "(seq ?a (seq ?b ?c))" => "(seq (seq ?a ?b) ?c)"),

        // LICM — commented out; back to loop fusion strategy.
        // rewrite!("licm-hoist-head";
        //     "(loop ?s ?e ?st (seq ?inv ?body))" =>
        //     "(seq ?inv (loop ?s ?e ?st ?body))"
        // ),
        // rewrite!("licm-hoist-tail";
        //     "(loop ?s ?e ?st (seq ?body ?inv))" =>
        //     "(seq (loop ?s ?e ?st ?body) ?inv)"
        // ),

        // Loop fusion / fission (bidirectional).
        rewrite!("loop-fusion";
            "(seq (loop ?s ?e ?st ?f) (loop ?s ?e ?st ?g))" =>
            "(loop ?s ?e ?st (seq ?f ?g))"
        ),
        rewrite!("loop-fission";
            "(loop ?s ?e ?st (seq ?f ?g))" =>
            "(seq (loop ?s ?e ?st ?f) (loop ?s ?e ?st ?g))"
        ),

        // Dead-loop elimination is safe: nop is its own isolated e-class and
        // loop's e-class never collapses into nop's e-class through this rule.
        rewrite!("loop-nop"; "(loop ?s ?e ?st nop)" => "nop"),
    ]
}

/// Identity / simplification rules — unsafe for the worst extractor.
///
/// Each of these collapses a larger expression into the e-class of a smaller
/// one (e.g. `(+ a 0)` → same class as `a`).  When the worst extractor then
/// assigns costs, it finds the larger form "worse", updates the class cost,
/// and diverges because the larger form references the same (now more
/// expensive) class.
fn make_identity_rules() -> Vec<Rewrite<LoopIR, ()>> {
    vec![
        rewrite!("add-zero-r"; "(+ ?a 0)"  => "?a"),
        rewrite!("add-zero-l"; "(+ 0 ?a)"  => "?a"),
        rewrite!("sub-zero";   "(- ?a 0)"  => "?a"),
        rewrite!("sub-self";   "(- ?a ?a)" => "0"),
        rewrite!("mul-one-r";  "(* ?a 1)"  => "?a"),
        rewrite!("mul-one-l";  "(* 1 ?a)"  => "?a"),
        rewrite!("mul-zero-r"; "(* ?a 0)"  => "0"),
        rewrite!("mul-zero-l"; "(* 0 ?a)"  => "0"),
        rewrite!("seq-nop-l";  "(seq nop ?a)" => "?a"),
        rewrite!("seq-nop-r";  "(seq ?a nop)" => "?a"),
    ]
}
