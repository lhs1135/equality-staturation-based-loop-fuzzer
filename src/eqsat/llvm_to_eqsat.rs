use super::language::LoopIR;
use egg::{Id, RecExpr};
use std::collections::HashMap;

/// Convert a normalized LLVM IR loop to a LoopIR expression.
///
/// `header` and `latch` identify the back-edge that was injected.  After
/// `mem2reg + loop-simplify + lcssa` the structure is canonical:
///   - header   has all phi nodes for the loop's induction variables
///   - latch    has the conditional back-edge to header
///   - preheader is the unique non-latch predecessor of header (guaranteed by loop-simplify)
///   - exit     is the unique non-header successor of latch
///
/// The body is represented as an opaque `(body <header-symbol>)` — the header
/// label uniquely identifies the loop.  During back-conversion (step 7) the
/// tracked block labels let us reconstruct the full IR.
pub fn ir_to_loopir(ir: &str, header: &str, latch: &str) -> Option<RecExpr<LoopIR>> {
    let lines: Vec<&str> = ir.lines().collect();
    let (blocks, pred_map) = parse_ir_structure(&lines);

    // Preheader: non-latch predecessor of header (loop-simplify guarantees exactly one).
    let preheader = pred_map.get(header)?
        .iter()
        .find(|p| p.as_str() != latch)?
        .clone();

    // Exit: non-header successor of latch.
    let latch_blk = blocks.iter().find(|b| b.label == latch)?;
    let exit = latch_blk.successors.iter()
        .find(|s| s.as_str() != header)?
        .clone();

    let phis = collect_header_phis(&lines, header, &preheader, latch);
    Some(build_loopir(header, &preheader, latch, &exit, &phis))
}

// ---------------------------------------------------------------------------
// IR structure parsing
// ---------------------------------------------------------------------------

struct SimpleBlock {
    label: String,
    successors: Vec<String>,
}

fn parse_ir_structure(lines: &[&str]) -> (Vec<SimpleBlock>, HashMap<String, Vec<String>>) {
    let mut blocks: Vec<SimpleBlock> = Vec::new();
    let mut in_fn = false;

    for &line in lines {
        let tr = line.trim();
        if !in_fn {
            if tr.starts_with("define ") { in_fn = true; }
            continue;
        }
        if tr == "}" { in_fn = false; continue; }

        if let Some(label) = block_label(line) {
            blocks.push(SimpleBlock { label: label.to_string(), successors: Vec::new() });
        } else if tr.starts_with("br ") {
            if let Some(blk) = blocks.last_mut() {
                blk.successors = br_targets(tr);
            }
        }
    }

    let mut pred_map: HashMap<String, Vec<String>> = HashMap::new();
    for blk in &blocks {
        for s in &blk.successors {
            pred_map.entry(s.clone()).or_default().push(blk.label.clone());
        }
    }

    (blocks, pred_map)
}

fn block_label(line: &str) -> Option<&str> {
    if line.starts_with(' ') || line.starts_with('\t') { return None; }
    let tr = line.trim();
    if tr.is_empty() || tr.starts_with(';') || tr.starts_with('{') { return None; }
    let colon = tr.find(':')?;
    let label = &tr[..colon];
    let after = tr[colon + 1..].trim();
    (label.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && (after.is_empty() || after.starts_with(';')))
    .then_some(label)
}

fn br_targets(line: &str) -> Vec<String> {
    let needle = "label %";
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = line[pos..].find(needle) {
        let start = pos + rel + needle.len();
        let end = line[start..]
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.' && c != '-')
            .map(|j| start + j)
            .unwrap_or(line.len());
        if end > start { out.push(line[start..end].to_string()); }
        pos = start;
    }
    out
}

// ---------------------------------------------------------------------------
// Phi parsing
// ---------------------------------------------------------------------------

struct PhiInfo {
    var: String,
    init: String,  // value incoming from preheader
    step: String,  // value incoming from latch
}

