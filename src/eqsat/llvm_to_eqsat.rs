use super::language::LoopIR;
use egg::{Id, RecExpr};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct LoopMeta {
    pub header:    String,
    pub preheader: String,
    pub latch:     String,
    pub exit:      String,
    /// (label, raw_lines) for each block in the loop, in IR text order:
    /// [header_block, ...body_blocks..., latch_block].
    /// Each inner Vec includes the label line and all instruction lines.
    pub loop_blocks: Vec<(String, Vec<String>)>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Convert one loop in `ir` to a LoopIR expression, collecting all raw block
/// data into a `LoopMeta` in the same single pass.
pub fn ir_to_loopir(ir: &str, header: &str, latch: &str) -> Option<(RecExpr<LoopIR>, LoopMeta)> {
    let lines: Vec<&str> = ir.lines().collect();
    let (blocks, pred_map) = parse_ir_structure(&lines);

    let preheader = pred_map.get(header)?
        .iter()
        .find(|p| p.as_str() != latch)?
        .clone();

    let latch_blk = blocks.iter().find(|b| b.label == latch)?;
    // The injected back-edge is always `br i1 %be, label %<back-edge>, label %<exit>`.
    // loop-simplify may insert a trivial back-edge block whose name is a suffix of the
    // header (e.g. "CF75.backedge"), so `find(s != header)` picks that block instead
    // of the real exit.  The exit is always the second (false-branch) target.
    let exit = latch_blk.successors.last()?.clone();

    let phis = collect_header_phis(&lines, header, &preheader, latch);
    let expr = build_loopir(header, &preheader, latch, &exit, &phis);
    let loop_blocks = collect_loop_blocks(&lines, header, latch);

    let meta = LoopMeta { header: header.to_string(), preheader, latch: latch.to_string(), exit, loop_blocks };
    Some((expr, meta))
}

/// Extract preamble (lines before loop1's header label) and epilogue (lines
/// from loop2's exit label to end of file).  Call this once with `norm_ir`
/// during step 4 so the back-converter never touches IR text again.
pub fn extract_preamble_epilogue(
    ir: &str,
    loop1_header: &str,
    loop2_exit: &str,
) -> (Vec<String>, Vec<String>) {
    let mut preamble: Vec<String> = Vec::new();
    let mut epilogue: Vec<String> = Vec::new();
    let mut phase = 0usize; // 0=preamble, 1=middle (loop blocks), 2=epilogue

    for line in ir.lines() {
        match phase {
            0 => {
                if block_label(line) == Some(loop1_header) {
                    phase = 1;
                } else {
                    preamble.push(line.to_string());
                }
            }
            1 => {
                if block_label(line) == Some(loop2_exit) {
                    phase = 2;
                    epilogue.push(line.to_string());
                }
                // else: loop blocks — skip
            }
            _ => epilogue.push(line.to_string()),
        }
    }

    (preamble, epilogue)
}

/// Reconstruct fused LLVM IR entirely from pre-collected metadata.
/// No IR text is read here — all data was gathered during `ir_to_loopir`.
///
/// Transformations applied:
///   h1       — loop1 phis with step_pred lat1→lat2; loop2 phis added with
///               init_pred pre2→pre1; exit branch target exit1→exit2
///   body1    — unchanged
///   lat1     — freeze+br-i1 replaced with `br label %h2`
///   h2       — phi lines removed; rest unchanged
///   body2    — unchanged
///   lat2     — back-edge target h2 → h1
pub fn loopir_to_llvm(
    meta1: &LoopMeta,
    meta2: &LoopMeta,
    preamble: &[String],
    epilogue: &[String],
) -> Option<String> {
    let mut out: Vec<String> = Vec::new();

    // Preamble (entry, pre1, any blocks before h1)
    out.extend_from_slice(preamble);

    let (_, h1_lines) = meta1.loop_blocks.first()?;
    let (_, h2_lines) = meta2.loop_blocks.first()?;

    // Loop2 phi lines, transplanted into h1 with init_pred redirected
    let pre1_label = format!("%{}", meta1.preheader);
    let pre2_label = format!("%{}", meta2.preheader);
    let lat1_label = format!("%{}", meta1.latch);
    let lat2_label = format!("%{}", meta2.latch);
    let exit1_label = format!("label %{}", meta1.exit);
    let exit2_label = format!("label %{}", meta2.exit);

    let loop2_phis: Vec<String> = h2_lines.iter()
        .filter(|l| l.contains("= phi "))
        .map(|l| l.replace(&pre2_label, &pre1_label))
        .collect();

    // Emit fused h1 block
    for line in h1_lines {
        if line.contains("= phi ") {
            // Redirect step_pred from lat1 → lat2
            out.push(line.replace(&lat1_label, &lat2_label));
        } else if line.trim().starts_with("br ") || line.trim().starts_with("ret ") {
            // Insert loop2 phis just before the first terminator
            out.extend(loop2_phis.iter().cloned());
            // Redirect exit branch from exit1 → exit2
            out.push(line.replace(&exit1_label, &exit2_label));
        } else {
            out.push(line.clone());
        }
    }

    // Body1 blocks (between h1 and lat1, exclusive on both ends)
    let n1 = meta1.loop_blocks.len();
    for (_, blk) in meta1.loop_blocks[1..n1.saturating_sub(1)].iter() {
        out.extend_from_slice(blk);
    }

    // Fused lat1: remove freeze + conditional br, add unconditional br to h2
    let (_, lat1_lines) = meta1.loop_blocks.last()?;
    let indent = "  ";
    let mut skip_br = false;
    for line in lat1_lines {
        let tr = line.trim();
        if tr.contains("= freeze i1 poison") {
            skip_br = true;
            continue;
        }
        if skip_br && tr.starts_with("br i1 ") {
            out.push(format!("{}br label %{}", indent, meta2.header));
            skip_br = false;
            continue;
        }
        out.push(line.clone());
    }

    // Stripped h2 block (phis removed)
    for line in h2_lines {
        if !line.contains("= phi ") {
            out.push(line.clone());
        }
    }

    // Body2 blocks
    let n2 = meta2.loop_blocks.len();
    for (_, blk) in meta2.loop_blocks[1..n2.saturating_sub(1)].iter() {
        out.extend_from_slice(blk);
    }

    // Fused lat2: redirect the back-edge target (first/true branch of the conditional)
    // from h2 (or its loop-simplify back-edge block) to h1.
    let (_, lat2_lines) = meta2.loop_blocks.last()?;
    for line in lat2_lines {
        if line.trim().starts_with("br i1 ") {
            let targets = br_targets(line.trim());
            if let Some(back_edge_tgt) = targets.first() {
                let from = format!("label %{}", back_edge_tgt);
                let to   = format!("label %{}", meta1.header);
                out.push(line.replacen(&from, &to, 1));
            } else {
                out.push(line.clone());
            }
        } else {
            out.push(line.clone());
        }
    }

    // Epilogue (exit2 block onwards)
    out.extend_from_slice(epilogue);

    Some(out.join("\n"))
}

// ---------------------------------------------------------------------------
// IR structure parsing
// ---------------------------------------------------------------------------

struct SimpleBlock {
    label:      String,
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
// Raw block collection
// ---------------------------------------------------------------------------

/// Collect raw lines for every block from `header` through `latch` (inclusive),
/// in the order they appear in the IR text.
fn collect_loop_blocks(lines: &[&str], header: &str, latch: &str) -> Vec<(String, Vec<String>)> {
    let mut result: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_fn = false;
    let mut collecting = false;
    let mut cur_label: Option<String> = None;
    let mut cur_lines: Vec<String> = Vec::new();

    for &line in lines {
        let tr = line.trim();
        if !in_fn {
            if tr.starts_with("define ") { in_fn = true; }
            continue;
        }
        if tr == "}" { break; }

        if let Some(lbl) = block_label(line) {
            // Flush the block we were accumulating
            if let Some(prev) = cur_label.take() {
                let is_latch = prev == latch;
                result.push((prev, std::mem::take(&mut cur_lines)));
                if is_latch { break; }
            }

            if lbl == header { collecting = true; }
            if collecting {
                cur_label = Some(lbl.to_string());
                cur_lines.push(line.to_string());
            }
        } else if collecting {
            cur_lines.push(line.to_string());
        }
    }

    // Flush if latch was the last block before `}`
    if let Some(lbl) = cur_label {
        result.push((lbl, cur_lines));
    }

    result
}

// ---------------------------------------------------------------------------
// Phi parsing
// ---------------------------------------------------------------------------

struct PhiInfo {
    var:  String,
    init: String,
    step: String,
}

fn collect_header_phis(lines: &[&str], header: &str, preheader: &str, latch: &str) -> Vec<PhiInfo> {
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
            break;
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

fn build_loopir(header: &str, preheader: &str, latch: &str, exit: &str, phis: &[PhiInfo]) -> RecExpr<LoopIR> {
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
