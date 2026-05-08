use egg::*;

// Programs are s-expressions:
define_language! {
    pub enum LoopIR {
        // Arithmetic
        "loop" = Loop([Id; 6]),      // (loop header preheader latch exit phis body)
        "phis" = Phis(Box<[Id]>),
        "phi"  = Phi([Id; 5]),       // (phi var init init_pred step step_pred)

        "seq"  = Seq([Id; 2]),       // (seq loop1 loop2)

        "body"        = Body(Id),
        "fused-body"  = FusedBody([Id; 2]),

        // Leaves
        Num(i64),
        Symbol(egg::Symbol),
    }
}
