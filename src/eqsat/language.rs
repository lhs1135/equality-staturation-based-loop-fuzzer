use egg::*;

// Programs are s-expressions:
//   (loop start end step body)   — a counted loop
//   (seq a b)                    — sequential composition
//   (load ptr idx)               — memory read
//   (store ptr idx val)          — memory write
//   (if cond then else)          — conditional
//   nop                          — no-operation (identity for seq)
//   Num(i64)                     — integer literal
//   Var(Symbol)                  — named variable / register
define_language! {
    pub enum LoopIR {
        // Arithmetic
        "+"  = Add([Id; 2]),
        "-"  = Sub([Id; 2]),
        "*"  = Mul([Id; 2]),
        "/"  = Div([Id; 2]),

        // Comparison
        "<"  = Lt([Id; 2]),
        "<=" = Le([Id; 2]),
        ">"  = Gt([Id; 2]),
        ">=" = Ge([Id; 2]),
        "==" = Eq([Id; 2]),
        "!=" = Ne([Id; 2]),

        // Boolean
        "and" = And([Id; 2]),
        "or"  = Or([Id; 2]),
        "not" = Not([Id; 1]),

        // Control flow: (if cond then else)
        "if"   = If([Id; 3]),

        // Loop: (loop start end step body)
        "loop" = Loop([Id; 4]),

        // Sequential composition: (seq a b)
        "seq" = Seq([Id; 2]),

        // No-op — identity element for seq
        "nop" = Nop,

        // Leaves
        Num(i64),
        Var(egg::Symbol),
    }
}