fn collect_header_phis(
    lines: &[&str],
    header: &str,
    preheader: &str,
    latch: &str,
) -> Vec<PhiInfo> {
    let mut phis = Vec::new();
    let mut in_hdr = false;

    for &line in lines {
        if let Some(lbl) = block_label(line) {
            in_hdr = lbl == header;
            continue;
        }
        if !in_hdr { continue; }
        let tr = line.trim();
        if tr.is_empty() || tr.starts_with(';') { continue; }

        if tr.contains("= phi ") {
            if let Some(phi) = parse_phi(tr, preheader, latch) {
                phis.push(phi);
            }
        } else {
            break; // phi nodes always lead the block
        }
    }
    phis
}

fn parse_phi(line: &str, preheader: &str, latch: &str) -> Option<PhiInfo> {
    let phi_off = line.find("= phi ")?;
    let var = line[..phi_off].trim().trim_start_matches('%').to_string();
    let rest = &line[phi_off + "= phi ".len()..];
    let arms = phi_arms(rest)?;

    let init = arms.iter().find(|(_, p)| p == preheader)
        .map(|(v, _)| v.clone()).unwrap_or_else(|| "undef".into());
    let step = arms.iter().find(|(_, p)| p == latch)
        .map(|(v, _)| v.clone()).unwrap_or_else(|| "undef".into());

    Some(PhiInfo { var, init, step })
}

/// Extract `(value, pred)` pairs from the portion of a phi line after the type.
///
/// Input:  `i32 [ 0, %pre1 ], [ %i_next, %body1 ]`
/// Output: `[("0", "pre1"), ("i_next", "body1")]`
fn phi_arms(s: &str) -> Option<Vec<(String, String)>> {
    let mut arms = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let end = i + 1 + s[i + 1..].find(']')?;
            let inner = s[i + 1..end].trim();
            let comma = inner.find(',')?;
            let val  = inner[..comma].trim().trim_start_matches('%').to_string();
            let pred = inner[comma + 1..].trim().trim_start_matches('%').to_string();
            arms.push((val, pred));
            i = end + 1;
        } else {
            i += 1;
        }
    }
    if arms.is_empty() { None } else { Some(arms) }
}

// ---------------------------------------------------------------------------
// RecExpr construction
// ---------------------------------------------------------------------------

fn build_loopir(
    header: &str,
    preheader: &str,
    latch: &str,
    exit: &str,
    phis: &[PhiInfo],
) -> RecExpr<LoopIR> {
    let mut expr: RecExpr<LoopIR> = RecExpr::default();

    let id_hdr  = expr.add(LoopIR::Symbol(header.into()));
    let id_pre  = expr.add(LoopIR::Symbol(preheader.into()));
    let id_lat  = expr.add(LoopIR::Symbol(latch.into()));
    let id_exit = expr.add(LoopIR::Symbol(exit.into()));

    let mut phi_ids: Vec<Id> = Vec::new();
    for phi in phis {
        let id_var  = expr.add(LoopIR::Symbol(phi.var.as_str().into()));
        let id_init = val_id(&mut expr, &phi.init);
        let id_ipre = expr.add(LoopIR::Symbol(preheader.into()));
        let id_step = val_id(&mut expr, &phi.step);
        let id_slat = expr.add(LoopIR::Symbol(latch.into()));
        phi_ids.push(expr.add(LoopIR::Phi([id_var, id_init, id_ipre, id_step, id_slat])));
    }
    let id_phis = expr.add(LoopIR::Phis(phi_ids.into_boxed_slice()));

    // Body: opaque placeholder identified by the header label.
    // Each loop gets a distinct symbol so pattern variables in the fusion rule
    // can distinguish loop1's body from loop2's body.
    let id_bsym = expr.add(LoopIR::Symbol(header.into()));
    let id_body = expr.add(LoopIR::Body(id_bsym));

    expr.add(LoopIR::Loop([id_hdr, id_pre, id_lat, id_exit, id_phis, id_body]));
    expr
}

fn val_id(expr: &mut RecExpr<LoopIR>, val: &str) -> Id {
    let v = val.trim();
    if v.starts_with('%') {
        expr.add(LoopIR::Symbol(v.trim_start_matches('%').into()))
    } else if let Ok(n) = v.parse::<i64>() {
        expr.add(LoopIR::Num(n))
    } else {
        expr.add(LoopIR::Symbol(v.into()))
    }
}
